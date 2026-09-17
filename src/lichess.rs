//! Playing on lichess.org, through their Board API.
//!
//! The board API is meant for real boards and third-party clients, and it
//! works with an ordinary lichess account. Lichess forbids engine help
//! there, so in a lichess game gambit is only your board: the model plays
//! no part, and the moves are yours.
//!
//! A token with the "Play games with the board API" right comes from
//! <https://lichess.org/account/oauth/token/create?scopes[]=board:play>.
//!
//! Two streams run on threads of their own, both newline-delimited JSON
//! that lichess holds open: the account's events, which say when a game
//! starts, and the game itself, which says what has been played and how
//! much time is left. Every line lands in the app as an [`Event`].

use std::io::BufRead;
use std::sync::mpsc::Sender;

const HOST: &str = "https://lichess.org";

/// What the streams tell the game.
#[derive(Debug, Clone)]
pub enum Event {
    /// A game is on, with its id and the colour you have.
    Started { id: String, white: bool, opponent: String },
    /// Where the game stands: the moves so far, and the clocks in seconds.
    Moves { moves: Vec<String>, white_time: u64, black_time: u64 },
    /// The game ended: "mate", "resign", "outoftime", "draw", "aborted".
    Ended { status: String, winner: Option<String> },
    Trouble(String),
}

fn token_of(cfg: &crate::config::Config) -> Result<String, String> {
    let t = if cfg.lichess_token.trim().is_empty() {
        std::env::var("LICHESS_TOKEN").unwrap_or_default()
    } else {
        cfg.lichess_token.trim().to_string()
    };
    if t.is_empty() {
        return Err("no lichess token yet".into());
    }
    Ok(t)
}

fn post(token: &str, path: &str, form: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    let answer = ureq::post(&format!("{HOST}{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(30))
        .send_form(form)
        .map_err(trouble)?
        .into_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&answer).map_err(|e| format!("{e}: {answer}"))
}

/// Who you are on lichess, which also says whether the token works.
pub fn whoami(cfg: &crate::config::Config) -> Result<String, String> {
    let token = token_of(cfg)?;
    let answer = ureq::get(&format!("{HOST}/api/account"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(trouble)?
        .into_string()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    v.get("username").and_then(|u| u.as_str()).map(String::from)
        .ok_or_else(|| "lichess did not say who you are".to_string())
}

/// Challenge lichess's own Stockfish, from level 1 to 8. Returns the game id.
pub fn play_computer(cfg: &crate::config::Config, level: u8, white: bool) -> Result<String, String> {
    let token = token_of(cfg)?;
    let v = post(&token, "/api/challenge/ai", &[
        ("level", &level.to_string()),
        ("clock.limit", "600"),
        ("clock.increment", "0"),
        ("color", if white { "white" } else { "black" }),
    ])?;
    v.get("id").and_then(|i| i.as_str()).map(String::from)
        .ok_or_else(|| format!("lichess sent no game id: {v}"))
}

/// Ask for a human opponent, ten minutes each. The call is held open by
/// lichess until somebody takes it, so it runs on a thread of its own and
/// the game turns up as an event.
pub fn seek(cfg: &crate::config::Config, rated: bool) -> Result<(), String> {
    let token = token_of(cfg)?;
    std::thread::spawn(move || {
        let _ = ureq::post(&format!("{HOST}/api/board/seek"))
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(std::time::Duration::from_secs(600))
            .send_form(&[
                ("rated", if rated { "true" } else { "false" }),
                ("time", "10"),
                ("increment", "0"),
            ]);
    });
    Ok(())
}

/// A game already under way, if there is one.
pub fn ongoing(cfg: &crate::config::Config) -> Result<Option<String>, String> {
    let token = token_of(cfg)?;
    let answer = ureq::get(&format!("{HOST}/api/account/playing"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(30))
        .call()
        .map_err(trouble)?
        .into_string()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    Ok(v.pointer("/nowPlaying/0/gameId").and_then(|i| i.as_str()).map(String::from))
}

pub fn send_move(cfg: &crate::config::Config, game: &str, uci: &str) -> Result<(), String> {
    let token = token_of(cfg)?;
    let v = post(&token, &format!("/api/board/game/{game}/move/{uci}"), &[])?;
    if v.get("ok").and_then(|o| o.as_bool()) == Some(true) { return Ok(()); }
    Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("lichess refused the move").to_string())
}

pub fn resign(cfg: &crate::config::Config, game: &str) -> Result<(), String> {
    let token = token_of(cfg)?;
    post(&token, &format!("/api/board/game/{game}/resign"), &[]).map(|_| ())
}

/// Watch the account for a game starting, and send each one on.
pub fn watch_events(cfg: &crate::config::Config, out: Sender<Event>) -> Result<(), String> {
    let token = token_of(cfg)?;
    std::thread::spawn(move || {
        let trouble = stream(&token, "/api/stream/event", |line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return true };
            if v.get("type").and_then(|t| t.as_str()) != Some("gameStart") { return true; }
            let g = v.get("game").cloned().unwrap_or_default();
            let Some(id) = g.get("gameId").or(g.get("id")).and_then(|i| i.as_str()) else { return true };
            let white = g.get("color").and_then(|c| c.as_str()) != Some("black");
            let opponent = g.pointer("/opponent/username").and_then(|u| u.as_str()).unwrap_or("lichess").to_string();
            out.send(Event::Started { id: id.to_string(), white, opponent }).is_ok()
        });
        if let Some(why) = trouble { let _ = out.send(Event::Trouble(why)); }
    });
    Ok(())
}

/// Follow one game: the moves as they are played, and the clocks.
pub fn watch_game(cfg: &crate::config::Config, game: &str, me: &str, out: Sender<Event>) -> Result<(), String> {
    let token = token_of(cfg)?;
    let path = format!("/api/board/game/stream/{game}");
    let me = me.to_string();
    std::thread::spawn(move || {
        let trouble = stream(&token, &path, |line| {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return true };
            let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if kind == "gameFull" {
                let white = v.pointer("/white/id").and_then(|u| u.as_str()).unwrap_or("")
                    .eq_ignore_ascii_case(&me);
                let opponent = if white {
                    v.pointer("/black/name").or(v.pointer("/black/id")).and_then(|u| u.as_str()).unwrap_or("lichess")
                } else {
                    v.pointer("/white/name").or(v.pointer("/white/id")).and_then(|u| u.as_str()).unwrap_or("lichess")
                }.to_string();
                if out.send(Event::Started { id: String::new(), white, opponent }).is_err() { return false; }
            }
            let state = if kind == "gameFull" { v.get("state").cloned().unwrap_or_default() } else { v.clone() };
            if kind != "gameFull" && kind != "gameState" { return true; }
            let moves: Vec<String> = state.get("moves").and_then(|m| m.as_str()).unwrap_or("")
                .split_whitespace().map(String::from).collect();
            let secs = |k: &str| state.get(k).and_then(|t| t.as_u64()).unwrap_or(0) / 1000;
            if out.send(Event::Moves { moves, white_time: secs("wtime"), black_time: secs("btime") }).is_err() {
                return false;
            }
            let status = state.get("status").and_then(|s| s.as_str()).unwrap_or("started").to_string();
            if status != "started" && status != "created" {
                let winner = state.get("winner").and_then(|w| w.as_str()).map(String::from);
                let _ = out.send(Event::Ended { status, winner });
                return false;
            }
            true
        });
        if let Some(why) = trouble { let _ = out.send(Event::Trouble(why)); }
    });
    Ok(())
}

/// Read a stream of JSON lines until `each` says to stop or lichess ends
/// it. Lichess sends a blank line now and then to keep it alive. Comes
/// back with what went wrong, when something did.
fn stream(token: &str, path: &str, mut each: impl FnMut(&str) -> bool) -> Option<String> {
    let answer = ureq::get(&format!("{HOST}{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(0))
        .call();
    let answer = match answer {
        Ok(a) => a,
        Err(e) => return Some(trouble(e)),
    };
    let reader = std::io::BufReader::new(answer.into_reader());
    for line in reader.lines() {
        let Ok(line) = line else { return Some("the stream broke off".into()) };
        if line.trim().is_empty() { continue; }
        if !each(&line) { return None; }
    }
    None
}

/// A lichess error, with its own words when it sent any.
fn trouble(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let msg = v.get("error").and_then(|m| m.as_str()).unwrap_or(&body).trim().to_string();
            let msg = if msg.is_empty() { "no reason given".to_string() } else { msg };
            match code {
                401 => format!("401: the token is not accepted ({msg})"),
                _ => format!("{code}: {msg}"),
            }
        }
        other => other.to_string(),
    }
}

/// Minutes and seconds off a clock.
pub fn clock(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn a_game_needs_a_token() {
        let cfg = Config::default();
        std::env::remove_var("LICHESS_TOKEN");
        assert_eq!(token_of(&cfg), Err("no lichess token yet".into()));
        let with = Config { lichess_token: "  abc  ".into(), ..Config::default() };
        assert_eq!(token_of(&with).unwrap(), "abc");
    }

    #[test]
    fn clocks_read_as_minutes_and_seconds() {
        assert_eq!(clock(600), "10:00");
        assert_eq!(clock(59), "0:59");
        assert_eq!(clock(3661), "61:01");
    }
}

//! The other player: a model, asked for one move at a time.
//!
//! Four ways to reach one. The `claude` command needs no key and is the
//! default. Anthropic's API and any OpenAI-shaped API (OpenAI,
//! OpenRouter, a server on your own machine) need a key. A command of
//! your own gets the question on standard input and prints a move.
//!
//! The question carries the position, the moves so far and every legal
//! move, so a model that can read has no excuse for an illegal answer.
//! One does come now and then, and the game asks again (see `main.rs`).

use std::io::Write;
use std::process::{Command, Stdio};

use crate::chess::{Color, Position};
use crate::config::Config;

/// What to ask for a position, with the legal moves spelled out.
pub fn question(pos: &Position, played: &[String], again: Option<&str>) -> String {
    let mut legal: Vec<String> = pos.legal_moves().iter().map(|m| pos.san(*m)).collect();
    legal.sort();
    let side = if pos.turn == Color::White { "White" } else { "Black" };
    let mut q = format!(
        "We are playing chess. You are {side}, and it is your move.\n\n\
         Position in FEN: {}\n",
        pos.to_fen());
    if !played.is_empty() { q.push_str(&format!("Moves so far: {}\n", numbered(played))); }
    q.push_str(&format!("Your legal moves: {}\n\n", legal.join(", ")));
    if let Some(bad) = again {
        q.push_str(&format!("Your last answer, {bad}, was not one of them. Read the list again.\n\n"));
    }
    q.push_str("Answer with one move from that list and nothing else: no explanation, no punctuation.");
    q
}

/// "1. e4 e5 2. Nf3" out of a list of moves.
fn numbered(played: &[String]) -> String {
    let mut out = String::new();
    for (i, mv) in played.iter().enumerate() {
        if i % 2 == 0 { out.push_str(&format!("{}. ", i / 2 + 1)); }
        out.push_str(mv);
        out.push(' ');
    }
    out.trim_end().to_string()
}

/// What came back: the move, and what the answer says about itself.
#[derive(Default, Debug, Clone)]
pub struct Answer {
    pub text: String,
    /// The model that actually answered, when it says so.
    pub model: Option<String>,
    /// What the answer cost, in dollars, when it says so.
    pub cost: Option<f64>,
}

impl Answer {
    fn plain(text: String) -> Answer { Answer { text, ..Answer::default() } }
}

/// Ask the opponent and hand back what it said.
pub fn ask(cfg: &Config, question: &str) -> Result<Answer, String> {
    match cfg.opponent.trim() {
        "claude" | "" => claude(cfg, question),
        "anthropic" => anthropic(cfg, question),
        "openai" => openai(cfg, question),
        "command" => shell(cfg, question),
        other => Err(format!("unknown opponent \"{other}\": use claude, anthropic, openai or command")),
    }
}

/// A model name as it reads in the header: claude-haiku-4-5-20251001
/// becomes haiku 4.5.
pub fn pretty_model(id: &str) -> String {
    let id = id.trim().trim_start_matches("claude-");
    let mut parts: Vec<&str> = id.split('-').collect();
    // The date at the end is noise.
    if parts.last().is_some_and(|p| p.len() == 8 && p.chars().all(|c| c.is_ascii_digit())) { parts.pop(); }
    let name = parts.remove(0);
    if parts.is_empty() { return name.to_string(); }
    if parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) {
        return format!("{name} {}", parts.join("."));
    }
    format!("{name} {}", parts.join(" "))
}

/// How the opponent is named in the header.
pub fn describe(cfg: &Config) -> String {
    let model = if cfg.model.trim().is_empty() { String::new() } else { format!(" {}", cfg.model.trim()) };
    match cfg.opponent.trim() {
        "claude" | "" => format!("claude -p{model}"),
        "anthropic" => format!("Anthropic API{model}"),
        "openai" => format!("{}{}", host_of(&cfg.base_url), model),
        "command" => cfg.command.split_whitespace().next().unwrap_or("command").to_string(),
        other => other.to_string(),
    }
}

fn host_of(url: &str) -> String {
    url.trim_start_matches("https://").trim_start_matches("http://")
        .split('/').next().unwrap_or(url).to_string()
}

fn claude(cfg: &Config, question: &str) -> Result<Answer, String> {
    let mut cmd = Command::new("claude");
    // The JSON form also says which model answered and what it cost.
    cmd.arg("-p").arg(question).arg("--output-format").arg("json");
    if !cfg.model.trim().is_empty() { cmd.arg("--model").arg(cfg.model.trim()); }
    // Away from any project, so the command has no repository to read.
    if let Ok(home) = std::env::var("HOME") { cmd.current_dir(home); }
    let out = cmd.stdin(Stdio::null()).output()
        .map_err(|e| format!("the claude command did not run ({e}). Install Claude Code, or set another opponent with o."))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(format!("claude said: {}", first_line(&why)));
    }
    Ok(from_claude_json(&String::from_utf8_lossy(&out.stdout)))
}

/// Pull the move, the model and the cost out of what `claude -p --output-format
/// json` prints. Plain text still works, in case that output ever changes.
fn from_claude_json(out: &str) -> Answer {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(out.trim()) else {
        return Answer::plain(out.trim().to_string());
    };
    let text = v.get("result").and_then(|r| r.as_str()).unwrap_or(out.trim()).trim().to_string();
    let model = v.get("modelUsage").and_then(|m| m.as_object())
        .and_then(|m| m.keys().next().cloned());
    let cost = v.get("total_cost_usd").and_then(|c| c.as_f64());
    Answer { text, model, cost }
}

fn key_for(cfg: &Config, env: &str) -> Result<String, String> {
    let key = if cfg.api_key.trim().is_empty() { std::env::var(env).unwrap_or_default() } else { cfg.api_key.trim().to_string() };
    if key.is_empty() { return Err(format!("no key: put one in ~/.gambit/config.yml or in {env}")); }
    Ok(key)
}

fn anthropic(cfg: &Config, question: &str) -> Result<Answer, String> {
    let key = key_for(cfg, "ANTHROPIC_API_KEY")?;
    let model = if cfg.model.trim().is_empty() { "claude-sonnet-5" } else { cfg.model.trim() };
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 64,
        "messages": [{"role": "user", "content": question}],
    });
    let answer = ureq::post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", &key)
        .set("anthropic-version", "2023-06-01")
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(120))
        .send_json(body)
        .map_err(describe_error)?
        .into_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    let text = v.pointer("/content/0/text").and_then(|t| t.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("no text in the answer: {}", first_line(&answer)))?;
    let said = v.get("model").and_then(|m| m.as_str()).map(String::from);
    Ok(Answer { text, model: said.or(Some(model.to_string())), cost: None })
}

fn openai(cfg: &Config, question: &str) -> Result<Answer, String> {
    let key = key_for(cfg, "OPENAI_API_KEY")?;
    let model = if cfg.model.trim().is_empty() { "gpt-5" } else { cfg.model.trim() };
    let base = cfg.base_url.trim().trim_end_matches('/');
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": question}],
    });
    let answer = ureq::post(&format!("{base}/chat/completions"))
        .set("authorization", &format!("Bearer {key}"))
        .set("content-type", "application/json")
        .timeout(std::time::Duration::from_secs(120))
        .send_json(body)
        .map_err(describe_error)?
        .into_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&answer).map_err(|e| e.to_string())?;
    let text = v.pointer("/choices/0/message/content").and_then(|t| t.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("no text in the answer: {}", first_line(&answer)))?;
    let said = v.get("model").and_then(|m| m.as_str()).map(String::from);
    Ok(Answer { text, model: said.or(Some(model.to_string())), cost: None })
}

fn shell(cfg: &Config, question: &str) -> Result<Answer, String> {
    if cfg.command.trim().is_empty() { return Err("no command set in ~/.gambit/config.yml".into()); }
    let mut child = Command::new("sh").arg("-c").arg(cfg.command.trim())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("{}: {e}", cfg.command.trim()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(question.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("the command said: {}", first_line(&String::from_utf8_lossy(&out.stderr))));
    }
    Ok(Answer::plain(String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

/// An API error, with the server's own words when it sent any.
fn describe_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let msg = v.pointer("/error/message").and_then(|m| m.as_str())
                .unwrap_or_else(|| first_line(&body));
            format!("{code}: {msg}")
        }
        other => other.to_string(),
    }
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_carries_the_position_and_every_legal_move() {
        let pos = Position::start();
        let q = question(&pos, &["e4".into(), "e5".into()], None);
        assert!(q.contains("You are White"));
        assert!(q.contains("rnbqkbnr/pppppppp"));
        assert!(q.contains("Moves so far: 1. e4 e5"));
        assert!(q.contains("Nf3") && q.contains("a3"), "the legal moves are listed: {q}");
        let again = question(&pos, &[], Some("Qz9"));
        assert!(again.contains("Qz9, was not one of them"));
    }

    #[test]
    fn a_command_of_your_own_answers_on_standard_input() {
        let cfg = Config { opponent: "command".into(), command: "grep -o 'You are [A-Za-z]*' | head -1".into(), ..Config::default() };
        assert_eq!(ask(&cfg, "We are playing chess. You are Black.").unwrap().text, "You are Black");
        let empty = Config { opponent: "command".into(), ..Config::default() };
        assert!(ask(&empty, "x").is_err());
        assert!(ask(&Config { opponent: "wat".into(), ..Config::default() }, "x").is_err());
    }

    #[test]
    fn the_claude_answer_says_which_model_played_and_what_it_cost() {
        let json = r#"{"result":"Nf6","total_cost_usd":0.0455,
            "modelUsage":{"claude-haiku-4-5-20251001":{"inputTokens":1071}}}"#;
        let a = from_claude_json(json);
        assert_eq!(a.text, "Nf6");
        assert_eq!(a.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert_eq!(a.cost, Some(0.0455));
        assert_eq!(from_claude_json("e4\n").text, "e4", "plain text still works");
        assert_eq!(pretty_model("claude-haiku-4-5-20251001"), "haiku 4.5");
        assert_eq!(pretty_model("claude-opus-5"), "opus 5");
        assert_eq!(pretty_model("gpt-5"), "gpt 5");
    }

    #[test]
    fn the_opponent_is_named_for_the_header() {
        assert_eq!(describe(&Config::default()), "claude -p");
        let m = Config { model: "opus".into(), ..Config::default() };
        assert_eq!(describe(&m), "claude -p opus");
        let o = Config { opponent: "openai".into(), base_url: "https://openrouter.ai/api/v1".into(), model: "x/y".into(), ..Config::default() };
        assert_eq!(describe(&o), "openrouter.ai x/y");
    }
}

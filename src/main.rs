//! gambit — chess against a language model, in the terminal. Part of Fe₂O₃.
//!
//! The rules live in `chess.rs`, the opponent in `opponent.rs`. This file
//! draws the board and takes your moves. While the model thinks, its call
//! runs on its own thread, so the board still answers keys; the loop wakes
//! once a second for the clock and sleeps between.

mod chess;
mod config;
mod lichess;
mod opponent;

use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use chess::{Color, Move, Over, Piece, Position, Square};
use config::Config;
use crust::cursor::Cursor;
use crust::{style, Crust, Input, Pane, Popup};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SIDE_W: u16 = 34;

/// How wide and tall one square is drawn, in cells. A cell is about twice
/// as tall as it is wide, so these pairs all come out roughly square. The
/// biggest one that fits the window wins, and the piece sits in the middle.
const SIZES: [(usize, usize); 4] = [(8, 4), (6, 3), (4, 2), (2, 1)];

fn square_size(cols: u16, rows: u16) -> (usize, usize) {
    let room = |(w, h): (usize, usize)| {
        8 * w + 5 + SIDE_W as usize <= cols as usize && 8 * h + 4 <= rows as usize
    };
    SIZES.into_iter().find(|s| room(*s)).unwrap_or(SIZES[SIZES.len() - 1])
}

mod t {
    pub const LIGHT: u8 = 180;
    pub const DARK: u8 = 95;
    pub const WHITE_PIECE: u8 = 231;
    pub const BLACK_PIECE: u8 = 16;
    pub const CURSOR: u8 = 28;
    pub const PICKED: u8 = 172;
    pub const TARGET: u8 = 108;
    pub const LAST: u8 = 66;
    pub const CHECK: u8 = 124;
    pub const FG: u8 = 252;
    pub const DIM: u8 = 244;
    pub const ACCENT: u8 = 179;
    pub const OK: u8 = 156;
    pub const ERR: u8 = 203;
    pub const BAR: u8 = 236;
}

/// A game on lichess: which one, who is on the other side, and the clocks
/// as they last stood.
struct Live {
    id: String,
    opponent: String,
    events: Receiver<lichess::Event>,
    /// Seconds left for White and for Black, and when that was said.
    clocks: (u64, u64),
    since: Instant,
    ended: bool,
}

/// A game in progress, and everything drawn about it.
struct App {
    cfg: Config,
    pos: Position,
    /// Every position played, to spot a threefold repetition.
    seen: Vec<String>,
    /// The moves as written, and the position before each of them.
    played: Vec<String>,
    back: Vec<Position>,
    me: Color,
    cursor: Square,
    picked: Option<Square>,
    last: Option<Move>,
    over: Option<Over>,
    /// The model's call, running on its own thread.
    thinking: Option<(Receiver<Result<opponent::Answer, String>>, Instant, u8)>,
    note: Option<(String, u8)>,
    /// The model that answered last, and what the game has cost so far.
    played_by: Option<String>,
    spent: f64,
    /// A game on lichess, when one is on.
    live: Option<Live>,
    /// The account's own event stream, which says when a game starts.
    lichess_events: Option<Receiver<lichess::Event>>,
    flip: bool,
    header: Pane,
    board_p: Pane,
    side_p: Pane,
    footer: Pane,
    cols: u16,
    rows: u16,
    /// Width and height of one square, chosen to fit the window.
    sq: (usize, usize),
    shown: [String; 4],
}

impl App {
    fn new(cfg: Config) -> App {
        let me = if cfg.side.trim().eq_ignore_ascii_case("black") { Color::Black } else { Color::White };
        let (cols, rows) = Crust::terminal_size();
        let mut app = App {
            cfg, pos: Position::start(), seen: Vec::new(), played: Vec::new(), back: Vec::new(),
            me, cursor: 4, picked: None, last: None, over: None, thinking: None, note: None,
            played_by: None, spent: 0.0, live: None, lichess_events: None,
            flip: me == Color::Black,
            header: Pane::new(1, 1, cols, 1, t::FG as u16, t::BAR as u16),
            board_p: Pane::new(1, 2, cols.saturating_sub(SIDE_W), rows.saturating_sub(2), t::FG as u16, 0),
            side_p: Pane::new(cols.saturating_sub(SIDE_W) + 1, 2, SIDE_W, rows.saturating_sub(2), t::FG as u16, 0),
            footer: Pane::new(1, rows, cols, 1, t::FG as u16, t::BAR as u16),
            cols, rows,
            sq: square_size(cols, rows),
            shown: Default::default(),
        };
        for p in [&mut app.header, &mut app.board_p, &mut app.side_p, &mut app.footer] {
            p.wrap = false;
            p.scroll = false;
        }
        app.seen.push(app.pos.key());
        app.cursor = if app.me == Color::White { 4 } else { 60 };
        app
    }

    // ---- Drawing ------------------------------------------------------

    fn render(&mut self) {
        let head = self.header_text();
        if head != self.shown[0] { self.header.set_text(&head); self.header.refresh(); self.shown[0] = head; }
        let board = self.board_text();
        if board != self.shown[1] { self.board_p.set_text(&board); self.board_p.refresh(); self.shown[1] = board; }
        let side = self.side_text();
        if side != self.shown[2] { self.side_p.set_text(&side); self.side_p.refresh(); self.shown[2] = side; }
        let foot = self.footer_text();
        if foot != self.shown[3] { self.footer.set_text(&foot); self.footer.refresh(); self.shown[3] = foot; }
        Cursor::hide();
    }

    fn header_text(&self) -> String {
        let you = if self.me == Color::White { "White" } else { "Black" };
        let left = format!(" {}  {}",
            style::bold(&style::fg("gambit", t::ACCENT)),
            style::fg(&format!("you play {you}"), t::DIM));
        let who = match (&self.live, &self.played_by) {
            (Some(live), _) => format!("lichess · {}", live.opponent),
            (None, Some(id)) => format!("{} · {}", opponent::describe(&self.cfg), opponent::pretty_model(id)),
            (None, None) => opponent::describe(&self.cfg),
        };
        let right = format!("{}  v{} ", style::fg(&who, t::OK), VERSION);
        let pad = (self.cols as usize).saturating_sub(crust::display_width(&left) + crust::display_width(&right));
        format!("{left}{}{right}", " ".repeat(pad))
    }

    /// The board, eight squares by eight, with the rank numbers down the
    /// left and the file letters underneath.
    fn board_text(&self) -> String {
        let (sq_w, sq_h) = self.sq;
        let mid_col = sq_w / 2;
        // The piece sits a row above the middle, so it has a line of its own
        // square under it rather than resting on the edge.
        let piece_row = (sq_h / 2).saturating_sub(1);
        let letter_row = piece_row + 1;
        let mut lines: Vec<String> = vec![String::new()];
        let check = self.pos.in_check(self.pos.turn);
        for row in 0..8 {
            let rank = if self.flip { row } else { 7 - row };
            for sub in 0..sq_h {
                let mut line = String::from("  ");
                let label = if sub == piece_row { format!("{} ", rank + 1) } else { "  ".to_string() };
                line.push_str(&style::fg(&label, t::DIM));
                for col in 0..8 {
                    let file = if self.flip { 7 - col } else { col };
                    let sq = chess::at(file, rank).unwrap();
                    let light = (file + rank) % 2 == 1;
                    let mut bg = if light { t::LIGHT } else { t::DARK };
                    if self.last.is_some_and(|m| m.from == sq || m.to == sq) { bg = t::LAST; }
                    if self.is_target(sq) { bg = t::TARGET; }
                    if Some(sq) == self.picked { bg = t::PICKED; }
                    if check && self.pos.piece_at(sq) == Some((self.pos.turn, Piece::King)) { bg = t::CHECK; }
                    if sq == self.cursor { bg = t::CURSOR; }
                    let here = self.pos.piece_at(sq);
                    let fg = match here {
                        Some((Color::White, _)) => t::WHITE_PIECE,
                        Some((Color::Black, _)) => t::BLACK_PIECE,
                        None => t::FG,
                    };
                    // The glyph on the middle row, its letter below when the
                    // square is tall enough: knight, bishop and pawn are hard
                    // to tell apart at this size otherwise.
                    let letters = sq_h >= 3;
                    let mark = match here {
                        Some((c, p)) if sub == piece_row => piece_glyph(p, c).to_string(),
                        Some((c, p)) if letters && sub == letter_row => piece_letter(p, c).to_string(),
                        _ => String::new(),
                    };
                    let cell = if mark.is_empty() {
                        " ".repeat(sq_w)
                    } else {
                        let after = sq_w.saturating_sub(mid_col + crust::display_width(&mark));
                        format!("{}{}{}", " ".repeat(mid_col), mark, " ".repeat(after))
                    };
                    line.push_str(&style::fb(&cell, fg, bg));
                }
                lines.push(line);
            }
        }
        self.add_taken(&mut lines, 4 + 8 * sq_w + 3, sq_h);
        let files: String = (0..8).map(|col| {
            let file = if self.flip { 7 - col } else { col };
            format!("{:^width$}", (b'a' + file as u8) as char, width = sq_w)
        }).collect();
        lines.push(format!("     {}", style::fg(&files, t::DIM)));
        lines.join("\n")
    }

    /// Write what each side has taken beside the board: the pieces you took
    /// under your end of it, theirs at the top, and who is ahead on points.
    fn add_taken(&self, lines: &mut [String], at: usize, sq_h: usize) {
        let (mine, theirs) = (lost(&self.pos, self.me), lost(&self.pos, self.me.other()));
        let score = points(&theirs) - points(&mine);
        let row = |pieces: &[Piece], color: Color| -> String {
            if pieces.is_empty() { return style::fg("nothing yet", t::DIM); }
            let glyphs: String = pieces.iter().map(|p| piece_glyph(*p, color)).collect();
            style::fg(&glyphs, if color == Color::White { t::WHITE_PIECE } else { t::BLACK_PIECE })
        };
        let lead = |n: i32| if n > 0 { style::fg(&format!("  +{n}"), t::OK) } else { String::new() };
        // Each pile goes beside the player who took it, so it follows the
        // board when you turn it around.
        let mine_at_bottom = (self.me == Color::White) != self.flip;
        let last = 8 * sq_h;
        let (they_row, you_row) = if mine_at_bottom { (1, last - 1) } else { (last - 1, 1) };
        for (row_at, label, pieces) in [
            (they_row, "they took", format!("{}{}", row(&mine, self.me), lead(-score))),
            (you_row, "you took", format!("{}{}", row(&theirs, self.me.other()), lead(score))),
        ] {
            put(lines, row_at, at, &style::fg(label, t::DIM));
            put(lines, row_at + 1, at, &pieces);
        }
    }

    /// A square the picked piece may move to.
    fn is_target(&self, sq: Square) -> bool {
        self.picked.is_some_and(|from| self.pos.legal_moves().iter().any(|m| m.from == from && m.to == sq))
    }

    fn side_text(&self) -> String {
        let mut l: Vec<String> = vec![String::new()];
        l.push(format!(" {}", style::bold(&style::fg("Moves", t::ACCENT))));
        l.push(String::new());
        let mut line = String::new();
        for (i, mv) in self.played.iter().enumerate() {
            if i % 2 == 0 { line = format!(" {:>3}. {:<9}", i / 2 + 1, mv); }
            else {
                line.push_str(&format!("{mv:<9}"));
                l.push(line.clone());
                line.clear();
            }
        }
        if !line.is_empty() { l.push(line); }
        if self.played.is_empty() { l.push(format!(" {}", style::fg("(none yet)", t::DIM))); }
        // Keep the last moves in view on a long game.
        let room = (self.rows as usize).saturating_sub(10);
        if l.len() > room + 3 {
            let cut = l.len() - room;
            l.drain(3..3 + cut);
        }
        if let Some(live) = &self.live {
            let gone = live.since.elapsed().as_secs();
            let (w, b) = live.clocks;
            let (w, b) = if live.ended {
                (w, b)
            } else if self.pos.turn == Color::White {
                (w.saturating_sub(gone), b)
            } else {
                (w, b.saturating_sub(gone))
            };
            let mine = if self.me == Color::White { w } else { b };
            let theirs = if self.me == Color::White { b } else { w };
            l.push(String::new());
            l.push(format!(" {}   {}",
                style::fg(&format!("you {}", lichess::clock(mine)), t::OK),
                style::fg(&format!("{} {}", live.opponent, lichess::clock(theirs)), t::DIM)));
        }
        l.push(String::new());
        l.push(format!(" {}", self.status()));
        if self.spent > 0.0 {
            l.push(format!(" {}", style::fg(&format!("this game has cost ${:.2}", self.spent), t::DIM)));
        }
        if let Some((note, color)) = &self.note {
            l.push(String::new());
            for line in wrap(note, SIDE_W as usize - 2) { l.push(format!(" {}", style::fg(&line, *color))); }
        }
        l.join("\n")
    }

    fn status(&self) -> String {
        if let Some(over) = self.over { return style::bold(&style::fg(&over.text(), t::ACCENT)); }
        if let Some((_, since, _)) = &self.thinking {
            return style::fg(&format!("Thinking… {}s", since.elapsed().as_secs()), t::DIM);
        }
        if self.pos.turn == self.me {
            let check = if self.pos.in_check(self.me) { ", and you are in check" } else { "" };
            style::fg(&format!("Your move{check}"), t::OK)
        } else {
            style::fg("Their move", t::DIM)
        }
    }

    fn footer_text(&self) -> String {
        style::fg(" arrows move  ENTER pick and place  / type a move  u back  n new  c sides  f flip  o opponent  L lichess  ? help  q quit ", t::DIM)
    }

    // ---- Playing ------------------------------------------------------

    /// Take the move if it is legal, then let the opponent think. In a
    /// lichess game the move goes there, and the answer comes back on the
    /// game's stream.
    fn play(&mut self, mv: Move) {
        if let Some(live) = &self.live {
            if live.ended { return; }
            if let Err(why) = lichess::send_move(&self.cfg, &live.id, &mv.uci()) {
                self.note = Some((format!("lichess would not take {}: {why}", self.pos.san(mv)), t::ERR));
                self.picked = None;
                return;
            }
        }
        let san = self.pos.san(mv);
        self.back.push(self.pos.clone());
        self.pos = self.pos.after(mv);
        self.played.push(san);
        self.seen.push(self.pos.key());
        self.last = Some(mv);
        self.picked = None;
        self.over = self.pos.over(&self.seen);
        self.note = None;
        if self.live.is_none() && self.over.is_none() && self.pos.turn != self.me { self.start_thinking(None); }
    }

    /// Ask the opponent, on a thread of its own.
    fn start_thinking(&mut self, again: Option<String>) {
        let attempt = self.thinking.as_ref().map(|(_, _, n)| n + 1).unwrap_or(0);
        let question = opponent::question(&self.pos, &self.played, again.as_deref());
        let cfg = self.cfg.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || { let _ = tx.send(opponent::ask(&cfg, &question)); });
        self.thinking = Some((rx, Instant::now(), attempt));
    }

    /// Look for the opponent's answer, and play it when it comes.
    fn collect_move(&mut self) {
        let Some((rx, _, attempt)) = &self.thinking else { return };
        let attempt = *attempt;
        let Ok(answer) = rx.try_recv() else { return };
        self.thinking = None;
        match answer {
            Err(why) => {
                self.note = Some((format!("The opponent did not answer. {why}"), t::ERR));
                self.play_fallback();
            }
            Ok(answer) => {
                if answer.model.is_some() { self.played_by = answer.model.clone(); }
                self.spent += answer.cost.unwrap_or(0.0);
                let word = answer.text.lines().last().unwrap_or("").trim().to_string();
                match self.pos.parse_move(&word) {
                    Some(mv) => self.play(mv),
                    None if attempt < 2 => {
                        self.note = Some((format!("\"{word}\" is not a legal move here. Asking again."), t::DIM));
                        self.start_thinking(Some(word));
                    }
                    None => {
                        self.note = Some((format!("Three answers, none of them legal (\"{word}\"). Playing for it."), t::ERR));
                        self.play_fallback();
                    }
                }
            }
        }
    }

    /// When the model cannot produce a move, gambit plays a plain one so
    /// the game goes on: the move that wins the most material two plies
    /// deep. It is weak on purpose, and the panel says when it happens.
    fn play_fallback(&mut self) {
        let Some(mv) = best_plain_move(&self.pos) else { return };
        self.play(mv);
    }

    /// Take back your move and the answer to it. Not on lichess: a move
    /// played there is played.
    fn undo(&mut self) {
        if self.thinking.is_some() || self.live.is_some() { return; }
        for _ in 0..2 {
            let Some(prev) = self.back.pop() else { break };
            self.pos = prev;
            self.played.pop();
            self.seen.pop();
            if self.pos.turn == self.me { break; }
        }
        self.last = None;
        self.picked = None;
        self.over = None;
        self.note = None;
    }

    /// Play the other colour from now on, and start again.
    fn change_sides(&mut self) {
        self.cfg.side = if self.me == Color::White { "black".into() } else { "white".into() };
        let _ = config::save(&self.cfg);
        self.new_game();
        self.note = Some((format!("You play {} now.", self.cfg.side.trim()), t::OK));
    }

    fn new_game(&mut self) {
        self.pos = Position::start();
        self.seen = vec![self.pos.key()];
        self.played.clear();
        self.back.clear();
        self.last = None;
        self.picked = None;
        self.over = None;
        self.note = None;
        self.me = if self.cfg.side.trim().eq_ignore_ascii_case("black") { Color::Black } else { Color::White };
        self.flip = self.me == Color::Black;
        self.cursor = if self.me == Color::White { 4 } else { 60 };
        // On lichess the other side is a person or their own engine.
        if self.live.is_none() && self.pos.turn != self.me { self.start_thinking(None); }
    }

    // ---- Keys ---------------------------------------------------------

    /// True when it is time to stop.
    fn handle(&mut self, key: &str) -> bool {
        match key {
            "q" => return true,
            "h" | "LEFT" => self.step(-1, 0),
            "l" | "RIGHT" => self.step(1, 0),
            "j" | "DOWN" => self.step(0, -1),
            "k" | "UP" => self.step(0, 1),
            "ENTER" | " " => self.pick(),
            "ESC" => { self.picked = None; }
            "/" => self.type_move(),
            "u" => self.undo(),
            "n" => self.new_game(),
            "f" => self.flip = !self.flip,
            "c" => self.change_sides(),
            "L" => self.lichess_menu(),
            "R" => self.resign(),
            "o" => self.choose_opponent(),
            "s" => self.save_pgn(),
            "?" => self.help(),
            _ => {}
        }
        false
    }

    fn step(&mut self, dx: i32, dy: i32) {
        let (dx, dy) = if self.flip { (-dx, -dy) } else { (dx, dy) };
        let (f, r) = (chess::file_of(self.cursor) + dx, chess::rank_of(self.cursor) + dy);
        if let Some(sq) = chess::at(f, r) { self.cursor = sq; }
    }

    /// Pick up a piece, or put the picked piece on this square.
    fn pick(&mut self) {
        if self.over.is_some() || self.pos.turn != self.me { return; }
        match self.picked {
            None => {
                if self.pos.piece_at(self.cursor).is_some_and(|(c, _)| c == self.me) {
                    self.picked = Some(self.cursor);
                }
            }
            Some(from) if from == self.cursor => self.picked = None,
            Some(from) => {
                let moves: Vec<Move> = self.pos.legal_moves().into_iter()
                    .filter(|m| m.from == from && m.to == self.cursor).collect();
                match moves.len() {
                    0 => {
                        // Not a move: take this as picking another piece.
                        self.picked = self.pos.piece_at(self.cursor)
                            .filter(|(c, _)| *c == self.me).map(|_| self.cursor);
                    }
                    1 => self.play(moves[0]),
                    _ => {
                        // A pawn reaching the last rank: which piece?
                        let want = self.ask_promotion();
                        if let Some(mv) = moves.into_iter().find(|m| m.promo == Some(want)) { self.play(mv); }
                    }
                }
            }
        }
    }

    fn ask_promotion(&mut self) -> Piece {
        let answer = self.footer.ask(" Queen, Rook, Bishop or Knight? (q/r/b/n) ", "q");
        Cursor::hide();
        match answer.trim().chars().next().unwrap_or('q').to_ascii_lowercase() {
            'r' => Piece::Rook,
            'b' => Piece::Bishop,
            'n' => Piece::Knight,
            _ => Piece::Queen,
        }
    }

    fn type_move(&mut self) {
        if self.over.is_some() || self.pos.turn != self.me { return; }
        let text = self.footer.ask(" your move: ", "");
        Cursor::hide();
        self.shown[3].clear();
        if text.trim().is_empty() { return; }
        match self.pos.parse_move(&text) {
            Some(mv) => self.play(mv),
            None => self.note = Some((format!("\"{}\" is not a legal move here.", text.trim()), t::ERR)),
        }
    }

    /// Pick who answers, and which model, from a menu.
    fn choose_opponent(&mut self) {
        const KINDS: [(&str, &str); 4] = [
            ("claude", "claude -p, the command. No key needed"),
            ("anthropic", "Anthropic's API. Needs a key"),
            ("openai", "OpenAI, OpenRouter, your own server. Needs a key"),
            ("command", "a command of your own"),
        ];
        let lines: Vec<String> = KINDS.iter()
            .map(|(k, what)| format!(" {:<10} {}", k, style::fg(what, t::DIM)))
            .collect();
        let mut menu = self.popup(58, lines.len() as u16);
        menu.pane.index = KINDS.iter().position(|(k, _)| *k == self.cfg.opponent.trim()).unwrap_or(0);
        let picked = menu.modal(&lines.join("\n"));
        menu.dismiss(&mut [&mut self.header, &mut self.board_p, &mut self.side_p, &mut self.footer]);
        self.shown = Default::default();
        let Some(i) = picked else { return };
        self.cfg.opponent = KINDS[i].0.to_string();
        match KINDS[i].0 {
            "claude" => self.choose_claude_model(),
            "command" => {
                let cmd = self.footer.ask(" command: ", &self.cfg.command);
                Cursor::hide();
                self.cfg.command = cmd.trim().to_string();
            }
            _ => {
                let model = self.footer.ask(" model (empty = the usual one): ", &self.cfg.model);
                Cursor::hide();
                self.cfg.model = model.trim().to_string();
            }
        }
        self.shown = Default::default();
        self.played_by = None;
        match config::save(&self.cfg) {
            Ok(()) => self.note = Some((format!("Opponent: {}. Kept in ~/.gambit/config.yml.", opponent::describe(&self.cfg)), t::OK)),
            Err(e) => self.note = Some((format!("Could not save the settings: {e}"), t::ERR)),
        }
    }

    /// The names `claude --model` takes. Anything else can be typed.
    fn choose_claude_model(&mut self) {
        const MODELS: [(&str, &str); 5] = [
            ("", "whatever the command uses by default"),
            ("haiku", "quickest and cheapest"),
            ("sonnet", "in between"),
            ("opus", "slowest, strongest, dearest"),
            ("?", "type a name, such as claude-opus-5"),
        ];
        let lines: Vec<String> = MODELS.iter()
            .map(|(m, what)| format!(" {:<8} {}", if m.is_empty() { "default" } else { m }, style::fg(what, t::DIM)))
            .collect();
        let mut menu = self.popup(54, lines.len() as u16);
        menu.pane.index = MODELS.iter().position(|(m, _)| *m == self.cfg.model.trim()).unwrap_or(0);
        let picked = menu.modal(&lines.join("\n"));
        menu.dismiss(&mut [&mut self.header, &mut self.board_p, &mut self.side_p, &mut self.footer]);
        self.shown = Default::default();
        let Some(i) = picked else { return };
        self.cfg.model = if MODELS[i].0 == "?" {
            let typed = self.footer.ask(" model: ", &self.cfg.model);
            Cursor::hide();
            typed.trim().to_string()
        } else {
            MODELS[i].0.to_string()
        };
    }

    // ---- lichess ------------------------------------------------------

    /// Start or leave a game on lichess. The model plays no part there:
    /// lichess forbids engine help on an ordinary account.
    fn lichess_menu(&mut self) {
        if self.cfg.lichess_token.trim().is_empty() && std::env::var("LICHESS_TOKEN").is_err() {
            self.note = Some((
                "Paste a lichess token with the board:play right. Make one at lichess.org/account/oauth/token".into(),
                t::ACCENT));
            self.render();
            let token = self.footer.ask(" lichess token: ", "");
            Cursor::hide();
            self.shown = Default::default();
            if token.trim().is_empty() { return; }
            self.cfg.lichess_token = token.trim().to_string();
            let _ = config::save(&self.cfg);
        }
        let who = match lichess::whoami(&self.cfg) {
            Ok(name) => name,
            Err(why) => { self.note = Some((format!("lichess: {why}"), t::ERR)); return; }
        };
        let items = [
            " play the lichess computer".to_string(),
            " look for a game, ten minutes, casual".to_string(),
            " look for a game, ten minutes, rated".to_string(),
            " take up the game I have going".to_string(),
            " leave lichess, play the model again".to_string(),
        ];
        let mut menu = self.popup(52, items.len() as u16);
        let picked = menu.modal(&items.join("\n"));
        menu.dismiss(&mut [&mut self.header, &mut self.board_p, &mut self.side_p, &mut self.footer]);
        self.shown = Default::default();
        match picked {
            Some(0) => self.lichess_computer(&who),
            Some(1) => self.lichess_seek(&who, false),
            Some(2) => self.lichess_seek(&who, true),
            Some(3) => match lichess::ongoing(&self.cfg) {
                Ok(Some(id)) => self.lichess_join(&who, &id),
                Ok(None) => self.note = Some(("No game going on lichess right now.".into(), t::DIM)),
                Err(why) => self.note = Some((format!("lichess: {why}"), t::ERR)),
            },
            Some(4) => {
                self.live = None;
                self.lichess_events = None;
                self.new_game();
                self.note = Some(("Back to the model.".into(), t::OK));
            }
            _ => {}
        }
    }

    fn lichess_computer(&mut self, who: &str) {
        let levels: Vec<String> = (1..=8).map(|l| format!(" level {l}{}", match l {
            1 => "   a beginner", 4 => "   club strength", 8 => "   no chance", _ => "" })).collect();
        let mut menu = self.popup(40, levels.len() as u16);
        menu.pane.index = 2;
        let picked = menu.modal(&levels.join("\n"));
        menu.dismiss(&mut [&mut self.header, &mut self.board_p, &mut self.side_p, &mut self.footer]);
        self.shown = Default::default();
        let Some(i) = picked else { return };
        let white = self.me == Color::White;
        match lichess::play_computer(&self.cfg, i as u8 + 1, white) {
            Ok(id) => self.lichess_join(who, &id),
            Err(why) => self.note = Some((format!("lichess: {why}"), t::ERR)),
        }
    }

    fn lichess_seek(&mut self, who: &str, rated: bool) {
        let (tx, rx) = channel();
        if let Err(why) = lichess::watch_events(&self.cfg, tx) {
            self.note = Some((format!("lichess: {why}"), t::ERR));
            return;
        }
        self.lichess_events = Some(rx);
        match lichess::seek(&self.cfg, rated) {
            Ok(()) => self.note = Some((format!("Looking for a {} game as {who}. It starts here when somebody takes it.",
                if rated { "rated" } else { "casual" }), t::OK)),
            Err(why) => self.note = Some((format!("lichess: {why}"), t::ERR)),
        }
    }

    /// Follow a game from now on: the board becomes that game.
    fn lichess_join(&mut self, who: &str, id: &str) {
        let (tx, rx) = channel();
        if let Err(why) = lichess::watch_game(&self.cfg, id, who, tx) {
            self.note = Some((format!("lichess: {why}"), t::ERR));
            return;
        }
        self.live = Some(Live {
            id: id.to_string(),
            opponent: "lichess".into(),
            events: rx,
            clocks: (600, 600),
            since: Instant::now(),
            ended: false,
        });
        self.new_game();
        self.thinking = None;
        self.note = Some((format!("Game {id} on lichess. Your moves only: engine help is against their rules."), t::ACCENT));
    }

    /// Take in whatever the lichess streams have said.
    fn collect_lichess(&mut self) {
        // A game that started while looking for one.
        let started = self.lichess_events.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(lichess::Event::Started { id, .. }) = started {
            let who = lichess::whoami(&self.cfg).unwrap_or_default();
            self.lichess_join(&who, &id);
        }
        let mut fresh = Vec::new();
        if let Some(live) = self.live.as_ref() {
            while let Ok(event) = live.events.try_recv() { fresh.push(event); }
        }
        for event in fresh {
            match event {
                lichess::Event::Started { white, opponent, .. } => {
                    if let Some(live) = self.live.as_mut() { live.opponent = opponent; }
                    self.me = if white { Color::White } else { Color::Black };
                    self.flip = !white;
                    self.cursor = if white { 4 } else { 60 };
                }
                lichess::Event::Moves { moves, white_time, black_time } => {
                    if let Some(live) = self.live.as_mut() {
                        live.clocks = (white_time, black_time);
                        live.since = Instant::now();
                    }
                    self.replay(&moves);
                }
                lichess::Event::Ended { status, winner } => {
                    if let Some(live) = self.live.as_mut() { live.ended = true; }

                    let says = match (status.as_str(), winner.as_deref()) {
                        ("mate", Some(w)) => format!("Checkmate: {w} wins"),
                        ("resign", Some(w)) => format!("Resigned: {w} wins"),
                        ("outoftime", Some(w)) => format!("Out of time: {w} wins"),
                        ("aborted", _) => "The game was aborted".to_string(),
                        (other, Some(w)) => format!("{other}: {w} wins"),
                        (other, None) => format!("The game ended in a draw ({other})"),
                    };
                    self.note = Some((says, t::ACCENT));
                }
                lichess::Event::Trouble(why) => self.note = Some((format!("lichess: {why}"), t::ERR)),
            }
        }
    }

    /// Set the board to the moves lichess has recorded.
    fn replay(&mut self, moves: &[String]) {
        let mut pos = Position::start();
        let mut seen = vec![pos.key()];
        let mut played = Vec::new();
        let mut last = None;
        for uci in moves {
            let Some(mv) = pos.parse_move(uci) else { break };
            played.push(pos.san(mv));
            pos = pos.after(mv);
            seen.push(pos.key());
            last = Some(mv);
        }
        self.pos = pos;
        self.seen = seen;
        self.played = played;
        self.back.clear();
        self.last = last;
        self.picked = None;
        self.over = self.pos.over(&self.seen);
    }

    fn resign(&mut self) {
        let Some(live) = &self.live else { return };
        if live.ended { return; }
        let id = live.id.clone();
        match lichess::resign(&self.cfg, &id) {
            Ok(()) => self.note = Some(("You resigned.".into(), t::DIM)),
            Err(why) => self.note = Some((format!("lichess: {why}"), t::ERR)),
        }
    }

    /// A bordered popup in the lower right corner, clear of the board.
    fn popup(&self, w: u16, h: u16) -> Popup {
        let x = self.cols.saturating_sub(w + 2).max(2);
        let y = self.rows.saturating_sub(h + 2).max(2);
        Popup::new(x, y, w, h, t::FG as u16, 234)
    }

    /// Write the game where other chess programs can read it.
    fn save_pgn(&mut self) {
        let path = config::dir().join("game.pgn");
        let result = match self.over {
            Some(Over::Checkmate(Color::White)) => "1-0",
            Some(Over::Checkmate(Color::Black)) => "0-1",
            Some(_) => "1/2-1/2",
            None => "*",
        };
        let (white, black) = if self.me == Color::White { ("You".to_string(), opponent::describe(&self.cfg)) } else { (opponent::describe(&self.cfg), "You".to_string()) };
        let mut pgn = format!("[Event \"gambit\"]\n[White \"{white}\"]\n[Black \"{black}\"]\n[Result \"{result}\"]\n\n");
        for (i, mv) in self.played.iter().enumerate() {
            if i % 2 == 0 { pgn.push_str(&format!("{}. ", i / 2 + 1)); }
            pgn.push_str(mv);
            pgn.push(' ');
        }
        pgn.push_str(result);
        pgn.push('\n');
        let _ = std::fs::create_dir_all(config::dir());
        self.note = Some(match std::fs::write(&path, pgn) {
            Ok(()) => (format!("Game written to {}", path.display()), t::OK),
            Err(e) => (format!("Could not write the game: {e}"), t::ERR),
        });
    }

    fn help(&mut self) {
        let k = |s: &str| style::fg(s, t::ACCENT);
        let lines = [
            format!(" {:<14} move the cursor", k("arrows / hjkl")),
            format!(" {:<14} pick a piece up, put it down", k("ENTER")),
            format!(" {:<14} let the piece go", k("ESC")),
            format!(" {:<14} type a move instead (e4, Nf3, e2e4)", k("/")),
            format!(" {:<14} take back your move and the answer", k("u")),
            format!(" {:<14} a new game", k("n")),
            format!(" {:<14} play the other colour, from a new game", k("c")),
            format!(" {:<14} turn the board around", k("f")),
            format!(" {:<14} pick the opponent and the model", k("o")),
            format!(" {:<14} play on lichess, or leave it", k("L")),
            format!(" {:<14} resign a lichess game", k("R")),
            format!(" {:<14} write the game to ~/.gambit/game.pgn", k("s")),
            format!(" {:<14} quit", k("q")),
            String::new(),
            format!(" {}", style::fg("The opponent is told the position and every legal", t::DIM)),
            format!(" {}", style::fg("move. An answer that is not one of them is asked", t::DIM)),
            format!(" {}", style::fg("again twice, then gambit plays a plain move.", t::DIM)),
            String::new(),
            format!(" {}", style::fg("ESC, q or ENTER closes this.", t::DIM)),
        ];
        let mut p = self.popup(56, lines.len() as u16);
        p.view(&lines.join("\n"));
        p.dismiss(&mut [&mut self.header, &mut self.board_p, &mut self.side_p, &mut self.footer]);
        self.shown = Default::default();
    }

    fn resized(&mut self) -> bool {
        let (cols, rows) = Crust::terminal_size();
        if cols == self.cols && rows == self.rows { return false; }
        self.cols = cols;
        self.rows = rows;
        self.sq = square_size(cols, rows);
        self.header = Pane::new(1, 1, cols, 1, t::FG as u16, t::BAR as u16);
        self.board_p = Pane::new(1, 2, cols.saturating_sub(SIDE_W), rows.saturating_sub(2), t::FG as u16, 0);
        self.side_p = Pane::new(cols.saturating_sub(SIDE_W) + 1, 2, SIDE_W, rows.saturating_sub(2), t::FG as u16, 0);
        self.footer = Pane::new(1, rows, cols, 1, t::FG as u16, t::BAR as u16);
        for p in [&mut self.header, &mut self.board_p, &mut self.side_p, &mut self.footer] {
            p.wrap = false;
            p.scroll = false;
        }
        Crust::clear_screen();
        self.shown = Default::default();
        true
    }
}

/// White gets the hollow pieces, black the filled ones. Two shapes beat
/// one shape in two colours when the squares are small.
fn piece_glyph(p: Piece, c: Color) -> char {
    match (c, p) {
        (Color::White, Piece::King) => '♔', (Color::White, Piece::Queen) => '♕',
        (Color::White, Piece::Rook) => '♖', (Color::White, Piece::Bishop) => '♗',
        (Color::White, Piece::Knight) => '♘', (Color::White, Piece::Pawn) => '♙',
        (Color::Black, Piece::King) => '♚', (Color::Black, Piece::Queen) => '♛',
        (Color::Black, Piece::Rook) => '♜', (Color::Black, Piece::Bishop) => '♝',
        (Color::Black, Piece::Knight) => '♞', (Color::Black, Piece::Pawn) => '♟',
    }
}

/// White in capitals, black in small letters, as chess diagrams write them.
/// The pieces `color` has lost. A pawn that turned into a queen still
/// stands on the board, so a count never goes below nothing.
fn lost(pos: &Position, color: Color) -> Vec<Piece> {
    const FULL: [(Piece, usize); 5] = [
        (Piece::Queen, 1), (Piece::Rook, 2), (Piece::Bishop, 2), (Piece::Knight, 2), (Piece::Pawn, 8),
    ];
    let mut out = Vec::new();
    for (piece, start) in FULL {
        let mut have = 0;
        for sq in 0..64u8 {
            if pos.piece_at(sq) == Some((color, piece)) { have += 1; }
        }
        for _ in 0..start.saturating_sub(have) { out.push(piece); }
    }
    out
}

/// Put `text` on line `row`, starting at column `at`.
fn put(lines: &mut [String], row: usize, at: usize, text: &str) {
    let Some(line) = lines.get_mut(row) else { return };
    let pad = at.saturating_sub(crust::display_width(line));
    line.push_str(&" ".repeat(pad));
    line.push_str(text);
}

/// What a pile of taken pieces is worth, the way players count.
fn points(pieces: &[Piece]) -> i32 {
    pieces.iter().map(|p| match p {
        Piece::Pawn => 1, Piece::Knight | Piece::Bishop => 3,
        Piece::Rook => 5, Piece::Queen => 9, Piece::King => 0,
    }).sum()
}

fn piece_letter(p: Piece, c: Color) -> char {
    let l = if p == Piece::Pawn { 'P' } else { p.letter() };
    if c == Color::White { l } else { l.to_ascii_lowercase() }
}

/// The move that leaves the most material after the best answer to it.
/// Two plies of counting, no more: this only stands in for a model that
/// will not produce a legal move.
fn best_plain_move(pos: &Position) -> Option<Move> {
    let mut best: Option<(i32, Move)> = None;
    for mv in pos.legal_moves() {
        let after = pos.after(mv);
        let replies = after.legal_moves();
        // After the answer it is our turn again, so `material` already
        // counts from our side. They pick the answer we like least.
        let score = if replies.is_empty() {
            if after.in_check(after.turn) { 100_000 } else { 0 }
        } else {
            replies.iter().map(|r| after.after(*r).material()).min().unwrap_or(0)
        };
        if best.is_none_or(|(b, _)| score > b) { best = Some((score, mv)); }
    }
    best.map(|(_, mv)| mv)
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() { line.push(' '); }
        line.push_str(word);
    }
    if !line.is_empty() { out.push(line); }
    out
}

fn main() {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--version" | "-V" | "-v" => { println!("gambit {VERSION}"); return; }
            "--help" | "-h" => {
                println!("gambit {VERSION} — chess against a language model (Fe2O3 suite)");
                println!();
                println!("Usage: gambit");
                println!();
                println!("You play with the cursor keys, or type moves like e4 and Nf3.");
                println!("The opponent is the `claude` command by default. Press o inside");
                println!("to use an API key instead (Anthropic, OpenAI or any OpenAI-shaped");
                println!("service), or a command of your own. Settings: ~/.gambit/config.yml");
                return;
            }
            _ => {}
        }
    }
    if unsafe { libc::isatty(0) } == 0 {
        eprintln!("gambit: stdin is not a terminal. Run it in one, or see gambit --help.");
        std::process::exit(1);
    }

    Crust::init();
    Crust::set_app_identity("Gambit");
    Crust::clear_screen();
    let mut app = App::new(config::load());
    if app.pos.turn != app.me { app.start_thinking(None); }
    app.render();
    loop {
        // One second of waiting at a time: enough for the thinking clock,
        // and nothing at all to do between keys.
        if let Some(key) = Input::getchr(Some(1)) {
            if app.handle(&key) { break; }
        }
        app.collect_move();
        app.collect_lichess();
        if app.resized() { /* panes remade */ }
        app.render();
    }
    Cursor::show();
    Crust::cleanup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plain_player_takes_a_free_queen_and_mates_when_it_can() {
        let free_queen = Position::from_fen("4k3/8/8/3q4/4P3/8/8/4K3 w - - 0 1").unwrap();
        let mv = best_plain_move(&free_queen).unwrap();
        assert_eq!(free_queen.san(mv), "exd5");
        let mate_in_one = Position::from_fen("6k1/5ppp/8/8/8/8/8/R3K2R w KQ - 0 1").unwrap();
        let mv = best_plain_move(&mate_in_one).unwrap();
        assert_eq!(mate_in_one.san(mv), "Ra8#");
    }

    #[test]
    fn the_board_grows_with_the_window() {
        assert_eq!(square_size(120, 40), (8, 4));
        assert_eq!(square_size(100, 30), (6, 3));
        assert_eq!(square_size(80, 24), (4, 2));
        assert_eq!(square_size(60, 12), (2, 1));
    }

    #[test]
    fn white_and_black_pieces_look_different() {
        assert_eq!(piece_glyph(Piece::Knight, Color::White), '♘');
        assert_eq!(piece_glyph(Piece::Knight, Color::Black), '♞');
        assert_eq!(piece_letter(Piece::Pawn, Color::White), 'P');
        assert_eq!(piece_letter(Piece::Knight, Color::Black), 'n');
    }

    #[test]
    fn the_pieces_missing_from_the_board_are_the_ones_taken() {
        let start = Position::start();
        assert!(lost(&start, Color::White).is_empty());
        // White is a queen and a pawn down; Black has lost a knight.
        let p = Position::from_fen("rnbqkb1r/pppppppp/8/8/8/8/PPPPPPP1/RNB1KBNR w KQkq - 0 1").unwrap();
        assert_eq!(lost(&p, Color::White), [Piece::Queen, Piece::Pawn]);
        assert_eq!(lost(&p, Color::Black), [Piece::Knight]);
        // A pawn that became a second queen is not a queen taken.
        let promoted = Position::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPP1/RNBQKBNQ w KQkq - 0 1").unwrap();
        assert_eq!(lost(&promoted, Color::White), [Piece::Rook, Piece::Pawn]);
    }

    #[test]
    fn taken_pieces_are_counted_the_way_players_count_them() {
        assert_eq!(points(&[Piece::Queen, Piece::Pawn]), 10);
        assert_eq!(points(&[Piece::Knight, Piece::Bishop, Piece::Rook]), 11);
        assert_eq!(points(&[]), 0);
    }

    #[test]
    fn text_wraps_to_the_panel_width() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert!(wrap("", 10).is_empty());
    }
}

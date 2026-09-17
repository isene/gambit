//! gambit — chess against a language model, in the terminal. Part of Fe₂O₃.
//!
//! The rules live in `chess.rs`, the opponent in `opponent.rs`. This file
//! draws the board and takes your moves. While the model thinks, its call
//! runs on its own thread, so the board still answers keys; the loop wakes
//! once a second for the clock and sleeps between.

mod chess;
mod config;
mod opponent;

use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use chess::{Color, Move, Over, Piece, Position, Square};
use config::Config;
use crust::cursor::Cursor;
use crust::{style, Crust, Input, Pane};

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
            played_by: None, spent: 0.0,
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
        let who = match &self.played_by {
            Some(id) => format!("{} · {}", opponent::describe(&self.cfg), opponent::pretty_model(id)),
            None => opponent::describe(&self.cfg),
        };
        let right = format!("{}  v{} ", style::fg(&who, t::OK), VERSION);
        let pad = (self.cols as usize).saturating_sub(crust::display_width(&left) + crust::display_width(&right));
        format!("{left}{}{right}", " ".repeat(pad))
    }

    /// The board, eight squares by eight, with the rank numbers down the
    /// left and the file letters underneath.
    fn board_text(&self) -> String {
        let (sq_w, sq_h) = self.sq;
        let (mid_col, mid_row) = (sq_w / 2, sq_h / 2);
        let mut lines: Vec<String> = vec![String::new()];
        let check = self.pos.in_check(self.pos.turn);
        for row in 0..8 {
            let rank = if self.flip { row } else { 7 - row };
            for sub in 0..sq_h {
                let mut line = String::from("  ");
                let label = if sub == mid_row { format!("{} ", rank + 1) } else { "  ".to_string() };
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
                    let mark = match (here, sq_h >= 3 && sub == mid_row + 1) {
                        (Some((c, p)), false) => piece_glyph(p, c).to_string(),
                        (Some((c, p)), true) => piece_letter(p, c).to_string(),
                        (None, _) => String::new(),
                    };
                    let cell = if mark.is_empty() || (sub != mid_row && !(sq_h >= 3 && sub == mid_row + 1)) {
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
        let files: String = (0..8).map(|col| {
            let file = if self.flip { 7 - col } else { col };
            format!("{:^width$}", (b'a' + file as u8) as char, width = sq_w)
        }).collect();
        lines.push(format!("     {}", style::fg(&files, t::DIM)));
        lines.join("\n")
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
        style::fg(" arrows move  ENTER pick and place  / type a move  u back  n new  f flip  o opponent  ? help  q quit ", t::DIM)
    }

    // ---- Playing ------------------------------------------------------

    /// Take the move if it is legal, then let the opponent think.
    fn play(&mut self, mv: Move) {
        let san = self.pos.san(mv);
        self.back.push(self.pos.clone());
        self.pos = self.pos.after(mv);
        self.played.push(san);
        self.seen.push(self.pos.key());
        self.last = Some(mv);
        self.picked = None;
        self.over = self.pos.over(&self.seen);
        self.note = None;
        if self.over.is_none() && self.pos.turn != self.me { self.start_thinking(None); }
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

    /// Take back your move and the answer to it.
    fn undo(&mut self) {
        if self.thinking.is_some() { return; }
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
        if self.pos.turn != self.me { self.start_thinking(None); }
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

    fn choose_opponent(&mut self) {
        let kind = self.footer.ask(" opponent (claude / anthropic / openai / command): ", &self.cfg.opponent);
        Cursor::hide();
        let kind = kind.trim().to_string();
        if kind.is_empty() { return; }
        self.cfg.opponent = kind;
        let model = self.footer.ask(" model (empty = the tool's own default): ", &self.cfg.model);
        Cursor::hide();
        self.cfg.model = model.trim().to_string();
        if self.cfg.opponent == "command" {
            let cmd = self.footer.ask(" command: ", &self.cfg.command);
            Cursor::hide();
            self.cfg.command = cmd.trim().to_string();
        }
        self.shown = Default::default();
        match config::save(&self.cfg) {
            Ok(()) => self.note = Some((format!("Opponent: {}. Kept in ~/.gambit/config.yml.", opponent::describe(&self.cfg)), t::OK)),
            Err(e) => self.note = Some((format!("Could not save the settings: {e}"), t::ERR)),
        }
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
        let mut p = Pane::new(4, 3, 60, 20, t::FG as u16, 234);
        p.wrap = false;
        p.scroll = false;
        let k = |s: &str| style::fg(s, t::ACCENT);
        let lines = [
            format!("  {}", style::bold(&style::fg("gambit — keys", t::ACCENT))),
            String::new(),
            format!("  {:<14} move the cursor", k("arrows / hjkl")),
            format!("  {:<14} pick a piece up, put it down", k("ENTER")),
            format!("  {:<14} let the piece go", k("ESC")),
            format!("  {:<14} type a move instead (e4, Nf3, e2e4)", k("/")),
            format!("  {:<14} take back your move and the answer", k("u")),
            format!("  {:<14} a new game", k("n")),
            format!("  {:<14} turn the board around", k("f")),
            format!("  {:<14} pick the opponent and the model", k("o")),
            format!("  {:<14} write the game to ~/.gambit/game.pgn", k("s")),
            format!("  {:<14} quit", k("q")),
            String::new(),
            format!("  {}", style::fg("The opponent gets the position and every legal", t::DIM)),
            format!("  {}", style::fg("move, and answers with one of them. An illegal", t::DIM)),
            format!("  {}", style::fg("answer is asked again twice.", t::DIM)),
            String::new(),
            format!("  {}", style::fg("Any key closes this.", t::DIM)),
        ];
        p.set_text(&lines.join("\n"));
        p.full_refresh();
        let _ = Input::getchr(None);
        Crust::clear_screen();
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
    fn text_wraps_to_the_panel_width() {
        assert_eq!(wrap("one two three four", 9), ["one two", "three", "four"]);
        assert!(wrap("", 10).is_empty());
    }
}

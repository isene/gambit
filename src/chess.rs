//! The rules of chess: the pieces, the moves they may make, and the
//! words for them. A move is written the way players write it (Nf3,
//! exd5, O-O), and a position the way computers pass it around (FEN).
//!
//! Positions are copied rather than undone. A whole position is 70 bytes,
//! so copying one costs less than keeping track of how to take a move
//! back, and nothing can go stale.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color { White, Black }

impl Color {
    pub fn other(self) -> Color {
        if self == Color::White { Color::Black } else { Color::White }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Piece { Pawn, Knight, Bishop, Rook, Queen, King }

impl Piece {
    /// The letter in a written move; a pawn has none.
    pub fn letter(self) -> char {
        match self { Piece::Pawn => ' ', Piece::Knight => 'N', Piece::Bishop => 'B', Piece::Rook => 'R', Piece::Queen => 'Q', Piece::King => 'K' }
    }

    pub fn from_letter(c: char) -> Option<Piece> {
        Some(match c.to_ascii_uppercase() {
            'P' => Piece::Pawn, 'N' => Piece::Knight, 'B' => Piece::Bishop,
            'R' => Piece::Rook, 'Q' => Piece::Queen, 'K' => Piece::King,
            _ => return None,
        })
    }

    /// What the piece is worth in pawns, for the simple fallback player.
    pub fn value(self) -> i32 {
        match self { Piece::Pawn => 100, Piece::Knight => 320, Piece::Bishop => 330, Piece::Rook => 500, Piece::Queen => 900, Piece::King => 20_000 }
    }
}

/// A square, 0 = a1 up to 63 = h8.
pub type Square = u8;

pub fn file_of(sq: Square) -> i32 { (sq % 8) as i32 }
pub fn rank_of(sq: Square) -> i32 { (sq / 8) as i32 }

pub fn at(file: i32, rank: i32) -> Option<Square> {
    (0..8).contains(&file).then_some(())?;
    (0..8).contains(&rank).then_some(())?;
    Some((rank * 8 + file) as Square)
}

/// "e4" for square 28.
pub fn name(sq: Square) -> String {
    format!("{}{}", (b'a' + (sq % 8)) as char, sq / 8 + 1)
}

pub fn parse_square(s: &str) -> Option<Square> {
    let mut c = s.chars();
    let file = c.next()?.to_ascii_lowercase() as i32 - 'a' as i32;
    let rank = c.next()?.to_digit(10)? as i32 - 1;
    at(file, rank)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Move {
    pub from: Square,
    pub to: Square,
    /// What a pawn turns into on the last rank.
    pub promo: Option<Piece>,
}

impl Move {
    /// The plain form computers use: e2e4, e7e8q.
    pub fn uci(&self) -> String {
        let mut s = format!("{}{}", name(self.from), name(self.to));
        if let Some(p) = self.promo { s.push(p.letter().to_ascii_lowercase()); }
        s
    }
}

/// How a game ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Over {
    Checkmate(Color),
    Stalemate,
    FiftyMoves,
    Repetition,
    NotEnoughPieces,
}

impl Over {
    pub fn text(self) -> String {
        match self {
            Over::Checkmate(w) => format!("Checkmate: {} wins", if w == Color::White { "White" } else { "Black" }),
            Over::Stalemate => "Stalemate: a draw".into(),
            Over::FiftyMoves => "Fifty moves without a capture or a pawn: a draw".into(),
            Over::Repetition => "The same position three times: a draw".into(),
            Over::NotEnoughPieces => "Too few pieces left to mate: a draw".into(),
        }
    }
}

/// Castling rights, in the order white short, white long, black short, black long.
const WK: usize = 0;
const WQ: usize = 1;
const BK: usize = 2;
const BQ: usize = 3;

#[derive(Clone, PartialEq, Eq)]
pub struct Position {
    pub board: [Option<(Color, Piece)>; 64],
    pub turn: Color,
    pub castling: [bool; 4],
    /// The square a pawn just skipped over, which may be captured on.
    pub ep: Option<Square>,
    /// Half moves since the last capture or pawn move.
    pub halfmove: u32,
    pub fullmove: u32,
}

impl Default for Position {
    fn default() -> Self { Position::start() }
}

impl Position {
    pub fn start() -> Position {
        Position::from_fen("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").expect("the starting position")
    }

    pub fn from_fen(fen: &str) -> Option<Position> {
        let mut parts = fen.split_whitespace();
        let rows = parts.next()?;
        let mut board = [None; 64];
        let mut rank = 7;
        let mut file = 0;
        for c in rows.chars() {
            match c {
                '/' => { rank -= 1; file = 0; }
                '1'..='8' => file += c.to_digit(10)? as i32,
                _ => {
                    let color = if c.is_ascii_uppercase() { Color::White } else { Color::Black };
                    let piece = Piece::from_letter(c)?;
                    board[at(file, rank)? as usize] = Some((color, piece));
                    file += 1;
                }
            }
        }
        let turn = if parts.next()? == "b" { Color::Black } else { Color::White };
        let rights = parts.next().unwrap_or("-");
        let castling = [rights.contains('K'), rights.contains('Q'), rights.contains('k'), rights.contains('q')];
        let ep = parts.next().and_then(parse_square);
        let halfmove = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let fullmove = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
        Some(Position { board, turn, castling, ep, halfmove, fullmove })
    }

    pub fn to_fen(&self) -> String {
        let mut out = String::new();
        for rank in (0..8).rev() {
            let mut empty = 0;
            for file in 0..8 {
                match self.board[at(file, rank).unwrap() as usize] {
                    None => empty += 1,
                    Some((color, piece)) => {
                        if empty > 0 { out.push_str(&empty.to_string()); empty = 0; }
                        let l = if piece == Piece::Pawn { 'P' } else { piece.letter() };
                        out.push(if color == Color::White { l } else { l.to_ascii_lowercase() });
                    }
                }
            }
            if empty > 0 { out.push_str(&empty.to_string()); }
            if rank > 0 { out.push('/'); }
        }
        let mut rights: String = [(WK, 'K'), (WQ, 'Q'), (BK, 'k'), (BQ, 'q')].iter()
            .filter(|(i, _)| self.castling[*i]).map(|(_, c)| *c).collect();
        if rights.is_empty() { rights.push('-'); }
        format!("{} {} {} {} {} {}", out,
            if self.turn == Color::White { "w" } else { "b" }, rights,
            self.ep.map(name).unwrap_or_else(|| "-".into()),
            self.halfmove, self.fullmove)
    }

    /// The part of a position that decides a repetition: the pieces, whose
    /// turn it is, the castling rights and the en passant square.
    pub fn key(&self) -> String {
        self.to_fen().split_whitespace().take(4).collect::<Vec<_>>().join(" ")
    }

    pub fn piece_at(&self, sq: Square) -> Option<(Color, Piece)> { self.board[sq as usize] }

    fn king(&self, color: Color) -> Option<Square> {
        (0..64).find(|&s| self.board[s as usize] == Some((color, Piece::King)))
    }

    /// Does `by` attack `sq`? Used for check and for castling.
    pub fn attacked(&self, sq: Square, by: Color) -> bool {
        let (f, r) = (file_of(sq), rank_of(sq));
        // Pawns attack toward their own side's far rank.
        let dir = if by == Color::White { -1 } else { 1 };
        for df in [-1, 1] {
            if let Some(s) = at(f + df, r + dir) {
                if self.board[s as usize] == Some((by, Piece::Pawn)) { return true; }
            }
        }
        for (df, dr) in [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)] {
            if let Some(s) = at(f + df, r + dr) {
                if self.board[s as usize] == Some((by, Piece::Knight)) { return true; }
            }
        }
        for df in -1..=1 {
            for dr in -1..=1 {
                if (df, dr) == (0, 0) { continue; }
                if let Some(s) = at(f + df, r + dr) {
                    if self.board[s as usize] == Some((by, Piece::King)) { return true; }
                }
            }
        }
        let rays: [((i32, i32), [Piece; 2]); 8] = [
            ((0, 1), [Piece::Rook, Piece::Queen]), ((0, -1), [Piece::Rook, Piece::Queen]),
            ((1, 0), [Piece::Rook, Piece::Queen]), ((-1, 0), [Piece::Rook, Piece::Queen]),
            ((1, 1), [Piece::Bishop, Piece::Queen]), ((1, -1), [Piece::Bishop, Piece::Queen]),
            ((-1, 1), [Piece::Bishop, Piece::Queen]), ((-1, -1), [Piece::Bishop, Piece::Queen]),
        ];
        for ((df, dr), kinds) in rays {
            let (mut nf, mut nr) = (f + df, r + dr);
            while let Some(s) = at(nf, nr) {
                if let Some((c, p)) = self.board[s as usize] {
                    if c == by && kinds.contains(&p) { return true; }
                    break;
                }
                nf += df;
                nr += dr;
            }
        }
        false
    }

    pub fn in_check(&self, color: Color) -> bool {
        self.king(color).is_some_and(|k| self.attacked(k, color.other()))
    }

    /// Every move that is allowed here.
    pub fn legal_moves(&self) -> Vec<Move> {
        self.pseudo_moves().into_iter()
            .filter(|m| !self.after(*m).in_check(self.turn))
            .collect()
    }

    /// Moves that follow how the pieces walk, before checking the king.
    fn pseudo_moves(&self) -> Vec<Move> {
        let mut out = Vec::with_capacity(48);
        let me = self.turn;
        for from in 0..64u8 {
            let Some((color, piece)) = self.board[from as usize] else { continue };
            if color != me { continue; }
            let (f, r) = (file_of(from), rank_of(from));
            match piece {
                Piece::Pawn => {
                    let dir = if me == Color::White { 1 } else { -1 };
                    let start_rank = if me == Color::White { 1 } else { 6 };
                    if let Some(one) = at(f, r + dir) {
                        if self.board[one as usize].is_none() {
                            self.add_pawn(&mut out, from, one, dir);
                            if r == start_rank {
                                if let Some(two) = at(f, r + 2 * dir) {
                                    if self.board[two as usize].is_none() { out.push(Move { from, to: two, promo: None }); }
                                }
                            }
                        }
                    }
                    for df in [-1, 1] {
                        let Some(to) = at(f + df, r + dir) else { continue };
                        let takes = self.board[to as usize].is_some_and(|(c, _)| c != me);
                        if takes || Some(to) == self.ep { self.add_pawn(&mut out, from, to, dir); }
                    }
                }
                Piece::Knight => {
                    for (df, dr) in [(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)] {
                        self.add_step(&mut out, from, f + df, r + dr, me);
                    }
                }
                Piece::King => {
                    for df in -1..=1 {
                        for dr in -1..=1 {
                            if (df, dr) != (0, 0) { self.add_step(&mut out, from, f + df, r + dr, me); }
                        }
                    }
                    self.add_castling(&mut out, me);
                }
                _ => {
                    let dirs: &[(i32, i32)] = match piece {
                        Piece::Rook => &[(0, 1), (0, -1), (1, 0), (-1, 0)],
                        Piece::Bishop => &[(1, 1), (1, -1), (-1, 1), (-1, -1)],
                        _ => &[(0, 1), (0, -1), (1, 0), (-1, 0), (1, 1), (1, -1), (-1, 1), (-1, -1)],
                    };
                    for (df, dr) in dirs {
                        let (mut nf, mut nr) = (f + df, r + dr);
                        while let Some(to) = at(nf, nr) {
                            match self.board[to as usize] {
                                None => out.push(Move { from, to, promo: None }),
                                Some((c, _)) => {
                                    if c != me { out.push(Move { from, to, promo: None }); }
                                    break;
                                }
                            }
                            nf += df;
                            nr += dr;
                        }
                    }
                }
            }
        }
        out
    }

    fn add_step(&self, out: &mut Vec<Move>, from: Square, file: i32, rank: i32, me: Color) {
        let Some(to) = at(file, rank) else { return };
        if self.board[to as usize].is_some_and(|(c, _)| c == me) { return; }
        out.push(Move { from, to, promo: None });
    }

    /// A pawn move, turned into four moves on the last rank.
    fn add_pawn(&self, out: &mut Vec<Move>, from: Square, to: Square, dir: i32) {
        let last = if dir == 1 { 7 } else { 0 };
        if rank_of(to) == last {
            for p in [Piece::Queen, Piece::Rook, Piece::Bishop, Piece::Knight] {
                out.push(Move { from, to, promo: Some(p) });
            }
        } else {
            out.push(Move { from, to, promo: None });
        }
    }

    fn add_castling(&self, out: &mut Vec<Move>, me: Color) {
        let (home, short, long) = if me == Color::White { (0, WK, WQ) } else { (56, BK, BQ) };
        let king = home + 4;
        if self.board[king as usize] != Some((me, Piece::King)) { return; }
        if self.in_check(me) { return; }
        let empty = |s: u8| self.board[s as usize].is_none();
        let safe = |s: u8| !self.attacked(s, me.other());
        if self.castling[short] && self.board[(home + 7) as usize] == Some((me, Piece::Rook))
            && empty(home + 5) && empty(home + 6) && safe(home + 5) && safe(home + 6) {
            out.push(Move { from: king, to: home + 6, promo: None });
        }
        if self.castling[long] && self.board[(home + 0) as usize] == Some((me, Piece::Rook))
            && empty(home + 1) && empty(home + 2) && empty(home + 3) && safe(home + 2) && safe(home + 3) {
            out.push(Move { from: king, to: home + 2, promo: None });
        }
    }

    /// The position this move leads to. The move must be one the pieces
    /// can make; whether it leaves the king in check is the caller's worry.
    pub fn after(&self, mv: Move) -> Position {
        let mut p = self.clone();
        let Some((color, piece)) = self.board[mv.from as usize] else { return p };
        let takes = self.board[mv.to as usize].is_some();
        p.board[mv.from as usize] = None;
        p.board[mv.to as usize] = Some((color, mv.promo.unwrap_or(piece)));
        // En passant: the pawn taken stands beside the square moved to.
        if piece == Piece::Pawn && Some(mv.to) == self.ep && !takes {
            let back = if color == Color::White { mv.to - 8 } else { mv.to + 8 };
            p.board[back as usize] = None;
        }
        // Castling moves the rook along with the king.
        if piece == Piece::King && (file_of(mv.from) - file_of(mv.to)).abs() == 2 {
            let home = if color == Color::White { 0 } else { 56 };
            let (rook_from, rook_to) = if file_of(mv.to) == 6 { (home + 7, home + 5) } else { (home, home + 3) };
            p.board[rook_to as usize] = p.board[rook_from as usize].take();
        }
        p.ep = if piece == Piece::Pawn && (rank_of(mv.from) - rank_of(mv.to)).abs() == 2 {
            at(file_of(mv.from), (rank_of(mv.from) + rank_of(mv.to)) / 2)
        } else { None };
        // A king or rook that has moved, or a rook that was taken, loses the right.
        if piece == Piece::King {
            if color == Color::White { p.castling[WK] = false; p.castling[WQ] = false; }
            else { p.castling[BK] = false; p.castling[BQ] = false; }
        }
        for (sq, right) in [(0u8, WQ), (7, WK), (56, BQ), (63, BK)] {
            if mv.from == sq || mv.to == sq { p.castling[right] = false; }
        }
        p.halfmove = if takes || piece == Piece::Pawn { 0 } else { self.halfmove + 1 };
        if color == Color::Black { p.fullmove += 1; }
        p.turn = color.other();
        p
    }

    /// The move as players write it: Nf3, exd5, O-O, e8=Q+, Qxh7#.
    pub fn san(&self, mv: Move) -> String {
        let Some((_, piece)) = self.board[mv.from as usize] else { return mv.uci() };
        let takes = self.board[mv.to as usize].is_some()
            || (piece == Piece::Pawn && Some(mv.to) == self.ep);
        let mut s = if piece == Piece::King && (file_of(mv.from) - file_of(mv.to)).abs() == 2 {
            if file_of(mv.to) == 6 { "O-O".to_string() } else { "O-O-O".to_string() }
        } else if piece == Piece::Pawn {
            let mut s = String::new();
            if takes { s.push((b'a' + (mv.from % 8)) as char); s.push('x'); }
            s.push_str(&name(mv.to));
            if let Some(p) = mv.promo { s.push('='); s.push(p.letter()); }
            s
        } else {
            let mut s = piece.letter().to_string();
            // Say which piece when more than one of them could go there.
            let rivals: Vec<Move> = self.legal_moves().into_iter()
                .filter(|o| o.to == mv.to && o.from != mv.from
                    && self.board[o.from as usize] == Some((self.turn, piece)))
                .collect();
            if !rivals.is_empty() {
                let same_file = rivals.iter().any(|o| file_of(o.from) == file_of(mv.from));
                let same_rank = rivals.iter().any(|o| rank_of(o.from) == rank_of(mv.from));
                if !same_file { s.push((b'a' + (mv.from % 8)) as char); }
                else if !same_rank { s.push_str(&(mv.from / 8 + 1).to_string()); }
                else { s.push_str(&name(mv.from)); }
            }
            if takes { s.push('x'); }
            s.push_str(&name(mv.to));
            s
        };
        let next = self.after(mv);
        if next.in_check(next.turn) {
            s.push(if next.legal_moves().is_empty() { '#' } else { '+' });
        }
        s
    }

    /// A move written either way: SAN (Nf3, exd5, O-O) or plain (g1f3).
    /// Only moves that are legal here come back.
    pub fn parse_move(&self, text: &str) -> Option<Move> {
        let legal = self.legal_moves();
        let t: String = text.trim().chars().filter(|c| !"+#!? \t".contains(*c)).collect();
        if t.is_empty() { return None; }
        for mv in &legal {
            if self.san(*mv).trim_end_matches(['+', '#']) == t { return Some(*mv); }
            if mv.uci() == t.to_lowercase() { return Some(*mv); }
        }
        // Castling, written every which way.
        let flat = t.to_uppercase().replace('0', "O");
        if flat == "O-O" || flat == "OO" || flat == "O-O-O" || flat == "OOO" {
            let long = flat.matches('O').count() == 3;
            return legal.iter().find(|m| {
                self.board[m.from as usize].map(|(_, p)| p) == Some(Piece::King)
                    && (file_of(m.from) - file_of(m.to)).abs() == 2
                    && (file_of(m.to) == 2) == long
            }).copied();
        }
        // A destination with no piece letter is a pawn move: e4, exd5.
        let lower = t.to_lowercase();
        let dest = parse_square(&lower[lower.len().saturating_sub(2)..])?;
        let promo = t.split('=').nth(1).and_then(|s| s.chars().next()).and_then(Piece::from_letter);
        let first = t.chars().next()?;
        let want = Piece::from_letter(first).filter(|_| first.is_ascii_uppercase()).unwrap_or(Piece::Pawn);
        let file_hint = if want == Piece::Pawn && t.len() > 2 { Some(first.to_ascii_lowercase() as i32 - 'a' as i32) } else { None };
        let mut hits = legal.iter().filter(|m| {
            m.to == dest
                && self.board[m.from as usize].map(|(_, p)| p) == Some(want)
                && (promo.is_none() || m.promo == promo)
                && file_hint.is_none_or(|f| file_of(m.from) == f)
        });
        let first_hit = hits.next().copied();
        if hits.next().is_some() { return None; }
        first_hit
    }

    /// How the game stands. `history` holds the key of every position
    /// played, this one included, so a third appearance is a draw.
    pub fn over(&self, history: &[String]) -> Option<Over> {
        if self.legal_moves().is_empty() {
            return Some(if self.in_check(self.turn) { Over::Checkmate(self.turn.other()) } else { Over::Stalemate });
        }
        if self.halfmove >= 100 { return Some(Over::FiftyMoves); }
        let key = self.key();
        if history.iter().filter(|k| **k == key).count() >= 3 { return Some(Over::Repetition); }
        if self.too_few_pieces() { return Some(Over::NotEnoughPieces); }
        None
    }

    /// King against king, or with one bishop or knight: nobody can mate.
    fn too_few_pieces(&self) -> bool {
        let mut minor = 0;
        for sq in 0..64 {
            match self.board[sq] {
                None => {}
                Some((_, Piece::King)) => {}
                Some((_, Piece::Bishop)) | Some((_, Piece::Knight)) => minor += 1,
                Some(_) => return false,
            }
        }
        minor <= 1
    }

    /// What the side to move has in pieces, less what the other side has.
    pub fn material(&self) -> i32 {
        let mut score = 0;
        for sq in 0..64 {
            if let Some((c, p)) = self.board[sq] {
                if p == Piece::King { continue; }
                score += if c == self.turn { p.value() } else { -p.value() };
            }
        }
        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count the positions reachable in `depth` moves. Every rule shows up
    /// in these numbers, so they are how a chess program proves itself.
    fn perft(p: &Position, depth: u32) -> u64 {
        if depth == 0 { return 1; }
        let moves = p.legal_moves();
        if depth == 1 { return moves.len() as u64; }
        moves.iter().map(|m| perft(&p.after(*m), depth - 1)).sum()
    }

    #[test]
    fn the_starting_position_has_the_known_move_counts() {
        let p = Position::start();
        assert_eq!(perft(&p, 1), 20);
        assert_eq!(perft(&p, 2), 400);
        assert_eq!(perft(&p, 3), 8_902);
        assert_eq!(perft(&p, 4), 197_281);
    }

    #[test]
    fn castling_en_passant_and_promotion_count_up() {
        // The positions chess programmers test with, from the chess wiki.
        let kiwipete = Position::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1").unwrap();
        assert_eq!(perft(&kiwipete, 1), 48);
        assert_eq!(perft(&kiwipete, 2), 2_039);
        assert_eq!(perft(&kiwipete, 3), 97_862);
        let endgame = Position::from_fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1").unwrap();
        assert_eq!(perft(&endgame, 3), 2_812);
        assert_eq!(perft(&endgame, 4), 43_238);
        let tangle = Position::from_fen("r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1").unwrap();
        assert_eq!(perft(&tangle, 3), 9_467);
        let promo = Position::from_fen("rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8").unwrap();
        assert_eq!(perft(&promo, 3), 62_379);
    }

    #[test]
    fn moves_are_written_and_read_the_way_players_do() {
        let p = Position::start();
        let e4 = p.parse_move("e4").unwrap();
        assert_eq!(p.san(e4), "e4");
        assert_eq!(e4.uci(), "e2e4");
        assert_eq!(p.parse_move("e2e4"), Some(e4));
        // Two knights can reach the same square, so the move says which.
        let two = Position::from_fen("8/8/8/8/8/8/8/R3K2R w KQ - 0 1").unwrap();
        assert_eq!(two.san(two.parse_move("O-O").unwrap()), "O-O");
        assert_eq!(two.san(two.parse_move("O-O-O").unwrap()), "O-O-O");
        let knights = Position::from_fen("8/8/8/3k4/8/8/2N1N3/4K3 w - - 0 1").unwrap();
        let d4 = knights.parse_move("Ncd4").unwrap();
        assert_eq!(knights.san(d4), "Ncd4");
        assert!(knights.parse_move("Nd4").is_none(), "which knight is not clear");
        // A pawn taking, and a pawn becoming a queen with check.
        let takes = Position::from_fen("rnbqkbnr/ppp1pppp/8/3p4/4P3/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 2").unwrap();
        assert_eq!(takes.san(takes.parse_move("exd5").unwrap()), "exd5");
        let promo = Position::from_fen("3k4/6P1/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(promo.san(promo.parse_move("g8=Q").unwrap()), "g8=Q+");
    }

    #[test]
    fn a_game_ends_when_the_rules_say_so() {
        let mate = Position::from_fen("rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3").unwrap();
        assert_eq!(mate.over(&[]), Some(Over::Checkmate(Color::Black)));
        let stuck = Position::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(stuck.over(&[]), Some(Over::Stalemate));
        let bare = Position::from_fen("8/8/4k3/8/8/3K1B2/8/8 w - - 0 1").unwrap();
        assert_eq!(bare.over(&[]), Some(Over::NotEnoughPieces));
        let long = Position::from_fen("8/8/4k3/8/8/3K1R2/8/8 w - - 100 80").unwrap();
        assert_eq!(long.over(&[]), Some(Over::FiftyMoves));
        let p = Position::start();
        assert_eq!(p.over(&[p.key(), p.key()]), None, "twice is not enough");
        assert_eq!(p.over(&[p.key(), p.key(), p.key()]), Some(Over::Repetition));
    }

    #[test]
    fn a_position_survives_a_trip_through_fen() {
        let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
        assert_eq!(Position::from_fen(fen).unwrap().to_fen(), fen);
        let p = Position::start();
        let after = p.after(p.parse_move("e4").unwrap());
        assert_eq!(after.to_fen(), "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1");
    }
}

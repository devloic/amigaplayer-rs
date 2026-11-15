//! Generation D (early driver) slot timing: costs fitted to the write logs of the 68k reference.
//!
//! A tick is a sequence of four voice slots (3, 2, 1, 0). A slot is named by its voice and the tokens of the
//! path it takes (see `slot_d`). A token has a duration and, if it writes, an offset for each of its writes. The
//! clock of a write is the tick start plus `START`, plus the durations of the tokens before it in the tick, plus
//! the write's offset in its token. The values are the least-squares fit, rounded to whole colour clocks, of about
//! 20 000 logged writes; a write can be one colour clock off, and the song-end wrap tokens (seen once per voice)
//! a few more.

/// Clock (colour clocks) from the tick start to the start of the first voice slot. Chosen so that the audio is
/// sample for sample the reference's (40 s checked); the reference's own log stamps are 4020 colour clocks later.
pub const START: i64 = 275;

/// (voice, token, write offsets, duration), in colour clocks.
const TOKENS: &[(usize, &str, &[i32], i32)] = &[
    (0, "D", &[], -19),
    (0, "F", &[], 42),
    (0, "Lp", &[-31, 25], 176),
    (0, "Ls", &[-62, -54], 0),
    (0, "N", &[-13, 49, 201, 347, 414], 0),
    (0, "O", &[-54], 0),
    (0, "R", &[-8, 0], 0),
    (0, "S", &[0], 0),
    (0, "i", &[], 60),
    (0, "p", &[], 78),
    (0, "s", &[], 58),
    (1, "D", &[], 322),
    (1, "F", &[], 899),
    (1, "Lp", &[159, 231], 207),
    (1, "Ls", &[129, 137], 40),
    (1, "N", &[-679, -601, -422, -276, -194], 0),
    (1, "O", &[137], 341),
    (1, "S", &[-150], 113),
    (1, "i", &[], 61),
    (1, "p", &[], 77),
    (1, "pw", &[], 92),
    (1, "s", &[], 58),
    (2, "D", &[], 130),
    (2, "F", &[], 233),
    (2, "Lp", &[159, 246], 239),
    (2, "Ls", &[128, 136], 40),
    (2, "N", &[-14, 80, 286, 432, 529], 532),
    (2, "O", &[136], 149),
    (2, "R", &[-8, 0], 0),
    (2, "S", &[41], 128),
    (2, "i", &[], 60),
    (2, "p", &[], 78),
    (2, "pw", &[], 88),
    (2, "s", &[], 58),
    (3, "D", &[], -5),
    (3, "F", &[], 97),
    (3, "Lp", &[22, 114], 247),
    (3, "Ls", &[-8, 0], 41),
    (3, "N", &[-14, 83, 288, 434, 536], 539),
    (3, "O", &[0], 14),
    (3, "R", &[-8, 0], 0),
    (3, "S", &[40], 132),
    (3, "i", &[], 60),
    (3, "p", &[], 78),
    (3, "pw", &[], 88),
    (3, "s", &[], 58),
];

fn token(v: usize, t: &str) -> Option<&'static (usize, &'static str, &'static [i32], i32)> {
    // a token never seen on this voice borrows the nearest voice that has it
    [v, (v + 1) % 4, (v + 3) % 4, (v + 2) % 4]
        .iter()
        .find_map(|&w| TOKENS.iter().find(|e| e.0 == w && e.1 == t))
}

/// The write offsets and the duration of a slot, in colour clocks from its start.
pub fn lookup(v: usize, key: &str, n: usize) -> (Vec<i32>, i32) {
    let mut offs = Vec::with_capacity(n);
    let mut acc = 0;
    for t in key.split_whitespace() {
        if let Some(e) = token(v, t) {
            offs.extend(e.2.iter().map(|o| acc + o));
            acc += e.3;
        }
    }
    offs.resize(n.max(1), 0);
    (offs, acc)
}

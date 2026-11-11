// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Vendored from audacious-uade-rust (https://fg.lolo.asia/adminlolo/audacious-uade-rust.git,
// commit c3ea17194c768987c48751cd97d638ccb04b1472), file crates/au-songdb/src/codec.rs, whose crate
// manifest gives the licence above. It is a Rust port of src/songdb/songdb.cc of
// audacious-uade by Matti Tiainen (https://github.com/mvtiaine/audacious-uade), which carries:
//     SPDX-License-Identifier: LGPL-2.1-or-later
//     Copyright (C) 2014-2026 Matti Tiainen <mvtiaine@cc.hut.fi>
// (audacious-uade as a whole is GPL-2.0-or-later; its src/common/ and src/songdb/ are also
// offered under LGPL-2.1-or-later). The texts are COPYING and COPYING.LGPL beside Cargo.toml.
// Changes for amigaplayer-rs: the `au_common::` path is a `crate::` path.

//! The song database's on-disk encodings, ported from the anonymous
//! namespace at the top of `src/songdb/songdb.cc`.
//!
//! The `.tsv` files are not really tab-separated values — they are a compact
//! binary-ish format that happens to be newline-delimited. Three encodings
//! appear:
//!
//! - **hex hashes** (`hex2hash`): the 12-character key each plugin builds as
//!   `XXH32(first 256 KiB)` then the low 16 bits of the file size, packed
//!   into 48 bits.
//! - **a base-64 variant offset by 45** (`b64d24`): digits start at `'-'`,
//!   six bits each, 1–4 characters, length inferred from the terminator.
//! - **delta-coded hashes** (`b64diff2hash`): each row of `xxh32idx.tsv`
//!   stores the *increment* from the previous hash, 1–6 characters.
//!
//! All three are reproduced exactly, including the parts that are wrong on
//! inputs the shipped database never contains — see [`hex2hash`].

use crate::songend::SongEndStatus;

/// A 48-bit database key. The C calls this `hash_t` and stores it in a
/// packed `uint48_t`; the width is what matters, so this is a `u64` that
/// never exceeds 48 bits.
pub type Hash = u64;

/// The C's `HASH_T_MAX`, used to seed its interpolation search.
pub const HASH_MAX: Hash = (1u64 << 48) - 1;

/// Port of `hex2hash`.
///
/// Reads exactly 12 characters into a 48-bit value.
///
/// # The digit conversion is not `from_digit`
///
/// The C branches on the raw byte:
///
/// ```c
/// if (c > 58) ret |= c - 87;   /* 'a'..'f' -> 10..15 */
/// else        ret |= c - 48;   /* '0'..'9' ->  0..9  */
/// ```
///
/// Two consequences, both reproduced:
///
/// - **Uppercase hex is silently wrong.** `'A'` (65) takes the first branch
///   and yields 65-87 = -22, not 10. Every producer in this project emits
///   lowercase (`common::to_hex` uses `{:02x}`), so the database and the
///   lookups agree — but a caller passing an uppercase key gets a wrong
///   answer rather than an error.
/// - **`':'` aliases `'a'`.** 58 is *not* greater than 58, so it takes the
///   second branch and yields 10 — the same value as `'a'`. Any byte in
///   `';'..='`'` also produces an out-of-range digit.
///
/// Rather than mask the result, this reproduces the C's wrapping arithmetic
/// so a malformed key maps to the same (wrong) slot it would in the C, and
/// therefore misses the same way.
pub fn hex2hash(hex: &[u8]) -> Hash {
    debug_assert!(hex.len() >= 12, "callers assert hash.size() >= 12");
    let mut ret: u64 = 0;
    for &c in hex.iter().take(12) {
        ret <<= 4;
        // `as i64` first: the C does signed arithmetic on `char`, so a byte
        // below the branch point must be able to go negative before it is
        // folded into the unsigned accumulator.
        let digit = if c > 58 { c as i64 - 87 } else { c as i64 - 48 };
        ret |= digit as u64;
    }
    // The C returns this as a 48-bit `uint48_t`, discarding anything above.
    ret & HASH_MAX
}

/// Port of `b64d24(const string_view&)` — the length-is-known form.
///
/// Six bits per character, biased by 45 (`'-'` is zero). Accepts 1–4
/// characters, yielding up to 24 bits.
///
/// The C `assert`s a length of 4 in its `else` arm; with `NDEBUG` that
/// assert vanishes and a longer slice reads only its first four characters.
/// This takes the same four, so an over-long field decodes identically
/// rather than panicking where the shipped build would not.
pub fn b64d24(b64: &[u8]) -> u32 {
    match b64.len() {
        0 => 0,
        1 => d(b64[0]),
        2 => (d(b64[0]) << 6) | d(b64[1]),
        3 => (d(b64[0]) << 12) | (d(b64[1]) << 6) | d(b64[2]),
        _ => (d(b64[0]) << 18) | (d(b64[1]) << 12) | (d(b64[2]) << 6) | d(b64[3]),
    }
}

/// One base-64 digit, biased by 45.
#[inline]
fn d(c: u8) -> u32 {
    // Wrapping, as the C's `char` arithmetic is: a byte below '-' produces a
    // large value rather than a panic, and lands in the same wrong slot.
    (c as i32).wrapping_sub(45) as u32
}

/// Port of `b64d24(const char*, int&)` — the length-is-inferred form.
///
/// Used where the field is a row prefix rather than a pre-split column: the
/// length is discovered by looking for the first `'\n'` or `'\t'` at offsets
/// 1, 2 or 3, defaulting to four characters.
///
/// Returns the value and how many characters it consumed.
pub fn b64d24_prefix(b64: &[u8]) -> (u32, usize) {
    let terminator = |i: usize| matches!(b64.get(i), Some(b'\n') | Some(b'\t'));
    if terminator(1) {
        (d(b64[0]), 1)
    } else if terminator(2) {
        ((d(b64[0]) << 6) | d(b64[1]), 2)
    } else if terminator(3) {
        ((d(b64[0]) << 12) | (d(b64[1]) << 6) | d(b64[2]), 3)
    } else {
        (
            (d(b64[0]) << 18) | (d(b64[1]) << 12) | (d(b64[2]) << 6) | d(b64[3]),
            4,
        )
    }
}

/// Port of `b64diff2hash`.
///
/// Decodes a 1–6 character increment and adds it to the running hash. The
/// terminator is `'\n'` only — unlike [`b64d24_prefix`], a tab does not end
/// the field here, because this is always the whole line.
///
/// The six-character arm is the interesting one: it is *not* a straight
/// 36-bit read. The C assembles two independent 18-bit halves and then
/// shifts, which matters because the intermediate must not overflow the
/// 32-bit arithmetic the other arms use.
pub fn b64diff2hash(prev: Hash, b64: &[u8]) -> Hash {
    let nl = |i: usize| b64.get(i) == Some(&b'\n');
    let delta: u64 = if nl(1) {
        d(b64[0]) as u64
    } else if nl(2) {
        ((d(b64[0]) << 6) | d(b64[1])) as u64
    } else if nl(3) {
        ((d(b64[0]) << 12) | (d(b64[1]) << 6) | d(b64[2])) as u64
    } else if nl(4) {
        ((d(b64[0]) << 18) | (d(b64[1]) << 12) | (d(b64[2]) << 6) | d(b64[3])) as u64
    } else if nl(5) {
        ((d(b64[0]) << 24)
            | (d(b64[1]) << 18)
            | (d(b64[2]) << 12)
            | (d(b64[3]) << 6)
            | d(b64[4])) as u64
    } else {
        let part1 = (d(b64[0]) << 12) | (d(b64[1]) << 6) | d(b64[2]);
        let part2 = (d(b64[3]) << 12) | (d(b64[4]) << 6) | d(b64[5]);
        ((part1 as u64) << 18) | (part2 as u64)
    };
    prev.wrapping_add(delta)
}

/// Port of `parse_songend`.
///
/// One character per status. Note these are *not* the first letters of
/// [`SongEndStatus::as_str`] — `'b'`, `'P'`, `'i'` and `'L'` are arbitrary,
/// and the mapping is case-sensitive (`'p'` and `'P'` are different
/// statuses). Anything unrecognised is [`SongEndStatus::None`].
pub fn parse_songend(s: &[u8]) -> SongEndStatus {
    match s {
        b"e" => SongEndStatus::Error,
        b"p" => SongEndStatus::Player,
        b"t" => SongEndStatus::Timeout,
        b"s" => SongEndStatus::DetectSilence,
        b"l" => SongEndStatus::DetectLoop,
        b"v" => SongEndStatus::DetectVolume,
        b"r" => SongEndStatus::DetectRepeat,
        b"b" => SongEndStatus::PlayerPlusSilence,
        b"P" => SongEndStatus::PlayerPlusVolume,
        b"i" => SongEndStatus::LoopPlusSilence,
        b"L" => SongEndStatus::LoopPlusVolume,
        b"n" => SongEndStatus::NoSound,
        _ => SongEndStatus::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex2hash_packs_twelve_lowercase_digits() {
        assert_eq!(hex2hash(b"000000000000"), 0);
        assert_eq!(hex2hash(b"ffffffffffff"), HASH_MAX);
        assert_eq!(hex2hash(b"0123456789ab"), 0x0123_4567_89ab);
        // A real key from the hash blacklist.
        assert_eq!(hex2hash(b"e2e642be13fc"), 0xe2e6_42be_13fc);
        // Only the first twelve characters are read.
        assert_eq!(hex2hash(b"0123456789abDISCARDED"), 0x0123_4567_89ab);
    }

    /// The two documented defects in the digit conversion. Pinned so that
    /// "fixing" `hex2hash` to accept uppercase shows up as a deliberate
    /// divergence rather than a silent one.
    #[test]
    fn hex2hash_reproduces_the_c_digit_quirks() {
        // Uppercase is not hex here: 'A' (65) - 87 wraps far negative.
        assert_ne!(hex2hash(b"AAAAAAAAAAAA"), hex2hash(b"aaaaaaaaaaaa"));
        // ':' (58) takes the '0'..'9' branch and yields 10, aliasing 'a'.
        assert_eq!(hex2hash(b"::::::::::::"), hex2hash(b"aaaaaaaaaaaa"));
    }

    #[test]
    fn b64d24_decodes_each_supported_width() {
        assert_eq!(b64d24(b"-"), 0);
        assert_eq!(b64d24(b"."), 1);
        assert_eq!(b64d24(b"--"), 0);
        assert_eq!(b64d24(b"-."), 1);
        assert_eq!(b64d24(b".-"), 64);
        assert_eq!(b64d24(b"---."), 1);
        assert_eq!(b64d24(b".---"), 1 << 18);
        // Widest value: four characters of 63.
        let max = [45 + 63u8; 4];
        assert_eq!(b64d24(&max), (1 << 24) - 1);
    }

    #[test]
    fn b64d24_prefix_infers_width_from_the_terminator() {
        assert_eq!(b64d24_prefix(b".\n"), (1, 1));
        assert_eq!(b64d24_prefix(b".\t"), (1, 1));
        assert_eq!(b64d24_prefix(b".-\n"), (64, 2));
        assert_eq!(b64d24_prefix(b".--\t"), (1 << 12, 3));
        // No terminator in the first four positions: four characters.
        assert_eq!(b64d24_prefix(b".---x"), (1 << 18, 4));
    }

    /// Tabs terminate a `b64d24` prefix but *not* a hash delta. Getting this
    /// backwards would misparse every row of one of the two file kinds.
    #[test]
    fn b64diff2hash_terminates_on_newline_only() {
        assert_eq!(b64diff2hash(0, b".\n"), 1);
        assert_eq!(b64diff2hash(100, b".\n"), 101);
        // A tab is an ordinary digit here, so this is a two-character value,
        // not a one-character one.
        assert_ne!(b64diff2hash(0, b".\t\n"), b64diff2hash(0, b".\n"));
    }

    #[test]
    fn b64diff2hash_decodes_each_width() {
        assert_eq!(b64diff2hash(0, b"..\n"), 65);
        assert_eq!(b64diff2hash(0, b"...\n"), (1 << 12) + (1 << 6) + 1);
        assert_eq!(b64diff2hash(0, b"-..-\n"), (1 << 12) + (1 << 6));
        assert_eq!(b64diff2hash(0, b"----.\n"), 1);
    }

    /// The six-character arm composes two 18-bit halves; a naive 36-bit
    /// read would agree on small values and diverge here.
    #[test]
    fn b64diff2hash_six_char_form_composes_two_halves() {
        assert_eq!(b64diff2hash(0, b"-----."), 1);
        assert_eq!(b64diff2hash(0, b".-----"), 1u64 << 30);
        assert_eq!(b64diff2hash(0, b"---.--"), 1u64 << 12);
        let max = [45 + 63u8; 6];
        assert_eq!(b64diff2hash(0, &max), (1u64 << 36) - 1);
    }

    #[test]
    fn b64diff2hash_accumulates() {
        let mut h = 0;
        for delta in [b".\n", b".\n", b".\n"] {
            h = b64diff2hash(h, delta);
        }
        assert_eq!(h, 3);
    }

    #[test]
    fn songend_codes_match_the_c_table() {
        let cases: [(&[u8], SongEndStatus); 12] = [
            (b"e", SongEndStatus::Error),
            (b"p", SongEndStatus::Player),
            (b"t", SongEndStatus::Timeout),
            (b"s", SongEndStatus::DetectSilence),
            (b"l", SongEndStatus::DetectLoop),
            (b"v", SongEndStatus::DetectVolume),
            (b"r", SongEndStatus::DetectRepeat),
            (b"b", SongEndStatus::PlayerPlusSilence),
            (b"P", SongEndStatus::PlayerPlusVolume),
            (b"i", SongEndStatus::LoopPlusSilence),
            (b"L", SongEndStatus::LoopPlusVolume),
            (b"n", SongEndStatus::NoSound),
        ];
        for (code, want) in cases {
            assert_eq!(parse_songend(code), want, "code {:?}", code[0] as char);
        }
    }

    /// Case matters, and unknown codes are `None` rather than an error.
    #[test]
    fn songend_codes_are_case_sensitive_and_default_to_none() {
        assert_ne!(parse_songend(b"p"), parse_songend(b"P"));
        assert_ne!(parse_songend(b"l"), parse_songend(b"L"));
        assert_eq!(parse_songend(b"E"), SongEndStatus::None);
        assert_eq!(parse_songend(b"z"), SongEndStatus::None);
        assert_eq!(parse_songend(b""), SongEndStatus::None);
        assert_eq!(parse_songend(b"pp"), SongEndStatus::None);
    }
}

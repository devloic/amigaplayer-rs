// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Written for amigaplayer-rs (not vendored, and not in audacious-uade, whose database is
// made by the tools of https://github.com/mvtiaine/audacious-uade-tools): the inverse of
// the parsers of db.rs and codec.rs, derived from them.

//! Writes a database in the four files [`SongDb::init`](crate::SongDb::init) reads.
//!
//! For tests (a synthetic database, so that the real one, which is not freely licensed,
//! need not be anywhere near a test) and for small private databases. It writes every row
//! in full: none of the "same as the row before" forms the real files save space with.

use crate::codec::{hex2hash, Hash};
use crate::songend::SongEndStatus;
use crate::Info;
use std::io::{Error, ErrorKind, Result, Write};
use std::path::Path;

/// The largest step between two rows of the hash index: six digits of six bits.
const MAX_DELTA: u64 = (1 << 36) - 1;

/// `n` in the database's base 64 (digits from `'-'`, most significant first), in `digits` digits.
fn b64(n: u64, digits: usize) -> String {
    (0..digits).rev().map(|i| (45 + ((n >> (6 * i)) & 63) as u8) as char).collect()
}

/// `n` in as few digits as hold it, at most `max`.
fn b64_min(n: u64, max: usize) -> String {
    let digits = (1..max).find(|&d| n < 1 << (6 * d)).unwrap_or(max);
    b64(n, digits)
}

/// The letter `codec::parse_songend` reads as `s` (`x`, which it does not know, for `None`).
fn status_letter(s: SongEndStatus) -> char {
    match s {
        SongEndStatus::Error => 'e',
        SongEndStatus::None => 'x',
        SongEndStatus::Player => 'p',
        SongEndStatus::Timeout => 't',
        SongEndStatus::DetectSilence => 's',
        SongEndStatus::DetectLoop => 'l',
        SongEndStatus::DetectVolume => 'v',
        SongEndStatus::DetectRepeat => 'r',
        SongEndStatus::PlayerPlusSilence => 'b',
        SongEndStatus::PlayerPlusVolume => 'P',
        SongEndStatus::LoopPlusSilence => 'i',
        SongEndStatus::LoopPlusVolume => 'L',
        SongEndStatus::NoSound => 'n',
    }
}

fn bad(why: String) -> Error {
    Error::new(ErrorKind::InvalidInput, why)
}

/// A string a column can hold: no tab or newline, and not the "as above" marker first.
fn column(s: &str) -> Result<&str> {
    if s.contains(['\t', '\n']) || s.starts_with('\x7f') {
        return Err(bad(format!("{s:?} cannot be a column of the database")));
    }
    Ok(s)
}

/// Writes `xxh32idx.tsv`, `songlengths.tsv`, `modinfos.tsv` and `metadata.tsv` into `dir`
/// (which exists), holding `entries`: a key (12 lowercase hex digits, as [`key`](crate::key)
/// gives it) and what the database says about that file.
///
/// What the formats cannot hold is an error: a key that is not 12 lowercase hex digits, is
/// zero or comes twice; subsongs that are not numbered consecutively, start above 127 or
/// are none; a length of 2^18 * 20 ms (87 minutes) or more, or not a multiple of 20 ms; a
/// year outside 1901..=2155 (0 is "unknown"); a tab or newline in a string. More than
/// 2^36 between two keys is bridged with rows of keys of their own (no subsong of theirs
/// has a status), so a lookup of such a key finds an empty entry: unavoidable in a small
/// database, and absent from the real one, whose 476,000 keys are close enough.
pub fn write(dir: &Path, entries: &[(String, Info)]) -> Result<()> {
    let mut rows: Vec<(Hash, &Info)> = Vec::new();
    for (key, info) in entries {
        if key.len() != 12 || !key.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
            return Err(bad(format!("{key:?} is not a key (12 lowercase hex digits)")));
        }
        let h = hex2hash(key.as_bytes());
        if h == 0 {
            return Err(bad("the key 000000000000 is reserved".into()));
        }
        rows.push((h, info));
    }
    rows.sort_by_key(|r| r.0);
    if rows.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(bad("a key comes twice".into()));
    }

    let (mut idx_f, mut len_f, mut mod_f, mut meta_f) = (String::new(), String::new(), String::new(), String::new());
    // row 0 is the reserved hash 0: its text is not read
    idx_f.push_str("--------\n");
    let (mut prev, mut row) = (0u64, 0u64);
    for (h, info) in rows {
        while h - prev > MAX_DELTA {
            // a bridge row: nothing known, and no format
            idx_f.push_str(&format!("{}\n", b64_min(MAX_DELTA, 6)));
            prev += MAX_DELTA;
            row += 1;
            len_f.push_str("-,x\n");
            mod_f.push_str(&format!("{}\t\t\n", b64_min(row, 4)));
        }
        idx_f.push_str(&format!("{}\n", b64_min(h - prev, 6)));
        prev = h;
        row += 1;
        if row >= 1 << 24 {
            return Err(bad("too many rows".into()));
        }

        // song lengths: "length,status" (or "length,!": ended by the player, a duplicate) per
        // subsong, then the first subsong's number
        let subs = &info.subsongs;
        let Some(first) = subs.first() else { return Err(bad("an entry without subsongs".into())) };
        if first.subsong > 127 || subs.iter().enumerate().any(|(i, s)| s.subsong as usize != first.subsong as usize + i) {
            return Err(bad("subsongs must be numbered consecutively, the first at most 127".into()));
        }
        let mut cols = Vec::new();
        for s in subs {
            let units = s.songend.length / 20;
            if s.songend.length % 20 != 0 || units >= 1 << 18 {
                return Err(bad(format!("a length of {} ms cannot be stored", s.songend.length)));
            }
            let status = match (s.songend.status, s.is_duplicate) {
                (SongEndStatus::Player, true) => '!',
                (_, true) => return Err(bad("only a subsong ended by the player can be a duplicate".into())),
                (st, false) => status_letter(st),
            };
            cols.push(format!("{},{status}", b64_min(units as u64, 4)));
        }
        len_f.push_str(&format!("{}\t{}\n", cols.join(" "), first.subsong));

        // format and channels: a row for every index (one the file does not mention reads
        // as the first format of the file: db.rs, `modinfo`)
        let (format, channels) = info.modinfo.as_ref().map_or(("", 0), |m| (m.format.as_str(), m.channels));
        mod_f.push_str(&format!("{}\t{}\t{}\n", b64_min(row, 4), column(format)?, channels));

        if let Some(m) = &info.metadata {
            if m.year != 0 && !(1901..=2155).contains(&m.year) {
                return Err(bad(format!("the year {} cannot be stored", m.year)));
            }
            let year = if m.year == 0 { String::new() } else { m.year.to_string() };
            meta_f.push_str(&format!(
                "{}\t{}\t{}\t{}\t{year}\n",
                b64_min(row, 4),
                column(&m.author)?,
                column(&m.publisher)?,
                column(&m.album)?
            ));
        }
    }
    for (name, text) in [("xxh32idx.tsv", idx_f), ("songlengths.tsv", len_f), ("modinfos.tsv", mod_f), ("metadata.tsv", meta_f)] {
        std::fs::File::create(dir.join(name))?.write_all(text.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_what_the_decoders_read() {
        use crate::codec::{b64d24, b64d24_prefix, b64diff2hash};
        for n in [0u64, 1, 63, 64, 4095, 4096, (1 << 18) - 1, 1 << 18, (1 << 24) - 1] {
            let s = b64_min(n, 4);
            assert_eq!(b64d24(s.as_bytes()) as u64, n, "{s}");
            assert_eq!(b64d24_prefix(format!("{s}\t").as_bytes()), (n as u32, s.len()));
        }
        for n in [1u64, 63, 64, (1 << 24) - 1, 1 << 24, (1 << 30) - 1, 1 << 30, MAX_DELTA] {
            let s = format!("{}\n", b64_min(n, 6));
            assert_eq!(b64diff2hash(1000, s.as_bytes()), 1000 + n, "{s:?}");
        }
    }

    #[test]
    fn status_letters_are_the_parsers() {
        for v in -1..=11 {
            let s = SongEndStatus::from_i8(v).unwrap();
            assert_eq!(crate::codec::parse_songend(status_letter(s).to_string().as_bytes()), s);
        }
    }
}

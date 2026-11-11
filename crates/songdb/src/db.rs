// SPDX-License-Identifier: LGPL-2.1-or-later OR GPL-2.0-or-later
//
// Vendored from audacious-uade-rust (https://fg.lolo.asia/adminlolo/audacious-uade-rust.git,
// commit c3ea17194c768987c48751cd97d638ccb04b1472), file crates/au-songdb/src/db.rs, whose crate
// manifest gives the licence above. It is a Rust port of src/songdb/songdb.cc of
// audacious-uade by Matti Tiainen (https://github.com/mvtiaine/audacious-uade), which carries:
//     SPDX-License-Identifier: LGPL-2.1-or-later
//     Copyright (C) 2014-2026 Matti Tiainen <mvtiaine@cc.hut.fi>
// (audacious-uade as a whole is GPL-2.0-or-later; its src/common/ and src/songdb/ are also
// offered under LGPL-2.1-or-later). The texts are COPYING and COPYING.LGPL beside Cargo.toml.
// Changes for amigaplayer-rs: `au_common::` paths are `crate::` paths.

//! Port of the database body of `src/songdb/songdb.cc`.
//!
//! # Owned state instead of globals (DESIGN.md A7)
//!
//! The C keeps the whole database in file-scope arrays whose sizes are
//! `-D` macros computed by `configure` from the `.tsv` line counts
//! (`HASH_IDX_SIZE=476329` and friends), plus an `initialized[]` flag array
//! and a `mutex` guarding two runtime maps. [`SongDb`] owns all of that
//! instead, and sizes its vectors from what it actually reads.
//!
//! This is a structural divergence, not a behavioural one, and it is worth
//! being precise about why it is safe:
//!
//! - The size macros only ever appear as **array bounds and search seeds**.
//!   Both search seeds feed interpolation searches over *sorted* arrays
//!   (`hash_idx` is asserted strictly ascending at parse time; the metadata
//!   table is built by an ascending scan). A seeded linear scan over sorted
//!   data finds exactly what a binary search finds, so replacing the seed
//!   changes running time and nothing else. `lookup_hash_index` uses
//!   `binary_search` and says so.
//! - `initialized[]` becomes `Option` on each table — the same question,
//!   asked where it cannot be forgotten.
//!
//! What is *not* changed is the record layout. The packed bitfields are
//! reproduced as masks in [`SubSongPacked`] and [`SongInfoPacked`], because
//! their **truncation is observable**: a song length wider than 18 bits or a
//! minimum subsong above 127 wraps in the C, and the shipped encoders can
//! emit both.
//!
//! # What the differential test can and cannot see
//!
//! `tests/differential_songdb.rs` compares this module against the C over
//! the whole shipped database, which makes it a strong check on the
//! *parsers*. Mutation testing (`PORTING.md` §4) measured where it is blind:
//!
//! | Mutation | Differential | Caught by |
//! |---|---|---|
//! | song length unit 20 ms → 10 ms | **red** | — |
//! | empty status ⇒ `None` not `Player` | **red** | — |
//! | 18-bit length mask widened to 24 | green | unit test |
//! | `has_subsongs` `len()>1` → `len()>0` | green | nothing — see below |
//!
//! - **The length mask is invisible to the corpus, structurally.** The 18-bit
//!   field saturates at 5,242,860 ms — 87 minutes. The longest song in the
//!   test corpus is 5.6 minutes, so no fixture can reach the truncation
//!   boundary however many are added. It is pinned instead by
//!   `packed_fields_truncate_like_the_c_bitfields`, which constructs the
//!   over-wide value directly. This is the case for keeping that unit test
//!   even though the differential one looks stronger.
//!
//! - **The `has_subsongs` mutation is inert, not uncaught.** The flag only
//!   gates a lookup into `subsongs`, and `subsongs` only ever receives an
//!   entry when a row had more than one subsong. Setting the flag for
//!   single-subsong rows therefore reaches a map miss and returns exactly
//!   what the unset flag returned. Nothing went red because nothing changed
//!   — the right conclusion is that the flag is redundant in this
//!   representation, not that the tests are weak. It is kept because the C
//!   stores it and the record layout is the thing being preserved.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::songend::{SongEnd, SongEndStatus};
use crate::strings::{split_view_n, split_view_x_n};

use crate::codec::{b64d24, b64d24_prefix, b64diff2hash, hex2hash, parse_songend, Hash};
use crate::{Info, MetaData, ModInfo, Source, SubSongInfo};

/// The C's `SEPARATOR`, joining multiple authors or publishers in one field.
const SEPARATOR: char = '\x7e';
/// The C's `REPEAT`: this field is the same as the previous row's.
const REPEAT: u8 = 0x7f;
/// The C's `AUTHOR_JOIN`, used when collapsing a multi-author field.
const AUTHOR_JOIN: &str = " & ";

/// The C's `UNKNOWN_AUTHOR_T`: index 0 of the author pool, always `""`.
const UNKNOWN_AUTHOR: u16 = 0;
/// The C's `STRING_NOT_FOUND`.
const STRING_NOT_FOUND: u16 = u16::MAX;

/// The C's `HASH_NOT_FOUND` (`UINT24_T_MAX`).
const HASH_NOT_FOUND: u32 = (1 << 24) - 1;

/// Port of `_SubSongInfo`'s three bitfields.
///
/// The C packs an 18-bit length, a **signed** 5-bit status and a 1-bit
/// duplicate flag into 24 bits. The widths are reproduced by masking on
/// construction, so a value too wide to store wraps here exactly as it does
/// there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubSongPacked {
    /// Song length in 20 ms units, 18 bits.
    songlength: u32,
    /// Status, stored in 5 bits two's-complement.
    songend: i8,
    is_duplicate: bool,
}

impl SubSongPacked {
    fn new(songlength: u32, songend: SongEndStatus, is_duplicate: bool) -> Self {
        Self {
            // 18-bit field. The C `assert`s `songlength <= (1 << 18)` — note
            // `<=`, itself off by one — and truncates under NDEBUG, which is
            // how the shipped build behaves.
            songlength: songlength & ((1 << 18) - 1),
            songend: sign_extend_5(songend.as_i8()),
            is_duplicate,
        }
    }

    /// Carries a previous entry's length and status forward under a new
    /// duplicate flag — the C's two-argument constructor.
    fn with_duplicate(other: Self, is_duplicate: bool) -> Self {
        Self { is_duplicate, ..other }
    }

    /// Port of `songlength_ms()`: the stored value is in 20 ms units.
    pub fn songlength_ms(self) -> u32 {
        self.songlength * 20
    }

    pub fn songend(self) -> SongEndStatus {
        // Unknown encodings cannot arise from `parse_songend`, but the field
        // is only five bits wide, so decode defensively rather than panic in
        // a lookup path.
        SongEndStatus::from_i8(self.songend).unwrap_or(SongEndStatus::None)
    }

    pub fn is_duplicate(self) -> bool {
        self.is_duplicate
    }
}

/// Truncates to the C's signed 5-bit field and sign-extends back.
///
/// `Status::ERROR` is -1, which is why the field is signed at all: it stores
/// as `0b11111` and must read back as -1 rather than 31.
fn sign_extend_5(v: i8) -> i8 {
    let masked = (v as u8) & 0x1f;
    if masked & 0x10 != 0 {
        (masked | 0xe0) as i8
    } else {
        masked as i8
    }
}

/// Port of `_SongInfo`: a 1-bit flag, a 7-bit minimum subsong, and the first
/// subsong's record inline.
#[derive(Debug, Clone, Copy, Default)]
struct SongInfoPacked {
    has_subsongs: bool,
    /// 7 bits, so values above 127 wrap as they do in the C.
    min_subsong: u8,
    info: SubSongPacked,
}

impl SongInfoPacked {
    fn new(has_subsongs: bool, min_subsong: u8, info: SubSongPacked) -> Self {
        Self { has_subsongs, min_subsong: min_subsong & 0x7f, info }
    }
}

/// Port of `_ModInfo`.
///
/// The derived `Default` is `{ format: 0, channels: 0 }`, which is the point:
/// it matches the C's zero-initialised file-scope array. Note that this is
/// *not* `{ format: STRING_NOT_FOUND, .. }`, and the difference is
/// observable — see [`SongDb::modinfo`].
#[derive(Debug, Clone, Copy, Default)]
struct ModInfoPacked {
    format: u16,
    channels: u8,
}

/// Port of `_MetaData`.
#[derive(Debug, Clone, Copy, Default)]
struct MetaDataPacked {
    hash: u32,
    author: u16,
    album: u16,
    publisher: u16,
    /// Years are stored as an offset from 1900 in one byte.
    year: u8,
}

/// The song database.
///
/// Replaces the C's file-scope arrays; see the module docs for why that is
/// behaviour-preserving.
#[derive(Default)]
pub struct SongDb {
    /// Strictly ascending; index 0 is the reserved 0 hash.
    hash_idx: Vec<Hash>,
    songinfos: Option<Vec<SongInfoPacked>>,
    /// Subsongs beyond the first, keyed by hash index.
    subsongs: HashMap<u32, Vec<SubSongPacked>>,
    modinfos: Option<Vec<ModInfoPacked>>,
    metadata: Option<Vec<MetaDataPacked>>,

    format_pool: Vec<String>,
    author_pool: Vec<String>,
    publisher_pool: Vec<String>,
    album_pool: Vec<String>,

    /// The C's `extra_subsongs`/`extra_modinfos`: runtime additions from
    /// `update()`, consulted only when the file database misses.
    extra_subsongs: HashMap<Hash, Vec<SubSongInfo>>,
    extra_modinfos: HashMap<Hash, ModInfo>,
}

impl SongDb {
    /// Port of `songdb::init`.
    ///
    /// `sources` empty means every source the directory provides, matching
    /// the C's default argument (which it resolves with `#if` on the size
    /// macros; here it is simply "whatever is present").
    ///
    /// The hash index is always loaded — it is the C's internal `SOURCE_HASH`
    /// and every lookup goes through it.
    pub fn init(dir: &Path, sources: &[Source]) -> std::io::Result<Self> {
        let want = |s: Source| sources.is_empty() || sources.contains(&s);

        let mut db = SongDb {
            // The C seeds `author_pool` with one empty string so that index 0
            // is always the unknown author.
            author_pool: vec![String::new()],
            ..Default::default()
        };

        db.parse_hashidx(&dir.join("xxh32idx.tsv"))?;

        if want(Source::Songlengths) {
            db.parse_songlengths(&dir.join("songlengths.tsv"))?;
        }
        if want(Source::ModInfos) {
            db.parse_modinfos(&dir.join("modinfos.tsv"))?;
        }
        if want(Source::Metadata) {
            db.parse_metadata(&dir.join("metadata.tsv"))?;
        }
        Ok(db)
    }

    /// Port of `parse_hashidx`. Each line is a delta from the previous hash;
    /// row 0 is the literal 0.
    fn parse_hashidx(&mut self, path: &Path) -> std::io::Result<()> {
        let mut prev: Hash = 0;
        for (i, line) in read_lines(path)?.enumerate() {
            let line = line?;
            let hash = if i == 0 { 0 } else { b64diff2hash(prev, &line) };
            // The C asserts this; it is also the precondition that lets
            // `lookup_hash_index` binary-search. Checked in release too,
            // because a non-monotonic file would silently break lookups.
            assert!(
                i == 0 || hash > prev,
                "{}: hash index is not strictly ascending at row {i}",
                path.display()
            );
            self.hash_idx.push(hash);
            prev = hash;
        }
        Ok(())
    }

    /// Port of `parse_songlengths`.
    ///
    /// Column 0 is a space-separated subsong list; column 1 is the minimum
    /// subsong. Rows line up with the hash index, starting at 1.
    fn parse_songlengths(&mut self, path: &Path) -> std::io::Result<()> {
        let mut infos = vec![SongInfoPacked::default(); self.hash_idx.len().max(1)];
        let mut idx: u32 = 1; // index 0 is reserved

        for line in read_lines(path)? {
            let line = line?;
            let line = std::str::from_utf8(&line).map_err(invalid_data)?;
            let cols = split_view_x_n::<2>(line, '\t');
            let subsongs = cols[0].split(' ').collect::<Vec<_>>();

            let mut entries = Vec::with_capacity(subsongs.len());
            // The C seeds this with (0, ERROR, false); an empty first field
            // therefore carries *that* forward, not a zeroed record.
            let mut cur = SubSongPacked::new(0, SongEndStatus::Error, false);
            for col in &subsongs {
                cur = if col.is_empty() {
                    // Empty: repeat the previous entry, clearing duplicate.
                    SubSongPacked::with_duplicate(cur, false)
                } else if *col == "!" {
                    // Bare "!": repeat the previous entry as a duplicate.
                    SubSongPacked::with_duplicate(cur, true)
                } else {
                    let e = split_view_n::<2>(col, ',');
                    let songlength = if e[0].is_empty() { 0 } else { b64d24(e[0].as_bytes()) };
                    // An empty or "!" status means PLAYER, not NONE.
                    let songend = if e[1].is_empty() || e[1] == "!" {
                        SongEndStatus::Player
                    } else {
                        parse_songend(e[1].as_bytes())
                    };
                    // The C also tests a third field here, which its own
                    // `split_view<2>` can never produce; that arm is dead.
                    let is_duplicate = e[1] == "!";
                    SubSongPacked::new(songlength, songend, is_duplicate)
                };
                entries.push(cur);
            }
            assert!(!entries.is_empty(), "{}: empty subsong list", path.display());

            let min_subsong: u8 = if cols[1].is_empty() {
                1
            } else {
                crate::strings::from_chars::<u8>(cols[1]).unwrap_or(1)
            };
            infos[idx as usize] = SongInfoPacked::new(subsongs.len() > 1, min_subsong, entries[0]);
            if entries.len() > 1 {
                entries.remove(0);
                entries.shrink_to_fit();
                self.subsongs.insert(idx, entries);
            }
            idx += 1;
        }
        self.songinfos = Some(infos);
        Ok(())
    }

    /// Port of `parse_modinfos`.
    ///
    /// A row whose body is empty repeats the previous format and channel
    /// count at a *relative* index; otherwise the index is absolute.
    fn parse_modinfos(&mut self, path: &Path) -> std::io::Result<()> {
        let mut infos = vec![ModInfoPacked::default(); self.hash_idx.len().max(1)];
        let mut idx: u32 = 0;
        let mut format: u16 = STRING_NOT_FOUND;
        let mut channels: u8 = 0;

        for line in read_lines(path)? {
            let line = line?;
            let (next_idx, len) = b64d24_prefix(&line);
            let tuple = &line[len..];
            if tuple.first() == Some(&b'\n') || tuple.is_empty() {
                idx += next_idx;
                infos[idx as usize] = ModInfoPacked { format, channels };
                continue;
            }
            idx = next_idx;
            let body = std::str::from_utf8(&tuple[1..]).map_err(invalid_data)?;
            let cols = split_view_x_n::<2>(body, '\t');
            if cols[0].as_bytes().first() != Some(&REPEAT) {
                format = self.format_pool.len() as u16;
                self.format_pool.push(cols[0].to_owned());
            }
            channels = if cols[1].is_empty() {
                0
            } else if cols[1].as_bytes()[0] == REPEAT {
                channels
            } else {
                crate::strings::from_chars::<u8>(cols[1]).unwrap_or(0)
            };
            infos[idx as usize] = ModInfoPacked { format, channels };
        }
        self.modinfos = Some(infos);
        Ok(())
    }

    /// Port of `parse_tsv` (metadata).
    ///
    /// Builds a sparse table indexed by hash index, then compacts it to the
    /// occupied rows — which leaves it sorted, the precondition the metadata
    /// lookup relies on.
    fn parse_metadata(&mut self, path: &Path) -> std::io::Result<()> {
        let n = self.hash_idx.len().max(1);
        let mut tmp = vec![MetaDataPacked::default(); n];
        let mut idx: u32 = 0;

        for line in read_lines(path)? {
            let line = line?;
            let (next_idx, len) = b64d24_prefix(&line);
            let tuple = &line[len..];
            let prev = tmp[idx as usize];
            if tuple.first() == Some(&b'\n') || tuple.is_empty() {
                // Body-less row: carry every field forward to a relative index.
                idx = prev.hash + next_idx;
                tmp[idx as usize] = MetaDataPacked { hash: idx, ..prev };
            } else {
                idx = next_idx;
                let body = std::str::from_utf8(&tuple[1..]).map_err(invalid_data)?;
                let mut item = self.parse_metadata_row(body, &prev);
                item.hash = idx;
                tmp[idx as usize] = item;
            }
        }

        // Compact, preserving ascending order.
        let table: Vec<MetaDataPacked> = tmp.into_iter().filter(|m| m.hash > 0).collect();
        self.metadata = Some(table);
        Ok(())
    }

    /// Port of `parse_tsv_row`.
    ///
    /// Four columns: authors, publishers, album, date. Each may be empty
    /// (absent), start with [`REPEAT`] (same as the previous row), or hold a
    /// [`SEPARATOR`]-joined list that is collapsed with [`AUTHOR_JOIN`].
    fn parse_metadata_row(&mut self, body: &str, prev: &MetaDataPacked) -> MetaDataPacked {
        let cols = split_view_x_n::<4>(body, '\t');
        let authors: Vec<&str> = cols[0].split(SEPARATOR).collect();
        let publishers: Vec<&str> = cols[1].split(SEPARATOR).collect();
        let album = cols[2];
        let date = cols[3];

        // Year: the first four characters of the date, as an offset from
        // 1900. A date shorter than four characters that is not a REPEAT
        // marker leaves the field at its default of 0 — the C's `if/else if`
        // chain has no final `else`.
        let year = if date.is_empty() {
            0
        } else if date.len() >= 4 {
            let year = crate::strings::from_chars::<i32>(&date[0..4]).unwrap_or(0);
            if year != 0 {
                (year - 1900) as u8
            } else {
                0
            }
        } else if date.as_bytes()[0] == REPEAT {
            prev.year
        } else {
            0
        };

        let author = if cols[0].is_empty() {
            UNKNOWN_AUTHOR
        } else if cols[0].as_bytes()[0] == REPEAT {
            prev.author
        } else if authors.len() == 1 {
            if authors[0].is_empty() {
                UNKNOWN_AUTHOR
            } else {
                push_pool(&mut self.author_pool, authors[0].to_owned())
            }
        } else {
            push_pool(
                &mut self.author_pool,
                crate::strings::mk_string(&authors, AUTHOR_JOIN),
            )
        };

        let publisher = if cols[1].is_empty() {
            STRING_NOT_FOUND
        } else if cols[1].as_bytes()[0] == REPEAT {
            prev.publisher
        } else if publishers.len() == 1 {
            // Note the asymmetry with authors: a single *empty* publisher is
            // still interned, rather than mapping to a sentinel.
            push_pool(&mut self.publisher_pool, publishers[0].to_owned())
        } else {
            push_pool(
                &mut self.publisher_pool,
                crate::strings::mk_string(&publishers, AUTHOR_JOIN),
            )
        };

        let album_idx = if album.is_empty() {
            STRING_NOT_FOUND
        } else if album.as_bytes()[0] == REPEAT {
            prev.album
        } else {
            push_pool(&mut self.album_pool, album.to_owned())
        };

        // `hash` is filled in by the caller, which knows the row index.
        MetaDataPacked { hash: 0, author, album: album_idx, publisher, year }
    }

    // ---- lookups -------------------------------------------------------

    /// Port of `_hashidx`.
    ///
    /// The C seeds an interpolation search with
    /// `((double)hash / HASH_T_MAX) * HASH_IDX_SIZE` and then walks linearly.
    /// Because `hash_idx` is strictly ascending (asserted at parse time),
    /// that walk finds the element if and only if a binary search does, so
    /// the seed is a performance detail and this uses `binary_search`.
    fn lookup_hash_index(&self, hash: Hash) -> Option<u32> {
        self.hash_idx.binary_search(&hash).ok().map(|i| i as u32)
    }

    fn hash_index_of(&self, hex: &str) -> Option<u32> {
        if hex.len() < 12 {
            return None;
        }
        let idx = self.lookup_hash_index(hex2hash(hex.as_bytes()))?;
        (idx != HASH_NOT_FOUND).then_some(idx)
    }

    fn make_subsong(&self, subsong: u8, info: SubSongPacked) -> SubSongInfo {
        SubSongInfo {
            subsong,
            songend: SongEnd::new(info.songend(), info.songlength_ms()),
            is_duplicate: info.is_duplicate(),
        }
    }

    fn pool_get(pool: &[String], s: u16) -> String {
        if s == STRING_NOT_FOUND {
            return String::new();
        }
        pool.get(s as usize).cloned().unwrap_or_default()
    }

    /// Port of `make_modinfo`.
    ///
    /// # Reproduced quirk
    ///
    /// The test is `format != STRING_NOT_FOUND || channels > 0`, and an
    /// index the modinfos file never mentions holds the C's zero-initialised
    /// `{ format: 0, channels: 0 }`. Since `0 != STRING_NOT_FOUND`, such a
    /// hash yields `Some` — reporting **the first format in the pool** with
    /// zero channels, rather than "no information". See `UPSTREAM_BUGS.md`.
    fn modinfo(&self, idx: u32) -> Option<ModInfo> {
        let table = self.modinfos.as_ref()?;
        let data = *table.get(idx as usize)?;
        (data.format != STRING_NOT_FOUND || data.channels > 0).then(|| ModInfo {
            format: Self::pool_get(&self.format_pool, data.format),
            channels: data.channels,
        })
    }

    /// Port of `make_meta` + `_find`. See [`Self::lookup_hash_index`] for
    /// why the C's seeded scan becomes a binary search.
    fn meta(&self, idx: u32) -> Option<MetaData> {
        let table = self.metadata.as_ref()?;
        let pos = table.binary_search_by_key(&idx, |m| m.hash).ok()?;
        let data = table[pos];
        Some(MetaData {
            author: Self::pool_get(&self.author_pool, data.author),
            album: Self::pool_get(&self.album_pool, data.album),
            publisher: Self::pool_get(&self.publisher_pool, data.publisher),
            year: if data.year > 0 { 1900 + data.year as u16 } else { 0 },
        })
    }

    /// Port of `songdb::lookup(hash, subsong)`.
    pub fn lookup_subsong(&self, hash: &str, subsong: u8) -> Option<SubSongInfo> {
        if let (Some(idx), Some(infos)) = (self.hash_index_of(hash), self.songinfos.as_ref()) {
            let info = *infos.get(idx as usize)?;
            if subsong < info.min_subsong {
                return None;
            }
            if info.min_subsong == subsong {
                return Some(self.make_subsong(subsong, info.info));
            }
            if info.has_subsongs {
                let extra = self.subsongs.get(&idx)?;
                let mut sub = info.min_subsong as u32 + 1;
                for e in extra {
                    if sub == subsong as u32 {
                        return Some(self.make_subsong(subsong, *e));
                    }
                    sub += 1;
                }
            }
            return None;
        }
        // Runtime additions.
        let h = hex2hash(hash.as_bytes());
        self.extra_subsongs
            .get(&h)?
            .iter()
            .find(|i| i.subsong == subsong)
            .filter(|i| i.songend.status != SongEndStatus::None)
            .cloned()
    }

    /// Port of `songdb::lookup(hash)`.
    pub fn lookup(&self, hash: &str) -> Option<Info> {
        if let Some(idx) = self.hash_index_of(hash) {
            let mut res = Info {
                subsongs: Vec::new(),
                modinfo: self.modinfo(idx),
                metadata: self.meta(idx),
            };
            let Some(infos) = self.songinfos.as_ref() else {
                return Some(res);
            };
            let info = *infos.get(idx as usize)?;
            res.subsongs.push(self.make_subsong(info.min_subsong, info.info));
            if info.has_subsongs {
                if let Some(extra) = self.subsongs.get(&idx) {
                    let mut sub = info.min_subsong.wrapping_add(1);
                    for e in extra {
                        res.subsongs.push(self.make_subsong(sub, *e));
                        sub = sub.wrapping_add(1);
                    }
                }
            }
            return Some(res);
        }

        let h = hex2hash(hash.as_bytes());
        let subsongs = self.extra_subsongs.get(&h);
        let modinfo = self.extra_modinfos.get(&h);
        if subsongs.is_none() && modinfo.is_none() {
            return None;
        }
        Some(Info {
            subsongs: subsongs.cloned().unwrap_or_default(),
            modinfo: modinfo.cloned(),
            metadata: None,
        })
    }

    /// Port of `songdb::update(hash, SubSongInfo, min, max)`.
    pub fn update_subsong(&mut self, hash: &str, info: SubSongInfo, min: u8, max: u8) {
        if crate::blacklist::is_blacklisted_songdb_hash(hash) {
            return;
        }
        assert!(min <= max, "minsubsong must not exceed maxsubsong");
        let h = hex2hash(hash.as_bytes());
        if let Some(infos) = self.extra_subsongs.get_mut(&h) {
            for old in infos.iter_mut() {
                if old.subsong == info.subsong {
                    // An existing, non-empty entry wins; the C logs and
                    // returns rather than overwriting.
                    if old.songend.status != SongEndStatus::None {
                        return;
                    }
                    *old = info;
                    return;
                }
            }
            return;
        }
        let infos = (min..=max)
            .map(|ss| {
                if ss == info.subsong {
                    info.clone()
                } else {
                    SubSongInfo {
                        subsong: ss,
                        songend: SongEnd::new(SongEndStatus::None, 0),
                        is_duplicate: false,
                    }
                }
            })
            .collect();
        self.extra_subsongs.insert(h, infos);
    }

    /// Port of `songdb::update(hash, ModInfo)`.
    pub fn update_modinfo(&mut self, hash: &str, info: ModInfo) {
        if hash.is_empty() || crate::blacklist::is_blacklisted_songdb_hash(hash) {
            return;
        }
        let h = hex2hash(hash.as_bytes());
        // First writer wins, as in the C.
        self.extra_modinfos.entry(h).or_insert(info);
    }

    /// Number of entries in the hash index — the C's `HASH_IDX_SIZE`.
    pub fn len(&self) -> usize {
        self.hash_idx.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hash_idx.is_empty()
    }
}

/// Interns a string and returns its pool index, as the C's `add_*` lambdas do.
///
/// The C pushes unconditionally — the pools hold duplicates by design (its
/// own comment says so), because deduplicating would cost more than the
/// repeated strings do.
fn push_pool(pool: &mut Vec<String>, s: String) -> u16 {
    let idx = pool.len() as u16;
    pool.push(s);
    idx
}

/// Yields raw lines *including* the trailing newline.
///
/// This matters: the parsers detect field widths by looking for `'\n'`
/// (`b64d24_prefix`, `b64diff2hash`) and strip a trailing byte
/// (`split_view_x_n`). Handing them newline-free lines would misparse every
/// row, and quietly — the values would be plausible.
fn read_lines(path: &Path) -> std::io::Result<impl Iterator<Item = std::io::Result<Vec<u8>>>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    Ok(std::iter::from_fn(move || {
        let mut buf = Vec::new();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => None,
            Ok(_) => Some(Ok(buf)),
            Err(e) => Some(Err(e)),
        }
    }))
}

fn invalid_data<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_bit_status_field_round_trips_including_error() {
        // ERROR is -1; a naive unsigned field reads it back as 31.
        assert_eq!(sign_extend_5(-1), -1);
        assert_eq!(sign_extend_5(0), 0);
        assert_eq!(sign_extend_5(11), 11);
        let p = SubSongPacked::new(100, SongEndStatus::Error, false);
        assert_eq!(p.songend(), SongEndStatus::Error);
        let p = SubSongPacked::new(100, SongEndStatus::NoSound, true);
        assert_eq!(p.songend(), SongEndStatus::NoSound);
        assert!(p.is_duplicate());
    }

    /// The packed widths truncate, and that truncation is observable in the
    /// reported song length. Storing these in a wider field would silently
    /// disagree with the C on any over-wide value.
    #[test]
    fn packed_fields_truncate_like_the_c_bitfields() {
        // 18-bit length field.
        let max = (1u32 << 18) - 1;
        assert_eq!(SubSongPacked::new(max, SongEndStatus::Player, false).songlength_ms(), max * 20);
        assert_eq!(SubSongPacked::new(1 << 18, SongEndStatus::Player, false).songlength_ms(), 0);
        assert_eq!(SubSongPacked::new((1 << 18) + 5, SongEndStatus::Player, false).songlength_ms(), 100);

        // 7-bit minimum subsong.
        let s = SongInfoPacked::new(false, 200, SubSongPacked::default());
        assert_eq!(s.min_subsong, 200 & 0x7f);
        assert_eq!(SongInfoPacked::new(false, 127, SubSongPacked::default()).min_subsong, 127);
    }

    #[test]
    fn songlength_is_stored_in_twenty_millisecond_units() {
        assert_eq!(SubSongPacked::new(0, SongEndStatus::Player, false).songlength_ms(), 0);
        assert_eq!(SubSongPacked::new(1, SongEndStatus::Player, false).songlength_ms(), 20);
        assert_eq!(SubSongPacked::new(50, SongEndStatus::Player, false).songlength_ms(), 1000);
    }

    #[test]
    fn with_duplicate_preserves_length_and_status() {
        let base = SubSongPacked::new(1234, SongEndStatus::DetectLoop, false);
        let dup = SubSongPacked::with_duplicate(base, true);
        assert_eq!(dup.songlength_ms(), base.songlength_ms());
        assert_eq!(dup.songend(), base.songend());
        assert!(dup.is_duplicate());
        assert!(!SubSongPacked::with_duplicate(dup, false).is_duplicate());
    }

    #[test]
    fn pool_lookup_maps_the_not_found_sentinel_to_empty() {
        let pool = vec!["zero".to_string(), "one".to_string()];
        assert_eq!(SongDb::pool_get(&pool, 0), "zero");
        assert_eq!(SongDb::pool_get(&pool, 1), "one");
        assert_eq!(SongDb::pool_get(&pool, STRING_NOT_FOUND), "");
        // Out of range must not panic in a lookup path.
        assert_eq!(SongDb::pool_get(&pool, 9), "");
    }

    #[test]
    fn hash_index_binary_search_finds_present_and_rejects_absent() {
        let db = SongDb { hash_idx: vec![0, 10, 20, 30, 40], ..Default::default() };
        assert_eq!(db.lookup_hash_index(0), Some(0));
        assert_eq!(db.lookup_hash_index(20), Some(2));
        assert_eq!(db.lookup_hash_index(40), Some(4));
        assert_eq!(db.lookup_hash_index(25), None);
        assert_eq!(db.lookup_hash_index(41), None);
    }

    #[test]
    fn short_hashes_are_rejected_rather_than_read_past_the_end() {
        let db = SongDb { hash_idx: vec![0, 10], ..Default::default() };
        assert_eq!(db.hash_index_of("abc"), None);
        assert_eq!(db.hash_index_of(""), None);
    }

    #[test]
    fn update_modinfo_keeps_the_first_writer() {
        let mut db = SongDb::default();
        db.update_modinfo("000000000001", ModInfo { format: "first".into(), channels: 4 });
        db.update_modinfo("000000000001", ModInfo { format: "second".into(), channels: 8 });
        let got = db.lookup("000000000001").unwrap().modinfo.unwrap();
        assert_eq!(got.format, "first");
        assert_eq!(got.channels, 4);
    }

    #[test]
    fn update_modinfo_refuses_blacklisted_and_empty_keys() {
        let mut db = SongDb::default();
        db.update_modinfo("af58e712052c", ModInfo { format: "x".into(), channels: 1 });
        assert!(db.lookup("af58e712052c").is_none());
        db.update_modinfo("", ModInfo { format: "x".into(), channels: 1 });
        assert!(db.extra_modinfos.is_empty());
    }

    #[test]
    fn update_subsong_fills_the_whole_declared_range() {
        let mut db = SongDb::default();
        let info = SubSongInfo {
            subsong: 3,
            songend: SongEnd::new(SongEndStatus::Player, 5000),
            is_duplicate: false,
        };
        db.update_subsong("000000000002", info, 1, 5);
        // The requested subsong carries its value...
        let got = db.lookup_subsong("000000000002", 3).unwrap();
        assert_eq!(got.songend.length, 5000);
        // ...and the rest are placeholders, which lookup filters out.
        assert!(db.lookup_subsong("000000000002", 1).is_none());
        assert_eq!(db.lookup("000000000002").unwrap().subsongs.len(), 5);
    }

    #[test]
    fn update_subsong_does_not_overwrite_a_populated_entry() {
        let mut db = SongDb::default();
        let mk = |len| SubSongInfo {
            subsong: 1,
            songend: SongEnd::new(SongEndStatus::Player, len),
            is_duplicate: false,
        };
        db.update_subsong("000000000003", mk(1000), 1, 2);
        db.update_subsong("000000000003", mk(9999), 1, 2);
        assert_eq!(db.lookup_subsong("000000000003", 1).unwrap().songend.length, 1000);
    }
}

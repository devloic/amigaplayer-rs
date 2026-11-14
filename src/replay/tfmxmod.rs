//! TFMX-MOD: a TFMX song and its samples in one file, as the NostalgicPlayer collections keep
//! the Turrican and Turrican 3 tunes (`... (TFMX-MOD).tfm`). The layout is the one
//! NostalgicPlayer's TFMX player reads (`TfmxIdentifier.IsTfmxModFile` and
//! `TfmxWorker.GetModuleLoadInfo`; Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer); this file is GPL-2.0-or-later, as the rest of amigaplayer-rs.
//!
//! The file: `TFMX-MOD`, then three little-endian longs: the offset of the samples, the offset
//! of an information block, a reserved long. The song part (an ordinary `mdat`) runs from 20
//! to the samples, the samples (an ordinary `smpl`) from there to the information block. The
//! block is a list of records, each a type byte, a little-endian length word and that many
//! bytes: 1 the author, 2 the game, 5 a flag byte, 6 the title; a type 0 ends it, and the fifth
//! byte after it is the song to start with (an entry of the song table). Several files hold the
//! same song and samples and differ only there (one per tune of a level).
//!
//! Nothing is converted: the song part and the samples are handed, in place, to the engine of
//! `tfmx.rs` that plays the same `mdat` with its `smpl` (Pro, 1.5 or 7 voices, by the plain
//! files' checks), which starts at the subsong of the start entry. The title is
//! "GAME - TITLE" from the block, as NostalgicPlayer shows it.

use super::{tfmx, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "TFMX-MOD", detect, new };

/// The container's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Head {
    /// The samples' offset (the song part's end).
    smpl: usize,
    /// The information block's offset (the samples' end).
    info: usize,
    /// The song-table entry to start with.
    start: u8,
    author: String,
    game: String,
    title: String,
}

fn le32(d: &[u8], o: usize) -> usize {
    d.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn text(b: &[u8]) -> String {
    // (Windows-1252 in NostalgicPlayer: Latin-1 for the letters these files use)
    let b = &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())];
    b.iter().map(|&c| c as char).collect::<String>().trim().to_string()
}

fn parse(d: &[u8]) -> Option<Head> {
    if !d.starts_with(b"TFMX-MOD") {
        return None;
    }
    let (smpl, info) = (le32(d, 8), le32(d, 12));
    // the song part long enough for its tables, the samples after it, both inside the file
    if smpl < 20 + 0x200 || info < smpl || info > d.len() {
        return None;
    }
    let mut h = Head { smpl, info, ..Head::default() };
    let mut p = info;
    while p < d.len() {
        let ty = d[p];
        if ty == 0 {
            h.start = d.get(p + 5).copied().unwrap_or(0);
            break;
        }
        let Some(len) = d.get(p + 1..p + 3).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize) else { break };
        let Some(body) = d.get(p + 3..p + 3 + len) else { break };
        match ty {
            1 => h.author = text(body),
            2 => h.game = text(body),
            6 => h.title = text(body),
            _ => {}
        }
        p += 3 + len;
    }
    Some(h)
}

fn detect(d: &[u8]) -> bool {
    parse(d).is_some_and(|h| tfmx::detect_any(&d[20..h.smpl]))
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let h = parse(d)?;
    let song = &d[20..h.smpl];
    if !tfmx::detect_any(song) {
        return None;
    }
    let inner = tfmx::new_with_samples(song, base + 20, base + h.smpl as u32, h.start, m)?;
    let title = match (h.game.is_empty(), h.title.is_empty()) {
        (false, false) => format!("{} - {}", h.game, h.title),
        (true, false) => h.title.clone(),
        (false, true) => h.game.clone(),
        (true, true) => String::new(),
    };
    Some(Box::new(TfmxMod { inner, title }))
}

/// The engine, with the file's title.
struct TfmxMod {
    inner: Box<dyn Replayer>,
    title: String,
}

impl Replayer for TfmxMod {
    fn format(&self) -> &'static str {
        self.inner.format()
    }
    fn title(&self) -> String {
        if self.title.is_empty() { self.inner.title() } else { self.title.clone() }
    }
    fn tick(&mut self, hw: &mut Hw) {
        self.inner.tick(hw)
    }
    fn tick_clocks(&self) -> u64 {
        self.inner.tick_clocks()
    }
    fn timer_delay(&self) -> u64 {
        self.inner.timer_delay()
    }
    fn next_tick_at(&self) -> Option<u64> {
        self.inner.next_tick_at()
    }
    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.inner.subsong_tick_at()
    }
    fn pc_music(&self) -> bool {
        self.inner.pc_music()
    }
    fn needs_fast_reference_cpu(&self) -> bool {
        self.inner.needs_fast_reference_cpu()
    }
    fn songend(&self) -> bool {
        self.inner.songend()
    }
    fn subsongs(&self) -> (u16, u16, u16) {
        self.inner.subsongs()
    }
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.inner.set_subsong(n, hw)
    }
    fn audio_irq(&mut self, hw: &mut Hw) -> bool {
        self.inner.audio_irq(hw)
    }
    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        self.inner.audio_irq_ch(ch, hw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TFMX-MOD file around `mdat` and `smpl`, with the information block of `start`.
    fn file(mdat: &[u8], smpl: &[u8], start: u8) -> Vec<u8> {
        let s = 20 + mdat.len();
        let i = s + smpl.len();
        let mut d = b"TFMX-MOD".to_vec();
        d.extend((s as u32).to_le_bytes());
        d.extend((i as u32).to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(mdat);
        d.extend(smpl);
        for (ty, b) in [(1u8, &b"Chris"[..]), (2, b"Game"), (5, b"\x03"), (6, b"Tune")] {
            d.push(ty);
            d.extend((b.len() as u16).to_le_bytes());
            d.extend(b);
        }
        d.extend([0, 1, 0, 0, 0, start, 0]);
        d
    }

    #[test]
    fn parses_the_container() {
        let mdat = vec![0u8; 0x300];
        let d = file(&mdat, &[1, 2, 3, 4], 8);
        let h = parse(&d).unwrap();
        assert_eq!((h.smpl, h.info, h.start), (0x314, 0x318, 8));
        assert_eq!((h.author.as_str(), h.game.as_str(), h.title.as_str()), ("Chris", "Game", "Tune"));
        // a song part that is not a TFMX song is not taken
        assert!(!detect(&d));
    }

    #[test]
    fn refuses_bad_offsets() {
        let mut d = file(&[0u8; 0x300], &[0; 4], 0);
        assert!(parse(&d).is_some());
        d[12..16].copy_from_slice(&(u32::MAX).to_le_bytes());
        assert!(parse(&d).is_none());
        d[8..12].copy_from_slice(&(0x10u32).to_le_bytes());
        assert!(parse(&d).is_none());
        assert!(parse(b"TFMX-MOD").is_none());
        // a truncated information block ends the list
        let d = file(&[0u8; 0x300], &[0; 4], 0);
        assert!(parse(&d[..d.len() - 9]).is_some());
    }
}

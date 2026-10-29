//! SoundTracker 2.6 (tag `MTN\0`) and IceTracker (`IT10`) modules, after NostalgicPlayer's
//! ModTracker player (Copyright (c) Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer): its identification (`ModTrackerIdentifier.TestModule`: the
//! tag at offset 1464) and its loader (`LoadSoundTracker26`: the number of tracks must be
//! the highest track referenced plus one, and no sample may miss more than 512 bytes).
//!
//! The header is a 31-instrument one: the title, 31 samples, the song length, the number
//! of tracks and then 128 positions of four track numbers each, one per voice, and the
//! tag. The tracks follow, 64 rows of a long each, and the samples. A ProTracker-family
//! player with independent tracks: the positions are laid out here as the patterns of a
//! ProTracker module (position `p` is pattern `p`, its voices the tracks), which the
//! built-in ProTracker plays from the samples where they lie in guest memory. The
//! reference, UADE's PTK-Prowiz, converts the modules to ProTracker modules too and plays
//! them as such; NostalgicPlayer's own SoundTracker effects for the two (the odd and even
//! speeds of ST2.6's Fxy, IceTracker's EFx) are not the reference's and are not followed.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::protracker::ProTracker;

pub const OPEN_ST26: Open = Open { name: "SoundTracker 2.6", detect: |d| kind(d) == Some(Kind::St26), new: |d, b, m| new(d, b, m, Kind::St26) };
pub const OPEN_ICE: Open = Open { name: "IceTracker", detect: |d| kind(d) == Some(Kind::Ice), new: |d, b, m| new(d, b, m, Kind::Ice) };

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    St26,
    Ice,
}

const HEADER: usize = 1468;

fn kind(d: &[u8]) -> Option<Kind> {
    if d.len() < HEADER {
        return None;
    }
    let k = match &d[1464..1468] {
        b"MTN\0" => Kind::St26,
        b"IT10" => Kind::Ice,
        _ => return None,
    };
    let (song_len, ntracks) = (d[950] as usize, d[951] as usize);
    // NostalgicPlayer reads the positions of up to 255 song positions from a table of 128
    // (and fails past it); the ProTracker layout has room for 128, and none for 0
    if song_len == 0 || song_len > 128 {
        return None;
    }
    if d[952..1464].iter().copied().max().unwrap() as usize + 1 != ntracks {
        return None;
    }
    let mut at = HEADER + ntracks * 256;
    if d.len() < at {
        return None;
    }
    for i in 0..31 {
        let o = 20 + i * 30;
        let len = u16::from_be_bytes([d[o + 22], d[o + 23]]) as usize * 2;
        if len != 0 {
            if d.len().saturating_sub(at) + 512 < len {
                return None;
            }
            at += len;
        }
    }
    Some(k)
}

/// The module laid out as a ProTracker (M.K.) module without its samples.
fn layout(d: &[u8]) -> Vec<u8> {
    let song_len = d[950] as usize;
    let mut m = vec![0u8; 1084 + song_len * 1024];
    m[..950].copy_from_slice(&d[..950]);
    m[950] = song_len as u8;
    m[951] = 0x7f;
    for p in 0..song_len {
        m[952 + p] = p as u8;
    }
    m[1080..1084].copy_from_slice(b"M.K.");
    for p in 0..song_len {
        for v in 0..4 {
            let t = d[952 + p * 4 + v] as usize;
            for r in 0..64 {
                let src = HEADER + t * 256 + r * 4;
                let dst = 1084 + p * 1024 + r * 16 + v * 4;
                m[dst..dst + 4].copy_from_slice(&d[src..src + 4]);
            }
            for r in 0..64 {
                let at = 1084 + p * 1024 + r * 16 + v * 4;
                // (the track's own cell: not a tempo put on the row before)
                let src = HEADER + t * 256 + r * 4;
                if d[src + 2] & 15 != 15 {
                    continue;
                }
                let arg = d[src + 3];
                if arg == 0 {
                    // F00 does nothing (ProTracker's would end the song)
                    m[at + 2] &= 0xf0;
                } else if arg >= 0x20 {
                    // two speeds, for the odd and the even rows: the reference plays the
                    // larger one at the tempo that keeps the rows' mean length, from the next
                    // row on, when that row's cell of the voice has no effect
                    let (x, y) = ((arg >> 4) as u32, (arg & 15) as u32);
                    let speed = x.max(y);
                    m[at + 3] = speed as u8;
                    let bpm = (250 * speed * 2 / (x + y)).div_ceil(2);
                    let next = at + 16;
                    if r < 63 && m[next + 2] & 15 == 0 && m[next + 3] == 0 {
                        m[next + 2] |= 15;
                        m[next + 3] = bpm as u8;
                    }
                }
            }
        }
    }
    m
}

struct St26 {
    name: &'static str,
    pt: ProTracker,
}

fn new(d: &[u8], base: u32, m: &mut Machine, k: Kind) -> Option<Box<dyn Replayer>> {
    if kind(d) != Some(k) {
        return None;
    }
    let samples = base + (HEADER + d[951] as usize * 256) as u32;
    let pt = ProTracker::new_at(&layout(d), base, Some(samples))?;
    pt.clear_loop_words(&mut m.bus.ram);
    let name = if k == Kind::St26 { OPEN_ST26.name } else { OPEN_ICE.name };
    Some(Box::new(St26 { name, pt }))
}

impl Replayer for St26 {
    fn format(&self) -> &'static str {
        self.name
    }

    fn title(&self) -> String {
        self.pt.title.clone()
    }

    fn songend(&self) -> bool {
        self.pt.songend
    }

    fn tick_clocks(&self) -> u64 {
        self.pt.tick_clocks()
    }

    fn tick(&mut self, hw: &mut Hw) {
        // as the engine runs the built-in ProTracker
        self.pt.tick(hw.bus);
        if self.pt.filter {
            hw.bus.ciaa.pra &= !2;
        } else {
            hw.bus.ciaa.pra |= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A module with one 64-byte sample (volume 48, no loop), `ntracks` tracks with `cells`
    /// at (track, row), and `positions` (four tracks each).
    fn module(tag: &[u8; 4], ntracks: usize, positions: &[[u8; 4]], cells: &[(usize, usize, [u8; 4])]) -> Vec<u8> {
        let mut d = vec![0u8; HEADER];
        d[..8].copy_from_slice(b"st26 mod");
        let o = 20;
        d[o..o + 4].copy_from_slice(b"smp1");
        d[o + 22..o + 24].copy_from_slice(&32u16.to_be_bytes());
        d[o + 25] = 48;
        d[o + 28..o + 30].copy_from_slice(&1u16.to_be_bytes());
        for i in 1..31 {
            d[20 + i * 30 + 28..20 + i * 30 + 30].copy_from_slice(&1u16.to_be_bytes());
        }
        d[950] = positions.len() as u8;
        d[951] = ntracks as u8;
        for (p, t) in positions.iter().enumerate() {
            d[952 + p * 4..956 + p * 4].copy_from_slice(t);
        }
        d[1464..1468].copy_from_slice(tag);
        let mut tr = vec![0u8; ntracks * 256];
        for &(t, r, c) in cells {
            tr[t * 256 + r * 4..t * 256 + r * 4 + 4].copy_from_slice(&c);
        }
        d.extend(tr);
        d.extend((0..64).map(|i| (i * 4) as u8));
        d
    }

    fn cell(p: u16, s: u8, e: u8, a: u8) -> [u8; 4] {
        [(s & 0xf0) | (p >> 8) as u8, p as u8, (s << 4) | e, a]
    }

    #[test]
    fn identifies_the_tags() {
        let d = module(b"MTN\0", 2, &[[0, 1, 0, 1]], &[]);
        assert_eq!(kind(&d), Some(Kind::St26));
        let mut ice = d.clone();
        ice[1464..1468].copy_from_slice(b"IT10");
        assert_eq!(kind(&ice), Some(Kind::Ice));
        // the number of tracks must be the highest one referenced plus one
        let mut bad = d.clone();
        bad[951] = 3;
        assert_eq!(kind(&bad), None);
        // no tag, song length 0, too short, missing most of a sample
        bad = d.clone();
        bad[1464..1468].copy_from_slice(b"M.K.");
        assert_eq!(kind(&bad), None);
        bad = d.clone();
        bad[950] = 0;
        assert_eq!(kind(&bad), None);
        assert_eq!(kind(&d[..1467]), None);
        bad = d.clone();
        bad[20 + 22..20 + 24].copy_from_slice(&1000u16.to_be_bytes());
        assert_eq!(kind(&bad), None);
        // and the 15-instrument SoundTrackers leave them alone
        assert_eq!(crate::replay::soundtracker::variant(&d), None);
    }

    #[test]
    fn lays_the_tracks_out_as_patterns() {
        let d = module(b"MTN\0", 3, &[[0, 1, 2, 0], [2, 2, 1, 0]], &[(1, 0, cell(428, 1, 0xc, 0x20)), (2, 63, cell(214, 1, 0, 0))]);
        let m = layout(&d);
        assert_eq!(&m[1080..1084], b"M.K.");
        assert_eq!(&m[950..954], &[2, 0x7f, 0, 1]);
        assert_eq!(m.len(), 1084 + 2 * 1024);
        // position 0: voice 1 plays track 1, voice 2 track 2
        assert_eq!(&m[1084 + 4..1084 + 8], &cell(428, 1, 0xc, 0x20));
        assert_eq!(&m[1084 + 63 * 16 + 8..1084 + 63 * 16 + 12], &cell(214, 1, 0, 0));
        // position 1: track 2 on voices 0 and 1, track 1 on voice 2
        let p1 = 1084 + 1024;
        assert_eq!(&m[p1 + 63 * 16..p1 + 63 * 16 + 4], &cell(214, 1, 0, 0));
        assert_eq!(&m[p1 + 63 * 16 + 4..p1 + 63 * 16 + 8], &cell(214, 1, 0, 0));
        assert_eq!(&m[p1 + 8..p1 + 12], &cell(428, 1, 0xc, 0x20));
    }

    #[test]
    fn converts_the_two_speeds() {
        let cells = [
            (0, 0, cell(428, 1, 0xf, 0x34)),
            (0, 8, cell(428, 1, 0xf, 0x31)),
            (0, 16, cell(428, 1, 0xf, 0x2f)),
            (0, 17, cell(428, 1, 0xc, 0x20)),
            (0, 24, cell(428, 1, 0xf, 0x13)),
            (0, 32, cell(428, 1, 0xf, 0x00)),
            (0, 63, cell(428, 1, 0xf, 0x70)),
        ];
        let d = module(b"MTN\0", 1, &[[0, 0, 0, 0]], &cells);
        let m = layout(&d);
        let at = |r: usize| &m[1084 + r * 16..1084 + r * 16 + 4];
        // the larger speed, and on the next row the tempo of the two's mean: 125 * 4 / 3.5
        assert_eq!(at(0), &cell(428, 1, 0xf, 4));
        assert_eq!(at(1), &cell(0, 0, 0xf, 143));
        // 187.5 rounds up
        assert_eq!(at(8), &cell(428, 1, 0xf, 3));
        assert_eq!(at(9), &cell(0, 0, 0xf, 188));
        // the next row's cell has an effect of its own: no tempo
        assert_eq!(at(16), &cell(428, 1, 0xf, 15));
        assert_eq!(at(17), &cell(428, 1, 0xc, 0x20));
        // a parameter under 0x20 is a speed; F00 does nothing; the last row has no next one
        assert_eq!(at(24), &cell(428, 1, 0xf, 0x13));
        assert_eq!(at(32), &cell(428, 1, 0, 0));
        assert_eq!(at(63), &cell(428, 1, 0xf, 7));
    }

    #[test]
    fn plays_from_the_samples_in_place() {
        let d = module(b"IT10", 2, &[[1, 0, 0, 0]], &[(1, 0, cell(428, 1, 0xc, 0x20))]);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ice"), d.clone()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "IceTracker");
        let mut out = vec![0f32; 2 * 960 * 10];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        assert!(reg(0xDFF0A6).contains(&428));
        assert!(reg(0xDFF0A4).contains(&32));
        assert!(reg(0xDFF0A8).contains(&0x20));
        // AUDxLC is the sample's own address in the module: after the two tracks
        let lc: Vec<usize> = reg(0xDFF0A0).iter().zip(reg(0xDFF0A2)).map(|(&h, l)| ((h as usize) << 16) | l as usize).collect();
        assert!(lc.iter().any(|&at| at != 0 && e.m.bus.ram[at + 2..at + 8] == d[HEADER + 512 + 2..HEADER + 512 + 8]));
        assert_eq!(crate::open::detect(&d).as_deref(), Some("IceTracker"));
    }
}

//! CustomMade (the Eagleplayer "CustomMade", prefixes `cm`, `rk`, `rkb`): raw memory dumps of
//! Ron Klaren / Ivo Zoer game sound drivers with their music, played by one fixed replayer of
//! its own. The dump's code is not run: the music tables are found in the image by marker byte
//! sequences of the driver (they are properties of the module files), the dump's absolute
//! pointers are translated by the image's origin, and the tracks, instruments, arpeggios,
//! vibratos and envelopes are played as the behaviour specification says.
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player; no original code. The only player data is the 84-entry period
//! table (typed in from the specification). The hunk-wrapped Ron Klaren modules belong to
//! `ronklaren.rs`; they fail the entry-word rule here.
//!
//! The writes come at the reference's times: each tick keeps a cycle count, with the fitted step
//! costs of the specification, and a write falls half the cycles after the call. The start (the
//! host's volume calls and the init writes) is placed by the specification's fits.
//!
//! Choices where the specification is silent or the reference undefined: a dump without the
//! `ptoff`, songs, arpeggio, instrument or timer marker (or no subsong) is not recognised;
//! period lookups outside the 84-entry table give 0 (index 84 is the status word `0x003F`);
//! a row that never ends (a pass loop of more than 4096 passes) stops the song; reads outside
//! the image give 0 and writes outside it are dropped; a subsong number of 16 or more is
//! refused. The image is modified as the specification says (phase steps, the 0x270F patch)
//! and not restored between subsongs.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "CustomMade", detect, new };

/// The player's own 84-entry period table.
#[rustfmt::skip]
const PERIODS: [u16; 84] = [
    0x1AC0, 0x1940, 0x17D0, 0x1680, 0x1530, 0x1400, 0x12E0, 0x11D0, 0x10D0, 0x0FE0, 0x0F00, 0x0E20,
    0x0D60, 0x0CA0, 0x0BE8, 0x0B40, 0x0A98, 0x0A00, 0x0970, 0x08E8, 0x0868, 0x07F0, 0x0780, 0x0710,
    0x06B0, 0x0650, 0x05F4, 0x05A0, 0x054C, 0x0500, 0x04B8, 0x0474, 0x0434, 0x03F8, 0x03C0, 0x0388,
    0x0358, 0x0328, 0x02FA, 0x02D0, 0x02A6, 0x0280, 0x025C, 0x023A, 0x021A, 0x01FC, 0x01E0, 0x01C4,
    0x01AC, 0x0194, 0x017D, 0x0168, 0x0153, 0x0140, 0x012E, 0x011D, 0x010D, 0x00FE, 0x00F0, 0x00E2,
    0x00D6, 0x00CA, 0x00BE, 0x00B4, 0x00AA, 0x00A0, 0x0097, 0x008F, 0x0087, 0x007F, 0x0078, 0x0071,
    0x006B, 0x0065, 0x005F, 0x005A, 0x0055, 0x0050, 0x004B, 0x0047, 0x0043, 0x003F, 0x003C, 0x0038,
];

/// The 12-byte marker of the detection rule.
const MARKER: [u8; 12] = [0x42, 0x28, 0x00, 0x30, 0x42, 0x28, 0x00, 0x31, 0x42, 0x28, 0x00, 0x32];

// Driver byte sequences the init scan looks for (long words at even offsets).
const M_ARPS: u32 = 0xD04149FA;
const M_SONGS_A: u32 = 0x48E7F8FC;
const M_SONGS_B: u32 = 0x48E7FFFE;
const M_PTOFF: u32 = 0xE94847F0;
const M_TIMER: u32 = 0x00BFD500;
const M_INSTRS: u32 = 0x42A8001C;
const M_VOC_A: u32 = 0xE44843FA;
const M_VOC_B: u32 = 0xE448207B;
const M_VOC0: u32 = 0x48E700F0;
/// The first two entries of the driver's own period table: the scan stops there.
const M_STOP: u32 = 0x1AC01940;
/// The word the songs search looks for.
const W_SONGS: u16 = 0x41FA;

/// The step costs of the tick, in cycles (fitted; only sums along a path matter).
const PER_K: [u64; 4] = [110, 141, 172, 180];
const SMP_K: [u64; 4] = [186, 217, 248, 256];
const VPRE_K: [u64; 4] = [0, 23, 46, 36];
const VPOST_K: [u64; 4] = [90, 121, 152, 0];
const WRAP_K: [u64; 4] = [0, 33, 62, 81];

/// Colour clocks between the host's eight volume writes.
const HOST_GAPS: [u64; 7] = [124, 136, 131, 241, 124, 136, 131];
/// The stop of a subsong change: DMA off 131 colour clocks after the last init write, then the
/// four volume writes 8, 6, 6, 6 colour clocks apart; the next host volume calls follow 224 later.
const STOP_GAPS: [u64; 5] = [131, 8, 6, 6, 6];
const STOP_TO_HOST: u64 = 224;
/// The first tick's DMA write comes this many colour clocks after the tick period past the last
/// init write.
const FIRST_TICK_DELAY: u64 = 312;
/// Cycles from a tick's call to its first write.
const FIRST_WRITE: u64 = 166;
/// A song stays out of subsong numbers of 16 or more.
const MAX_SUBSONGS: usize = 15;
/// Passes of one row parse before the song is stopped.
const MAX_PASSES: u32 = 4096;

/// What the init scan finds (image offsets).
#[derive(Clone, Debug)]
struct Layout {
    songs: usize,
    arps: usize,
    instrs: usize,
    timer: u16,
    origin: u32,
    /// Scan positions examined, the marker offset and the words looked at: the start-up's fit.
    positions: u64,
    marker: usize,
    q: u64,
}

fn rw(d: &[u8], o: usize) -> u16 {
    be16(d, o)
}

fn rl(d: &[u8], o: usize) -> u32 {
    be32(d, o)
}

fn s16(w: u16) -> i64 {
    w as i16 as i64
}

/// The offset of the marker (an even offset, 8 <= o < 408), when the file has one.
fn find_marker(d: &[u8]) -> Option<usize> {
    (8..408usize).step_by(2).find(|&o| d.get(o..o + 12).is_some_and(|s| s == MARKER))
}

fn entry_ok(d: &[u8]) -> bool {
    let w0 = rw(d, 0);
    ((w0 == 0x4EF9 || w0 == 0x4EB9) && rw(d, 6) == 0x4EF9) || (w0 == 0x6000 && rw(d, 4) == 0x6000)
}

/// An image offset from a base and a signed word offset, when it lies inside the image.
fn off(d: &[u8], base: usize, delta: i64) -> Option<usize> {
    let v = base as i64 + delta;
    (v >= 0 && (v as usize) < d.len()).then_some(v as usize)
}

/// The init scan of the specification (1.3).
fn scan(d: &[u8]) -> Option<Layout> {
    let marker = find_marker(d)?;
    let (mut songs, mut arps, mut instrs, mut ptoff, mut voc0) = (None, None, None, None, None);
    let mut timer = 0u16;
    let (mut origin, mut oldplay) = (0u32, 0u32);
    let (mut positions, mut q_words) = (0u64, 0u64);
    let mut o = 0usize;
    while positions < 2048 && o + 4 <= d.len() {
        positions += 1;
        let l = rl(d, o);
        if l == M_ARPS {
            arps = off(d, o + 4, s16(rw(d, o + 4)));
        } else if l == M_SONGS_A || l == M_SONGS_B {
            if oldplay != 0 && origin == 0 {
                origin = oldplay.wrapping_sub(o as u32);
            }
            let found = (0..21usize).find(|&i| rw(d, o + 2 * i) == W_SONGS);
            match found {
                Some(i) => {
                    q_words += i as u64 + 1;
                    let qq = o + 2 * i;
                    songs = off(d, qq + 2, s16(rw(d, qq + 2)));
                    o = qq + 2;
                }
                None => {
                    q_words += 21;
                    o += 42;
                }
            }
        } else if l == M_PTOFF {
            ptoff = Some(s16(rw(d, o + 4)));
        } else if l == M_TIMER {
            let hi = if o > 0 { d.get(o - 1).copied().unwrap_or(0) } else { 0 };
            let lo = d.get(o + 7).copied().unwrap_or(0);
            timer = (hi as u16) << 8 | lo as u16;
            if rw(d, o + 20) == 0x4E71 {
            } else if rw(d, o + 28) == 0x21FC {
                oldplay = rl(d, o + 30);
            } else if rw(d, o + 32) == 0xC000 {
                oldplay = rl(d, o + 22);
            } else {
                oldplay = rl(d, o + 32);
            }
        } else if l == M_INSTRS {
            instrs = off(d, o + 6, s16(rw(d, o + 6)));
        } else if l == M_VOC_A || l == M_VOC_B {
            if let Some(v0) = voc0 {
                let p = off(d, o + 4, s16(rw(d, o + 4))).map_or(0, |a| rl(d, a));
                if origin == 0 {
                    origin = p.wrapping_sub(v0 as u32);
                }
            }
        } else if l == M_VOC0 {
            voc0 = off(d, o + 6, s16(rw(d, o + 6)));
        }
        o += 2;
        if rl(d, o) == M_STOP {
            break;
        }
    }
    let songs = songs? as i64 + ptoff?;
    if songs < 0 || songs as usize >= d.len() || timer == 0 {
        return None;
    }
    Some(Layout { songs: songs as usize, arps: arps?, instrs: instrs?, timer, origin, positions, marker, q: q_words })
}

/// The number of subsongs: 16-byte entries up to the first voice record (hardware address of
/// audio channel 0 in its first long).
fn count_subsongs(d: &[u8], songs: usize) -> usize {
    let mut n = 0;
    while n < 64 && songs + 16 * n + 4 <= d.len() && rl(d, songs + 16 * n) & 0x00FF_FFFF != 0x00DF_F0A0 {
        n += 1;
    }
    n
}

fn detect(d: &[u8]) -> bool {
    d.len() > 3000 && entry_ok(d) && scan(d).is_some_and(|l| count_subsongs(d, l.songs) > 0)
}

/// A voice's state.
#[derive(Clone, Copy, Default)]
struct Voice {
    note: u8,
    transp: u16,
    period: u16,
    target: u16,
    pspeed: u8,
    row: u8,
    vdelay: u8,
    vpos: u16,
    pdelay: u8,
    estage: u8,
    ecnt: u8,
    espeed: u8,
    vol: u8,
    arp: [u8; 12],
    aidx: u8,
    /// Image offset of the instrument record.
    instr: usize,
    /// Track pointer, and the track list's start and current entry (image offsets as u32 so that
    /// a wild pointer stays wild).
    p: u32,
    tl_start: u32,
    tl_cur: u32,
    rep: u16,
    oneshot: bool,
}

pub struct CustomMade {
    img: Vec<u8>,
    base: u32,
    lay: Layout,
    nsub: usize,
    sub: usize,
    v: [Voice; 4],
    playing: bool,
    /// Per voice: the song-end flag (set = the voice's list has not wrapped since the last report).
    flags: [bool; 4],
    songend: bool,
    lastvol: [u8; 4],
    // the tick's clock model
    t0: u64,
    cyc: u64,
    stops: u32,
    // timeline bookkeeping
    last_init_len: u64,
    first_tick: Option<u64>,
    period_cc: u64,
    instruments: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let lay = scan(d)?;
    let nsub = count_subsongs(d, lay.songs).min(MAX_SUBSONGS);
    if nsub == 0 {
        return None;
    }
    let mut instruments = 0u64;
    while instruments < 256 {
        let o = lay.instrs + 32 * instruments as usize;
        if o + 32 > d.len() || rl(d, o + 28) != 0 {
            break;
        }
        instruments += 1;
    }
    let mut r = CustomMade {
        img: d.to_vec(),
        base,
        period_cc: 5 * (lay.timer as u64 + 1),
        lay,
        nsub,
        sub: 0,
        v: [Voice::default(); 4],
        playing: false,
        flags: [true; 4],
        songend: false,
        lastvol: [0; 4],
        t0: 0,
        cyc: 0,
        stops: 0,
        last_init_len: 0,
        first_tick: None,
        instruments,
    };
    let mut hw = Hw { bus: &mut m.bus };
    // the host's first volume call comes after the init scan (fitted to the reference)
    // (plus a constant and a term in the image size, the host's load: fitted to the corpus,
    // the spec's own terms alone leave 430 to 470 colour clocks)
    let first = 427
        + d.len() as u64 * 862 / 1_000_000
        + 10
        + 154 * r.lay.positions
        + 77 * r.instruments
        + 38 * r.nsub as u64
        + 26 * ((r.lay.marker as u64 - 8) / 2)
        + 17 * r.lay.q;
    r.at(&mut hw, first);
    r.host_volumes(&mut hw);
    r.init_sound(0, &mut hw);
    Some(Box::new(r))
}

impl CustomMade {
    fn at(&self, hw: &mut Hw, t: u64) {
        if t > hw.bus.clock {
            hw.bus.clock = t;
        }
    }

    // ---- image access (reads outside the image give 0) ----
    fn b(&self, o: usize) -> u8 {
        self.img.get(o).copied().unwrap_or(0)
    }
    fn w(&self, o: usize) -> u16 {
        rw(&self.img, o)
    }
    fn l(&self, o: usize) -> u32 {
        rl(&self.img, o)
    }
    fn rel(&self, p: u32) -> u32 {
        p.wrapping_sub(self.lay.origin)
    }
    fn rel_us(&self, p: u32) -> usize {
        self.rel(p) as usize
    }
    fn set_b(&mut self, hw: Option<&mut Hw>, o: i64, v: u8) {
        if o >= 0 && (o as usize) < self.img.len() {
            self.img[o as usize] = v;
            if let Some(hw) = hw {
                hw.bus.write_bytes(self.base.wrapping_add(o as u32), &[v]);
            }
        }
    }

    /// The period table lookup by signed index; outside the table: the status word at 84, else 0.
    fn period_at(&self, idx: i32) -> u16 {
        match idx {
            0..=83 => PERIODS[idx as usize],
            84 => {
                if self.playing {
                    0x003F
                } else {
                    0x013F
                }
            }
            _ => 0,
        }
    }
    /// The entry at 16-bit byte offset `off` (read as signed).
    fn period_off(&self, off: u16) -> u16 {
        self.period_at((off as i16 as i32) >> 1)
    }
    /// `period[n + transposition]` with the clean 16-bit index.
    fn period_clean(&self, n: u8, transp: u16) -> u16 {
        self.period_at((n as u16).wrapping_add(transp) as i16 as i32)
    }

    // ---- the clock model ----
    fn cost(&mut self, c: u64) {
        self.cyc += c;
    }
    fn stamp(&self, hw: &mut Hw) {
        let t = self.t0 + self.cyc / 2;
        if t > hw.bus.clock {
            hw.bus.clock = t;
        }
    }
    fn w_dma(&self, hw: &mut Hw, v: u16) {
        self.stamp(hw);
        hw.dmacon(v);
    }
    fn w_loc(&self, hw: &mut Hw, k: usize, a: u32) {
        self.stamp(hw);
        hw.loc(k, a);
    }
    fn w_len(&self, hw: &mut Hw, k: usize, n: u16) {
        self.stamp(hw);
        hw.len(k, n);
    }
    fn w_per(&self, hw: &mut Hw, k: usize, p: u16) {
        self.stamp(hw);
        hw.per(k, p);
    }
    fn w_vol(&self, hw: &mut Hw, k: usize, v: u16) {
        self.stamp(hw);
        hw.vol(k, v);
    }

    // ---- host and init ----
    /// The host's volume and balance calls, twice: eight volume writes of the last volumes.
    fn host_volumes(&mut self, hw: &mut Hw) {
        for round in 0..2 {
            for k in 0..4 {
                let vol = self.lastvol[k] as u16;
                hw.vol(k, vol);
                let i = round * 4 + k;
                if i < 7 {
                    hw.wait(HOST_GAPS[i]);
                }
            }
        }
    }

    /// The sample descriptor of an instrument record: (data image offset, address, length words).
    fn sample_of(&self, instr: usize) -> (u32, u16) {
        let d = self.rel_us(self.l(instr));
        let data = self.l(d);
        (self.base.wrapping_add(self.rel(data)), self.w(d + 4))
    }

    /// Track list entries of voice `k` of subsong entry `e`, and the E / U figures of the host
    /// time fit.
    fn list_counts(&self, e: usize) -> (u64, u64) {
        let (mut total, mut u, mut max) = (0u64, 0u64, 0u64);
        for k in 0..4 {
            let tl = self.rel_us(self.l(self.lay.songs + 16 * e + 4 * k));
            let mut n = 0u64;
            // (counted up to a long of 0xFFFFFFFF: a list that lacks it runs on into the data)
            while n < 4096 {
                let at = tl.wrapping_add(12 * n as usize);
                if at >= self.img.len() || self.l(at) == 0xFFFF_FFFF {
                    break;
                }
                n += 1;
            }
            total += n;
            if n > max {
                u += 1;
                max = n;
            }
        }
        (total, u)
    }

    /// InitSound(n): state, then the writes of the init at their fitted times. `n` is 0-based.
    fn init_sound(&mut self, n: usize, hw: &mut Hw) {
        self.sub = n;
        self.playing = false;
        self.songend = false;
        self.flags = [true; 4];
        let e = n.min(MAX_SUBSONGS - 1);
        let (ent, u) = self.list_counts(e);
        // the first init write falls this long after the last host volume write
        let t_first = hw.bus.clock + (2411 + 49 * ent + 15 * u + 1) / 2;
        let mut cyc = 0u64;
        for k in 0..4 {
            let tl = self.rel(self.l(self.lay.songs + 16 * e + 4 * k));
            let tl_us = tl as usize;
            let mut patched = 0;
            if self.w(tl_us + 10) == 0x270F {
                self.set_b(Some(hw), tl as i64 + 10, 0x00);
                self.set_b(Some(hw), tl as i64 + 11, 0x3E);
                // (the reference spends 9 colour clocks more on the voice that gets the patch)
                patched = 18;
            }
            let rep = self.w(tl_us + 10).wrapping_sub(1);
            let p = self.rel(self.l(tl_us));
            let transp = self.w(tl_us + 6);
            let instr = self.lay.instrs;
            let v = &mut self.v[k];
            let (period, target) = (v.period, v.target);
            *v = Voice {
                period,
                target,
                oneshot: true,
                instr,
                p,
                transp,
                rep,
                tl_start: tl,
                tl_cur: tl,
                ..Voice::default()
            };
            let (addr, len) = self.sample_of(instr);
            let t = t_first + cyc / 2;
            self.at(hw, t);
            hw.loc(k, addr);
            self.at(hw, t + 10);
            hw.len(k, len);
            self.last_init_len = hw.bus.clock;
            cyc += 775 + patched;
        }
        self.playing = true;
        self.first_tick = Some(self.last_init_len + self.period_cc + FIRST_TICK_DELAY - FIRST_WRITE / 2);
    }
}

impl Replayer for CustomMade {
    fn format(&self) -> &'static str {
        "CustomMade"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        self.period_cc
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.first_tick
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        // (numbered from 1, as the Eagleplayer does)
        (1, self.nsub as u16, self.sub as u16 + 1)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).saturating_sub(1).min(self.nsub - 1);
        // the host stops (DMA off, the four volumes to 0), calls volume and balance again, and
        // runs InitSound
        let mut t = self.last_init_len + STOP_GAPS[0];
        self.at(hw, t);
        hw.dmacon(0x000F);
        for k in 0..4 {
            t += STOP_GAPS[k + 1];
            self.at(hw, t);
            hw.vol(k, 0);
        }
        self.lastvol = [0; 4];
        self.at(hw, t + STOP_TO_HOST);
        self.host_volumes(hw);
        self.init_sound(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.playing {
            return;
        }
        self.t0 = hw.bus.clock;
        self.cyc = 0;
        self.stops = 0;
        for k in 0..4 {
            self.voice(k, hw);
        }
    }
}

// ---- the tick ----
impl CustomMade {
    fn voice(&mut self, k: usize, hw: &mut Hw) {
        let bit = 1u16 << k;
        self.cost(FIRST_WRITE);
        self.w_dma(hw, 0x8000 | bit);
        if self.v[k].oneshot {
            self.v[k].oneshot = false;
            self.cost(53);
            self.w_len(hw, k, 1);
        } else {
            self.cost(21);
        }
        if self.v[k].row == 0 {
            self.parse_row(k, hw);
        } else {
            self.v[k].row -= 1;
            self.effects(k, hw);
        }
    }

    /// Stop (0x83 / 0x85): the status becomes stopped and the song end is reported.
    fn stop(&mut self) {
        self.playing = false;
        self.songend = true;
        self.stops += 1;
    }

    /// The writes of a sample start: DMA off for the voice, the start and the length of the
    /// current instrument's sample (the costs between them are the caller's).
    fn sample_writes(&mut self, k: usize, hw: &mut Hw) {
        self.w_dma(hw, 1 << k);
        self.cost(108);
        let (addr, len) = self.sample_of(self.v[k].instr);
        self.w_loc(hw, k, addr);
        self.cost(20);
        self.w_len(hw, k, len);
        self.cost(SMP_K[k]);
    }

    fn parse_row(&mut self, k: usize, hw: &mut Hw) {
        self.v[k].pspeed = 0;
        let del = self.b(self.v[k].instr + 13);
        if del != 0 {
            self.v[k].vdelay = del.wrapping_mul(4).wrapping_sub(1);
            self.cost(159);
        } else {
            self.cost(133);
        }
        // the scratch value: its high byte matters for zero-duration notes
        let mut s: u16 = 0x8000;
        for _ in 0..MAX_PASSES {
            self.cost(47);
            let mut p = self.v[k].p as usize;
            // arpeggio
            if self.b(p) == 0x80 {
                let x = self.b(p + 1) as usize;
                for i in 0..12 {
                    self.v[k].arp[i] = self.b(self.lay.arps + 12 * x + i);
                }
                self.v[k].aidx = 0;
                s = (12 * x) as u16;
                p += 2;
                self.cost(180);
            }
            // portamento
            if self.b(p) == 0x81 {
                let (n, sp, d) = (self.b(p + 1), self.b(p + 2), self.b(p + 3));
                let target = self.period_clean(n, self.v[k].transp);
                let v = &mut self.v[k];
                v.estage = 0;
                v.target = target;
                v.pspeed = sp;
                v.row = d.wrapping_mul(4).wrapping_sub(1);
                v.p = (p + 4) as u32;
                let per = v.period;
                self.cost(124);
                self.w_per(hw, k, per);
                self.cost(PER_K[k]);
                return;
            }
            // instrument
            if self.b(p) == 0x82 {
                let i = self.b(p + 1) as usize;
                let instr = self.lay.instrs + 32 * i;
                let v = &mut self.v[k];
                v.vol = 0;
                v.estage = 0;
                v.instr = instr;
                s = (32 * i) as u16;
                p += 2;
                v.p = p as u32;
                self.cost(96);
                self.sample_writes(k, hw);
            }
            // stop
            if self.b(p) == 0x83 {
                self.cost(110 + u64::from(self.stops > 0));
                self.stop();
                return;
            }
            self.cost(152);
            // envelope speed
            if self.b(p) == 0x84 {
                self.v[k].espeed = self.b(p + 1);
                p += 2;
                self.cost(28);
            }
            // the second stop
            if self.b(p) == 0x85 {
                self.cost(u64::from(self.stops > 0));
                self.stop();
                return;
            }
            self.cost(49);
            if self.b(p) & 0x80 != 0 {
                self.v[k].p = p as u32;
                self.end_of_track(k, &mut s);
                continue;
            }
            // a note
            let (n, d) = (self.b(p), self.b(p + 1));
            if d == 0 {
                let o = ((s & 0xFF00) | n as u16).wrapping_add(self.v[k].transp).wrapping_mul(2);
                let per = self.period_off(o);
                let v = &mut self.v[k];
                v.note = n;
                v.period = per;
                v.p = (p + 2) as u32;
                s = o;
                continue;
            }
            let per = self.period_clean(n, self.v[k].transp);
            let v = &mut self.v[k];
            v.row = d.wrapping_mul(4).wrapping_sub(1);
            v.estage = 0;
            v.note = n;
            v.period = per;
            v.p = (p + 2) as u32;
            self.sample_writes(k, hw);
            if self.b(self.v[k].instr + 8) != 0 {
                self.v[k].oneshot = true;
                self.cost(16);
            }
            self.w_per(hw, k, per);
            self.cost(PER_K[k]);
            return;
        }
        // a row that never ends: stop the song
        self.stop();
    }

    /// An end-of-track byte at the track pointer: repeat the track, or go to the next entry of
    /// the list (wrapping at its end).
    fn end_of_track(&mut self, k: usize, s: &mut u16) {
        if self.v[k].rep != 0 {
            self.v[k].rep -= 1;
            self.cost(42);
            let e = self.v[k].tl_cur as usize;
            self.v[k].p = self.rel(self.l(e));
            self.v[k].transp = self.w(e + 6);
            return;
        }
        self.cost(139);
        self.v[k].tl_cur = self.v[k].tl_cur.wrapping_add(12);
        if self.b(self.v[k].tl_cur as usize) & 0x80 != 0 {
            self.v[k].tl_cur = self.v[k].tl_start;
            self.flags[k] = false;
            self.cost(WRAP_K[k]);
            if self.flags.iter().all(|f| !f) {
                self.songend = true;
                self.flags = [true; 4];
                self.cost(282);
            } else {
                self.cost(198);
            }
        }
        let e = self.v[k].tl_cur as usize;
        let rep = self.w(e + 10).wrapping_sub(1);
        self.v[k].p = self.rel(self.l(e));
        self.v[k].transp = self.w(e + 6);
        self.v[k].rep = rep;
        *s = rep;
    }

    fn effects(&mut self, k: usize, hw: &mut Hw) {
        self.cost(200);
        let ins = self.v[k].instr;
        // phase
        let speed = self.b(ins + 9);
        if speed != 0 {
            if self.v[k].pdelay != 0 {
                self.v[k].pdelay -= 1;
                self.cost(47);
            } else {
                self.v[k].pdelay = speed - 1;
                self.phase_step(k, hw);
            }
        }
        // pitch
        if self.v[k].pspeed != 0 {
            let v = &self.v[k];
            let (mut c, t, sp) = (v.period, v.target, v.pspeed as u16);
            if t.wrapping_sub(c) & 0x8000 == 0 {
                c = c.wrapping_add(sp);
                if t.wrapping_sub(c) & 0x8000 != 0 {
                    c = t;
                    self.cost(4);
                }
            } else {
                c = c.wrapping_sub(sp);
                if t.wrapping_sub(c) & 0x8000 == 0 {
                    c = t;
                    self.cost(14);
                }
            }
            self.v[k].period = c;
            self.w_per(hw, k, c);
            self.cost(PER_K[k]);
        } else {
            self.cost(14);
            let v = &self.v[k];
            let a = v.arp[(v.aidx as usize).min(11)];
            let o = (a.wrapping_add(v.note) as u16).wrapping_add(v.transp).wrapping_mul(2);
            let per = self.period_off(o);
            self.v[k].period = per;
            self.w_per(hw, k, per);
            if self.v[k].aidx == 0 {
                self.v[k].aidx = 11;
                self.cost(8);
            } else {
                self.v[k].aidx -= 1;
            }
        }
        // vibrato
        let vs = self.b(ins + 11);
        if vs == 0 {
            self.cost(16);
        } else if self.v[k].vdelay != 0 {
            self.v[k].vdelay -= 1;
            self.cost(63);
        } else {
            self.cost(293);
            let vd = self.rel_us(self.l(ins + 4));
            let table = self.rel_us(self.l(vd));
            let x = self.b(table.wrapping_add(self.v[k].vpos as usize)) as i8 as i32;
            let depth = self.b(ins + 12) as i32;
            let per = (x * depth + self.v[k].period as i32) as u16;
            self.w_per(hw, k, per);
            self.cost(PER_K[k]);
            let np = self.v[k].vpos.wrapping_sub(vs as u16);
            if np & 0x8000 != 0 {
                self.v[k].vpos = self.w(vd + 4).wrapping_mul(2);
                self.cost(80);
            } else {
                self.v[k].vpos = np;
            }
        }
        // envelope
        self.v[k].ecnt = self.v[k].ecnt.wrapping_sub(1);
        if self.v[k].ecnt & 0x80 == 0 {
            self.cost(34);
            return;
        }
        self.v[k].ecnt = self.v[k].espeed;
        self.cost(383);
        let st = self.v[k].estage as usize;
        let t = self.b(ins + 14 + st);
        let e = self.b(ins + 18 + st);
        let mut v = self.v[k].vol;
        let last = st == 3;
        let mut advance = false;
        if t.wrapping_sub(v) & 0x80 == 0 {
            v = v.wrapping_add(e);
            if v.wrapping_sub(0x3F) & 0x80 != 0 {
                if t.wrapping_sub(v) & 0x80 != 0 {
                    v = t;
                    advance = true;
                    self.cost(if last { 60 } else { 74 });
                } else {
                    self.cost(29);
                }
            } else {
                v = 0x3F;
                advance = true;
                self.cost(if last { 55 } else { 69 });
            }
        } else {
            v = v.wrapping_sub(e);
            if t.wrapping_sub(v) & 0x80 == 0 {
                v = t;
                advance = true;
                self.cost(if last { 21 } else { 35 });
            }
        }
        if advance && !last {
            self.v[k].estage += 1;
        }
        self.v[k].vol = v;
        self.cost(VPRE_K[k]);
        self.lastvol[k] = v & 0x3F;
        self.w_vol(hw, k, (v & 0x3F) as u16);
        self.cost(VPOST_K[k]);
    }

    /// One step of the waveform modulation of voice `k`'s instrument.
    fn phase_step(&mut self, k: usize, hw: &mut Hw) {
        let ins = self.v[k].instr;
        let len = self.b(ins + 10);
        let d = self.rel_us(self.l(ins));
        let b6 = self.w(d + 6);
        let bw = (b6 & 0xFF00) | (b6 as u8).wrapping_sub(len) as u16;
        let data = self.rel(self.l(d)) as i64;
        let pos = self.b(ins + 24);
        let val = self.b(ins + 22);
        self.set_b(Some(hw), data + bw as i16 as i64 + pos as i64, val);
        let dir = self.b(ins + 23);
        let turn;
        let np;
        if dir & 0x80 == 0 {
            np = pos.wrapping_add(1);
            turn = np == len.wrapping_mul(2);
            self.cost(if turn { 327 } else { 285 });
        } else {
            np = pos.wrapping_sub(1);
            turn = np == 0;
            self.cost(if turn { 297 } else { 265 });
        }
        if turn {
            self.set_b(None, ins as i64 + 23, !dir);
            self.set_b(None, ins as i64 + 22, !val);
        }
        self.set_b(None, ins as i64 + 24, np);
    }
}

#[cfg(test)]
#[path = "custommade_tests.rs"]
mod tests;

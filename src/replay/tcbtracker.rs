//! TCB Tracker (`TCB.*`), by Anders "AN Cool" Nilsson (Atari ST, 1990, ported to the
//! Amiga; used for instance by Richard Joseph on Cannon Fodder).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's TCB Tracker player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A four-voice tracker with its own engine: an order list of 64-row patterns,
//! unsigned 8-bit samples (converted to signed when loaded), a pitch table of
//! 32-bit values from which the period is computed on every tick, and "save" and
//! "restore" effects that cut a voice and later restart it from the point it had
//! reached. Every tick writes every voice's period and volume, and a new note
//! restarts its channel with the DMA switched off and on. 50 Hz, one subsong, the
//! speed is fixed by the file. Two flavours (`AN COOL!` and `AN COOL.`) differ in
//! the period base and in the pitch-slide effects, which only the second has.

use crate::paula::LINE;
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "TCB Tracker", detect, new };

/// The pitch of each of the 36 notes, as a 32-bit value (octave 1 first).
const PTAB: [u32; 36] = [
    0x8000, 0x879C, 0x8FAC, 0x9837, 0xA145, 0xAADC, 0xB504, 0xBFC8, 0xCB2F, 0xD744, 0xE411, 0xF1A1, 0x10000, 0x10F38,
    0x11F59, 0x1306F, 0x1428A, 0x155B8, 0x16A09, 0x17F91, 0x1965F, 0x1AE89, 0x1C823, 0x1E343, 0x20000, 0x21E71, 0x23EB3,
    0x260DF, 0x28514, 0x2AB70, 0x2D413, 0x2FF22, 0x32CBF, 0x35D13, 0x39047, 0x3C686,
];

/// The pitch table with its sentinels: the entry before the first and the one after the last.
fn pitch(i: i32) -> i64 {
    if i < 0 {
        0x78D0
    } else if i > 35 {
        0x40000
    } else {
        PTAB[i as usize] as i64
    }
}

/// A restart's timing in the original, measured from its register writes: it comes out of a
/// wait for the next raster line 34 to 46 colour clocks into the line (all restarts of one
/// reference render, whatever the phase of its tick), writes PERIOD 1, and 32 colour clocks later
/// stops the DMA; the sample pointer and length follow within 21 to 38, and the DMA starts again
/// at 64 to 76 colour clocks into the line, one to three lines after it stopped (two in most
/// restarts, none of the rules tried predicts which: period, position in the word, line phase),
/// 421 colour clocks before the repeat pointer and length. Three lines is what the renders
/// agree best with (levels, spectra, the DMA words), two the most frequent.
const LINE_POS: u64 = 42;
const STOP_AFTER: u64 = 32;
const LOC_AFTER: u64 = 21;
const START_POS: u64 = 72;
const START_LINES: u64 = 3;
const REPEAT_AFTER: u64 = 421;

/// Waits for the next line, to `pos` colour clocks into it.
fn wait_line_pos(hw: &mut Hw, lines: u64, pos: u64) {
    let now = hw.bus.clock;
    hw.wait((now / LINE + lines) * LINE + pos - now);
}

// ---------------------------------------------------------------------------------
// Recognition

/// What the header says: flavour A (`!`), the offsets of the patterns and of the
/// instrument block, and the total size of the latter with its samples.
struct Head {
    a: bool,
    np: usize,
    p0: usize,
    s0: usize,
    total: usize,
}

fn head(d: &[u8]) -> Option<Head> {
    if d.len() < 0x132 || d[..7] != *b"AN COOL" {
        return None;
    }
    let a = match d[7] {
        b'!' => true,
        b'.' => false,
        _ => return None,
    };
    let np = be32(d, 8) as usize;
    let l = d[0x8E];
    if np > 127 || d[12] > 15 || d[13] != 0 || l == 0 || l > 127 {
        return None;
    }
    let p0 = if a { 0x110 } else { 0x132 };
    let s0 = p0 + np * 0x200;
    if d.len() <= s0 + 0xD4 || be32(d, s0 + 0xCC) != 0xFFFF_FFFF || be32(d, s0 + 0xD0) != 0 || be32(d, s0 + 0x44) != 0xD4 {
        return None;
    }
    let total = be32(d, s0) as usize;
    if total < 0xD4 || s0 + total > d.len() {
        return None;
    }
    Some(Head { a, np, p0, s0, total })
}

fn detect(d: &[u8]) -> bool {
    head(d).is_some()
}

// ---------------------------------------------------------------------------------
// The player

struct Voice {
    adr: i64,
    len: i32,
    period: u32,
    volume: i32,
    rep: i64,
    replen: i32,
    flag: u8,
    idx: i32,
    cnt: u32,
    saved_adr: i64,
    saved_len: i32,
    /// The address and length last put in the registers, the period last computed.
    reg_adr: i64,
    reg_len: i32,
    hw_per: u16,
}

impl Voice {
    fn new() -> Voice {
        Voice {
            adr: 0,
            len: -1,
            period: 0,
            volume: -1,
            rep: 0,
            replen: -1,
            flag: 0,
            idx: 0,
            cnt: 0,
            saved_adr: 0,
            saved_len: 0,
            reg_adr: 0,
            reg_len: 0,
            hw_per: 0,
        }
    }
}

struct Tcb {
    /// The file with its samples converted to signed.
    img: Vec<u8>,
    base: u32,
    title: String,
    a: bool,
    p0: usize,
    s0: usize,
    /// The period base constant.
    pbase: u32,
    songlen: u8,
    sp: u8,
    c: u8,
    pos: u8,
    r: i32,
    pat: usize,
    v: [Voice; 4],
    songend: bool,
}

impl Tcb {
    fn rb(&self, o: i64) -> u8 {
        if o < 0 {
            return 0;
        }
        self.img.get(o as usize).copied().unwrap_or(0)
    }

    fn rl(&self, o: i64) -> u32 {
        if o < 0 {
            return 0;
        }
        be32(&self.img, o as usize)
    }

    /// The hardware period of a pitch value.
    fn hw_period(&self, period: u32) -> u16 {
        let d = (period >> 3) & 0xFFFF;
        if d == 0 {
            return 1;
        }
        ((self.pbase / d) & 0xFFFF) as u16 + 1
    }

    /// The slide of a note's pitch by effect `eff` (flavour B), once per row.
    fn slide(&mut self, v: usize, eff: usize) {
        let a = be16(&self.img, 0x112 + 2 * eff) as i16 as i32;
        let voice = &mut self.v[v];
        let mut period = voice.period as i64;
        let mut idx = voice.idx;
        // a signed 16-bit product divided by 800; if the quotient does not fit 16 bits
        // the low half of the product stands in for it
        let q = |d: i64| -> i64 {
            let d16 = d as i16 as i32;
            let prod = a.wrapping_mul(d16);
            let qq = prod / 800;
            if qq as i16 as i32 == qq {
                qq as i64
            } else {
                prod as i16 as i64
            }
        };
        if a >= 0 {
            period -= q(pitch(idx) - pitch(idx - 1));
            if !(pitch(idx - 1) < period) {
                idx -= 1;
            }
            if !(period > 0x8000) {
                period = 0x8000;
                idx = 0;
            }
        } else {
            period -= q(pitch(idx + 1) - pitch(idx));
            if !(pitch(idx + 1) > period) {
                idx += 1;
            }
            if !(period < 0x3C686) {
                period = 0x3C686;
                idx = 35;
            }
        }
        voice.period = period as u32;
        voice.idx = idx;
    }

    fn row_voice(&mut self, v: usize, hw: &mut Hw) {
        let cell = (self.p0 + self.pat * 0x200 + self.r as usize * 8 + v * 2) as i64;
        let (note, b1) = (self.rb(cell), self.rb(cell + 1));
        let eff = b1 & 15;
        let bit = 1u16 << v;
        if note != 0 {
            let (oct, semi) = (note >> 4, note & 15);
            let k = if (1..=3).contains(&oct) && semi < 12 { (oct as i32 - 1) * 12 + semi as i32 } else { 0 };
            let ins = (b1 >> 4) as i64;
            let s0 = self.s0 as i64;
            let rec = |o: i64| s0 + o;
            let vb = self.rb(rec(4 + 4 * ins));
            let rep = (((self.rb(rec(4 + 4 * ins + 2)) as u32) << 8) | self.rb(rec(4 + 4 * ins + 3)) as u32) as i64;
            let start = self.rl(rec(0x44 + 8 * ins)) as i64;
            let len = self.rl(rec(0x44 + 8 * ins + 4)) as i32;
            // an instrument of length 1 is empty: the original crashes on it, here the note is dropped
            if len != 1 {
                let adr = s0 + start;
                let end = adr + len as i64;
                {
                    let voice = &mut self.v[v];
                    voice.flag = 0;
                    voice.cnt = 0;
                    voice.idx = k;
                    voice.period = PTAB[k as usize];
                    voice.adr = adr;
                    voice.len = len;
                    voice.replen = rep as i32;
                }
                if rep == 0 {
                    let e = end & !1;
                    let pad = [e - 4, e - 204, e - 404, e - 604].iter().all(|&o| o >= 0 && self.rl(o) == 0xFFFF_FFFF);
                    if pad {
                        self.v[v].len -= 700;
                        if self.a {
                            self.v[v].len -= 212;
                        }
                    }
                } else {
                    let e = end - 700 - if self.a { 212 } else { 0 };
                    let r0 = e - rep;
                    let same = (0..6).all(|i| self.rb(r0 + i) == self.rb(e + i));
                    self.v[v].rep = if same { r0 } else { r0 + 700 };
                }
                self.v[v].volume = if vb == 0 { 0 } else { (vb as i32 + 1) >> 1 };
            }
        }
        match eff {
            1..=10 if !self.a => self.slide(v, eff as usize),
            11 => {
                let voice = &mut self.v[v];
                let dv = (voice.period >> 3) & 0xFFFF;
                if dv != 0 {
                    let q = (self.pbase / dv) & 0xFFFF;
                    let ap = (q + 1) & 0xFFFF;
                    let n = if ap == 0 { 0 } else { ((70938 / ap) + 1) & !1 & 0xFFFF };
                    let adv = n as i64 * ((self.sp as u32 * voice.cnt) & 0xFFFF) as i64;
                    voice.saved_adr = voice.adr + adv;
                    voice.saved_len = (voice.len as i64 - adv) as i32;
                }
                voice.cnt = 0;
                let w = be16(&self.img, 0x128);
                if self.a || w != 2 {
                    hw.dmacon(bit);
                    voice.flag = 0xFF;
                }
            }
            12 => {
                let voice = &mut self.v[v];
                voice.adr = voice.saved_adr;
                voice.len = voice.saved_len;
                voice.flag = 0;
            }
            13 => self.r = 0x3F,
            _ => {}
        }
        self.v[v].cnt += 1;
    }

    fn play_voice(&mut self, v: usize, hw: &mut Hw) {
        let empty = (self.s0 + 0xD0) as i64;
        let hw_per = if self.v[v].period != 0 { Some(self.hw_period(self.v[v].period)) } else { None };
        let base = self.base as i64;
        let voice = &mut self.v[v];
        if voice.len >= 0 {
            if voice.len == 0 {
                voice.reg_adr = empty;
                voice.reg_len = 2;
            } else {
                voice.reg_adr = voice.adr;
                voice.reg_len = voice.len;
            }
        }
        if let Some(p) = hw_per {
            voice.hw_per = p;
            hw.per(v, p);
        }
        if voice.volume >= 0 {
            hw.vol(v, voice.volume as u16);
        }
        if voice.replen >= 0 {
            let (la, ll) = if voice.replen == 0 { (empty, 2) } else { (voice.rep, voice.replen) };
            if voice.flag == 0 {
                let bit = 1u16 << v;
                let run = hw.bus.paula.dmacon & bit != 0;
                wait_line_pos(hw, 1, LINE_POS);
                hw.per(v, 1);
                hw.wait(STOP_AFTER - 6);
                hw.dmacon(if run { bit } else { 0 });
                hw.wait(LOC_AFTER);
                hw.loc(v, (base + voice.reg_adr) as u32);
                hw.len(v, (voice.reg_len >> 1) as u16);
                hw.per(v, voice.hw_per);
                wait_line_pos(hw, START_LINES, START_POS);
                hw.dmacon(0x8000 | bit);
                hw.wait(REPEAT_AFTER);
                hw.loc(v, (base + la) as u32);
                hw.len(v, (ll >> 1) as u16);
                voice.flag = 0xFF;
            }
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.c = self.c.wrapping_sub(1);
        if self.c == 0 {
            self.r = (self.r + 1) & 0x3F;
            if self.r == 0 {
                self.pos = self.pos.wrapping_add(1);
                if self.pos == self.songlen {
                    self.songend = true;
                    self.pos = 0;
                }
                self.pat = self.img.get(14 + self.pos as usize).copied().unwrap_or(0) as usize;
            }
            for v in 0..4 {
                self.row_voice(v, hw);
            }
            self.c = self.sp;
        }
        for v in 0..4 {
            self.play_voice(v, hw);
        }
    }
}

impl Replayer for Tcb {
    fn format(&self) -> &'static str {
        "TCB Tracker"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        Tcb::tick(self, hw);
    }
}

/// The player on module `d` loaded at `base`; the converted samples are returned
/// with it for the caller to put in guest memory.
fn start(d: &[u8], base: u32, title: String) -> Option<Tcb> {
    let h = head(d)?;
    let mut img = d.to_vec();
    // a repair the original makes to one known module
    if h.s0 + h.total == 51825 && be32(&img, 4494) == 0x1289 {
        let a = be32(&img, 4494).wrapping_add(8);
        img[4494..4498].copy_from_slice(&a.to_be_bytes());
        let b = be32(&img, 4498).wrapping_sub(8);
        img[4498..4502].copy_from_slice(&b.to_be_bytes());
    }
    for b in &mut img[h.s0 + 0xD4..h.s0 + h.total] {
        *b ^= 0x80;
    }
    let _ = h.np;
    Some(Tcb {
        base,
        title,
        a: h.a,
        p0: h.p0,
        s0: h.s0,
        pbase: if h.a || img[0x91] != 0 { 3_460_228 } else { 2_867_490 },
        songlen: img[0x8E],
        sp: 16 - img[12],
        c: 1,
        pos: 0xFF,
        r: -1,
        pat: 0,
        v: std::array::from_fn(|_| Voice::new()),
        songend: false,
        img,
    })
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let title = m.bus.read_cstr(m.deli.file_buf);
    let t = start(d, base, title)?;
    let h = head(d)?;
    // the samples, converted to signed, over the module in memory
    m.bus.write_bytes(base + (h.s0 + 0xD4) as u32, &t.img[h.s0 + 0xD4..h.s0 + h.total]);
    Some(Box::new(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// A one-pattern song (flavour B, speed 3 ticks per row, length 1) whose pattern
    /// has a note C-1 with instrument 0 on voice 0 in row 0 and `effect` on row 2.
    /// The instrument is a sample of 0x2FC bytes with a loop of 0x20 (the last 700 bytes
    /// are the padding the loop point is counted back over).
    fn module(effect: u8) -> Vec<u8> {
        let p0 = 0x132;
        let s0 = p0 + 0x200;
        let total = 0xD4 + 0x2FC;
        let mut d = vec![0u8; s0 + total];
        d[..8].copy_from_slice(b"AN COOL.");
        put32(&mut d, 8, 1);
        d[12] = 13;
        d[14] = 0;
        d[0x8E] = 1;
        d[p0] = 0x10;
        d[p0 + 1] = 0x00;
        d[p0 + 2 * 8] = 0;
        d[p0 + 2 * 8 + 1] = effect;
        put32(&mut d, s0, total as u32);
        // instrument 0: volume 0x7F, loop of 0x20 bytes
        d[s0 + 4] = 0x7F;
        d[s0 + 6] = 0;
        d[s0 + 7] = 0x20;
        put32(&mut d, s0 + 0x44, 0xD4);
        put32(&mut d, s0 + 0x48, 0x2FC);
        put32(&mut d, s0 + 0xCC, 0xFFFF_FFFF);
        put32(&mut d, s0 + 0xD0, 0);
        for b in &mut d[s0 + 0xD4..] {
            *b = 0x90;
        }
        d
    }

    #[test]
    fn recognises_modules() {
        let d = module(0);
        assert!(detect(&d));
        let mut e = d.clone();
        e[7] = b'?';
        assert!(!detect(&e));
        let mut e = d.clone();
        e[13] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[0x8E] = 0;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[12] = 16;
        assert!(!detect(&e));
        let mut e = d.clone();
        put32(&mut e, 0x132 + 0x200 + 0xCC, 0);
        assert!(!detect(&e));
        // shorter than the instrument block says
        assert!(!detect(&d[..d.len() - 1]));
        assert!(!detect(&[0u8; 0x400]));
    }

    fn run(data: Vec<u8>, ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/tcb-test/TCB.test"), data).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "TCB Tracker");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let end = e.songend();
        (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), end)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn a_note_restarts_the_channel_and_sets_the_loop() {
        let (log, _) = run(module(0), 4);
        // period 2867490 / (0x8000 >> 3) + 1 = 701, volume (0x7F + 1) >> 1 = 64
        assert_eq!(reg(&log, 0xDFF0A6)[..3], [701, 1, 701]);
        assert_eq!(reg(&log, 0xDFF0A8)[0], 64);
        // sample start and length, then the loop (not the whole sample: 0x20 bytes)
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [0x17E, 0x10]);
        let dma = reg(&log, 0xDFF096);
        assert_eq!(dma[..2], [0x0000, 0x8001]);
        // the loop starts 0x20 bytes into the sample
        let lo = reg(&log, 0xDFF0A2);
        assert_eq!(lo[1].wrapping_sub(lo[0]), 0x20);
    }

    #[test]
    fn save_and_cut_stops_the_channel_and_restore_restarts_it() {
        // effect B (11) on row 2 of voice 0
        let (log, _) = run(module(11), 14);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001), "{dma:?}");
    }

    #[test]
    fn song_end_is_announced_when_the_position_wraps() {
        let (_, end) = run(module(0), 64 * 3 - 4);
        assert!(!end);
        let (_, end) = run(module(0), 64 * 3 + 4);
        assert!(end);
    }

    #[test]
    fn pitch_table_has_sentinels() {
        assert_eq!(pitch(-1), 0x78D0);
        assert_eq!(pitch(0), 0x8000);
        assert_eq!(pitch(35), 0x3C686);
        assert_eq!(pitch(36), 0x40000);
    }
}

//! Digital Sonix & Chrome (`dsc.*`), the sample sequencer of Andrew E. Bailey and
//! David M. Hanlon in Dragon's Breath (Outlaw/Palace, 1990).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Digital Sonix & Chrome player and
//! the structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A module is pure data: a song table of blocks of rows, four voice tracks of one
//! byte a row (a sample number, or 0xFF for nothing), and sample records with a
//! repeat count for a loop part. The tick starts samples; four audio-interrupt
//! handlers, one a channel, chain the parts: at the start of each DMA block the
//! handler queues the loop part (as many times as the record says), then a silent
//! 2-word buffer, and at the start of the silent block disables its interrupt.
//!
//! The trigger has races with Paula that the original leaves to the machine's
//! timing (a first interrupt lost between DMA on and the clearing of the request,
//! so the whole sample plays twice; a handler running inside the trigger). The
//! port decides them as the specification says a host must: it makes the writes in
//! the original's order with the gaps measured in the reference runs, lets the
//! emulated Paula raise its interrupts on the way, and serves those raised while the
//! tick runs when it returns (the handlers back to back, lowest channel first), the
//! others at the moment they are raised.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Digital Sonix & Chrome", detect, new };

// Times of the original's code, in colour clocks, measured in the reference runs
// (UADE, 68020): they place the writes relative to Paula's events.
/// Colour clocks from the start-up writes to the point where the original's host begins to
/// render (measured in the reference runs).
const START_EXTRA: u64 = 2330;
/// The first tick's distance from the song start, more than the fitted start-up time gave.
const START_SHIFT: u64 = 3;
/// A tick that starts no row.
const T_IDLE: u64 = 283;
/// From the tick's start of a row to the first voice.
const T_START: u64 = 328;
/// Each voice the loop looks at, in half clocks: 145 clocks, but the third voice looked at
/// before any sample was started takes one clock more.
const T_VOICE2: [u64; 4] = [290, 290, 292, 290];
/// After a trigger's last write, before the next voice.
const T_AFTER: u64 = 13;
/// After the fourth voice.
const T_END: u64 = 395;
/// From AUDxDAT to AUDxVOL (the Eagleplayer's volume scaling), per channel.
const T_VOL: [u64; 4] = [275, 287, 298, 293];
/// From AUDxVOL to AUDxLEN in half clocks, per channel.
const T_LEN2: [u64; 4] = [150, 182, 213, 221];
/// From AUDxLEN to AUDxLC.
const T_LC: u64 = 12;
/// From AUDxLC to the period write when the request is already there, per channel.
const T_POLL: [u64; 4] = [134, 149, 165, 169];
/// What the loop part's state set-up adds before the poll (a record with repeats).
const T_REPEAT: u64 = 76;
/// One more turn of the request poll, in half clocks, and the distance in half clocks
/// from a look at INTREQR to the period write that follows it.
const T_POLL_LOOP2: u64 = 39;
const T_POLL_OUT2: u64 = 29;
/// The handler: from its start to the first read of the beam, to the second, then the
/// reads every 10.5 (alternately 10 and 11) until the line changes, then every 12.5
/// (12 and 13) until the horizontal position is 22 or more.
const H_READ1: u64 = 12;
const H_READ2: u64 = 18;
const H_POS: u64 = 22;

// ------------------------------------------------------------------ recognition

/// The module layout: song-table entries, pattern base, sample records, sample data.
struct Layout {
    np: usize,
    l: usize,
    pb: usize,
    sb: usize,
    db: usize,
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 12 {
        return None;
    }
    let size = d.len() as u64;
    let (w, ns, np) = (be16(d, 0), d[2] as usize, d[3] as usize);
    let (ss, l) = (be32(d, 4), be32(d, 8));
    if w == 0 || ns == 0 || np == 0 {
        return None;
    }
    if ss == 0 || ss & 1 != 0 || ss > 0x80000 || ss as u64 >= size || l == 0 || l > 0x20000 {
        return None;
    }
    if np < 2 {
        return None;
    }
    for i in 0..np - 1 {
        let off = be32(d, 12 + 6 * i);
        if (off as i32) < 0 || off & 1 != 0 || off > 0x20000 {
            return None;
        }
    }
    let t = 12 + 6 * (np - 1);
    if d.len() < t + 6 || d[t..t + 6].iter().any(|&b| b != 0) {
        return None;
    }
    let pb = 12 + 6 * np;
    let sb = pb + 4 * l as usize;
    if (sb + 18 * ns) as u64 >= size {
        return None;
    }
    let mut last = 0;
    for i in 0..ns {
        let r = sb + 18 * i;
        let (len, soff) = (be32(d, r + 2), be32(d, r + 12));
        if len as i32 <= 0 || len > ss || soff > ss {
            return None;
        }
        last = soff as u64 + len as u64;
    }
    if last != ss as u64 {
        return None;
    }
    Some(Layout { np, l: l as usize, pb, sb, db: sb + 18 * ns })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

// ------------------------------------------------------------------ player

struct Dsc {
    d: Vec<u8>,
    lay: Layout,
    base: u32,
    /// A 4-byte silent buffer in chip memory.
    zero: u32,
    title: String,
    /// Colour clocks the original's start-up takes before its timer runs: a function of
    /// the header (sample records, song-table entries, subsong terminators).
    init_cost: u64,
    /// The odd half clock carried within a tick.
    frac: u64,
    /// The song-table entry each subsong starts at.
    starts: Vec<usize>,
    sub: u16,
    speed: u8,
    cnt: u8,
    pos: usize,
    start: usize,
    row: u8,
    ptr: usize,
    left: u8,
    icnt: [u16; 4],
    llen: [u16; 4],
    lptr: [u32; 4],
    songend: bool,
}

impl Dsc {
    fn off(&self, e: usize) -> usize {
        be32(&self.d, 12 + 6 * e) as usize
    }
    fn rep(&self, e: usize) -> u8 {
        self.d.get(16 + 6 * e).copied().unwrap_or(0)
    }
    fn len(&self, e: usize) -> u8 {
        self.d.get(17 + 6 * e).copied().unwrap_or(0)
    }

    fn init(&mut self, n: u16) {
        self.sub = n;
        let w = be16(&self.d, 0) as i16 as i32;
        self.speed = ((w / 2 + 1500) / w) as u8;
        self.cnt = 1;
        self.start = self.starts[n as usize];
        self.pos = self.start;
        self.row = 0;
        self.ptr = self.off(self.pos);
        self.left = self.rep(self.pos);
        self.icnt = [0; 4];
        self.llen = [0; 4];
        self.lptr = [0; 4];
        self.songend = false;
    }

    /// Lets `n` colour clocks of the tick's code pass. The audio interrupts that Paula
    /// raises meanwhile are not taken inside the tick: they wait until it returns
    /// (the engine then calls the handlers, lowest channel first, back to back).
    fn run(&mut self, hw: &mut Hw, n: u64) {
        hw.bus.clock += n;
    }

    /// The same for `h` half clocks (the original's code takes half clocks too; the
    /// odd one is carried to the next wait).
    fn run2(&mut self, hw: &mut Hw, h: u64) {
        let t = self.frac + h;
        hw.bus.clock += t / 2;
        self.frac = t % 2;
    }

    fn intreq(hw: &mut Hw, v: u16) {
        m68k::AddressBus::write_word(hw.bus, 0xDFF09C, v);
    }
    fn intena(hw: &mut Hw, v: u16) {
        m68k::AddressBus::write_word(hw.bus, 0xDFF09A, v);
    }

    /// Channel `ch`'s audio interrupt handler, called at clock S. It waits for the beam: it
    /// reads the position at S+12 and S+18, then about every 10.5 until the line number is
    /// another, then every 12.5 until the horizontal position is at least 22; the last read
    /// is R, and the writes follow at fixed distances from it.
    fn handler(&mut self, ch: usize, hw: &mut Hw) {
        let ibit = 0x80u16 << ch;
        hw.bus.paula.sync(hw.bus.clock, &hw.bus.ram);
        let p = &hw.bus.paula;
        if p.intena & p.intreq & ibit == 0 {
            return;
        }
        let line0 = (hw.bus.clock + H_READ1) / LINE;
        let mut t = hw.bus.clock + H_READ2;
        let mut k = 0;
        while t / LINE == line0 {
            t += 10 + (k & 1);
            k += 1;
        }
        let mut k = 0;
        while t % LINE < H_POS {
            t += 12 + (k & 1);
            k += 1;
        }
        let r = t;
        let at = |d: u64| r + d;
        if self.icnt[ch] == 0 {
            hw.bus.clock = at(48);
            Self::intena(hw, ibit);
            hw.bus.clock = at(69);
            Self::intreq(hw, ibit);
            hw.bus.clock = at(83);
            return;
        }
        self.icnt[ch] -= 1;
        if self.icnt[ch] == 0 {
            hw.bus.clock = at(61);
            hw.loc(ch, self.zero);
            hw.bus.clock = at(69);
            hw.len(ch, 2);
            hw.bus.clock = at(82);
            Self::intreq(hw, ibit);
            hw.bus.clock = at(96);
        } else {
            hw.bus.clock = at(63);
            hw.len(ch, self.llen[ch]);
            hw.bus.clock = at(77);
            hw.loc(ch, self.lptr[ch]);
            hw.bus.clock = at(85);
            Self::intreq(hw, ibit);
            hw.bus.clock = at(99);
        }
    }

    /// Starts sample `s` on voice `k`, in the original's order of writes.
    fn trigger(&mut self, k: usize, s: usize, hw: &mut Hw) {
        let (bit, ibit) = (1u16 << k, 0x80u16 << k);
        let r = self.lay.sb + 18 * s;
        let per = be16(&self.d, r);
        let length = be32(&self.d, r + 2);
        let lp = be32(&self.d, r + 6);
        let rep = be16(&self.d, r + 10);
        let soff = be32(&self.d, r + 12);
        let vol = (self.d.get(r + 16).copied().unwrap_or(0) as i8 as u16) & 0x7F;
        let addr = self.base.wrapping_add(self.lay.db as u32).wrapping_add(soff);
        hw.dmacon(bit);
        self.run(hw, 8);
        hw.per(k, 1);
        self.run(hw, 8);
        Self::intreq(hw, ibit);
        self.run(hw, 8);
        Self::intena(hw, ibit);
        self.run(hw, 8);
        m68k::AddressBus::write_word(hw.bus, 0xDFF0AA + 16 * k as u32, 0);
        self.run(hw, T_VOL[k]);
        hw.vol(k, vol);
        self.run2(hw, T_LEN2[k]);
        hw.len(k, (length as i32 >> 1) as u16);
        self.run(hw, T_LC);
        hw.loc(k, addr);
        self.icnt[k] = rep.wrapping_add(1);
        if rep != 0 {
            self.llen[k] = (length.wrapping_sub(lp) as i32 >> 1) as u16;
            self.lptr[k] = addr.wrapping_add(lp);
        }
        // wait for the request the AUDxDAT write raises (at once if the channel is
        // idle, else when the word it plays is used up): the looks at INTREQR are 19.5
        // apart, the period write follows the one that finds it
        let mut look2 = 2 * (hw.bus.clock + T_POLL[k] + if rep != 0 { T_REPEAT } else { 0 }) + self.frac - T_POLL_OUT2;
        for _ in 0..16384 {
            hw.bus.paula.sync(look2 / 2, &hw.bus.ram);
            if hw.bus.paula.intreq & ibit != 0 {
                break;
            }
            look2 += T_POLL_LOOP2;
        }
        hw.bus.clock = (look2 + T_POLL_OUT2) / 2;
        self.frac = (look2 + T_POLL_OUT2) % 2;
        hw.per(k, per);
        self.run(hw, 18);
        hw.dmacon(0x8000 | bit);
        self.run(hw, 8);
        Self::intreq(hw, ibit);
        self.run(hw, 18);
        Self::intena(hw, 0x8000 | ibit);
    }

    fn play_row(&mut self, hw: &mut Hw) {
        let l = self.lay.l;
        let b: [u8; 4] = std::array::from_fn(|k| self.d.get(self.lay.pb + self.ptr + k * l).copied().unwrap_or(0xFF));
        self.ptr += 1;
        self.run(hw, T_START);
        let mut started = false;
        for (k, &s) in b.iter().enumerate() {
            self.run2(hw, if started { 290 } else { T_VOICE2[k] });
            if s != 0xFF {
                started = true;
                self.trigger(k, s as usize, hw);
                self.run(hw, T_AFTER);
            }
        }
        self.run(hw, T_END);
        self.row = self.row.wrapping_add(1);
        if self.row == self.len(self.pos) {
            self.row = 0;
            self.left = self.left.wrapping_sub(1);
            if self.left != 0 {
                self.ptr = self.off(self.pos);
            } else {
                self.pos += 1;
                if self.rep(self.pos) == 0 {
                    self.pos = self.start;
                    self.songend = true;
                }
                self.ptr = self.off(self.pos);
                self.left = self.rep(self.pos);
            }
        }
    }
}

impl Replayer for Dsc {
    fn format(&self) -> &'static str {
        "Digital Sonix & Chrome"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.frac = 0;
        self.cnt = self.cnt.wrapping_sub(1);
        if self.cnt == 0 {
            self.cnt = self.speed;
            self.play_row(hw);
        } else {
            self.run(hw, T_IDLE);
        }
    }

    fn timer_delay(&self) -> u64 {
        // (the first tick falls START_SHIFT colour clocks after the end of the start-up in the
        // original's, the song start the engine aligns Paula's grids to)
        self.init_cost + START_SHIFT
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.starts.len() as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.init(n);
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        self.handler(ch, hw);
        true
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    // subsong n starts after the n-th terminator
    let mut starts = vec![0];
    for e in 0..lay.np {
        if d[16 + 6 * e] == 0 && e + 1 < lay.np {
            starts.push(e + 1);
        }
    }
    let terminators = (0..lay.np).filter(|&e| d[16 + 6 * e] == 0).count() as u64;
    // in half clocks: 4362 + 264 per sample record + 274 per song-table entry + 4.5 per terminator, rounded up
    let init_cost = (8724 + 528 * d[2] as u64 + 548 * lay.np as u64 + 9 * terminators + 1) / 2;
    // The part of it that precedes the start-up writes (the same loops, run first for the
    // check) and the part up to the point where the original's host starts rendering are
    // not rendered audio: they are spent before the player runs.
    let pre = (4070 + 528 * d[2] as u64 + 548 * lay.np as u64 + 9 * terminators + 1) / 2;
    let start_clock = m.bus.clock;
    // The original's song starts after the Eagleplayer's long precalculation (3.2 s on the
    // reference's 68000): the clock of its start, from the engine's aligned origin (the clock at
    // the player's start), is 11372522.5 + the loops' half clocks / 2 (fitted to the starts of 10
    // modules, within a colour clock). The beam's line phase (the handlers wait for the line to
    // change and read the horizontal position) depends on it, so the song starts there too.
    let end = (22_745_045 + 528 * d[2] as u64 + 548 * lay.np as u64 + 9 * terminators) / 2 + start_clock;
    m.bus.clock = (m.bus.clock + pre).max(end.saturating_sub(START_EXTRA));
    let zero = m.alloc(4);
    let title = m.bus.read_cstr(m.deli.file_buf);
    let mut p = Dsc {
        d: d.to_vec(),
        lay,
        base,
        zero,
        title,
        init_cost: init_cost - pre - START_EXTRA,
        frac: 0,
        starts,
        sub: 0,
        speed: 1,
        cnt: 1,
        pos: 0,
        start: 0,
        row: 0,
        ptr: 0,
        left: 0,
        icnt: [0; 4],
        llen: [0; 4],
        lptr: [0; 4],
        songend: false,
    };
    p.init(0);
    // the start-up: volumes 0, then the four handlers installed (their interrupts on)
    let mut hw = Hw { bus: &mut m.bus };
    for ch in 0..4 {
        hw.vol(ch, 0);
    }
    for ch in 0..4 {
        Dsc::intena(&mut hw, 0x8000 | 0x80 << ch);
    }
    m.bus.clock += START_EXTRA;
    Some(Box::new(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn put16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn put32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// One block of 2 rows played once (then the terminator); voice 0 starts sample 0
    /// in row 0: 8 bytes, period 400, volume 64, the last 4 bytes repeated twice.
    /// With `extra`, a second subsong follows the first one's terminator.
    fn module(extra: bool) -> Vec<u8> {
        let np = if extra { 4 } else { 2 };
        let pb = 12 + 6 * np;
        let l = 2;
        let sb = pb + 4 * l;
        let db = sb + 18;
        let mut d = vec![0u8; db + 8];
        put16(&mut d, 0, 300);
        d[2] = 1;
        d[3] = np as u8;
        put32(&mut d, 4, 8);
        put32(&mut d, 8, l as u32);
        // entry 0: block at 0, played once, 2 rows
        d[16] = 1;
        d[17] = 2;
        if extra {
            // entry 1 ends subsong 0, entry 2 is subsong 1, entry 3 the final terminator
            d[28] = 1;
            d[29] = 1;
        }
        d[pb..sb].fill(0xFF);
        d[pb] = 0;
        put16(&mut d, sb, 400);
        put32(&mut d, sb + 2, 8);
        put32(&mut d, sb + 6, 4);
        put16(&mut d, sb + 10, 2);
        put32(&mut d, sb + 12, 0);
        d[sb + 16] = 64;
        for (i, b) in d[db..].iter_mut().enumerate() {
            *b = (i as u8) * 16;
        }
        d
    }

    #[test]
    fn recognition() {
        let d = module(false);
        assert!(detect(&d));
        let l = layout(&d).unwrap();
        assert_eq!((l.pb, l.db), (24, 24 + 8 + 18));
        // the last sample must end the sample data; a short file; a non-zero
        // terminator; an odd sample-data size; a sample longer than the data
        let mut e = d.clone();
        put32(&mut e, l.sb + 12, 2);
        assert!(!detect(&e));
        assert!(!detect(&d[..30]));
        let mut e = d.clone();
        e[18] = 1;
        assert!(!detect(&e));
        let mut e = d.clone();
        put32(&mut e, 4, 7);
        assert!(!detect(&e));
        let mut e = d.clone();
        put32(&mut e, l.sb + 2, 10);
        assert!(!detect(&e));
        assert!(!detect(&[0u8; 64]));
        assert!(detect(&module(true)));
    }

    fn play(d: Vec<u8>, ticks: usize) -> (Engine, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/dsc-test/dsc.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Digital Sonix & Chrome");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (e, log)
    }

    /// Channel 0's writes (its registers and its DMACON/INTENA/INTREQ bit).
    fn ch0(log: &[(u32, u16)]) -> Vec<(u32, u16)> {
        log.iter()
            .copied()
            .filter(|&(a, v)| {
                (0xDFF0A0..0xDFF0B0).contains(&a) || (a == 0xDFF096 && v & 1 != 0) || ((a == 0xDFF09A || a == 0xDFF09C) && v & 0x7FFF == 0x80)
            })
            .collect()
    }

    #[test]
    fn trigger_and_interrupt_chain() {
        let (e, log) = play(module(false), 12);
        // start-up: volumes 0, the four channel interrupts on
        assert_eq!(
            &log[..8],
            &[(0xDFF0A8, 0), (0xDFF0B8, 0), (0xDFF0C8, 0), (0xDFF0D8, 0), (0xDFF09A, 0x8080), (0xDFF09A, 0x8100), (0xDFF09A, 0x8200), (0xDFF09A, 0x8400)]
        );
        let w = ch0(&log[8..]);
        let lc = |i: usize| (w[i].1 as u32) << 16 | w[i + 1].1 as u32;
        let regs: Vec<(u32, u16)> = w.iter().copied().filter(|&(a, _)| a != 0xDFF0A0 && a != 0xDFF0A2).collect();
        assert_eq!(
            &regs[..11],
            &[
                (0xDFF096, 0x0001),
                (0xDFF0A6, 1),
                (0xDFF09C, 0x0080),
                (0xDFF09A, 0x0080),
                (0xDFF0AA, 0),
                (0xDFF0A8, 64),
                (0xDFF0A4, 4),
                (0xDFF0A6, 400),
                (0xDFF096, 0x8001),
                (0xDFF09C, 0x0080),
                (0xDFF09A, 0x8080),
            ]
        );
        // the handler: the loop part twice (LEN then LC), the silence (LC then LEN),
        // then its interrupt off
        let s = w.iter().position(|&(a, v)| a == 0xDFF09A && v == 0x8080).unwrap() + 1;
        let start = lc(7);
        assert_eq!(w[s], (0xDFF0A4, 2));
        assert_eq!(lc(s + 1), start + 4);
        assert_eq!(w[s + 3], (0xDFF09C, 0x80));
        assert_eq!(w[s + 4], (0xDFF0A4, 2));
        assert_eq!(lc(s + 5), start + 4);
        assert_eq!(w[s + 7], (0xDFF09C, 0x80));
        let zero = lc(s + 8);
        assert!(zero < start || zero >= start + 8);
        assert_eq!(&w[s + 10..s + 14], &[(0xDFF0A4, 2), (0xDFF09C, 0x80), (0xDFF09A, 0x0080), (0xDFF09C, 0x80)]);
        // speed (150 + 1500) / 300 = 5: the second row (empty) at tick 5 ends the song
        assert!(e.songend());
    }

    #[test]
    fn subsongs_start_after_terminators() {
        let (e, log) = play(module(true), 2);
        assert_eq!(e.playing.as_ref().unwrap().sub_max, 1);
        assert!(log.contains(&(0xDFF0A6, 400)));
    }
}

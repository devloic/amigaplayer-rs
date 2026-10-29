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
//! emulated Paula raise its interrupts on the way, and runs the handlers at the
//! moment they are raised, also inside the tick (the music tick does not block the
//! audio interrupts).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Digital Sonix & Chrome", detect, new };

// Gaps between the original's writes, in colour clocks, measured in the reference
// runs (UADE, 68020): they only place the writes relative to Paula's events.
/// From the tick to the voice loop.
const T_START: u64 = 313;
/// Each voice the loop looks at.
const T_VOICE: u64 = 58;
/// After a trigger's last write, before the next voice.
const T_AFTER: u64 = 12;
/// From AUDxDAT to AUDxVOL (the Eagleplayer's volume scaling), per channel.
const T_VOL: [u64; 4] = [240, 248, 256, 254];
/// From AUDxVOL to AUDxLEN, per channel.
const T_LEN: [u64; 4] = [52, 64, 76, 80];
/// From AUDxLC to the period write when the request is already there, per channel.
const T_POLL: [u64; 4] = [110, 122, 134, 138];
/// What the loop part's state set-up adds before the poll (a record with repeats).
const T_REPEAT: u64 = 56;
/// One more turn of the request poll (the gaps of the delayed polls are multiples of it).
const T_POLL_LOOP: u64 = 14;
/// From AUDxLC (and the state set-up) to the poll's first test of INTREQR.
const T_POLL_TEST: u64 = 12;
/// The handler waits for the next raster line, then writes at about this
/// horizontal position (colour clocks from the line start).
const H_FREE: u64 = 66;
const H_SILENCE: u64 = 76;
const H_LOOP: u64 = 77;
/// The interrupt's entry, before the handler reads the beam.
const H_ENTRY: u64 = 20;
/// From the handler's last write to the end of the interrupt.
const H_EXIT: u64 = 16;

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

    /// The handlers of the interrupts that are requested and enabled now.
    fn service(&mut self, hw: &mut Hw) {
        loop {
            let p = &hw.bus.paula;
            let pend = p.pending_audio_irqs();
            match (0..4).find(|&ch| pend & 0x80 << ch != 0) {
                Some(ch) => self.handler(ch, hw),
                None => return,
            }
        }
    }

    /// Lets `n` colour clocks of the tick's code pass; the audio interrupts that
    /// Paula raises meanwhile are taken when raised, and their handlers' time is
    /// added.
    fn run(&mut self, hw: &mut Hw, n: u64) {
        let mut left = n;
        loop {
            self.service(hw);
            if left == 0 {
                return;
            }
            let target = hw.bus.clock + left;
            let stop = hw.bus.paula.sync_to_irq(target, &hw.bus.ram);
            let stop = stop.max(hw.bus.clock);
            left = target - stop;
            hw.bus.clock = stop;
        }
    }

    fn intreq(hw: &mut Hw, v: u16) {
        m68k::AddressBus::write_word(hw.bus, 0xDFF09C, v);
    }
    fn intena(hw: &mut Hw, v: u16) {
        m68k::AddressBus::write_word(hw.bus, 0xDFF09A, v);
    }

    /// Channel `ch`'s audio interrupt handler.
    fn handler(&mut self, ch: usize, hw: &mut Hw) {
        let ibit = 0x80u16 << ch;
        hw.bus.paula.sync(hw.bus.clock, &hw.bus.ram);
        let p = &hw.bus.paula;
        if p.intena & p.intreq & ibit == 0 {
            return;
        }
        // the wait for the next raster line and a horizontal position
        let line = ((hw.bus.clock + H_ENTRY) / LINE + 1) * LINE;
        let at = |h: u64| line + h;
        if self.icnt[ch] == 0 {
            hw.bus.clock = at(H_FREE);
            Self::intena(hw, ibit);
            hw.wait(18);
        } else {
            self.icnt[ch] -= 1;
            if self.icnt[ch] == 0 {
                hw.bus.clock = at(H_SILENCE);
                hw.loc(ch, self.zero);
                hw.wait(8);
                hw.len(ch, 2);
                hw.wait(10);
            } else {
                hw.bus.clock = at(H_LOOP);
                hw.len(ch, self.llen[ch]);
                hw.wait(10);
                hw.loc(ch, self.lptr[ch]);
                hw.wait(14);
            }
        }
        Self::intreq(hw, ibit);
        hw.wait(H_EXIT);
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
        self.run(hw, T_LEN[k]);
        hw.len(k, (length as i32 >> 1) as u16);
        self.run(hw, 6);
        hw.loc(k, addr);
        self.icnt[k] = rep.wrapping_add(1);
        if rep != 0 {
            self.llen[k] = (length.wrapping_sub(lp) as i32 >> 1) as u16;
            self.lptr[k] = addr.wrapping_add(lp);
        }
        // wait for the request the AUDxDAT write raises (at once if the channel is
        // idle, else when the word it plays is used up)
        self.run(hw, T_POLL_TEST + if rep != 0 { T_REPEAT } else { 0 });
        for _ in 0..16384 {
            hw.bus.paula.sync(hw.bus.clock, &hw.bus.ram);
            if hw.bus.paula.intreq & ibit != 0 {
                break;
            }
            self.run(hw, T_POLL_LOOP);
        }
        self.run(hw, T_POLL[k] - T_POLL_TEST);
        hw.per(k, per);
        self.run(hw, 16);
        hw.dmacon(0x8000 | bit);
        self.run(hw, 8);
        Self::intreq(hw, ibit);
        self.run(hw, 16);
        Self::intena(hw, 0x8000 | ibit);
    }

    fn play_row(&mut self, hw: &mut Hw) {
        let l = self.lay.l;
        let b: [u8; 4] = std::array::from_fn(|k| self.d.get(self.lay.pb + self.ptr + k * l).copied().unwrap_or(0xFF));
        self.ptr += 1;
        for (k, &s) in b.iter().enumerate() {
            self.run(hw, T_VOICE);
            if s != 0xFF {
                self.trigger(k, s as usize, hw);
                self.run(hw, T_AFTER);
            }
        }
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
        self.run(hw, T_START);
        self.cnt = self.cnt.wrapping_sub(1);
        if self.cnt == 0 {
            self.cnt = self.speed;
            self.play_row(hw);
        }
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
    let zero = m.alloc(4);
    let title = m.bus.read_cstr(m.deli.file_buf);
    let mut p = Dsc {
        d: d.to_vec(),
        lay,
        base,
        zero,
        title,
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

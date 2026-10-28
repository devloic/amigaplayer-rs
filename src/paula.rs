//! Paula's audio channels in colour clocks, after UADE's emulation: the DMA start
//! at the next line, one DMA word per channel at the start of each line (every
//! 227 colour clocks), the 2/3 state pair that plays a word's high and low byte
//! for a period each, the length counter reload with its interrupt, attached
//! channels, and the interrupt a channel without DMA raises for its next word.
//! Its output goes through [`OutputStage`]. Given the same register writes at
//! the same colour clocks it produces UADE's samples to the bit, except where
//! UADE departs from the hardware on purpose: it plays volume 64 as 63 and
//! raises periods below 16 to 16, where Paula plays full volume and any period.
//! (Checked against the register writes, DMA fetches and output of UADE runs,
//! with those two differences switched to UADE's behaviour.)
//!
//! Register writes take effect at the colour clock Paula has been run to
//! (`sync`), so the owner runs it to the current time before each one.

use crate::paulaout::{Filter, OutputStage};

/// PAL colour clock, the rate at which Paula counts periods.
pub const PAULA_CLOCK_PAL: f64 = 3_546_895.0;
/// Colour clocks per line (PAL).
pub const LINE: u64 = 227;

pub const DMAF_SETCLR: u16 = 0x8000;
pub const DMAF_MASTER: u16 = 0x0200;
pub const INTF_SETCLR: u16 = 0x8000;
pub const INTF_INTEN: u16 = 0x4000;
pub const INTF_AUD0: u16 = 0x0080;

/// One channel: the programmer-visible registers and the state machine.
#[derive(Clone, Default)]
pub struct Channel {
    pub lc: u32,
    pub len: u16,
    pub per: u16,
    pub vol: u8,
    pub dat: u16,
    /// 0 when the channel modulates another (ADKCON), else all ones.
    adk_mask: i32,
    evtime: u64,
    dmaen: bool,
    intreq2: bool,
    data_written: u8,
    pt: u32,
    state: u8,
    wlen: u32,
    current_sample: i32,
    nextdat: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quirks {
    Hardware,
    /// Volume 64 plays as 63, periods below 16 as 16 (for the UADE comparison).
    Uade,
}

#[derive(Clone)]
pub struct Paula {
    pub ch: [Channel; 4],
    pub dmacon: u16,
    pub intena: u16,
    pub intreq: u16,
    /// INTREQ bits set since the owner last took them: a request raised while
    /// the bit was already set, or set and cleared between two looks at INTREQ,
    /// still counts.
    pub raised: u16,
    pub adkcon: u16,
    pub out: OutputStage,
    /// Samples produced so far (left, right), for the owner to take.
    pub samples: std::collections::VecDeque<(i16, i16)>,
    /// The colour clock the channels have been run to.
    last: u64,
    next_line: u64,
    quirks: Quirks,
    /// The built-in ProTracker's register writes, which go to Paula directly and
    /// not through the bus's write log: ($DFF000 + register, value, colour clock).
    /// Kept for DELI_WRITE_LOG when set.
    pub native_log: Option<Vec<(u32, u16, u64)>>,
}

fn setclr(r: &mut u16, v: u16) {
    if v & 0x8000 != 0 {
        *r |= v & 0x7FFF;
    } else {
        *r &= !v;
    }
}

/// The big-endian word at `a` of `ram` (DMA reaches all of it).
pub fn ram_word(ram: &[u8], a: u32) -> u16 {
    let a = a as usize & !1;
    match ram.get(a..a + 2) {
        Some(w) => u16::from_be_bytes([w[0], w[1]]),
        None => 0,
    }
}

impl Paula {
    /// Paula at colour clock 0, as AmigaOS leaves it (DMA and interrupt master
    /// enables on), at 48 kHz with no filter.
    pub fn new() -> Self {
        Self::with_output(48000, Filter::None)
    }

    pub fn with_output(rate: u32, filter: Filter) -> Self {
        let mut ch: [Channel; 4] = Default::default();
        for c in &mut ch {
            c.per = 65535;
            c.adk_mask = -1;
        }
        Paula {
            ch,
            dmacon: DMAF_MASTER,
            intena: INTF_INTEN,
            intreq: 0,
            raised: 0,
            adkcon: 0,
            out: OutputStage::new(rate, filter),
            samples: Default::default(),
            last: 0,
            next_line: LINE,
            quirks: Quirks::Hardware,
            native_log: None,
        }
    }

    /// Back to the power-on state, keeping the time and the output format.
    pub fn reset(&mut self) {
        let (out, last, next_line, quirks) = (self.out.clone(), self.last, self.next_line, self.quirks);
        let log = self.native_log.take();
        *self = Self::with_output(out.rate(), out.filter());
        (self.out, self.last, self.next_line, self.quirks, self.native_log) = (out, last, next_line, quirks, log);
    }

    /// Plays volume 64 as 63 and periods below 16 as 16, as UADE does.
    pub fn set_uade_quirks(&mut self, on: bool) {
        self.quirks = if on { Quirks::Uade } else { Quirks::Hardware };
    }

    /// The colour clock the channels have been run to.
    pub fn now(&self) -> u64 {
        self.last
    }

    /// Audio interrupt bits that are both requested and enabled.
    pub fn pending_audio_irqs(&self) -> u16 {
        if self.intena & INTF_INTEN == 0 {
            return 0;
        }
        self.intena & self.intreq & (0xF << 7)
    }

    /// The channel's current output, -1..1, for scopes.
    pub fn output(&self, i: usize) -> f32 {
        let c = &self.ch[i];
        ((c.current_sample * c.vol as i32) & c.adk_mask) as f32 / (128.0 * 64.0)
    }

    /// Runs the channels to colour clock `t`.
    pub fn sync(&mut self, t: u64, ram: &[u8]) {
        self.sync_with(t, &mut |a| ram_word(ram, a), false);
    }

    /// Runs the channels towards `t`, stopping early at the colour clock where a
    /// channel raises an enabled audio interrupt. Returns the clock reached.
    pub fn sync_to_irq(&mut self, t: u64, ram: &[u8]) -> u64 {
        self.sync_with(t, &mut |a| ram_word(ram, a), true)
    }

    fn sync_with(&mut self, t: u64, mem: &mut impl FnMut(u32) -> u16, stop_at_irq: bool) -> u64 {
        if t <= self.last {
            return self.last;
        }
        while self.next_line <= t {
            let h = self.next_line;
            if let Some(stop) = self.update(h, mem, stop_at_irq) {
                return stop;
            }
            self.line_start(mem);
            self.next_line += LINE;
        }
        self.update(t, mem, stop_at_irq).unwrap_or(t)
    }

    pub fn write_dmacon(&mut self, v: u16) {
        setclr(&mut self.dmacon, v);
        self.dmacon &= 0x1FFF;
        let to_line = self.next_line - self.last;
        for i in 0..4 {
            let c = &mut self.ch[i];
            c.dmaen = self.dmacon & DMAF_MASTER != 0 && self.dmacon & (1 << i) != 0;
            if c.dmaen {
                if c.state == 0 {
                    // DMA starts at the next line
                    c.state = 1;
                    c.pt = c.lc;
                    c.wlen = c.len as u32;
                    c.data_written = 2;
                    c.evtime = to_line;
                }
            } else if c.state == 1 || c.state == 5 {
                c.state = 0;
                c.current_sample = 0;
            }
        }
    }

    pub fn write_intena(&mut self, v: u16) {
        setclr(&mut self.intena, v);
    }

    pub fn write_intreq(&mut self, v: u16) {
        setclr(&mut self.intreq, v);
        if v & 0x8000 != 0 {
            self.raised |= v & 0x7FFF;
        }
    }

    pub fn write_adkcon(&mut self, v: u16) {
        setclr(&mut self.adkcon, v);
        let t = self.adkcon | self.adkcon >> 4;
        for i in 0..4 {
            self.ch[i].adk_mask = ((t as i32 >> i) & 1) - 1;
        }
    }

    /// Write to an audio register; `reg` is the offset from $DFF0A0.
    pub fn write_audio_reg(&mut self, reg: u32, v: u16) {
        let n = (reg >> 4) as usize & 3;
        let uade = self.quirks == Quirks::Uade;
        let c = &mut self.ch[n];
        match reg & 0xF {
            0x0 => c.lc = (c.lc & 0xFFFF) | (v as u32) << 16,
            0x2 => c.lc = (c.lc & !0xFFFF) | (v & 0xFFFE) as u32,
            0x4 => c.len = v,
            0x6 => c.per = if v == 0 { 65535 } else if uade { v.max(16) } else { v },
            0x8 => c.vol = if v & 64 != 0 { if uade { 63 } else { 64 } } else { (v & 63) as u8 },
            0xA => {
                // manual mode: the CPU hands over a word
                c.dat = v;
                if c.state == 0 && self.intreq & (INTF_AUD0 << n) == 0 {
                    c.state = 2;
                    self.intreq |= INTF_AUD0 << n;
                    self.raised |= INTF_AUD0 << n;
                    c.evtime = c.per as u64;
                }
            }
            _ => {}
        }
    }

    /// The audio DMA of the line that starts now.
    fn line_start(&mut self, mem: &mut impl FnMut(u32) -> u16) {
        for c in &mut self.ch {
            if c.data_written != 2 {
                continue;
            }
            c.data_written = 0;
            c.nextdat = mem(c.pt);
            if c.wlen != 1 {
                c.pt = c.pt.wrapping_add(2);
            }
            if c.state == 2 || c.state == 3 {
                if c.wlen == 1 {
                    c.pt = c.lc;
                    c.wlen = c.len as u32;
                    c.intreq2 = true;
                } else {
                    c.wlen = c.wlen.wrapping_sub(1) & 0xFFFF;
                }
            }
        }
    }

    /// Runs the state machines and the output stage to `t`. With `stop_at_irq`,
    /// returns early at the clock where an enabled audio interrupt is raised.
    fn update(&mut self, t: u64, mem: &mut impl FnMut(u32) -> u16, stop_at_irq: bool) -> Option<u64> {
        let mut n = t - self.last;
        while n > 0 {
            let mut best = n + 1;
            for c in &self.ch {
                if c.state != 0 && best > c.evtime {
                    best = c.evtime;
                }
            }
            best = best.min(self.out.until_sample() as u64).min(n);
            let o: [i32; 4] = std::array::from_fn(|i| {
                let c = &self.ch[i];
                (c.current_sample * c.vol as i32) & c.adk_mask
            });
            if let Some(s) = self.out.step(o, best as u32) {
                self.samples.push_back(s);
            }
            for c in &mut self.ch {
                c.evtime = c.evtime.wrapping_sub(best);
            }
            n -= best;
            let before = self.intreq;
            for i in 0..4 {
                if self.ch[i].evtime == 0 && self.ch[i].state != 0 {
                    self.handler(i, mem);
                }
            }
            if stop_at_irq && self.intreq & !before & self.intena & 0x780 != 0 && self.intena & INTF_INTEN != 0 {
                self.last = t - n;
                return Some(self.last);
            }
        }
        self.last = t;
        None
    }

    /// A channel's state machine when its timer runs out.
    fn handler(&mut self, nr: usize, mem: &mut impl FnMut(u32) -> u16) {
        let adkcon = self.adkcon;
        let audav = adkcon & (1 << nr) != 0;
        let audap = adkcon & (16 << nr) != 0;
        let napnav = (!audav && !audap) || audav;
        let bit = INTF_AUD0 << nr;
        let c = &mut self.ch[nr];
        match c.state {
            1 => {
                // the first line after DMA was turned on
                c.evtime = LINE;
                c.state = 5;
                self.intreq |= bit;
                self.raised |= bit;
                if c.wlen != 1 {
                    c.wlen = c.wlen.wrapping_sub(1) & 0xFFFF;
                }
                c.nextdat = mem(c.pt);
            }
            5 => {
                // the second line after DMA was turned on
                c.evtime = c.per as u64;
                c.dat = c.nextdat;
                c.current_sample = (c.dat >> 8) as i8 as i32;
                c.state = 2;
                if napnav {
                    c.data_written = 2;
                }
            }
            2 => {
                // the low byte
                c.current_sample = c.dat as u8 as i8 as i32;
                c.evtime = c.per as u64;
                c.state = 3;
                if audap {
                    if c.intreq2 && c.dmaen {
                        self.intreq |= bit;
                        self.raised |= bit;
                    }
                    let c = &mut self.ch[nr];
                    c.intreq2 = false;
                    c.dat = c.nextdat;
                    if c.dmaen {
                        c.data_written = 2;
                    }
                    let dat = c.dat;
                    if nr < 3 {
                        self.ch[nr + 1].per = if dat == 0 { 65535 } else { dat };
                    }
                }
            }
            3 => {
                // the next word's high byte
                c.evtime = c.per as u64;
                if self.intreq & bit != 0 && !c.dmaen {
                    c.state = 0;
                    c.current_sample = 0;
                    return;
                }
                c.state = 2;
                if (c.intreq2 && c.dmaen && napnav) || (napnav && !c.dmaen) {
                    self.intreq |= bit;
                    self.raised |= bit;
                }
                let c = &mut self.ch[nr];
                c.intreq2 = false;
                c.dat = c.nextdat;
                c.current_sample = (c.dat >> 8) as i8 as i32;
                if c.dmaen && napnav {
                    c.data_written = 2;
                }
                let dat = c.dat;
                if audav && nr < 3 {
                    self.ch[nr + 1].vol = dat.min(64) as u8;
                }
            }
            _ => c.state = 0,
        }
    }

    /// A register write at colour clock `t` ($DFF000 + `reg`), running the channels
    /// there first; DMA fetches go through `mem`.
    #[cfg(test)]
    fn write_at(&mut self, t: u64, reg: u32, v: u16, mem: &mut impl FnMut(u32) -> u16) {
        self.sync_with(t, mem, false);
        match reg & 0x1FE {
            0x096 => self.write_dmacon(v),
            0x09A => self.write_intena(v),
            0x09C => self.write_intreq(v),
            0x09E => self.write_adkcon(v),
            r @ 0x0A0..=0x0DF => self.write_audio_reg(r - 0xA0, v),
            _ => {}
        }
    }
}

impl Default for Paula {
    fn default() -> Self {
        Paula::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel started on a two-word sample: interrupt at the first line,
    /// output from the second, and the sample loops.
    #[test]
    fn dma_start_and_loop() {
        let ram = [0x40u8, 0xC0, 0x20, 0xE0];
        let mut mem = |a: u32| u16::from_be_bytes([ram[(a & 3) as usize], ram[(a & 3 | 1) as usize]]);
        let mut p = Paula::new();
        p.write_at(10, 0xA4, 2, &mut mem);
        p.write_at(10, 0xA6, 124, &mut mem);
        p.write_at(10, 0xA8, 64, &mut mem);
        p.write_at(20, 0x96, 0x8201, &mut mem);
        p.sync_with(LINE - 1, &mut mem, false);
        assert_eq!(p.intreq & 0x80, 0);
        p.sync_with(LINE, &mut mem, false);
        assert_eq!(p.intreq & 0x80, 0x80, "interrupt at the first line");
        assert_eq!(p.ch[0].state, 5);
        p.sync_with(2 * LINE, &mut mem, false);
        assert_eq!((p.ch[0].state, p.ch[0].current_sample), (2, 0x40));
        p.sync_with(2 * LINE + 124, &mut mem, false);
        assert_eq!(p.ch[0].current_sample, 0xC0u8 as i8 as i32);
    }

    /// Without DMA a channel asks for its next word: the 4Voc NotePlayer genie
    /// turns DMA off and waits for that interrupt to know the channel stopped.
    #[test]
    fn dma_off_requests_a_word() {
        let ram = [0x40u8, 0xC0];
        let mut mem = |a: u32| u16::from_be_bytes([ram[(a & 1) as usize], ram[(a & 1 | 1) as usize]]);
        let mut p = Paula::new();
        p.write_at(0, 0xA4, 1, &mut mem);
        p.write_at(0, 0xA6, 200, &mut mem);
        p.write_at(0, 0x96, 0x8201, &mut mem);
        p.write_at(3 * LINE, 0x9C, 0x0080, &mut mem);
        p.write_at(3 * LINE, 0xA6, 1, &mut mem);
        p.write_at(3 * LINE, 0x96, 0x0001, &mut mem);
        assert_eq!(p.sync_with(4 * LINE, &mut mem, false), 4 * LINE);
        assert_ne!(p.intreq & 0x80, 0);
    }

    /// Replays the register writes and DMA fetches of a UADE run (its comparison
    /// log, UADE_WRITE_LOG) and compares every sample with its `--write-audio`
    /// dump (recorded from reset, UADE_WRITE_AUDIO_ALL=1):
    /// UADE_WRITE_LOG=run.log UADE_WRITE_AUDIO=run.bin UADE_RATE=48000 UADE_FILTER=a500
    #[test]
    #[ignore]
    fn matches_uade_run() {
        let log = std::fs::read_to_string(std::env::var("UADE_WRITE_LOG").unwrap()).unwrap();
        let data = std::fs::read(std::env::var("UADE_WRITE_AUDIO").unwrap()).unwrap();
        let rate: u32 = std::env::var("UADE_RATE").map(|r| r.parse().unwrap()).unwrap_or(48000);
        let filter = match std::env::var("UADE_FILTER").as_deref() {
            Ok("a500") => Filter::A500,
            Ok("a1200") => Filter::A1200,
            _ => Filter::None,
        };
        let mut uade = Vec::new();
        for f in data[16..].chunks_exact(12) {
            if f[0] & 0x80 != 0 && f[5] == 8 {
                uade.push(i16::from_be_bytes([f[6], f[7]]));
            }
        }
        // the fetches, in order: each must come at the address UADE fetched
        let mut fetches = std::collections::VecDeque::new();
        let mut writes = Vec::new();
        for l in log.lines() {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w[0].starts_with('F') {
                fetches.push_back((u32::from_str_radix(w[1], 16).unwrap(), u16::from_str_radix(w[2], 16).unwrap()));
            } else {
                writes.push((w[2].parse::<u64>().unwrap(), u32::from_str_radix(&w[0][3..], 16).unwrap(), u16::from_str_radix(w[1], 16).unwrap()));
            }
        }
        let mut p = Paula::with_output(rate, filter);
        p.quirks = Quirks::Uade;
        // UADE's score leaves the master enables to the player
        p.dmacon = 0;
        p.intena = 0;
        let mut bad = None;
        let mut mem = |a: u32| -> u16 {
            match fetches.pop_front() {
                Some((fa, v)) if fa == a => v,
                other => {
                    bad.get_or_insert(format!("fetch at ${a:06x}, UADE fetched {other:x?}"));
                    0
                }
            }
        };
        for &(t, reg, v) in &writes {
            p.write_at(t, reg, v, &mut mem);
        }
        assert_eq!(bad, None);
        let ours: Vec<i16> = p.samples.iter().flat_map(|&(l, r)| [l, r]).collect();
        let n = ours.len().min(uade.len());
        let first = (0..n).find(|&i| ours[i] != uade[i]);
        assert_eq!(first.map(|i| i / 2), None, "first different sample (of {})", n / 2);
        assert!(n > 1000);
        eprintln!("{} samples bit-exact", n / 2);
    }
}

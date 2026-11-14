//! Brian Postma's SoundMon 2.0 ("V.2") and 2.2 ("V.3"), ported from NostalgicPlayer's
//! SoundMon player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! The two versions follow their own original players, which differ more than
//! NostalgicPlayer's merged one lets on: version 2.0 is checked against SoundMon
//! V5.02 and 2.2 against SoundMon 2.2 V2.1 (Eagleplayers, adapted by DEFECT).
//! Where NostalgicPlayer differs from them, this follows the originals:
//!
//! * Paula voice n plays the step column 3 - n, and the columns are read 3 to 0.
//! * The sample pointer and length are written every tick, a one-shot sample's as
//!   the player's empty word with length 0; the arpeggio sets the period of a
//!   2.0 voice for one tick only, and 2.0 cycles it over 3 ticks, not 4.
//! * A new instrument comes with no range check; 2.0 turns DMA off only for a
//!   voice whose instrument changes (or that played a one-shot sample), and
//!   turns each restarted voice on by itself.
//! * A restart writes the volume byte, $FF for "the instrument's", before the
//!   instrument's volume; volumes are not clamped to 64.
//! * A 2.0 sample plays its loop only; a 2.2 one plays whole, then its loop.
//! * 2.0 synth sounds take their LFO and EG delays as words (with the byte before
//!   each) and an EG start value from a byte NostalgicPlayer skips; 2.2's
//!   averaging works (NostalgicPlayer never runs it), over all 32 bytes from the
//!   byte before the waveform.
//! * Jumps go where the original goes: 2.0's repeat counter (starting at 1, so a
//!   repeat end without a start does not jump) sets the step and carries on with
//!   the next row; 2.2's jump starts the step's first row.
//! * DMA off and on are as far apart as in the originals, so Paula restarts a
//!   voice that is still playing.
//!
//! The synth effects change the waveforms in the module, in guest memory, at the
//! moment the original does, and restore them when the voice restarts.
//!
//! A voice that has played a one-shot sample goes on playing from the player's
//! empty word, with length 0: 131072 bytes of the player's own memory. The port
//! keeps the originals' variables there, in their layout (from memory dumps of the
//! two Eagleplayers: the voices' structures, the song position and counters, the
//! saved waveforms, the period table and the samples' addresses), as they are
//! after each tick: equal to the originals' on 89 2.0 and 61 2.2 modules at four
//! moments each.

use super::{be16, Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "SoundMon", detect, new };

/// Bytes of the player's empty sample: what Paula reads after a one-shot sample,
/// at length 0 (65536 words). The original's is its own memory, which is not silent:
/// the empty word is followed by the player's variables (`write_state`), which this
/// keeps there as the original does. What follows those is whatever the machine has
/// after the player's hunk (zeros when the Eagleplayer is loaded alone, the next
/// Eagleplayer's code when a whole directory of them is loaded); here zeros.
const EMPTY_SIZE: u32 = 0x20000;
/// Where the reference players (the Eagleplayers on this engine's 68k host, each loaded alone
/// as tools/refcmp.py runs them) have their empty word, from their write logs: the variables
/// that follow hold its address, so a voice reading them plays the same bytes only from the
/// same address. Elsewhere when that memory is taken.
const EMPTY_AT_20: u32 = 0xad_fdf6;
const EMPTY_AT_22: u32 = 0xad_fdd2;

#[derive(Default)]
struct Voice {
    /// Bit 15: the voice restarts on this row.
    period: u16,
    /// $FF: the instrument's.
    volume: u8,
    instrument: u8,
    /// The sample pointer and length (words) written every tick.
    addr: u32,
    len: u16,
    note: u8,
    arp_value: u8,
    auto_slide: u8,
    auto_arp: u8,
    eg_ptr: u16,
    lfo_ptr: u16,
    adsr_ptr: u16,
    mod_ptr: u16,
    /// Words in 2.0, bytes in 2.2.
    eg_count: u16,
    lfo_count: u16,
    adsr_count: u16,
    mod_count: u8,
    fx_count: u8,
    synth: bool,
    old_eg: u8,
    eg_control: u8,
    lfo_control: u8,
    adsr_control: u8,
    mod_control: u8,
    vibrato: u8,
    fx_control: u8,
    /// The waveform the synth effects change (0: none), and its original bytes.
    wave: u32,
    saved: [u8; 32],
}

pub struct SoundMon {
    /// Version 2.2 ("V.3"), else 2.0 ("V.2").
    v22: bool,
    title: String,
    d: Vec<u8>,
    steps: u16,
    /// Guest address of the waveforms.
    waves: u32,
    /// Guest address of each instrument's sample.
    samples: [u32; 15],
    empty: u32,
    /// The module's end in guest memory.
    end: u32,
    step: u16,
    /// The row in the track, times 3.
    pat: u8,
    st: u8,
    tr: u8,
    count: u8,
    delay: u8,
    arp: u8,
    vib: usize,
    /// 2.0's repeat counter.
    rep: u8,
    /// 2.2's jump.
    jump: bool,
    new_pos: u8,
    /// The voices whose DMA goes off for the new row, then (2.2) those restarted.
    mask: u16,
    voices: [Voice; 4],
    songend: bool,
    /// The original's time since its last write.
    c: Cost,
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 512 && matches!(&d[26..29], b"V.2" | b"V.3")
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let v22 = &d[26..29] == b"V.3";
    let title: String = d[..26].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
    let wave_num = d[29] as u32;
    let steps = be16(d, 30);
    // the waveforms follow the tracks, up to the highest one any step plays
    let mut max_track = 1i16;
    // the steps that raise it (the original's set-up time counts them)
    let mut raised = 0;
    for k in 0..steps as usize * 4 {
        let t = be16(d, 0x200 + 4 * k) as i16;
        if t > max_track {
            raised += 1;
        }
        max_track = max_track.max(t);
    }
    let waves = base + 0x200 + steps as u32 * 16 + max_track as u32 * 48;
    let mut samples = [0; 15];
    let mut a = waves + wave_num * 64;
    for (k, s) in samples.iter_mut().enumerate() {
        *s = a;
        let o = 0x20 + 32 * k;
        if d[o] != 0xff {
            a += be16(d, o + 0x18).wrapping_mul(2) as u32;
        }
    }
    let at = if v22 { EMPTY_AT_22 } else { EMPTY_AT_20 };
    // the reference's hunk ends at the heap's end, the stack's memory follows (what Paula reads
    // there is left as it is, in both)
    let size = EMPTY_SIZE.min(crate::machine::NATIVE_HI.saturating_sub(at));
    let empty = match m.heap.alloc_abs(at, size) {
        Some(_) => {
            m.bus.ram[at as usize..(at + size) as usize].fill(0);
            at
        }
        None => m.alloc(EMPTY_SIZE),
    };
    if empty == 0 {
        return None;
    }
    let smp = (0..15).filter(|&k| d[0x20 + 32 * k] != 0xff).count() as u64;
    let long = (0..15).filter(|&k| d[0x20 + 32 * k] != 0xff && be16(d, 0x20 + 32 * k + 0x18) > 1).count() as u64;
    let (load, cyc) = if v22 { LOAD_22 } else { LOAD_20 };
    m.bus.clock += load + (cyc[0] * steps as u64 + cyc[1] * raised + cyc[2] * smp + cyc[3] * long + cyc[4]) / 2;
    let mut voices: [Voice; 4] = Default::default();
    for v in &mut voices {
        v.addr = empty;
        v.len = 1;
    }
    Some(Box::new(SoundMon {
        v22,
        title,
        d: d.to_vec(),
        steps,
        waves,
        samples,
        empty,
        end: base + d.len() as u32,
        step: 0,
        pat: 0,
        st: 0,
        tr: 0,
        count: 1,
        delay: 6,
        arp: 1,
        vib: 0,
        // the original starts it at 1: a repeat end without its start does not jump
        rep: 1,
        jump: false,
        new_pos: 0,
        mask: 0,
        voices,
        songend: false,
        c: Cost { v22, ..Default::default() },
    }))
}

/// A note's period. A note past the table makes the original read its own memory
/// there; this plays period 0 (Paula's slowest) instead.
fn period(note: i32) -> u16 {
    usize::try_from(note + 35).ok().and_then(|i| PERIODS.get(i)).copied().unwrap_or(0)
}

/// The length the original writes: 1 (and 0) as 0.
fn plen(len: u16) -> u16 {
    if len <= 1 {
        0
    } else {
        len
    }
}

/// DMA on as the original's ENPP_DMAMask does it, waiting for the DMA to start.
fn dma_on(c: &mut Cost, hw: &mut Hw, site: u16, mask: u16) {
    c.site = site;
    put(c, hw, 0);
    hw.dmacon(0x8000 | mask & 0xF);
    dma_wait(c, hw);
}

/// The original's time from the module's start to the song's (its set-up), in colour clocks: a
/// base, and the cycles (halved, rounded down) of each step, of each step entry that raises the
/// highest track, of each sample and each sample longer than a word (and a constant). Fitted on
/// the reference players' logs of 89 and 74 modules and their byte-modified copies, exact on all.
/// The engine's song start is 100 colour clocks into the module's.
const LOAD_20: (u64, [u64; 5]) = (17249 - 100, [172, 14, 41, 18, 1]);
const LOAD_22: (u64, [u64; 5]) = (18029 - 100, [164, 14, 57, 18, 0]);

/// Colour clocks from the song's start to the first tick's interrupt, past the timer's period;
/// the time from there to the first write is the first segment's (see `cycles`).
const TIMER_DELAY_20: u64 = 331;
const TIMER_DELAY_22: u64 = 214;

/// A counter's decrement: 2.0 keeps words, 2.2 bytes.
fn dec(v22: bool, c: u16) -> u16 {
    if v22 {
        (c as u8).wrapping_sub(1) as u16
    } else {
        c.wrapping_sub(1)
    }
}

/// Steps a table pointer; a control of 1 plays the table once. Returns 0, 1 when the table
/// wraps, 2 when it ends (its events are 47 + that).
fn advance(ptr: &mut u16, control: &mut u8, len: u16) -> u16 {
    *ptr = ptr.wrapping_add(1);
    if *ptr == len {
        *ptr = 0;
        if *control == 1 {
            *control = 0;
            return 2;
        }
        return 1;
    }
    0
}

impl Replayer for SoundMon {
    fn format(&self) -> &'static str {
        if self.v22 {
            "SoundMon 2.2"
        } else {
            "SoundMon 2.0"
        }
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn timer_delay(&self) -> u64 {
        if self.v22 { TIMER_DELAY_22 } else { TIMER_DELAY_20 }
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
        self.write_state(hw.bus);
    }
}

impl SoundMon {
    fn tick_inner(&mut self, hw: &mut Hw) {
        // Paula has played up to now from the waveforms as they were
        let b = &mut *hw.bus;
        self.c.tick(b);
        b.paula.sync(b.clock, &b.ram);
        if self.v22 {
            self.arp = self.arp.wrapping_sub(1) & 3;
            self.vib = (self.vib + 1) & 7;
            ev(&mut self.c, hw.bus, if self.vib == 0 { 140 } else { 141 });
            ev(&mut self.c, hw.bus, if self.arp == 3 { 142 } else { 143 });
            for i in 0..4 {
                self.effects(i, hw);
            }
            for i in 0..4 {
                self.synth(i, hw);
            }
            self.write_state(hw.bus);
        } else {
            for i in 0..4 {
                self.synth(i, hw);
            }
            // the synth step's variables are in memory before the tick's first write
            self.write_state(hw.bus);
            self.arp = self.arp.wrapping_sub(1);
            for i in 0..4 {
                self.effects(i, hw);
            }
            if self.arp == 0 {
                ev(&mut self.c, hw.bus, 59);
                self.arp = 3;
            }
        }

        self.count = self.count.wrapping_sub(1);
        if self.count != 0 {
            ev(&mut self.c, hw.bus, 60);
            return;
        }
        ev(&mut self.c, hw.bus, 61);
        self.count = self.delay;
        self.next_row(hw);
        self.c.site = 201;
        put(&mut self.c, hw, 0);
        hw.dmacon(self.mask & 0xF);
        if self.v22 {
            for v in &mut self.voices {
                ev(&mut self.c, hw.bus, if v.period & 0x8000 == 0 { 91 } else if v.wave != 0 { 92 } else { 93 });
                if v.period & 0x8000 != 0 && v.wave != 0 {
                    hw.bus.write_bytes(v.wave, &v.saved);
                    v.wave = 0;
                }
            }
            self.mask = 0;
            for i in 0..4 {
                if self.voices[i].period & 0x8000 != 0 {
                    self.play_22(i, hw);
                } else {
                    ev(&mut self.c, hw.bus, 120);
                }
            }
            dma_on(&mut self.c, hw, 200, self.mask);
        } else {
            for i in 0..4 {
                if self.voices[i].period & 0x8000 != 0 {
                    self.play_20(i, hw);
                } else {
                    ev(&mut self.c, hw.bus, 119);
                }
            }
        }
    }
}

impl SoundMon {
    /// The player's variables as the original keeps them after its empty word, where a voice
    /// that has played a one-shot sample goes on reading them (522 bytes in 2.0, 556 in 2.2).
    /// 2.0: the word, four voices of 32 bytes, 16 bytes of song state, four saved waveforms
    /// (address and 32 bytes), the 84 periods, the 15 samples' addresses and a long of zeros.
    /// 2.2: the word, four voices of 36 bytes, the song state with the vibrato table (and the
    /// pointer into it), the last DMA word, the waveforms' address, the saved waveforms, the
    /// periods and the samples' addresses.
    fn write_state(&self, bus: &mut crate::bus::Bus) {
        let w16 = |s: &mut [u8], o: usize, v: u16| s[o..o + 2].copy_from_slice(&v.to_be_bytes());
        let w32 = |s: &mut [u8], o: usize, v: u32| s[o..o + 4].copy_from_slice(&v.to_be_bytes());
        let mut s = vec![0u8; if self.v22 { 0x22c } else { 0x20a }];
        // a voice: 32 bytes in 2.0, 36 in 2.2
        let size = if self.v22 { 36 } else { 32 };
        for (i, v) in self.voices.iter().enumerate() {
            let o = 2 + size * i;
            w16(&mut s, o, v.period);
            s[o + 2] = v.volume;
            s[o + 3] = v.instrument;
            w32(&mut s, o + 4, v.addr);
            w16(&mut s, o + 8, v.len);
            s[o + 10] = v.note;
            s[o + 11] = v.arp_value;
            s[o + 12] = v.auto_slide;
            s[o + 13] = v.auto_arp;
            w16(&mut s, o + 14, v.eg_ptr);
            w16(&mut s, o + 16, v.lfo_ptr);
            w16(&mut s, o + 18, v.adsr_ptr);
            if self.v22 {
                w16(&mut s, o + 20, v.mod_ptr);
                s[o + 22] = v.eg_count as u8;
                s[o + 23] = v.lfo_count as u8;
                s[o + 24] = v.adsr_count as u8;
                s[o + 25] = v.mod_count;
                s[o + 26] = v.fx_count;
                s[o + 27] = v.synth as u8;
                s[o + 28] = v.old_eg;
                s[o + 29] = v.eg_control;
                s[o + 30] = v.lfo_control;
                s[o + 31] = v.adsr_control;
                s[o + 32] = v.mod_control;
                s[o + 34] = v.vibrato;
                s[o + 35] = v.fx_control;
            } else {
                w16(&mut s, o + 20, v.eg_count);
                w16(&mut s, o + 22, v.lfo_count);
                w16(&mut s, o + 24, v.adsr_count);
                s[o + 26] = v.synth as u8;
                s[o + 28] = v.old_eg;
                s[o + 29] = v.eg_control;
                s[o + 30] = v.lfo_control;
                s[o + 31] = v.adsr_control;
            }
            // the waveform the synth effects change, and its bytes
            let b = if self.v22 { 0xb8 } else { 0x92 } + 36 * i;
            w32(&mut s, b, v.wave);
            s[b + 4..b + 36].copy_from_slice(&v.saved);
        }
        let g = 2 + 4 * size;
        w16(&mut s, g, self.step);
        s[g + 2] = self.pat;
        s[g + 3] = self.st;
        s[g + 4] = self.tr;
        s[g + 5] = self.count;
        s[g + 6] = self.delay;
        s[g + 7] = self.arp;
        s[g + 8] = self.rep;
        s[g + 9] = self.b(29);
        let (periods, samples) = if self.v22 {
            w32(&mut s, g + 12, self.empty + 0xa2 + 2 * self.vib as u32);
            for (k, v) in VIBRATO.iter().enumerate() {
                w16(&mut s, 0xa2 + 2 * k, *v as u16);
            }
            w16(&mut s, 0xb2, 0x8000 | self.mask);
            w32(&mut s, 0xb4, self.waves);
            (0x148, 0x1f0)
        } else {
            w16(&mut s, g + 10, self.mask);
            w32(&mut s, g + 12, self.waves);
            (0x122, 0x1ca)
        };
        for (k, p) in PERIODS.iter().enumerate() {
            w16(&mut s, periods + 2 * k, *p);
        }
        for (k, a) in self.samples.iter().enumerate() {
            w32(&mut s, samples + 4 * k, *a);
        }
        bus.write_bytes(self.empty, &s);
    }

    fn b(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// An instrument's 32 bytes (0: the module's header, as the original takes it).
    fn inst(&self, n: u8) -> [u8; 32] {
        std::array::from_fn(|k| self.b(n as usize * 32 + k))
    }

    /// A waveform's address.
    fn wave(&self, n: u8) -> u32 {
        self.waves + 64 * n as u32
    }

    fn sample(&self, inst: u8) -> u32 {
        // past the 15 the original reads a sample address from its own memory past
        // its table; this plays the empty sample there (with the same lengths and
        // volumes, from the module's bytes where the instrument would be)
        (inst as usize).checked_sub(1).and_then(|i| self.samples.get(i)).copied().unwrap_or(self.empty)
    }

    /// A table's byte at `ptr` (the original indexes with a signed word).
    fn table(&self, hw: &Hw, n: u8, ptr: u16) -> u8 {
        hw.bus.r8(self.wave(n).wrapping_add(ptr as i16 as u32))
    }

    /// The real-time effects: autoslide, vibrato (2.2), arpeggio; the period,
    /// sample pointer and length every tick.
    fn effects(&mut self, i: usize, hw: &mut Hw) {
        let v22 = self.v22;
        let (arp, vib) = (self.arp, VIBRATO[self.vib] as i32);
        let v = &mut self.voices[i];
        v.period = v.period.wrapping_add(v.auto_slide as i8 as u16);
        if v22 {
            ev(&mut self.c, hw.bus, if v.vibrato != 0 { 50 } else { 51 });
            if v.vibrato != 0 {
                ev(&mut self.c, hw.bus, 300 + divs_var(vib, v.vibrato as i16));
            }
            let p = if v.vibrato != 0 { ((vib / v.vibrato as i32) as u16).wrapping_add(v.period) } else { v.period };
            self.c.site = 202;
            put(&mut self.c, hw, i);
            hw.per(i, p);
        } else if v.lfo_control == 0 {
            ev(&mut self.c, hw.bus, 52);
            // 2.0: the LFO sets the period instead
            self.c.site = 203;
            put(&mut self.c, hw, i);
            hw.per(i, v.period);
        } else if i == 0 {
            ev(&mut self.c, hw.bus, 53);
        }
        if !v22 && v.lfo_control != 0 && i != 0 {
            ev(&mut self.c, hw.bus, 54);
        }
        if v22 {
            ev(&mut self.c, hw.bus, if v.vibrato != 0 { 180 } else { 181 });
        }
        self.c.site = 204;
        put(&mut self.c, hw, i);
        hw.loc(i, v.addr);
        self.c.site = 205;
        ev(&mut self.c, hw.bus, if v.len <= 1 { 150 } else { 151 });
        put(&mut self.c, hw, i);
        hw.len(i, plen(v.len));
        if v.arp_value == 0 && v.auto_arp == 0 {
            ev(&mut self.c, hw.bus, 55);
        }
        if v.arp_value != 0 || v.auto_arp != 0 {
            ev(&mut self.c, hw.bus, 56 + (arp as u16).min(2));
            ev(&mut self.c, hw.bus, if v.arp_value != 0 { 135 } else { 136 });
            let add = match arp {
                0 => (v.arp_value >> 4) + (v.auto_arp >> 4),
                1 => (v.arp_value & 15) + (v.auto_arp & 15),
                _ => 0,
            };
            let p = period(add.wrapping_add(v.note) as i8 as i32);
            self.c.site = 206;
            put(&mut self.c, hw, i);
            hw.per(i, p);
            ev(&mut self.c, hw.bus, 160 + (arp as u16).min(2));
            ev(&mut self.c, hw.bus, if v.arp_value != 0 { 165 } else { 166 });
            ev(&mut self.c, hw.bus, if v.auto_arp != 0 { 167 } else { 168 });
            if v22 {
                v.period = p;
            }
        }
    }

    fn next_row(&mut self, hw: &mut Hw) {
        self.mask = 0;
        for i in 0..4 {
            let e = 0x200 + (self.step as usize * 16 & 0xffff) + 4 * (3 - i);
            let track = self.w(e);
            self.st = self.b(e + 2);
            self.tr = self.b(e + 3);
            let row = 0x200
                + self.pat as usize
                + (self.steps as usize * 16 & 0xffff)
                + track.wrapping_sub(1) as usize * 48;
            let (note, io, data) = (self.b(row), self.b(row + 1), self.b(row + 2));
            let opt = io & 15;
            let (v22, st, tr) = (self.v22, self.st, self.tr);
            let v = &mut self.voices[i];
            ev(&mut self.c, hw.bus, if note != 0 { 63 } else { 62 });
            if note != 0 {
                v.auto_slide = 0;
                v.auto_arp = 0;
                if v22 {
                    v.vibrato = 0;
                }
                // a transpose optional can leave out the note transpose
                if !(opt != 0xa || data & 0xf0 == 0) {
                    ev(&mut self.c, hw.bus, 64);
                }
                let n = if opt != 0xa || data & 0xf0 == 0 { note.wrapping_add(tr) as i8 as i32 } else { note as i32 };
                v.note = n as u8;
                v.period = period(n);
                let restart = !v22 || opt < 0xd;
                if restart {
                    ev(&mut self.c, hw.bus, 65);
                    v.period |= 0x8000;
                    v.volume = 0xff;
                }
                let mut inst = io >> 4;
                if inst == 0 {
                    ev(&mut self.c, hw.bus, 66);
                    inst = v.instrument;
                }
                if opt != 0xa || data & 0x0f == 0 {
                    inst = inst.wrapping_add(st);
                } else {
                    ev(&mut self.c, hw.bus, 67);
                }
                let change = if v22 {
                    ev(&mut self.c, hw.bus, if !restart { 132 } else if !v.synth { 133 } else { 134 });
                    restart && (!v.synth || inst != v.instrument)
                } else {
                    ev(&mut self.c, hw.bus, if v.len == 1 { 130 } else { 131 });
                    v.len == 1 || inst != v.instrument
                };
                if change {
                    ev(&mut self.c, hw.bus, 68);
                    v.instrument = inst;
                    self.mask |= 1 << i;
                }
            }
            self.optional(i, opt, data, hw);
        }

        if self.jump {
            ev(&mut self.c, hw.bus, 88);
            self.jump = false;
            self.pat = 0;
            self.step = self.step & 0xff00 | self.new_pos as u16;
            return;
        }
        self.pat += 3;
        if self.pat == 0x30 {
            ev(&mut self.c, hw.bus, 89);
            self.pat = 0;
            self.step = self.step.wrapping_add(1);
            if self.step == self.steps {
                ev(&mut self.c, hw.bus, 90);
                self.step = 0;
                self.songend = true;
            }
        }
    }

    fn optional(&mut self, i: usize, opt: u8, data: u8, hw: &mut Hw) {
        let v22 = self.v22;
        let v = &mut self.voices[i];
        ev(&mut self.c, hw.bus, 70 + opt as u16);
        match opt {
            0 => v.arp_value = data,
            1 => {
                v.volume = data;
                ev(&mut self.c, hw.bus, if !v22 || !v.synth { 86 } else { 87 });
                if !v22 || !v.synth {
                    self.c.site = 207;
                    put(&mut self.c, hw, i);
                    hw.vol(i, data as u16);
                }
            }
            2 => {
                self.count = data;
                self.delay = data;
            }
            // ENPP_PokeCommand's LED filter
            3 if data == 0 => {
                // 2.0's goes through a host call (its time counts anew), 2.2's not
                if !v22 {
                    host(&mut self.c);
                }
                ev(&mut self.c, hw.bus, 137);
                hw.bus.ciaa.pra |= 2
            }
            3 => hw.bus.ciaa.pra &= !2,
            4 => {
                v.period = v.period.wrapping_sub(data as u16);
                v.arp_value = 0;
            }
            5 => {
                v.period = v.period.wrapping_add(data as u16);
                v.arp_value = 0;
            }
            6 if v22 => v.vibrato = data,
            6 => self.rep = data,
            7 if v22 => {
                self.new_pos = data;
                self.jump = true;
            }
            7 => {
                self.rep = self.rep.wrapping_sub(1);
                if self.rep != 0 {
                    ev(&mut self.c, hw.bus, 85);
                    self.step = data as u16;
                }
            }
            8 => v.auto_slide = data,
            9 if !v22 => v.auto_arp = data,
            0xb if v22 => v.fx_control = data,
            9 | 0xd | 0xe | 0xf if v22 => {
                v.auto_arp = data;
                if opt == 0xd {
                    v.fx_control ^= 1;
                }
                ev(&mut self.c, hw.bus, if opt == 0xf { 192 } else if v.adsr_control == 0 { 190 } else { 191 });
                if opt != 0xf {
                    v.adsr_ptr = 0;
                    if v.adsr_control == 0 {
                        v.adsr_control = 1;
                    }
                }
            }
            _ => {}
        }
    }

    /// The volume a restart writes: the voice's byte, then the instrument's if that is $FF.
    fn restart_volume(c: &mut Cost, hw: &mut Hw, i: usize, volume: u8, inst_volume: u16, synth: bool) {
        ev(c, hw.bus, if volume == 0xff { 104 } else { 105 });
        c.site = 208;
        put(c, hw, i);
        hw.vol(i, volume as u16);
        if volume == 0xff {
            ev(c, hw.bus, if synth { 138 } else { 139 });
            c.site = 209;
            put(c, hw, i);
            hw.vol(i, inst_volume);
        }
    }

    /// The first ADSR volume of a synth sound.
    fn adsr_start(&mut self, i: usize, table: u8, inst_volume: u8, hw: &mut Hw) {
        let t = self.table(hw, table, 0);
        let v = &mut self.voices[i];
        ev(&mut self.c, hw.bus, 103);
        if v.volume == 0xff {
            ev(&mut self.c, hw.bus, 106);
            v.volume = inst_volume;
        }
        self.c.site = 210;
        put(&mut self.c, hw, i);
        hw.vol(i, (t.wrapping_add(0x80) as u16 >> 2) * v.volume as u16 >> 6);
    }

    fn save_wave(&mut self, i: usize, wave: u32, hw: &Hw) {
        let v = &mut self.voices[i];
        v.wave = wave;
        for (k, s) in v.saved.iter_mut().enumerate() {
            *s = hw.bus.r8(wave + k as u32);
        }
    }

    /// 2.0's restart of a voice.
    fn play_20(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        v.period &= 0x7fff;
        // the waveform the last sound changed (the pointer stays)
        if v.wave != 0 {
            ev(&mut self.c, hw.bus, 100);
            hw.bus.write_bytes(v.wave, &v.saved);
        }
        self.c.site = 211;
        put(&mut self.c, hw, i);
        hw.per(i, v.period);
        let inst = v.instrument;
        let n = self.inst(inst);
        let w = |o: usize| be16(&n, o);
        if n[0] == 0xff {
            let wave = self.wave(n[1]);
            ev(&mut self.c, hw.bus, 101);
            let v = &mut self.voices[i];
            v.synth = true;
            v.eg_ptr = 0;
            v.lfo_ptr = 0;
            v.adsr_ptr = 0;
            v.eg_count = w(0x16).wrapping_add(1);
            v.lfo_count = w(0xe).wrapping_add(1);
            v.adsr_count = 1;
            v.eg_control = n[0x11];
            v.lfo_control = n[9];
            v.adsr_control = n[4];
            v.old_eg = n[0x13];
            v.addr = wave;
            v.len = w(2);
            self.c.site = 212;
            put(&mut self.c, hw, i);
            hw.loc(i, wave);
            self.c.site = 213;
            ev(&mut self.c, hw.bus, if v.len <= 1 { 150 } else { 151 });
            put(&mut self.c, hw, i);
            hw.len(i, plen(v.len));
            if n[4] != 0 {
                self.adsr_start(i, n[5], n[0x19], hw);
            } else {
                Self::restart_volume(&mut self.c, hw, i, v.volume, n[0x19] as u16, true);
            }
            self.save_wave(i, wave, hw);
            // the EG's start value: the waveform's first bytes inverted
            ev(&mut self.c, hw.bus, if n[0x11] == 0 { 94 } else if n[0x13] == 0 { 95 } else { 96 });
            if n[0x11] != 0 && n[0x13] != 0 {
                let e = n[0x13] >> 3;
                self.voices[i].old_eg = e;
                // a value below 8 has the original invert 65536 bytes; this stops at
                // the module's end
                let count = if e == 0 { 0x10000 } else { e as u32 };
                for a in wave..(wave + count).min(self.end) {
                    ev(&mut self.c, hw.bus, 107);
                    let x = hw.bus.r8(a);
                    hw.bus.w8(a, x.wrapping_neg());
                }
            }
        } else {
            ev(&mut self.c, hw.bus, 102);
            let a = self.sample(inst);
            let empty = self.empty;
            let v = &mut self.voices[i];
            v.wave = 0;
            v.synth = false;
            v.lfo_control = 0;
            v.adsr_control = 0;
            self.c.site = 214;
            put(&mut self.c, hw, i);
            hw.loc(i, a);
            self.c.site = 215;
            ev(&mut self.c, hw.bus, if w(0x18) <= 1 { 150 } else { 151 });
            put(&mut self.c, hw, i);
            hw.len(i, plen(w(0x18)));
            Self::restart_volume(&mut self.c, hw, i, v.volume, w(0x1e), false);
            // only the loop plays: its pointer is written before DMA starts
            v.len = w(0x1c);
            v.addr = a + w(0x1a) as u32;
            if v.len == 1 {
                ev(&mut self.c, hw.bus, 108);
                v.addr = empty;
            } else {
                self.c.site = 216;
                put(&mut self.c, hw, i);
                hw.loc(i, v.addr);
                ev(&mut self.c, hw.bus, 109);
                self.c.site = 217;
                ev(&mut self.c, hw.bus, if v.len <= 1 { 150 } else { 151 });
                put(&mut self.c, hw, i);
                hw.len(i, plen(v.len));
            }
        }
        dma_on(&mut self.c, hw, 218, 1 << i);
    }

    /// 2.2's restart of a voice.
    fn play_22(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        v.period &= 0x7fff;
        self.c.site = 219;
        put(&mut self.c, hw, i);
        hw.per(i, v.period);
        let inst = v.instrument;
        let n = self.inst(inst);
        let w = |o: usize| be16(&n, o);
        if n[0] == 0xff {
            let wave = self.wave(n[1]);
            ev(&mut self.c, hw.bus, 110);
            let v = &mut self.voices[i];
            v.synth = true;
            v.eg_ptr = 0;
            v.lfo_ptr = 0;
            v.adsr_ptr = 0;
            v.mod_ptr = 0;
            v.eg_count = n[0x14].wrapping_add(1) as u16;
            v.lfo_count = n[0xe].wrapping_add(1) as u16;
            v.adsr_count = 1;
            v.mod_count = n[0x1c].wrapping_add(1);
            v.fx_count = n[0x18].wrapping_add(1);
            v.fx_control = n[0x16];
            v.eg_control = n[0x10];
            v.lfo_control = n[9];
            v.adsr_control = n[4];
            v.mod_control = n[0x19];
            v.old_eg = 0;
            v.addr = wave;
            v.len = w(2);
            self.c.site = 220;
            put(&mut self.c, hw, i);
            hw.loc(i, wave);
            self.c.site = 221;
            ev(&mut self.c, hw.bus, if v.len <= 1 { 150 } else { 151 });
            put(&mut self.c, hw, i);
            hw.len(i, plen(v.len));
            if n[4] != 0 {
                self.adsr_start(i, n[5], n[0x1d], hw);
            } else {
                Self::restart_volume(&mut self.c, hw, i, v.volume, n[0x1d] as u16, true);
            }
            ev(&mut self.c, hw.bus, if n[0x10] != 0 { 97 } else if n[0x19] != 0 { 98 } else if n[0x16] != 0 { 99 } else { 115 });
            if n[0x10] != 0 || n[0x19] != 0 || n[0x16] != 0 {
                self.save_wave(i, wave, hw);
            }
        } else {
            ev(&mut self.c, hw.bus, 111);
            let a = self.sample(inst);
            let empty = self.empty;
            let v = &mut self.voices[i];
            v.synth = false;
            v.lfo_control = 0;
            self.c.site = 222;
            put(&mut self.c, hw, i);
            hw.loc(i, a);
            self.c.site = 223;
            ev(&mut self.c, hw.bus, if w(0x18) <= 1 { 150 } else { 151 });
            put(&mut self.c, hw, i);
            hw.len(i, plen(w(0x18)));
            Self::restart_volume(&mut self.c, hw, i, v.volume, w(0x1e), false);
            v.len = w(0x1c);
            let loop_start = w(0x1a);
            v.addr = a + loop_start as u32;
            ev(&mut self.c, hw.bus, if v.len == 1 { 116 } else if loop_start >> 1 != 0 { 117 } else { 118 });
            if v.len == 1 {
                v.addr = empty;
            } else if loop_start >> 1 != 0 {
                // the original means to write the length up to the loop end here,
                // but calls ENPP_PokeLen through the voice's synth buffer pointer
                // instead of its EaglePlayer base: it jumps into its own code and
                // writes nothing. The whole sample plays, then (from the next tick)
                // its loop.
            } else {
                self.c.site = 224;
                put(&mut self.c, hw, i);
                hw.loc(i, v.addr);
                ev(&mut self.c, hw.bus, 109);
                self.c.site = 225;
                ev(&mut self.c, hw.bus, if v.len <= 1 { 150 } else { 151 });
                put(&mut self.c, hw, i);
                hw.len(i, plen(v.len));
            }
        }
        self.mask |= 1 << i;
    }

    /// The synth effects of a voice playing a synth sound.
    fn synth(&mut self, i: usize, hw: &mut Hw) {
        let v22 = self.v22;
        if v22 {
        }
        if !self.voices[i].synth {
            ev(&mut self.c, hw.bus, 10);
            return;
        }
        ev(&mut self.c, hw.bus, 11);
        let n = self.inst(self.voices[i].instrument);
        let w = |o: usize| be16(&n, o);
        // the instrument's offsets that differ between the versions
        let (lfo_speed, eg_table, eg_len, eg_speed) = if v22 { (0xf, 0x11, 0x12, 0x15) } else { (0x10, 0x12, 0x14, 0x18) };

        ev(&mut self.c, hw.bus, if self.voices[i].adsr_control == 0 { 12 } else if dec(v22, self.voices[i].adsr_count) != 0 { 13 } else { 14 });
        if self.voices[i].adsr_control != 0 {
            let v = &mut self.voices[i];
            v.adsr_count = dec(v22, v.adsr_count);
            if v.adsr_count == 0 {
                v.adsr_count = n[8] as u16;
                let t = self.table(hw, n[5], self.voices[i].adsr_ptr);
                let v = &mut self.voices[i];
                self.c.site = 226;
                put(&mut self.c, hw, i);
                hw.vol(i, (t.wrapping_add(0x80) as u16 >> 2) * v.volume as u16 >> 6);
                let v = &mut self.voices[i];
                let k = advance(&mut v.adsr_ptr, &mut v.adsr_control, w(6));
                if k != 0 {
                    ev(&mut self.c, hw.bus, 47 + k);
                }
            }
        }

        ev(&mut self.c, hw.bus, if self.voices[i].lfo_control == 0 { 15 } else if dec(v22, self.voices[i].lfo_count) != 0 { 16 } else { 17 });
        if self.voices[i].lfo_control != 0 {
            let v = &mut self.voices[i];
            v.lfo_count = dec(v22, v.lfo_count);
            if v.lfo_count == 0 {
                v.lfo_count = n[lfo_speed] as u16;
                let mut t = self.table(hw, n[0xa], self.voices[i].lfo_ptr) as i8 as i32;
                if n[0xb] != 0 {
                    ev(&mut self.c, hw.bus, 18);
                    ev(&mut self.c, hw.bus, 300 + divs_var(t, n[0xb] as i16));
                    t /= n[0xb] as i32;
                }
                let v = &mut self.voices[i];
                self.c.site = 227;
                put(&mut self.c, hw, i);
                hw.per(i, v.period.wrapping_add(t as u16));
                let k = advance(&mut v.lfo_ptr, &mut v.lfo_control, w(0xc));
                if k != 0 {
                    ev(&mut self.c, hw.bus, 47 + k);
                }
            }
        }

        let wave = self.voices[i].wave;
        if v22 && wave == 0 {
            ev(&mut self.c, hw.bus, 19);
            return;
        }
        ev(&mut self.c, hw.bus, if self.voices[i].eg_control == 0 { 20 } else if dec(v22, self.voices[i].eg_count) != 0 || wave == 0 { 21 } else { 22 });
        if self.voices[i].eg_control != 0 {
            let v = &mut self.voices[i];
            v.eg_count = dec(v22, v.eg_count);
            // without a waveform the counter stays at 0
            if v.eg_count == 0 && wave != 0 {
                if v22 && v.fx_control == 1 && v.fx_count.wrapping_sub(1) == 0 {
                    ev(&mut self.c, hw.bus, 182);
                }
                v.eg_count = n[eg_speed] as u16;
                let t = self.table(hw, n[eg_table], self.voices[i].eg_ptr);
                self.eg(i, t.wrapping_add(0x80) >> 3, hw);
                let v = &mut self.voices[i];
                let k = advance(&mut v.eg_ptr, &mut v.eg_control, w(eg_len));
                if k != 0 {
                    ev(&mut self.c, hw.bus, 47 + k);
                }
            }
        }
        if !v22 {
            return;
        }
        self.fx(i, n[0x17], wave, hw);

        ev(&mut self.c, hw.bus, if self.voices[i].mod_control == 0 { 42 } else if self.voices[i].mod_count.wrapping_sub(1) != 0 { 43 } else { 44 });
        if self.voices[i].mod_control != 0 {
            let v = &mut self.voices[i];
            v.mod_count = v.mod_count.wrapping_sub(1);
            if v.mod_count == 0 {
                v.mod_count = n[0x1b];
                let t = self.table(hw, n[0x1a], self.voices[i].mod_ptr);
                hw.bus.w8(wave + 32, t);
                let v = &mut self.voices[i];
                let k = advance(&mut v.mod_ptr, &mut v.mod_control, w(0x1e));
                if k != 0 {
                    ev(&mut self.c, hw.bus, 47 + k);
                }
            }
        }
    }

    /// The EG: inverts the waveform's first bytes up to the new value, or restores
    /// them down to it.
    fn eg(&mut self, i: usize, new: u8, hw: &mut Hw) {
        let v = &mut self.voices[i];
        let old = v.old_eg;
        v.old_eg = new;
        ev(&mut self.c, hw.bus, if new > old { 25 } else if new < old { 26 } else { 27 });
        if new > old {
            for k in old..new {
                ev(&mut self.c, hw.bus, 23);
                hw.bus.w8(v.wave + k as u32, v.saved[k as usize & 31].wrapping_neg());
            }
        } else {
            for k in (new..old).rev() {
                ev(&mut self.c, hw.bus, 24);
                hw.bus.w8(v.wave + k as u32, v.saved[k as usize & 31]);
            }
        }
    }

    /// 2.2's waveform effects.
    fn fx(&mut self, i: usize, speed: u8, wave: u32, hw: &mut Hw) {
        let bus = &mut *hw.bus;
        let v = &mut self.voices[i];
        ev(&mut self.c, bus, 30 + v.fx_control.min(7) as u16);
        // towards a target waveform, by `speed` a byte
        let mut transform = |bus: &mut crate::bus::Bus, target: &dyn Fn(&crate::bus::Bus, u32) -> u8| {
            for k in 0..32 {
                let s = target(bus, k) as i8;
                let a = wave + k;
                let x = bus.r8(a);
                ev(&mut self.c, bus, if s > x as i8 { 37 } else if s < x as i8 { 38 } else { 39 });
                if s > x as i8 {
                    bus.w8(a, x.wrapping_add(speed));
                } else if s < x as i8 {
                    bus.w8(a, x.wrapping_sub(speed));
                }
            }
        };
        let saved = v.saved;
        match v.fx_control {
            1 => {
                v.fx_count = v.fx_count.wrapping_sub(1);
                ev(&mut self.c, bus, if v.fx_count == 0 { 40 } else { 41 });
                if v.fx_count == 0 {
                    v.fx_count = speed;
                    // averaging, from the byte before the waveform to the one after
                    let mut prev = bus.r8(wave.wrapping_sub(1)) as i8 as i16;
                    for k in (0..32).step_by(2) {
                        let a = wave + k;
                        let x = (bus.r8(a + 1) as i8 as i16 + prev) >> 1;
                        bus.w8(a, x as u8);
                        prev = (bus.r8(a + 2) as i8 as i16 + x) >> 1;
                        bus.w8(a + 1, prev as u8);
                    }
                }
            }
            2 => transform(bus, &|_, k| saved[31 - k as usize]),
            3 | 5 => transform(bus, &|_, k| saved[k as usize]),
            4 => transform(bus, &|bus, k| bus.r8(wave + 0x40 + k)),
            6 => {
                v.fx_count = v.fx_count.wrapping_sub(1);
                ev(&mut self.c, bus, if v.fx_count == 0 { 46 } else { 47 });
                if v.fx_count == 0 {
                    v.fx_control = 0;
                    v.fx_count = 1;
                    for k in 0..32 {
                        let x = bus.r8(wave + 0x40 + k);
                        bus.w8(wave + k, x);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The data-dependent part of a 68000 DIVS.W's cycles (the count of Jorge Cwik's "68000 DIVS
/// timing"), in steps of two cycles, less 50: event 300 + this in the cost tables.
fn divs_var(dividend: i32, divisor: i16) -> u16 {
    let mut m: u32 = 6;
    if dividend < 0 {
        m += 1;
    }
    if dividend.unsigned_abs() >> 16 >= divisor.unsigned_abs() as u32 {
        return (m + 2) as u16;
    }
    let mut q = dividend.unsigned_abs() / divisor.unsigned_abs() as u32;
    m += 55;
    if divisor >= 0 {
        if dividend >= 0 {
            m -= 1;
        } else {
            m += 1;
        }
    }
    for _ in 0..15 {
        if (q as i16) >= 0 {
            m += 1;
        }
        q <<= 1;
    }
    (m - 50) as u16
}

/// Logs each event (and each write's site) in the write log (address $EE0000, the event as the
/// value), to fit the tables again on new reference logs.
const COST_LOG: bool = false;

fn log(bus: &mut Bus, id: u16) {
    if COST_LOG {
        let t = bus.clock;
        if let Some(l) = &mut bus.write_log {
            l.push((0xEE0000, id, t));
        }
    }
}

/// The original's time, in 68000 cycles (the engine's count; a colour clock is two), fitted on the
/// write logs of the reference players (89 modules of 2.0, 74 of 2.2, 4 s each). The engine runs a
/// player's code from a whole colour clock and each host call (every Paula write here goes
/// through the host's poke functions, and the LED filter's ENPP_PokeCommand) starts the count
/// anew, so a write comes `floor(cycles / 2)` colour clocks after the previous one, where `cycles`
/// is the code path between them: the segment's cost by the two writes' sites and voices
/// (`SEG_20`, `SEG_22`: site * 4 + voice, 0 the tick's interrupt, 1 the end of the DMA wait) plus
/// each event's on the way (`EV_20`, `EV_22`).
///
/// The sites are the write calls: 200 DMA on, 201 the row's DMA off, 202/203/206 the effects'
/// periods, 204/205 their pointer and length, 207 a row's volume, 208/209 a restart's volume
/// byte and the instrument's, 210 the ADSR's first volume, 211-218 2.0's restart, 219-225
/// 2.2's, 226 the ADSR's volume, 227 the LFO's period.
///
/// The events, where the code path branches: 1 the tick; the synth step 10 none, 11 a synth
/// voice, 12-14 the ADSR off / waiting / a step, 15-17 the LFO the same, 18 its division (and
/// 300 + `divs_var`), 19 no waveform (2.2), 20-22 the EG, 23/24 a byte inverted / restored,
/// 25-27 more / fewer / the same; 2.2's waveform effects 30 + effect, 37-39 a byte raised /
/// lowered / kept, 40/41 averaging or waiting, 46/47 effect 6 the same, 42-44 the modulation off
/// / waiting / a step, 48/49 a table wrapping (49: played once), 182 an EG step on a tick of
/// averaging; the effects 50/51 vibrato or not (300 + `divs_var` its division, 180/181 after the
/// period), 52-54 2.0's period (the LFO's or not), 55-58 the arpeggio none or its step, 135/136
/// by the voice's value or the effect's, 160-168 after its period; 59 2.0's arpeggio counter
/// wrapping, 140-143 2.2's vibrato and arpeggio counters wrapping or not; 60/61 no row / a row;
/// a voice's row 62 no note, 63 a note, 64 the transpose off, 65 a restart, 66 the instrument
/// kept, 67 its transpose off, 68 a new instrument, 130-134 the instrument check's path; 70 +
/// optional, 85 a repeat, 86/87 a row's volume written or not, 88-90 the jump, the track's end,
/// the song's end, 137 the LED filter on; 2.2's DMA off 91-93 by voice; restarts 100-109 (2.0)
/// and 110-118 (2.2) by sound kind, 104/105 a volume byte of $FF or not, 138/139 for a synth
/// sound or a sample, 94-99 and 115 the EG's start and the saved waveform, 119/120 a voice not
/// restarted; 150/151 a length of 0 or 1 words, or more. Many events always come together, so
/// the split of their time is one of many that fit (some are negative); their sums are what was
/// measured.
#[derive(Default)]
struct Cost {
    v22: bool,
    cyc: i32,
    /// Colour clocks owed from host calls since the last write.
    owed: i32,
    /// The last write's site * 4 + voice.
    last: u16,
    /// The next write's site.
    site: u16,
}

const TICK: u16 = 0;
const WAIT: u16 = 1;

impl Cost {
    fn tick(&mut self, bus: &mut Bus) {
        log(bus, 1);
        self.cyc = 0;
        self.owed = 0;
        self.last = TICK;
    }
}

/// An event of the original's code path (see `Cost`).
fn ev(c: &mut Cost, bus: &mut Bus, id: u16) {
    log(bus, id);
    let t = if c.v22 { EV_22 } else { EV_20 };
    c.cyc += t.binary_search_by(|e| e.0.cmp(&id)).map_or(0, |i| t[i].1);
}

/// A host call that writes nothing: the count starts anew.
fn host(c: &mut Cost) {
    c.owed += c.cyc.div_euclid(2);
    c.cyc = 0;
}

/// The time up to the write at `c.site` of voice `ch`.
fn put(c: &mut Cost, hw: &mut Hw, ch: usize) {
    log(hw.bus, c.site);
    let k = c.site * 4 + ch as u16;
    let t = if c.v22 { SEG_22 } else { SEG_20 };
    let s = t.binary_search_by(|e| (e.0, e.1).cmp(&(c.last, k))).map_or(0, |i| t[i].2);
    let n = c.owed + (c.cyc + s).div_euclid(2);
    hw.wait(n.max(0) as u64);
    c.cyc = 0;
    c.owed = 0;
    c.last = k;
}

/// The host's wait for the DMA to start after a DMA on: to the tenth line from the write's.
fn dma_wait(c: &mut Cost, hw: &mut Hw) {
    let (now, line) = (hw.bus.clock, crate::paula::LINE);
    hw.wait((now / line + 10) * line - now);
    c.cyc = 0;
    c.owed = 0;
    c.last = WAIT;
}

include!("soundmon_costs.rs");

include!("soundmon_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One step of one track with one note, all voices, of instrument 1: a synth
    /// sound on the module's one waveform, at volume 64 (its offset in `mark`'s layout).
    fn module(mark: &[u8; 3], volume_at: usize) -> Vec<u8> {
        let mut d = b"test".to_vec();
        d.resize(26, 0);
        d.extend(mark);
        d.extend([1, 0, 1]); // 1 waveform, 1 step
        d.resize(0x200, 0);
        d[0x20] = 0xff; // synth, waveform 0
        d[0x23] = 16; // of 16 words
        d[0x20 + volume_at] = 64;
        for _ in 0..4 {
            d.extend([0, 1, 0, 0]); // track 1, no transposes
        }
        d.extend([25, 0x10, 0]); // note 25, instrument 1
        d.resize(0x240, 0);
        d.extend((0..64).map(|k| (k * 4) as u8)); // the waveform
        d
    }

    fn play(d: Vec<u8>, format: &str) -> Vec<(u32, u16, u64)> {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.bp"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, format);
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        e.m.bus.write_log.take().unwrap()
    }

    /// The player's variables follow its empty word in guest memory, in each version's layout.
    #[test]
    fn keeps_its_variables_after_the_empty_word() {
        for (mark, volume_at, periods, voice) in [(b"V.2", 0x19, 0x122, 32), (b"V.3", 0x1d, 0x148, 36)] {
            let mut e = Engine::new(48000.0);
            e.open_replayers = true;
            e.m.bus.write_log = Some(Vec::new());
            e.play_data(Path::new("test.bp"), module(mark, volume_at)).unwrap();
            let mut out = vec![0f32; 2 * 4800];
            e.render(&mut out);
            let log = e.m.bus.write_log.take().unwrap();
            // the first tick points every voice at the empty word
            let empty = (reg(&log, 0xDFF0A0)[0] as u32) << 16 | reg(&log, 0xDFF0A2)[0] as u32;
            let bus = &e.m.bus;
            assert_eq!(bus.r16(empty), 0);
            for i in 0..4 {
                let v = empty + 2 + voice * i;
                assert_eq!(bus.r16(v), super::PERIODS[60], "{mark:?} voice {i}");
                assert_eq!(bus.r8(v + 3), 1, "the instrument");
                assert_eq!(bus.r16(v + 8), 16, "the length");
            }
            for (k, p) in super::PERIODS.iter().enumerate() {
                assert_eq!(bus.r16(empty + periods + 2 * k as u32), *p);
            }
        }
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_synth_note_on_paula() {
        let log = play(module(b"V.2", 0x19), "SoundMon 2.0");
        // the first tick's empty voices, then the note's period, waveform and volume
        assert_eq!(&reg(&log, 0xDFF0A6)[..2], &[0, super::PERIODS[60]]);
        assert_eq!(&reg(&log, 0xDFF0A4)[..2], &[0, 16]);
        assert_eq!(&reg(&log, 0xDFF0A8)[..2], &[0xff, 64]);
        // 2.0 turns each voice on by itself
        assert_eq!(&reg(&log, 0xDFF096)[..5], &[0x000F, 0x8001, 0x8002, 0x8004, 0x8008]);

        let log = play(module(b"V.3", 0x1d), "SoundMon 2.2");
        assert_eq!(&reg(&log, 0xDFF0A6)[..2], &[0, super::PERIODS[60]]);
        assert_eq!(&reg(&log, 0xDFF0A8)[..2], &[0xff, 64]);
        assert_eq!(&reg(&log, 0xDFF096)[..2], &[0x000F, 0x800F]);
    }
}

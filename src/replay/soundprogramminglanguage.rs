//! Sound Programming Language (`spl.*`), Holger Gehrmann's game-music driver of 1987-88.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no original
//! code. The specification was drawn up from the Paula register writes of the 68k reference
//! player and beam-read traces, and this player was written from it alone and checked
//! against those writes (value, order and tick-relative time).
//!
//! A module is an Amiga executable (one code hunk) that carries the game's whole sound
//! driver, its tables, the note streams and the samples, behind a small header of the
//! Wanted Team (the `!SOPROL!` tag, the entry points, the number of subsongs, the title
//! and the timer). The module's code is never run: this player interprets the module's data
//! as the specification states. Three generations of the driver exist (family 1: Boulder
//! Dash, family 2: Clever & Smart and Graffiti Man, family 3: Hollywood Poker); they share
//! the note and period maths and the stream language and differ in the channel record, the
//! per-tick effects, the tempo and the song start. The offsets of each module's tables and
//! variables inside the code hunk are the specification's per-module layout parameters; a
//! module is recognised when its hunk has the header and one of the known layouts.
//!
//! The hunk is the driver's memory: its own copy is kept here as a mutable byte array
//! (the specification's memory model), and the writes the driver makes into the sample area
//! are made in the module's memory in the guest as well.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Sound Programming Language", detect, new };

#[derive(Clone, Copy, PartialEq, Debug)]
enum Fam {
    F1,
    F2,
    F3,
}

/// The layout parameters of one module (offsets in the code hunk), SPEC section 9.
struct Layout {
    fam: Fam,
    /// The timer reload value of the header (hunk offset 0x34).
    timer: u32,
    /// Graffiti Man: the song select, stop and transposition commands exist.
    gm: bool,
    /// The load address of the reference run: only the values the driver stores into its own
    /// tables (the call stacks) depend on it (Clever & Smart's overflow, SPEC 4.9).
    ref_base: u32,
    recs: usize,
    period: usize,
    transp: usize,
    transp_val: u8,
    /// Family 1: the sample base variable (a long) and its initial value.
    sbase: usize,
    sbase_val: u32,
    /// Family 2: the length table (words) and the offset table (longs).
    lens: usize,
    offs: usize,
    stacks: [usize; 4],
    /// Initial stream positions per song.
    starts: &'static [[u32; 4]],
    /// Families 1 and 3: the hardware preset (length word of the first sample area, offset
    /// of the sample voice 0 starts on).
    preset: Option<(usize, usize)>,
    /// Clever & Smart writes ADKCON once at its start.
    adkcon: bool,
    /// The host's first write of the start in the reference run (colour clock from reset), and
    /// the colour clocks from the last write of the start to the first tick's start.
    t0: u64,
    tdelay: u64,
    /// Colour clocks from the end of a start to the host's DMACON of the next, and from the last
    /// volume write to the preset's first write (first start, later ones).
    dma_gap: f64,
    pre_gap: [f64; 2],
    cost: &'static [f64; N_C],
    /// The costs of Graffiti Man's songs that run two voices.
    cost2: Option<&'static [f64; N_C]>,
    gaps: &'static Gaps,
}

const LAYOUTS: &[Layout] = &[
    // Boulder Dash
    Layout {
        fam: Fam::F1,
        timer: 11823,
        gm: false,
        ref_base: 0x16188,
        recs: 0x0B6,
        period: 0x2F6,
        transp: 0x23E,
        transp_val: 9,
        sbase: 0x0B2,
        sbase_val: 0x1372,
        lens: 0,
        offs: 0,
        stacks: [0x12CC, 0x12F4, 0x131C, 0x1344],
        starts: &[[0xA26, 0xABE, 0xBBE, 0xC96]],
        preset: Some((0x1378, 0x13A2)),
        adkcon: false,
        t0: 18161,
        tdelay: 59324,
        dma_gap: 0.0,
        pre_gap: [121.0, 121.0],
        cost: &COST_BD,
        cost2: None,
        gaps: &G_BD,
    },
    // Clever & Smart
    Layout {
        fam: Fam::F2,
        timer: 14188,
        gm: false,
        ref_base: 0x16750,
        recs: 0x138,
        period: 0x2F8,
        transp: 0x137,
        transp_val: 9,
        sbase: 0,
        sbase_val: 0,
        lens: 0x1182,
        offs: 0x11BE,
        stacks: [0x1102, 0x112A, 0x1152, 0x117A],
        starts: &[[0xD3A, 0xDD2, 0xEEA, 0xEFA]],
        preset: None,
        adkcon: true,
        t0: 5257,
        tdelay: 71434,
        dma_gap: 0.0,
        pre_gap: [0.0, 0.0],
        cost: &COST_CS,
        cost2: None,
        gaps: &G_CS,
    },
    // Graffiti Man
    Layout {
        fam: Fam::F2,
        timer: 14188,
        gm: true,
        ref_base: 0x15B50,
        recs: 0x13C,
        period: 0x2F2,
        transp: 0x13B,
        transp_val: 7,
        sbase: 0,
        sbase_val: 0,
        lens: 0x1A6C,
        offs: 0x1A40,
        stacks: [0x19A0, 0x19C8, 0x19F0, 0x1A18],
        starts: &[
            [0x1380, 0x1440, 0x14F0, 0x1518],
            [0x15C0, 0x18E0, 0x1950, 0x1978],
            [0x1690, 0x18F0, 0x1950, 0x1978],
            [0x1748, 0x1900, 0x1950, 0x1978],
            [0x17F0, 0x1910, 0x1950, 0x1978],
            [0x1850, 0x1920, 0x1950, 0x1978],
            [0x18B8, 0x1930, 0x1960, 0x1988],
            [0x18CC, 0x1940, 0x196C, 0x1994],
        ],
        preset: None,
        adkcon: false,
        t0: 8596,
        tdelay: 71462,
        dma_gap: 311.5,
        pre_gap: [0.0, 0.0],
        cost: &COST_GM,
        cost2: Some(&COST_GM2),
        gaps: &G_GM,
    },
    // Hollywood Poker
    Layout {
        fam: Fam::F3,
        timer: 40000,
        gm: false,
        ref_base: 0x15AD8,
        recs: 0x0EE,
        period: 0x1F2,
        transp: 0x156,
        transp_val: 5,
        sbase: 0,
        sbase_val: 0,
        lens: 0,
        offs: 0,
        stacks: [0xCBE, 0xCC6, 0xCEE, 0xD16],
        starts: &[[0x862, 0x676, 0x6DE, 0x71E], [0x862, 0x872, 0x8BA, 0x912]],
        preset: Some((0xD44, 0xD6E)),
        adkcon: false,
        t0: 13100,
        tdelay: 200553,
        dma_gap: 100.0,
        pre_gap: [202.0, 258.0],
        cost: &COST_HP,
        cost2: None,
        gaps: &G_HP,
    },
];

/// The period table (SPEC 3.2): 16 words.
const PERIODS: [u16; 16] =
    [0xD600, 0xCA00, 0xBE80, 0xB400, 0xA980, 0xA000, 0x9700, 0x8E80, 0x8680, 0x7F00, 0x7800, 0x7100, 0x6B00, 0x6B00, 0x6B00, 0x6B00];

/// The mask of the tempo of family 2 (steps every third tick).
const TEMPO_MASK: u32 = 0x9249_2492;

/// The code hunk of a module file: the hunk's bytes, if the file has the standard header
/// the Eagleplayer checks (SPEC 2.1).
fn hunk(d: &[u8]) -> Option<&[u8]> {
    if be32(d, 0) != 0x3F3 || d.len() < 0x58 {
        return None;
    }
    if d[0x14] == 0 || be32(d, 0x20) != 0x70FF_4E75 || &d[0x24..0x2C] != b"!SOPROL!" {
        return None;
    }
    if be32(d, 0x2C) == 0 || be32(d, 0x30) == 0 || be32(d, 0x34) == 0 {
        return None;
    }
    let n = (be32(d, 0x1C) as usize).checked_mul(4)?;
    d.get(0x20..0x20 + n.min(d.len() - 0x20))
}

/// The known layout this hunk has: the period table where the layout puts it, the timer
/// of the family, the global transposition and (family 1) the sample base, and every
/// table inside the hunk.
fn layout(h: &[u8]) -> Option<&'static Layout> {
    LAYOUTS.iter().find(|l| {
        let fits = |o: usize, n: usize| o + n <= h.len();
        if be32(h, 0x34) != l.timer || !fits(l.period, 32) || !fits(l.recs, 0x3A * 4) || !fits(l.transp, 1) {
            return false;
        }
        if (0..16).any(|i| be16(h, l.period + 2 * i) != PERIODS[i]) || h[l.transp] != l.transp_val {
            return false;
        }
        if l.stacks.iter().any(|&s| !fits(s, 0x28)) {
            return false;
        }
        match l.fam {
            Fam::F1 => fits(l.sbase, 4) && be32(h, l.sbase) == l.sbase_val,
            Fam::F2 => fits(l.lens, 22) && fits(l.offs, 44) && l.starts.iter().all(|s| s.iter().all(|&p| (p as usize) < h.len())),
            Fam::F3 => true,
        }
    })
}

fn detect(d: &[u8]) -> bool {
    hunk(d).and_then(layout).is_some()
}

// ---------------------------------------------------------------------------------
// The player

#[derive(Clone, Copy, Default)]
struct Voice {
    pos: u32,
    sp: u32,
    wait: u8,
    def: u8,
}

/// Costs in half colour clocks.
fn cc(x: f64) -> i64 {
    (x * 2.0).round() as i64
}

pub struct Spl {
    l: &'static Layout,
    img: Vec<u8>,
    /// Guest address of the hunk's first byte.
    hbase: u32,
    v: [Voice; 4],
    song: u16,
    songs: u16,
    sub: u16,
    tick_ctr: u16,
    tempo: u16,
    // timing
    t: i64,
    /// Clock of the last write of the start.
    end_t: u64,
    /// Colour clocks from the last write of the start to its return.
    lag: u64,
    cost: &'static [f64; N_C],
    last_loop: bool,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let h = hunk(d)?;
    let l = layout(h)?;
    let songs = (be32(h, 0x14) as usize).clamp(1, l.starts.len()) as u16;
    let mut r = Spl {
        l,
        img: h.to_vec(),
        hbase: base + 0x20,
        v: [Voice::default(); 4],
        song: 1,
        songs,
        sub: 0,
        tick_ctr: 0,
        tempo: 0,
        t: 0,
        end_t: 0,
        lag: 0,
        cost: l.cost,
        last_loop: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    r.host_start(&mut hw, true);
    r.start(1, &mut hw);
    Some(Box::new(r))
}

impl Spl {
    fn rb(&self, o: usize) -> u8 {
        self.img.get(o).copied().unwrap_or(0)
    }
    fn rw(&self, o: usize) -> u16 {
        ((self.rb(o) as u16) << 8) | self.rb(o + 1) as u16
    }
    fn rl(&self, o: usize) -> u32 {
        ((self.rw(o) as u32) << 16) | self.rw(o + 2) as u32
    }
    fn wb(&mut self, o: usize, v: u8) {
        if let Some(b) = self.img.get_mut(o) {
            *b = v;
        }
    }
    fn ww(&mut self, o: usize, v: u16) {
        self.wb(o, (v >> 8) as u8);
        self.wb(o + 1, v as u8);
    }
    fn wl(&mut self, o: usize, v: u32) {
        self.ww(o, (v >> 16) as u16);
        self.ww(o + 2, v as u16);
    }

    fn rec(&self, v: usize) -> usize {
        self.l.recs + v * if self.l.fam == Fam::F3 { 16 } else { 0x3A }
    }

    // --- the hardware, with the clock of the write

    fn stamp(&self, hw: &mut Hw) {
        let c = ((self.t + 1) >> 1) as u64;
        if c > hw.bus.clock {
            hw.bus.clock = c;
        }
    }
    fn w_len(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.len(ch, n);
    }
    fn w_per(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.per(ch, n);
    }
    fn w_vol(&self, hw: &mut Hw, ch: usize, n: u16) {
        self.stamp(hw);
        hw.vol(ch, n & 0x7F);
    }
    fn w_loc(&self, hw: &mut Hw, ch: usize, a: u32) {
        self.stamp(hw);
        hw.loc(ch, a);
    }
    fn w_dma(&self, hw: &mut Hw, n: u16) {
        self.stamp(hw);
        hw.dmacon(n);
    }
    fn adv(&mut self, x: f64) {
        self.t += cc(x);
    }
    /// `n` times the cost of class `k`.
    fn c(&mut self, k: usize, n: f64) {
        self.t += cc(self.cost[k] * n);
    }

    /// The host's own writes: the volume registers cleared, all four voices, twice (SPEC 7);
    /// before the start entry of every subsong after the first, DMACON and three rounds.
    fn host_start(&mut self, hw: &mut Hw, first: bool) {
        self.t = ((hw.bus.clock + if first { 0 } else { self.lag }) as i64) * 2;
        if first {
            hw.wait(self.l.t0.saturating_sub(hw.bus.clock));
            self.t = (hw.bus.clock as i64) * 2;
            self.vols(hw, false);
            self.adv(249.0);
            self.vols(hw, false);
        } else {
            self.adv(self.l.dma_gap);
            self.w_dma(hw, 0x000F);
            self.adv(8.0);
            self.vols(hw, true);
            self.adv(228.0);
            self.vols(hw, false);
            self.adv(249.0);
            self.vols(hw, false);
        }
    }
    /// Four volume writes of 0 (voices 0..3), `fast`: 8 colour clocks apart.
    fn vols(&mut self, hw: &mut Hw, fast: bool) {
        const G: [f64; 4] = [0.0, 132.0, 144.0, 139.0];
        for c in 0..4 {
            self.adv(if fast { if c == 0 { 0.0 } else { 8.0 } } else { G[c] });
            self.w_vol(hw, c, 0);
        }
    }

    /// The start entry for subsong `n` (1-based): stream positions, call stacks, counters,
    /// then the hardware preset (SPEC 4.7, 4.10, 5.1).
    fn start(&mut self, n: u16, hw: &mut Hw) {
        let l = self.l;
        let si = match l.fam {
            Fam::F3 => (n as usize).clamp(1, 2) - 1,
            Fam::F2 if l.gm => (n as usize).wrapping_sub(1),
            _ => 0,
        };
        self.song = n;
        self.cost = match l.cost2 {
            Some(c) if !matches!(n, 1 | 7 | 8) => c,
            _ => l.cost,
        };
        if let Some(st) = l.starts.get(si) {
            for v in 0..4 {
                self.v[v] = Voice { pos: st[v], sp: l.stacks[v] as u32, wait: 0, def: 0 };
            }
        } else {
            for v in 0..4 {
                self.v[v].sp = l.stacks[v] as u32;
                self.v[v].wait = 0;
                self.v[v].def = 0;
            }
        }
        self.tick_ctr = 0;
        self.tempo = 0;
        if let Some((lenw, lc)) = l.preset {
            let len = self.rw(lenw) / 2;
            if l.fam == Fam::F1 {
                let r = self.rec(0);
                self.ww(r + 4, len);
            }
            self.adv(l.pre_gap[(n > 1) as usize]);
            self.w_len(hw, 0, len);
            self.adv(10.0);
            self.w_per(hw, 0, 0x01AC);
            self.adv(10.0);
            self.w_vol(hw, 0, 0x40);
            self.adv(10.0);
            self.w_dma(hw, 0x800F);
            self.adv(14.0);
            self.w_loc(hw, 0, self.hbase + lc as u32);
            self.adv(30.0);
            self.stamp(hw);
            hw.bus.write_word(0xDFF09E, 0x0088);
        } else if l.adkcon {
            self.adv(107.0);
            self.stamp(hw);
            hw.bus.write_word(0xDFF09E, 0x0088);
        }
        self.stamp(hw);
        // Graffiti Man's start entry takes 15.5 colour clocks per song number to return
        if l.gm {
            self.adv(15.5 * n as f64);
        }
        self.lag = (((self.t + 1) >> 1) as u64).saturating_sub(hw.bus.clock);
        self.end_t = hw.bus.clock + self.lag;
    }

    // --- the stack of the call commands (SPEC 3.4); the values stored are BASE + position

    fn push(&mut self, v: usize, val: u32) {
        let sp = self.v[v].sp as usize;
        self.wl(sp, val);
        self.v[v].sp += 4;
    }
    fn pop(&mut self, v: usize) -> u32 {
        self.v[v].sp = self.v[v].sp.wrapping_sub(4);
        self.rl(self.v[v].sp as usize)
    }

    // --- notes

    /// Applies T to the note byte (SPEC 3.2 step 1).
    fn transpose(b: u8, t: u8) -> u8 {
        let mut b = b;
        for _ in 0..t {
            b = b.wrapping_add(1);
            if b & 15 == 12 {
                b = b.wrapping_add(0x10) & 0xF0;
            }
        }
        b
    }

    fn period_of(&self, b: u8) -> u16 {
        let e = self.rw(self.l.period + 2 * (b & 15) as usize);
        e >> (b >> 4)
    }

    fn gt(&self) -> u8 {
        self.rb(self.l.transp)
    }
}


// ---------------------------------------------------------------------------------
// Timing (colour clocks), SPEC 8: medians fitted to the reference logs

// Classes of cost between two writes (a table of costs per family below).
const LEAD: usize = 0; // tick start to the first event
const EFF_CLR: usize = 1; // an effect part tested, flag clear
const EFF_DEC: usize = 2; // flag set, counter running
const EFF_NOWR: usize = 3; // flag set, counter out, nothing to write
// 4: arpeggio, counter out, up to the write (voice 0; ARP_PRE_V)
const ARP_POST: usize = 5; // after the arpeggio write
// 6: the same for the decay write (voice 0; DEC_PRE_V)
const DEC_POST: usize = 7;
const HEAD: usize = 8; // head of a parser step
// 9: unused
const DUR: usize = 10; // stream commands
const NOP: usize = 11;
const CALL: usize = 12;
const RET: usize = 13;
const JUMP: usize = 14;
const LOAD: usize = 15;
const OTHER: usize = 16;
const TRANS: usize = 17; // per step of the transposition loop (families 1 and 3)
const AFTER: usize = 18; // end of a note-on to the start of the next voice (+ voice 0..3: 18..21)
const NEXT: usize = 22; // between the notes of a multi-note
const SKIPV: usize = 23; // a voice skipped (+ voice 0..3)
const AFTERL: usize = 27; // the same after a note-on that ends with a loop part (+ voice)
const DEC_PRE_V: [usize; 4] = [6, 31, 32, 33]; // the decay write's pre-cost by voice
const ARP_PRE_V: [usize; 4] = [4, 34, 35, 36];
const N_C: usize = 37;

/// Costs between two writes per module (fitted to the module's reference logs), by class.
const COST_BD: [f64; N_C] =
    [0.0, 62.2, 88.0, 95.7, 0.0, 127.7, 0.0, 176.7, 57.2, 0.0, 117.7, 35.5, 78.6, 105.7, 185.6, 561.7, 0.0, 26.0, 461.2, 456.2, 469.9, 462.4, 289.0, 60.3, 68.3, 68.3, 65.6, 533.2, 528.2, 541.9, 534.4, 145.3, 0.0, 0.0, 124.3, 0.0, 0.0];
const COST_CS: [f64; N_C] =
    [0.0, 16.0, 41.0, 0.0, 0.0, 215.3, 21.2, 224.9, 343.8, 0.0, 116.4, 34.8, 76.1, 101.0, 196.6, 461.6, 0.0, 0.0, 488.0, 484.7, 487.1, 486.6, 424.0, 64.4, 60.0, 64.4, 62.9, 560.0, 556.7, 559.1, 558.6, 21.2, 71.0, 81.8, 0.0, 0.0, 8.0];
const COST_GM: [f64; N_C] =
    [57.0, 22.3, 46.2, 0.0, 0.0, 0.0, 43.3, 194.3, 385.0, 0.0, 126.6, 36.7, 78.8, 98.8, 175.7, 427.1, 90.4, 0.0, 509.9, 542.8, 506.4, 519.7, 519.7, 68.3, 105.7, 62.6, 78.9, 583.6, 614.8, 578.4, 591.7, 75.9, 122.2, 115.2, 0.0, 0.0, 0.0];
const COST_GM2: [f64; N_C] =
    [0.0, 18.7, 43.4, 0.0, 0.0, 0.0, 116.2, 132.4, 474.3, 0.0, 124.9, 34.3, 67.9, 115.6, 182.4, 461.2, 120.6, 0.0, 506.0, 506.0, 506.0, 506.0, 506.0, 71.9, 71.9, 71.9, 71.9, 583.3, 578.0, 578.0, 578.0, 143.2, 116.2, 116.2, 0.0, 0.0, 0.0];
const COST_HP: [f64; N_C] =
    [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 115.8, 36.4, 72.2, 112.1, 183.6, 135.4, 0.0, 26.0, 419.3, 420.3, 422.6, 420.7, 420.7, 60.4, 67.9, 64.0, 64.1, 491.3, 492.3, 494.6, 492.7, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

/// Gaps of a note-on, per voice.
struct Gaps {
    /// DMACON off to PER
    per: f64,
    lch: [f64; 4],
    len: [f64; 4],
    vol: [f64; 4],
    on: [f64; 4],
    /// The wait's post time to the DMACON off: base per voice, per octave shift and per
    /// step of the transposition loop.
    post: [f64; 4],
    shift: f64,
    trans: f64,
    /// The envelope restart: volume write to LEN=1, DMACON off to LEN, LEN to PER, PER to DMACON on.
    env: [[f64; 4]; 3],
    env_len: f64,
}

const G_BD: Gaps = Gaps {
    per: 6.0,
    lch: [151.0, 167.0, 183.0, 187.0],
    len: [60.0, 76.0, 91.0, 95.0],
    vol: [156.0, 183.0, 210.0, 210.0],
    on: [90.0, 106.0, 121.0, 125.0],
    post: [405.5, 417.8, 430.1, 447.1],
    shift: 17.5,
    trans: 0.0,
    env: [[93.0, 104.5, 116.0, 127.5], [70.5, 82.2, 93.8, 105.5], [86.5, 98.2, 109.8, 121.5]],
    env_len: 20.0,
};
const G_CS: Gaps = Gaps {
    per: 6.0,
    lch: [142.0, 158.0, 173.0, 177.0],
    len: [87.0, 102.0, 117.0, 121.0],
    vol: [156.0, 183.0, 210.0, 210.0],
    on: [100.0, 116.0, 131.0, 135.0],
    post: [423.7, 421.4, 418.2, 420.3],
    shift: 3.4,
    trans: 25.6,
    env: [[98.3, 108.0, 117.8, 127.5], [74.3, 84.0, 93.8, 103.5], [90.3, 100.0, 109.8, 119.5]],
    env_len: 41.0,
};
const G_GM: Gaps = Gaps {
    per: 6.0,
    lch: [142.0, 158.0, 173.0, 177.0],
    len: [87.0, 102.0, 117.0, 121.0],
    vol: [156.0, 183.0, 210.0, 210.0],
    on: [100.0, 116.0, 131.0, 135.0],
    post: [444.8, 446.0, 440.5, 444.4],
    shift: 0.0,
    trans: 26.0,
    env: [[98.3, 108.0, 117.8, 127.5], [74.3, 84.0, 93.8, 103.5], [90.3, 100.0, 109.8, 119.5]],
    env_len: 41.0,
};
const G_HP: Gaps = Gaps {
    per: 6.0,
    lch: [101.0, 116.0, 131.0, 136.0],
    len: [66.0, 82.0, 98.0, 101.0],
    vol: [157.0, 184.0, 210.0, 210.0],
    on: [92.0, 108.0, 124.0, 127.0],
    post: [159.5, 173.2, 186.9, 199.7],
    shift: 19.3,
    trans: 0.0,
    env: [[93.0, 104.5, 116.0, 127.5], [70.5, 82.2, 93.8, 105.5], [86.5, 98.2, 109.8, 121.5]],
    env_len: 20.0,
};

impl Spl {
    fn gaps(&self) -> &'static Gaps {
        self.l.gaps
    }

    /// The driver's busy-wait, which the host turned into a raster wait (SPEC 8.2): the first
    /// beam read 41 colour clocks after the last write, the next 8 later, then every 12.5; the
    /// wait ends at the first read in the ninth line after the first read's, and the next
    /// write comes `post` colour clocks after that read.
    fn beam_wait(&mut self, post: f64) {
        let r0 = self.t + 82;
        let target = (r0.div_euclid(454) + 9) * 454;
        let r1 = r0 + 16;
        let r = if target <= r1 { r1 } else { r1 + (target - r1 + 24) / 25 * 25 };
        self.t = r + cc(post);
    }

    // ---- note-on

    /// One full note-on of the note byte `b` on voice `v` (SPEC 4.3, 4.4, 5.2).
    fn note_on(&mut self, hw: &mut Hw, v: usize, b: u8) {
        self.last_loop = false;
        match self.l.fam {
            Fam::F1 => self.note_on1(hw, v, b),
            Fam::F2 => self.note_on2(hw, v, b),
            Fam::F3 => self.note_on3(hw, v, b),
        }
    }

    fn init_runtime(&mut self, rec: usize, vol: u16, env_reload: u16) {
        for o in (0x22..0x2A).step_by(2) {
            self.ww(rec + o, 0);
        }
        self.ww(rec + 0x26, vol);
        self.ww(rec + 0x2A, vol);
        self.ww(rec + 0x28, env_reload);
    }

    fn history(&mut self, rec: usize, per: u16) {
        for o in [0x38, 0x36, 0x34] {
            let x = self.rw(rec + o - 2);
            self.ww(rec + o, x);
        }
        self.ww(rec + 0x32, per);
    }

    fn note_on1(&mut self, hw: &mut Hw, v: usize, b: u8) {
        let g = self.gaps();
        let rec = self.rec(v);
        let t = self.gt().wrapping_add(self.rw(rec + 0x20) as u8);
        let b = Self::transpose(b, t);
        self.c(TRANS, t as f64);
        let oct = (self.rw(rec + 0x0C) as u8) << 4;
        let bf = b.wrapping_add(oct);
        let per = self.period_of(bf);
        self.w_len(hw, v, 1);
        self.w_per(hw, v, 1);
        self.beam_wait(g.post[v] + g.shift * (bf >> 4) as f64);
        let vol = self.rw(rec + 0x1E);
        let env = self.rw(rec + 0x14);
        self.init_runtime(rec, vol, env);
        self.w_dma(hw, 1 << v);
        self.adv(g.per);
        self.w_per(hw, v, per);
        self.history(rec, per);
        let lc = self.hbase.wrapping_add(self.rl(self.l.sbase)).wrapping_add(self.rl(rec));
        self.adv(g.lch[v]);
        self.w_loc(hw, v, lc);
        self.adv(g.len[v]);
        self.w_len(hw, v, self.rw(rec + 4));
        self.adv(g.vol[v]);
        self.w_vol(hw, v, vol);
        self.adv(g.on[v]);
        self.w_dma(hw, 0x8000 | 1 << v);
    }

    fn note_on2(&mut self, hw: &mut Hw, v: usize, b: u8) {
        if b == 0 {
            return;
        }
        let g = self.gaps();
        let l = self.l;
        let rec = self.rec(v);
        let s = self.rw(rec) as usize;
        let len = self.rw(l.lens + 2 * s);
        let start = self.rw(rec + 0x14);
        self.w_len(hw, v, 1);
        self.w_per(hw, v, 1);
        let t = self.gt().wrapping_add(self.rw(rec + 0x18) as u8);
        let b = Self::transpose(b, t);
        let oct = (self.rw(rec + 4) as u8) << 4;
        let bf = b.wrapping_add(oct);
        let per = self.period_of(bf);
        self.beam_wait(g.post[v] + g.trans * t as f64 + g.shift * (bf >> 4) as f64);
        let vol = self.rw(rec + 0x16);
        let env = self.rw(rec + 0x0C);
        self.init_runtime(rec, vol, env);
        self.w_dma(hw, 1 << v);
        self.adv(g.per);
        self.w_per(hw, v, per);
        self.history(rec, per);
        let off = self.rl(l.offs + 4 * s);
        self.adv(g.lch[v]);
        self.w_loc(hw, v, self.hbase.wrapping_add(off).wrapping_add(start as u32));
        // the first long of the sample is cleared, in the hunk and in the guest's copy
        self.wl(off as usize, 0);
        hw.bus.write_bytes(self.hbase.wrapping_add(off), &[0, 0, 0, 0]);
        self.adv(g.len[v]);
        self.w_len(hw, v, (len.wrapping_sub(start)) / 2);
        self.adv(g.vol[v]);
        self.w_vol(hw, v, vol);
        self.adv(g.on[v]);
        self.w_dma(hw, 0x8000 | 1 << v);
        let d = self.rw(rec + 2) / 2;
        self.last_loop = d != 0;
        self.beam_wait(if d == 0 { 70.0 } else { 84.0 });
        if d == 0 {
            self.w_len(hw, v, 1);
        } else {
            self.w_len(hw, v, len.wrapping_sub(d));
            self.adv([76.0, 92.0, 102.0, 112.0][v]);
            self.w_loc(hw, v, self.hbase.wrapping_add(off).wrapping_add(2 * d as u32));
        }
    }

    fn note_on3(&mut self, hw: &mut Hw, v: usize, b: u8) {
        let g = self.gaps();
        let rec = self.rec(v);
        let t = self.gt();
        let b = Self::transpose(b, t);
        self.c(TRANS, t as f64);
        let bf = b.wrapping_add(self.rb(rec + 8) << 4);
        let per = self.period_of(bf);
        self.w_len(hw, v, 1);
        self.w_per(hw, v, 1);
        self.beam_wait(g.post[v] + g.shift * (bf >> 4) as f64);
        self.w_dma(hw, 1 << v);
        self.adv(g.per);
        self.w_per(hw, v, per);
        self.adv(g.lch[v]);
        self.w_loc(hw, v, self.hbase.wrapping_add(self.rl(rec)));
        self.adv(g.len[v]);
        self.w_len(hw, v, self.rw(rec + 4));
        self.adv(g.vol[v]);
        self.w_vol(hw, v, self.rb(rec + 0x0F) as u16);
        self.adv(g.on[v]);
        self.w_dma(hw, 0x8000 | 1 << v);
    }

    // ---- per-tick effects (families 1 and 2), SPEC 4.5

    fn effects(&mut self, hw: &mut Hw, nvoices: usize) {
        let f2 = self.l.fam == Fam::F2;
        for v in 0..nvoices {
            let rec = self.rec(v);
            let (flags, arp, dec, envr, step) = if f2 {
                (self.rw(rec + 6), self.rw(rec + 8), self.rw(rec + 0x0A), self.rw(rec + 0x0C), self.rw(rec + 0x0E))
            } else {
                (self.rw(rec + 0x0E), self.rw(rec + 0x10), self.rw(rec + 0x12), self.rw(rec + 0x14), self.rw(rec + 0x16))
            };
            // arpeggio
            if flags & 1 != 0 {
                let c = self.rw(rec + 0x22);
                if c != 0 {
                    self.ww(rec + 0x22, c - 1);
                    self.c(EFF_DEC, 1.0);
                } else {
                    self.ww(rec + 0x22, arp);
                    let (a, b, c2, d) = (self.rw(rec + 0x32), self.rw(rec + 0x34), self.rw(rec + 0x36), self.rw(rec + 0x38));
                    if flags & 2 != 0 {
                        self.ww(rec + 0x32, d);
                        self.ww(rec + 0x38, c2);
                        self.ww(rec + 0x36, b);
                        self.ww(rec + 0x34, a);
                    } else {
                        self.ww(rec + 0x32, b);
                        self.ww(rec + 0x34, a);
                    }
                    self.c(ARP_PRE_V[v], 1.0);
                    let p = self.rw(rec + 0x34);
                    self.w_per(hw, v, p);
                    self.c(ARP_POST, 1.0);
                }
            } else {
                self.c(EFF_CLR, 1.0);
            }
            // decay
            if flags & 4 != 0 {
                let c = self.rw(rec + 0x24);
                if c != 0 {
                    self.ww(rec + 0x24, c - 1);
                    self.c(EFF_DEC, 1.0);
                } else {
                    self.ww(rec + 0x24, dec);
                    let vol = self.rw(rec + 0x26);
                    if vol != 0 {
                        let nv = vol.wrapping_sub(8);
                        self.ww(rec + 0x26, nv);
                        self.c(DEC_PRE_V[v], 1.0);
                        self.w_vol(hw, v, nv);
                        self.c(DEC_POST, 1.0);
                    } else {
                        self.c(EFF_NOWR, 1.0);
                    }
                }
            } else {
                self.c(EFF_CLR, 1.0);
            }
            // envelope restart
            if flags & 8 != 0 {
                let c = self.rw(rec + 0x28);
                if c != 0 {
                    self.ww(rec + 0x28, c - 1);
                    self.c(EFF_DEC, 1.0);
                } else {
                    self.ww(rec + 0x28, envr);
                    let ev = self.rw(rec + 0x2A);
                    if ev != 0 {
                        let mut nv = ev.wrapping_sub(step);
                        if nv & 0x8000 != 0 {
                            nv = 0;
                        }
                        self.ww(rec + 0x2A, nv);
                        self.ww(rec + 0x26, nv);
                        let g = self.gaps();
                        self.adv(100.0);
                        self.w_vol(hw, v, nv);
                        self.adv(g.env[0][v]);
                        self.w_len(hw, v, 1);
                        self.w_per(hw, v, 1);
                        self.beam_wait(54.0);
                        self.w_dma(hw, 1 << v);
                        let rl = if f2 {
                            let s = self.rw(rec) as usize;
                            self.rw(self.l.lens + 2 * s)
                        } else {
                            self.rw(rec + 4)
                        };
                        self.adv(g.env_len);
                        self.w_len(hw, v, rl);
                        self.adv(g.env[1][v]);
                        let p = self.rw(rec + 0x32);
                        self.w_per(hw, v, p);
                        self.adv(g.env[2][v]);
                        self.w_dma(hw, 0x8000 | 1 << v);
                    } else {
                        self.c(EFF_NOWR, 1.0);
                    }
                }
            } else {
                self.c(EFF_CLR, 1.0);
            }
        }
    }

    // ---- the parser, SPEC 3.3

    /// One parser step over the first `nvoices` voices; false when a stop command ended it.
    fn parse(&mut self, hw: &mut Hw, nvoices: usize) -> bool {
        let l = self.l;
        self.c(HEAD, 1.0);
        for v in 0..nvoices {
            if self.v[v].wait != 0 {
                self.v[v].wait -= 1;
                if self.v[v].wait != 0 {
                    self.c(SKIPV + v, 1.0);
                    continue;
                }
            }
            let mut guard = 0;
            loop {
                guard += 1;
                if guard > 4096 {
                    break;
                }
                let pos = self.v[v].pos as usize;
                let b = self.rb(pos);
                match b {
                    0xF4 => {
                        self.v[v].pos += 1;
                        self.c(NOP, 1.0);
                    }
                    0xF3 => {
                        self.v[v].pos = self.rl(pos + 1);
                        self.c(JUMP, 1.0);
                    }
                    0xF1 => {
                        // The driver pushes BASE + position. Only Clever & Smart's overflowing stack
                        // makes the value visible (it lands in the sample length table, SPEC 4.9), and
                        // then it depends on the load address: the reference run's is used here
                        // (Layout::ref_base), so that the AUDxLEN values equal that run's.
                        self.push(v, l.ref_base.wrapping_add(pos as u32 + 1));
                        self.v[v].pos = self.rl(pos + 1);
                        self.c(CALL, 1.0);
                    }
                    0xF2 => {
                        let x = self.pop(v);
                        self.v[v].pos = x.wrapping_sub(l.ref_base).wrapping_add(4);
                        self.c(RET, 1.0);
                    }
                    0xF0 => {
                        let n = match l.fam {
                            Fam::F1 => 0x22,
                            Fam::F2 => 0x1A,
                            Fam::F3 => 0x10,
                        };
                        let rec = self.rec(v);
                        for i in 0..n {
                            let x = self.rb(pos + 1 + i);
                            self.wb(rec + i, x);
                        }
                        self.v[v].pos += 1 + n as u32;
                        self.c(LOAD, 1.0);
                    }
                    0xC0..=0xEF => {
                        self.v[v].wait = b - 0xC0;
                        self.v[v].def = b - 0xC0;
                        self.v[v].pos += 1;
                        self.c(DUR, 1.0);
                    }
                    0xF5 if l.gm => {
                        self.song = 0;
                        return false;
                    }
                    0xF6 if l.gm => {
                        let x = self.rb(pos + 1);
                        self.wb(l.transp, x);
                        self.v[v].pos += 2;
                        self.c(OTHER, 1.0);
                    }
                    _ => {
                        self.note(hw, v);
                        break;
                    }
                }
            }
        }
        true
    }

    /// A note byte at the stream position of voice `v`: the note-on(s), then the stream
    /// advances and the wait counter reloads (SPEC 3.5).
    fn note(&mut self, hw: &mut Hw, v: usize) {
        let pos = self.v[v].pos as usize;
        let rec = self.rec(v);
        let n = match self.l.fam {
            Fam::F3 => 1,
            Fam::F1 | Fam::F2 => {
                let fl = if self.l.fam == Fam::F1 { self.rw(rec + 0x0E) } else { self.rw(rec + 6) };
                if fl & 1 == 0 {
                    1
                } else if fl & 0x20 != 0 {
                    4
                } else {
                    2
                }
            }
        };
        for i in 0..n {
            if i > 0 {
                self.c(NEXT, 1.0);
            }
            let nb = self.rb(pos + i);
            self.note_on(hw, v, nb);
        }
        self.v[v].pos += n as u32;
        self.v[v].wait = self.v[v].def;
        self.c(if self.last_loop { AFTERL } else { AFTER } + v, 1.0);
    }

    // ---- the tick

    fn tick_inner(&mut self, hw: &mut Hw) {
        self.t = (hw.bus.clock as i64) * 2;
        self.c(LEAD, 1.0);
        match self.l.fam {
            Fam::F1 => {
                self.effects(hw, 4);
                if self.tempo == 0 {
                    self.tempo = 7;
                    self.parse(hw, 4);
                } else if self.tempo == 3 {
                    self.tempo = 2;
                    self.parse(hw, 4);
                } else {
                    self.tempo -= 1;
                }
            }
            Fam::F2 => {
                let n = if !self.l.gm || matches!(self.song, 1 | 7 | 8) { 4 } else { 2 };
                if !self.l.gm || self.song != 0 {
                    self.effects(hw, n);
                }
                self.tick_ctr = self.tick_ctr.wrapping_add(1);
                if TEMPO_MASK >> (self.tick_ctr & 31) & 1 != 0 {
                    self.parse(hw, n);
                }
            }
            Fam::F3 => {
                self.parse(hw, 4);
            }
        }
    }
}

impl Replayer for Spl {
    fn format(&self) -> &'static str {
        "Sound Programming Language"
    }
    fn title(&self) -> String {
        let o = (self.rl(0x18) as usize).min(self.img.len());
        let e = self.img[o..].iter().position(|&b| b == 0).unwrap_or(0);
        String::from_utf8_lossy(&self.img[o..o + e]).into_owned()
    }
    fn songend(&self) -> bool {
        false
    }
    fn tick_clocks(&self) -> u64 {
        (self.l.timer as u64 + 1) * 5
    }
    fn timer_delay(&self) -> u64 {
        // the engine adds one period to the clock of the last start write
        (self.l.tdelay + self.lag).saturating_sub(self.tick_clocks())
    }
    fn subsong_tick_at(&mut self) -> Option<u64> {
        Some(self.end_t + self.l.tdelay)
    }
    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.songs - 1, self.sub)
    }
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n.min(self.songs - 1);
        self.host_start(hw, false);
        self.start(self.sub + 1, hw);
    }
    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }
}

#[cfg(test)]
#[path = "soundprogramminglanguage_tests.rs"]
mod tests;

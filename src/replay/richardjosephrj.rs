//! Richard Joseph game-music container (Eagleplayer "Richard Joseph", module prefix `RJ.`).
//!
//! Not the Richard Joseph Player (`rjp.*` with `smp.*`), which has its own port in
//! `richardjoseph.rs`; this file is the other, DeliTracker-style container: an AmigaDOS load
//! file with one chip hunk holding a 68-byte header, the game's sound driver, its data and its
//! samples.
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! and execution traces of the 68k reference player; no original code. The driver's data tables
//! (patterns, instruments, modulation sets, envelopes, period tables) are located in the image
//! by the layout parameters of the specification and interpreted; the driver's own code is never
//! run. Every register write is placed at the clock of the original's code from the
//! specification's cycle ledger, including the raster-line waits of the adapter's emulated audio
//! interrupt, which depend on the absolute colour clock.
//!
//! What is supported: the two driver variants the specification could study (C, from the game
//! Cadaver, and S, from Speedball 2), recognised by the header's routine addresses, and the
//! three start-routine shapes of the specification (C title tune, C song table, S request),
//! recognised by the InitSong address. A file with another driver layout or another start
//! routine is detected (the header is the format) but not played: `new` returns `None`, so the
//! 68k player (if present) is used. No real `RJ.*` module is known; the spec's six synthetic
//! files are the only reference.
//!
//! Choices where the specification is silent (all safe, none can hang or panic):
//! * song end is host policy: the end is reported (`songend`) and the music keeps running;
//! * the Speedball 2 priority rejection cannot arise: the start routine writes priority 15 into
//!   the request and no voice holds more, so every request passes and no priority is tracked;
//! * a tempo command with operand 0 leaves the tempo as it was;
//! * a 0x98 duration command with count 0 sums no bytes (the specification's 65 536 would read
//!   far past the pattern) and costs as a count of 0;
//! * a command byte above 0x7F that is not a multiple of 4 up to 0xD4 stops the voice without
//!   a song-end test, as does a return with no call on the stack, a loop end that finds a call
//!   frame, a stack deeper than 32 frames, and an event fetch of more than 100 000 steps;
//! * a pattern byte read outside the image reads as a stop; other reads outside it read zero;
//! * a looped instrument with a non-zero loop offset behaves as the specification's table says
//!   for both variants (it was seen on C only);
//! * the start-up scan stops at the end of the image when no marker word is found.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Richard Joseph", detect, new };

/// Colour clocks between two ticks (the host's timer period).
const TICK: u64 = 70940;
/// The image (hunk data) starts at this file offset.
const HDR: usize = 32;
/// Event-fetch steps after which a voice is stopped (a pattern of commands that never ends).
const FETCH_LIMIT: u32 = 100_000;
/// Deepest call/loop stack.
const STACK_MAX: usize = 32;

/// Variant and table layout, in image offsets (spec 2).
struct Layout {
    s: bool,
    tempo: u32,
    pattab: u32,
    instab: u32,
    rbase: u32,
    instab0: u32,
    kill: u32,
    silent: u32,
    modtab: u32,
    envtab: u32,
    ptabs: [u32; 3],
    songtab: u32,
    clear: u64,
    /// Smallest image that holds the layout's tables.
    min_len: usize,
}

static LAYOUT_C: Layout = Layout {
    s: false,
    tempo: 0x48,
    pattab: 0xD48,
    instab: 0x40D62,
    rbase: 0,
    instab0: 0x40D62,
    kill: 0x40D62,
    silent: 0xD70,
    modtab: 0x938,
    envtab: 0x966,
    ptabs: [0; 3],
    songtab: 0x8F8,
    clear: 0x112,
    min_len: 0x41000,
};

static LAYOUT_S: Layout = Layout {
    s: true,
    tempo: 0x178C,
    pattab: 0,
    instab: 0,
    rbase: 0x17C6,
    instab0: 0x17A8,
    kill: 0x17A8,
    silent: 0x17B6,
    modtab: 0x3F2,
    envtab: 0x54C,
    ptabs: [0xF0E, 0xF6E, 0xFCE],
    songtab: 0x32A,
    clear: 0x114,
    min_len: 0x1800,
};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Start {
    /// C: the game's title-tune routine (one subsong): patterns 1, 2, 4, 7 on voices 0..3.
    Title,
    /// C: the driver's song-table routine.
    Table,
    /// S: the game's request routine.
    Request,
}

/// The layout and start routine of a file by the header's routine addresses (play, audio,
/// init-player, init-song, sample-info), or `None` for a driver this port does not know.
fn recognise(d: &[u8]) -> Option<(&'static Layout, Start)> {
    let h = |o: usize| be32(d, HDR + o);
    let key = (h(12), h(16), h(20), h(24), h(28));
    let (lay, start) = match key {
        (0x40F8C, 0x40F18, 0x410FC, 0x8B6, 0x40D62) => (&LAYOUT_C, Start::Title),
        (0x40F8C, 0x40F18, 0x410FC, 0x412B4, 0x40D62) => (&LAYOUT_C, Start::Table),
        (0x5BC, 0x55C, 0x732, 0x128, 0x2EF92) => (&LAYOUT_S, Start::Request),
        _ => return None,
    };
    (d.len() >= HDR + lay.min_len).then_some((lay, start))
}

/// Spec 1.2.
fn detect(d: &[u8]) -> bool {
    d.len() >= HDR + 68
        && be32(d, 0) == 0x0000_03F3
        && d[20] != 0
        && be32(d, 32) == 0x70FF_4E75
        && be32(d, 36) == 0x522E_4A4F
        && be32(d, 40) == 0x5345_5048
        && [44, 48, 52, 56].iter().all(|&o| be32(d, o) != 0)
}

#[derive(Clone, Copy, PartialEq)]
enum Frame {
    Ret(u32),
    /// Pointer after the loop start's operand, and the count left.
    Loop(u32, u16),
}

#[derive(Clone, Default)]
struct Voice {
    e: u16,
    ptab: u32,
    env: u32,
    stack: Vec<Frame>,
    smp: u32,
    loopflag: bool,
    loopoff: u16,
    len: u16,
    vol: u16,
    trig: bool,
    ecnt: u8,
    eact: bool,
    eph: u8,
    pstart: u32,
    ptr: u32,
    dur: u16,
    durset: u16,
    basevol: u16,
    baseper: u16,
    outper: u16,
    vcnt: [u16; 2],
    pcnt: [u16; 3],
    vstep: [u16; 2],
    pstep: [u16; 3],
    vdw: [u16; 2],
    pdw: [u16; 3],
    vdel: [u16; 2],
    vcur: [u16; 2],
    pdel: [u16; 3],
    pcur: [u16; 3],
    voff: u16,
    poff: u16,
    mflags: u16,
    transp: i16,
}

pub struct RichardJosephRj {
    img: Vec<u8>,
    /// Guest address of the image.
    ib: u32,
    lay: &'static Layout,
    start: Start,
    nsub: u16,
    sub: u16,
    /// The colour clock of the reference's run origin.
    origin: u64,
    v: [Voice; 4],
    ie: u16,
    ir: u16,
    f: [bool; 4],
    rf: [bool; 4],
    /// The running routine's start (reference time) and its cycle count.
    t0: u64,
    t: u64,
    /// Reference time at which the last host call ended.
    end: u64,
    first_tick: Option<u64>,
    songend: bool,
}

fn sx(b: u8) -> u16 {
    b as i8 as i16 as u16
}

impl RichardJosephRj {
    fn r8(&self, o: u32) -> u8 {
        self.img.get(o as usize).copied().unwrap_or(0)
    }

    fn r16(&self, o: u32) -> u16 {
        be16(&self.img, o as usize)
    }

    fn r32(&self, o: u32) -> u32 {
        be32(&self.img, o as usize)
    }

    fn abs(&self, o: u32) -> u32 {
        self.ib.wrapping_add(o)
    }

    // ---- the cycle ledger and the writes

    fn stamp(&self, hw: &mut Hw) {
        hw.bus.clock = self.origin + self.t0 + self.t / 2;
    }

    fn dmacon(&mut self, hw: &mut Hw, cy: u64, v: u16) {
        self.t += cy;
        self.stamp(hw);
        hw.dmacon(v);
    }

    fn wvol(&mut self, hw: &mut Hw, cy: u64, c: usize, v: u16) {
        self.t += cy;
        self.stamp(hw);
        hw.vol(c, v & 0x7F);
    }

    fn wloc(&mut self, hw: &mut Hw, cy: u64, c: usize, a: u32) {
        self.t += cy;
        self.stamp(hw);
        hw.loc(c, a);
    }

    fn wlen(&mut self, hw: &mut Hw, cy: u64, c: usize, n: u16) {
        self.t += cy;
        self.stamp(hw);
        hw.len(c, n);
    }

    fn wper(&mut self, hw: &mut Hw, cy: u64, c: usize, p: u16) {
        self.t += cy;
        self.stamp(hw);
        hw.per(c, p);
    }

    /// The colour clock of the running routine's cursor, for raster-line waits.
    fn line(&self) -> u64 {
        (self.t0 + self.t / 2) / 227
    }

    /// Spec 8.6.
    fn dma_wait(&mut self) {
        self.t += 46;
        for _ in 0..9 {
            self.t += 16;
            let r = self.line();
            loop {
                self.t += 16;
                let l = self.line();
                self.t += 9;
                if l != r {
                    break;
                }
            }
            self.t += 10;
        }
        self.t += 44;
    }

    /// Spec 5.4 and 8.5: the adapter's audio interrupt emulation.
    fn audio(&mut self, hw: &mut Hw, entry: u64) {
        self.t += entry;
        self.dma_wait();
        self.t += 84;
        for k in 0..4 {
            self.t += 43;
            if self.ir & self.ie & (0x80 << k) != 0 {
                self.t += if k == 0 { 30 } else { 34 };
                self.main(hw, k);
            }
        }
        self.t += 60;
        self.ir = 0;
        self.t += 20 + 16;
    }

    fn main(&mut self, hw: &mut Hw, k: usize) {
        let ibit = 0x80u16 << k;
        self.t += 53;
        let e = self.v[k].e.wrapping_sub(1);
        self.v[k].e = e;
        if self.v[k].loopflag {
            let lo = self.v[k].loopoff;
            let a = self.v[k].smp.wrapping_add(lo as u32);
            let n = self.v[k].len.wrapping_sub(lo >> 1);
            self.wloc(hw, 50, k, a);
            self.wlen(hw, 36, k, n);
            self.ie = ibit;
            self.t += 24 + 10;
        } else if e == 0 {
            self.ie = ibit;
            self.t += 21 + 24;
            self.dmacon(hw, 20, 1 << k);
            self.t += 10;
        } else {
            self.t += 21;
            let a = self.ib;
            self.wloc(hw, 24, k, a);
            self.wlen(hw, 16, k, 1);
        }
        self.t += 24 + 16;
        self.ir = ibit;
    }

    // ---- the song-end test (5.5, 8.11)

    fn song_end_test(&mut self, v: usize) {
        self.f[v] = false;
        self.t += 32 + 33 * (v as u64 + 1) + [22, 26, 26, 16][v] + 21;
        if self.f.iter().all(|&x| !x) {
            self.t += 68;
            // the notification re-bases the clock (8.1)
            self.t0 += self.t / 2;
            self.t = 0;
            self.songend = true;
            self.t += 16;
            self.f = self.rf;
        }
        self.t += 44;
    }

    // ---- patterns, instruments

    fn pattern(&self, n: u8) -> u32 {
        let l = self.lay;
        if l.s {
            let t = l.rbase.wrapping_add(self.r32(l.rbase));
            l.rbase.wrapping_add(self.r32(t.wrapping_add(4 * n as u32)))
        } else {
            self.r32(l.pattab + 4 * n as u32)
        }
    }

    /// The S period-table selector's cost (8.8).
    fn sel_cost(sel: u8) -> u64 {
        37 + if sel != 0 { 41 } else { 0 } + if sel > 1 { 24 } else { 0 } + 16
    }

    /// Loads instrument record at image offset `rec` into voice `v`; returns the selector cost.
    fn load_instrument(&mut self, v: usize, rec: u32) -> u64 {
        let l = self.lay;
        let (loopflag, loopoff, len) = (self.r16(rec) & 0x8000 != 0, self.r16(rec + 2), self.r16(rec + 4));
        let (smp, ptab, cost) = if l.s {
            let sel = self.r8(rec + 13);
            let t = match sel {
                0 => l.ptabs[0],
                1 => l.ptabs[1],
                _ => l.ptabs[2],
            };
            (self.abs(l.rbase.wrapping_add(self.r32(rec + 6))), t, Self::sel_cost(sel))
        } else {
            (self.abs(self.r32(rec + 6)), self.r32(rec + 10), 0)
        };
        let vs = &mut self.v[v];
        vs.loopflag = loopflag;
        vs.loopoff = loopoff;
        vs.len = len;
        vs.smp = smp;
        vs.ptab = ptab;
        cost
    }

    // ---- voice start (6.2, 8.8)

    fn voice_start(&mut self, hw: &mut Hw, v: usize, pat: u8) {
        let l = self.lay;
        let s = l.s;
        let ibit = 0x80u16 << v;
        let silent = self.abs(l.silent);
        {
            let keep = (self.v[v].e, self.v[v].transp);
            self.v[v] = Voice { e: keep.0, transp: keep.1, ..Voice::default() };
        }
        self.t += 892;
        self.ie = ibit;
        self.ir = ibit;
        self.t += 60 + 60;
        self.dmacon(hw, 20, 1 << v);
        self.t += 16;
        self.wvol(hw, 12, v, 0);
        self.wloc(hw, if s { 20 } else { 24 }, v, silent);
        self.wlen(hw, 16, v, 1);
        self.wper(hw, 36, v, 1);
        self.dmacon(hw, 16, 0x8000 | 1 << v);
        self.audio(hw, 10);
        self.t += if s { 16 } else { 20 };
        self.v[v].pstart = self.pattern(pat);
        self.v[v].ptr = self.v[v].pstart;
        self.t += if s { 54 } else { 30 };
        self.t += 62;
        let sc = self.load_instrument(v, l.instab0);
        if s {
            self.v[v].smp = silent;
            self.t += 100 + 18 + sc;
        } else {
            self.t += 108;
        }
        self.v[v].baseper = 0x100;
        self.v[v].outper = 0x100;
        self.t += 32;
        let (smp, len) = (self.v[v].smp, self.v[v].len);
        self.wloc(hw, if s { 24 } else { 28 }, v, smp);
        self.wlen(hw, 20, v, len);
        self.wper(hw, 16, v, 0x100);
        if !s {
            self.dmacon(hw, 40, 1 << v);
            self.t += 16;
        }
        self.t += 168;
    }

    // ---- driver init (6.1, 8.4)

    fn driver_init(&mut self, hw: &mut Hw) {
        let l = self.lay;
        let s = l.s;
        let silent = self.abs(l.silent);
        self.t += 148;
        if s {
            self.t += 24;
            self.stamp(hw);
            hw.bus.write_byte(0xBFE001, 0x02);
            self.t += 56;
        } else {
            self.t += 12;
        }
        self.dmacon(hw, 36, 0x000F);
        self.t += 16;
        self.ie = 0x0780;
        self.ir = 0x0780;
        self.t += 40 + 20 + 16 + 12;
        self.t += 16;
        self.stamp(hw);
        hw.bus.write_word(0xDFF09E, 0x00FF);
        self.t += 44 + 30 * l.clear;
        self.t += if s { 112 } else { 48 };
        self.t += 16;
        for (i, vs) in self.v.iter_mut().enumerate() {
            *vs = Voice { e: 2, smp: silent, len: 8, ..Voice::default() };
            let _ = i;
        }
        for c in 0..4 {
            self.wvol(hw, 12, c, 0);
        }
        self.t += 4;
        for c in 0..4 {
            self.wlen(hw, 12, c, 1);
        }
        for c in 0..4 {
            self.wper(hw, 12, c, 1);
        }
        self.t += 128;
        for c in 0..4 {
            self.wloc(hw, 16, c, silent);
        }
        self.t += 52;
        for c in 0..4 {
            self.wlen(hw, 12, c, 8);
        }
        self.t += 60;
        for c in 0..4 {
            self.wper(hw, 12, c, 0x100);
        }
        self.dmacon(hw, 16, 0x000F);
        self.t += if s { 600 } else { 616 };
    }

    // ---- InitSound and EndSound (5.3, 5.6, 8.3)

    fn init_sound(&mut self, hw: &mut Hw, t0: u64, n: u16) {
        self.t0 = t0;
        self.t = 0;
        self.f = [true; 4];
        self.songend = false;
        self.t += 1124 + 32;
        self.driver_init(hw);
        self.t += 64;
        self.start_routine(hw, n);
        self.t += 12;
        self.scan(n);
        self.t += 52;
        self.rf = self.f;
        self.end = self.t0 + self.t / 2;
    }

    fn init_song_addr(&self) -> u32 {
        // the header's InitSong (image offset of the long at image offset 24)
        self.r32(24)
    }

    /// Spec 5.3 step 4 and 8.3: the scan of the start routine's code for the table of voices
    /// that take part.
    fn scan(&mut self, n: u16) {
        let a = self.init_song_addr() as usize;
        let mut i = 0usize;
        while a + 2 * i + 2 <= self.img.len() {
            let w = be16(&self.img, a + 2 * i);
            if w == 0x4E75 {
                self.t += 21;
                return;
            }
            if w == 0x41F9 {
                self.t += 21 + 21 + 20;
                let table = be32(&self.img, a + 2 * i + 2);
                for j in 0..4u32 {
                    self.t += 17;
                    if self.r8(table.wrapping_add(4 * n as u32 + j)) == 0 {
                        self.f[j as usize] = false;
                        self.t += if j == 0 { 20 } else { 24 };
                    }
                }
                return;
            }
            self.t += 21 + 21 + 9;
            i += 1;
        }
    }

    /// Spec 4 and 8.8: the start routines of the three known shapes.
    fn start_routine(&mut self, hw: &mut Hw, n: u16) {
        let l = self.lay;
        match self.start {
            Start::Title => {
                self.t += 128;
                for (v, p) in [1u8, 2, 4, 7].into_iter().enumerate() {
                    self.t += 36;
                    self.voice_start(hw, v, p);
                }
                self.t += 148;
            }
            Start::Table => {
                self.t += 168;
                for v in 0..4 {
                    let p = self.r8(l.songtab + 4 * n as u32 + v as u32);
                    self.t += 12 + 18;
                    self.voice_start(hw, v, p);
                    self.t += 14;
                    if p == 0 {
                        break;
                    }
                }
                self.t += 148;
            }
            Start::Request => {
                let e = l.songtab + 6 * n as u32;
                let flag = self.r8(e + 1);
                let pats = [self.r8(e + 2), self.r8(e + 3), self.r8(e + 4), self.r8(e + 5)];
                self.t += 166 + 180 + 8 + 23;
                if flag & 0x80 != 0 {
                    self.t += 148 + 148;
                    return;
                }
                self.t += 17;
                // the written priority 15 is non-zero and no voice holds more, so the request is
                // accepted
                self.t += 18;
                for (i, &p) in pats.iter().enumerate().take(3) {
                    self.t += 21;
                    if p != 0 {
                        self.t += if i < 2 { 23 } else { 21 };
                    }
                }
                self.t += 24;
                for (v, &p) in pats.iter().enumerate() {
                    self.t += 21;
                    if p != 0 {
                        self.t += 60;
                        self.voice_start(hw, v, p);
                        self.t += 36;
                    }
                    self.t += 22;
                }
                self.t += 148 + 148;
            }
        }
    }

    fn end_sound(&mut self, hw: &mut Hw) {
        self.dmacon(hw, 28, 0x000F);
        self.t += 4;
        for c in 0..4 {
            self.wvol(hw, 12, c, 0);
        }
        self.t += 16;
        self.end = self.t0 + self.t / 2;
    }

    // ---- the tick (6.3, 8.9)

    fn run_tick(&mut self, hw: &mut Hw) {
        let s = self.lay.s;
        self.t += if s { 463 } else { 439 };
        self.t += 60;
        for v in 0..4 {
            self.t += if s { 55 } else { 51 };
            if self.v[v].ptr != 0 {
                self.t += 23;
                if self.v[v].dur == 0 {
                    self.fetch(hw, v);
                }
                self.mod_step(v);
            }
            self.t += if s { 30 } else { 26 };
        }
        self.t += 18;
        self.env_pass();
        self.t += 18;
        self.writer_pass(hw);
        self.t += 36 + 320;
        self.end = self.t0 + self.t / 2;
    }

    /// Stops a voice in a case the specification leaves open (see the module comment).
    fn halt(&mut self, v: usize) {
        let vs = &mut self.v[v];
        vs.ptr = 0;
        vs.pstart = 0;
        vs.trig = false;
    }

    fn restart_vol(&mut self, v: usize) {
        let vs = &mut self.v[v];
        vs.vdel = vs.vdw;
        vs.vcur = vs.vcnt;
        self.t += 140;
    }

    fn restart_pitch(&mut self, v: usize) {
        let vs = &mut self.v[v];
        vs.pdel = vs.pdw;
        vs.pcur = vs.pcnt;
        self.t += 198;
    }

    /// The silence sequence and the note cut share their shape: DMA off, volume 0, the kill
    /// sample, length 1, period 1.
    fn fetch(&mut self, hw: &mut Hw, v: usize) {
        let l = self.lay;
        let s = l.s;
        let ibit = 0x80u16 << v;
        let kill = self.abs(l.kill);
        self.t += 21;
        if self.v[v].loopoff == 0 {
            self.dmacon(hw, 20, 1 << v);
            self.ie = ibit;
            self.ir = ibit;
            self.t += 60 + 60 + 16;
            self.wvol(hw, 16, v, 0);
            self.wloc(hw, if s { 20 } else { 24 }, v, kill);
            self.wlen(hw, 16, v, 1);
            self.wper(hw, 36, v, 1);
            self.dmacon(hw, 16, 0x8000 | 1 << v);
            self.audio(hw, 10);
            self.t += if s { 8 } else { 12 };
        }
        self.t += 27 + 4;
        let mut steps = 0u32;
        loop {
            steps += 1;
            if steps > FETCH_LIMIT {
                self.halt(v);
                break;
            }
            let p = self.v[v].ptr;
            let b = if (p as usize) < self.img.len() { self.r8(p) } else { 0xAC };
            self.v[v].ptr = p.wrapping_add(1);
            self.t += 21;
            if b < 0x80 {
                self.note(hw, v, b);
                break;
            }
            self.t += 38;
            let op = |me: &Self| me.r8(me.v[v].ptr);
            match b {
                0x80 => {
                    let n = op(self);
                    self.v[v].ptr += 1;
                    self.v[v].basevol = n as u16;
                    self.t += 30;
                }
                0x84 | 0xA0 | 0xA4 => {
                    self.v[v].ptr += 1;
                    self.t += 18;
                }
                0x88 => {
                    self.t += 62;
                    self.v[v].ptr = self.v[v].pstart;
                    self.song_end_test(v);
                    self.t += 14;
                }
                0x8C => {
                    let n = op(self) as u16;
                    self.v[v].ptr += 1;
                    let tempo = self.r16(l.tempo);
                    self.v[v].durset = n.wrapping_mul(tempo);
                    self.t += if s { 92 } else { 96 };
                }
                0x90 => break,
                0x94 => {
                    let n = op(self) as u16;
                    self.v[v].ptr += 1;
                    if n != 0 {
                        let t = 750 / n;
                        self.img[l.tempo as usize..l.tempo as usize + 2].copy_from_slice(&t.to_be_bytes());
                    }
                    self.t += if s { 162 } else { 166 };
                }
                0x98 => {
                    let k = op(self) as u32;
                    self.v[v].ptr += 1;
                    let mut sum = 0u16;
                    for i in 0..k {
                        sum = sum.wrapping_add(self.r8(self.v[v].ptr + i) as u16);
                    }
                    self.v[v].ptr += k;
                    let tempo = self.r16(l.tempo);
                    self.v[v].durset = sum.wrapping_mul(tempo);
                    self.t += 24 + 22 * k as u64 + if s { 84 } else { 88 };
                }
                0x9C => {
                    let n = op(self) as u32;
                    self.v[v].ptr += 1;
                    let a = l.modtab.wrapping_add(15 * n);
                    let g: Vec<u16> = (0..15).map(|i| sx(self.r8(a.wrapping_add(i)))).collect();
                    let vs = &mut self.v[v];
                    vs.vcnt = [g[0], g[1]];
                    vs.pcnt = [g[2], g[3], g[4]];
                    vs.vstep = [g[5], g[6]];
                    vs.pstep = [g[7], g[8], g[9]];
                    vs.vdw = [g[10], g[11]];
                    vs.pdw = [g[12], g[13], g[14]];
                    vs.mflags &= !3;
                    vs.vdel = [0; 2];
                    vs.pdel = [0; 3];
                    vs.voff = 0;
                    vs.poff = 0;
                    self.t += 850;
                }
                0xA8 => {
                    let n = op(self) as u16;
                    self.v[v].ptr += 1;
                    self.v[v].mflags |= n;
                    self.t += 34;
                }
                0xAC => {
                    self.t += 12 + if s { 94 + 8 } else { 8 };
                    self.t += 36 + 24;
                    self.v[v].pstart = 0;
                    self.ie = ibit;
                    self.song_end_test(v);
                    self.t += 8;
                    self.ir = ibit;
                    self.t += 60;
                    self.dmacon(hw, 20, 1 << v);
                    self.v[v].trig = false;
                    self.t += 16 + 10;
                    self.v[v].ptr = 0;
                    self.t += 52;
                    self.v[v].dur = self.v[v].durset;
                    return;
                }
                0xB0 => {
                    let n = op(self);
                    let after = self.v[v].ptr + 1;
                    if self.v[v].stack.len() >= STACK_MAX {
                        self.halt(v);
                        break;
                    }
                    self.v[v].stack.push(Frame::Ret(after));
                    self.v[v].ptr = self.pattern(n);
                    self.t += if s { 92 } else { 68 };
                }
                0xB4 => {
                    self.t += 22;
                    match self.v[v].stack.pop() {
                        Some(Frame::Ret(p)) => self.v[v].ptr = p,
                        _ => {
                            self.halt(v);
                            break;
                        }
                    }
                }
                0xB8 => {
                    let n = op(self);
                    self.v[v].ptr += 1;
                    if n == 0 {
                        self.v[v].transp = 0;
                        self.t += 39;
                    } else {
                        self.v[v].transp = self.v[v].transp.wrapping_add(n as i8 as i16);
                        self.t += 47;
                    }
                }
                0xBC => {
                    let n = op(self);
                    self.v[v].ptr += 1;
                    self.v[v].transp = n as i8 as i16;
                    self.t += 34;
                }
                0xC0 => {
                    let n = op(self) as u16;
                    self.v[v].ptr += 1;
                    if self.v[v].stack.len() >= STACK_MAX {
                        self.halt(v);
                        break;
                    }
                    let after = self.v[v].ptr;
                    self.v[v].stack.push(if n == 0 { Frame::Loop(0, 0) } else { Frame::Loop(after, n) });
                    self.t += 47;
                }
                0xC4 => {
                    self.t += 27;
                    match self.v[v].stack.last().copied() {
                        Some(Frame::Loop(p, c)) if p != 0 => {
                            self.t += 39;
                            self.v[v].ptr = p;
                            let c = c.wrapping_sub(1);
                            let last = self.v[v].stack.len() - 1;
                            if c == 0 {
                                self.t += 26;
                                self.v[v].stack[last] = Frame::Loop(0, 0);
                            } else {
                                self.v[v].stack[last] = Frame::Loop(p, c);
                            }
                        }
                        Some(Frame::Loop(_, _)) => {
                            self.t += 18;
                            self.v[v].stack.pop();
                        }
                        _ => {
                            self.halt(v);
                            break;
                        }
                    }
                }
                0xC8 => {
                    let n = op(self) as u32;
                    self.v[v].ptr += 1;
                    self.v[v].env = l.envtab + 8 * n;
                    self.t += 62;
                }
                0xCC => {
                    self.v[v].env = 0;
                    self.t += 26;
                }
                0xD0 => {
                    let n = op(self) as u32;
                    self.v[v].ptr += 1;
                    let rec = if s {
                        l.rbase.wrapping_add(self.r32(l.rbase + 4)).wrapping_add(14 * n)
                    } else {
                        l.instab + 14 * n
                    };
                    let sc = self.load_instrument(v, rec);
                    self.t += if s { 252 + sc } else { 192 };
                }
                0xD4 => {
                    let n = op(self);
                    self.v[v].ptr = self.pattern(n);
                    self.t += if s { 80 } else { 56 };
                }
                _ => {
                    // not a command of the driver: the original would jump through garbage
                    self.halt(v);
                    break;
                }
            }
        }
        self.t += 52;
        self.v[v].dur = self.v[v].durset;
    }

    fn note(&mut self, hw: &mut Hw, v: usize, b: u8) {
        let l = self.lay;
        let s = l.s;
        let ibit = 0x80u16 << v;
        let kill = self.abs(l.kill);
        self.t += 12;
        self.dmacon(hw, 20, 1 << v);
        self.ie = ibit;
        self.ir = ibit;
        self.t += 60 + 60 + 16;
        self.wvol(hw, 16, v, 0);
        self.wloc(hw, if s { 20 } else { 24 }, v, kill);
        self.wlen(hw, 16, v, 1);
        self.wper(hw, 16, v, 1);
        self.t += 25;
        let vs = &mut self.v[v];
        if vs.env != 0 {
            vs.eact = true;
            vs.eph = 0;
            vs.vol = 0;
            vs.ecnt = 0;
            self.t += 64;
        }
        self.t += 62;
        let vs = &mut self.v[v];
        let i = (b as i16).wrapping_mul(4).wrapping_add(vs.transp);
        let a = vs.ptab.wrapping_add((2 * i as i32) as u32);
        let per = self.r16(a);
        let vs = &mut self.v[v];
        vs.baseper = per;
        vs.trig = true;
        self.t += 16 + 18;
        self.restart_vol(v);
        self.t += 16 + 18;
        self.restart_pitch(v);
        self.t += 16;
        let vs = &mut self.v[v];
        vs.voff = 0;
        vs.poff = 0;
    }

    // ---- modulation (6.5, 8.12)

    fn mod_step(&mut self, v: usize) {
        self.v[v].dur = self.v[v].dur.wrapping_sub(1);
        self.t += 41;
        if self.v[v].env == 0 {
            self.t += 12;
            let mut found = false;
            for st in 0..2 {
                self.t += 21;
                if self.v[v].vdel[st] != 0 {
                    self.t += 30;
                    self.v[v].vdel[st] -= 1;
                    found = true;
                    break;
                }
                self.t += 21;
                if self.v[v].vcur[st] != 0 {
                    self.t += 78;
                    let vs = &mut self.v[v];
                    vs.vcur[st] -= 1;
                    vs.voff = vs.voff.wrapping_add(vs.vstep[st]);
                    vs.vdel[st] = vs.vdw[st];
                    found = true;
                    break;
                }
                self.t += 18;
            }
            self.t += 13;
            if !found {
                self.t += 16 + 25;
                self.v[v].voff = 0;
                if self.v[v].mflags & 1 != 0 {
                    self.t += 18;
                    self.restart_vol(v);
                }
            }
        }
        self.t += 12;
        let mut found = false;
        for st in 0..3 {
            self.t += 21;
            if self.v[v].pdel[st] != 0 {
                self.t += 30;
                self.v[v].pdel[st] -= 1;
                found = true;
                break;
            }
            self.t += 21;
            if self.v[v].pcur[st] != 0 {
                self.t += 82;
                let vs = &mut self.v[v];
                vs.pcur[st] -= 1;
                vs.poff = vs.poff.wrapping_add(sx(vs.pstep[st] as u8));
                vs.pdel[st] = vs.pdw[st];
                found = true;
                break;
            }
            self.t += 18;
        }
        self.t += 13;
        if !found {
            self.t += 25;
            if self.v[v].mflags & 2 != 0 {
                self.t += 18;
                self.restart_pitch(v);
            }
        }
        self.t += 25;
        if self.v[v].env == 0 {
            self.t += 44;
            let vs = &mut self.v[v];
            vs.vol = vs.basevol.wrapping_add(vs.voff) & 0x3F;
        }
        let vs = &mut self.v[v];
        vs.outper = vs.baseper.wrapping_add(vs.poff);
        self.t += 36;
    }

    // ---- envelopes (6.6, 8.13)

    fn env_pass(&mut self) {
        for v in 0..4 {
            self.t += 12 + if v < 3 { 18 } else { 0 };
            self.t += 27;
            if self.v[v].ptr == 0 {
                self.t += 16;
                continue;
            }
            self.t += 25;
            if self.v[v].env == 0 {
                self.t += 16;
                continue;
            }
            self.t += 25;
            if self.v[v].ecnt != 0 {
                self.t += 25;
                self.v[v].ecnt -= 1;
                if self.v[v].ecnt != 0 {
                    self.t += 16;
                    continue;
                }
            }
            self.t += 18;
            self.env_step(v);
            self.t += 16;
        }
    }

    fn env_step(&mut self, v: usize) {
        let s = self.lay.s;
        let e: Vec<u8> = (0..8).map(|i| self.r8(self.v[v].env + i)).collect();
        let vs = &mut self.v[v];
        let lo = (vs.vol & 0xFF) as u8;
        let hi = vs.vol & 0xFF00;
        let mut cost = 21u64;
        if !vs.eact {
            vs.eph = 0;
            cost += 16 + 21;
            if vs.vol != 0 {
                vs.ecnt = e[7];
                cost += 20 + 24 + 9;
                if lo < e[6] {
                    vs.vol = 0;
                    cost += 4;
                } else {
                    vs.vol = hi | (lo - e[6]) as u16;
                }
                cost += 12;
            }
            cost += 16;
            self.t += cost;
            return;
        }
        cost += 21;
        let mut sustain = false;
        if vs.eph == 0 {
            vs.ecnt = e[1];
            let x = hi | (lo.wrapping_add(e[0])) as u16;
            cost += 20 + if s { 20 } else { 24 } + 21;
            if x >= vs.basevol {
                vs.eph = 1;
                vs.vol = vs.basevol;
                cost += 28;
            } else {
                vs.vol = x;
            }
            cost += 12 + 16;
        } else {
            cost += 17;
            if vs.eph == 1 {
                vs.ecnt = e[3];
                let mut l2 = lo.wrapping_sub(e[2]);
                cost += 20 + 24 + 17;
                if l2 >= 0x40 {
                    l2 = 0;
                    cost += 4;
                }
                cost += 21;
                if l2 >= e[4] {
                    vs.vol = hi | l2 as u16;
                    cost += 12 + 16;
                } else {
                    vs.eph = 2;
                    cost += 26;
                    sustain = true;
                }
            } else {
                sustain = true;
            }
        }
        if sustain {
            vs.vol = e[4] as u16;
            cost += 28 + 35;
            if (vs.dur as u8 as i8) <= e[5] as i8 {
                vs.eact = false;
                cost += 16 + 16;
            } else {
                cost += 16;
            }
        }
        self.t += cost;
    }

    // ---- writer pass (6.7, 8.14)

    fn writer_pass(&mut self, hw: &mut Hw) {
        let s = self.lay.s;
        const TAB: [u64; 4] = [23, 54, 85, 93];
        for c in 0..4usize {
            let ibit = 0x80u16 << c;
            self.t += 12 + if c < 3 { 18 } else { 0 };
            self.t += 16;
            let per = self.v[c].outper;
            self.wper(hw, 40, c, per);
            self.t += 42;
            // CV(c): the volume stage with its write
            let vol = self.v[c].vol;
            self.wvol(hw, 24 + 17 * (c as u64 + 1) + 74 + if c == 2 { 0 } else { 10 } + 18 + 12, c, vol);
            self.t += 28;
            self.t += 18 + 20 + TAB[c] + 36;
            self.t += 28 + 12 + 21;
            if self.v[c].trig {
                self.t += 16;
                self.v[c].trig = false;
                let (smp, len) = (self.v[c].smp, self.v[c].len);
                self.wloc(hw, if s { 24 } else { 28 }, c, smp);
                self.wlen(hw, 40, c, len);
                self.v[c].e = 2;
                self.t += 10 + 20 + TAB[c] + 92;
                self.t += 28;
                self.dmacon(hw, if s { 16 } else { 20 }, 0x8000 | 1 << c);
                self.t += 60;
                self.ir = ibit;
                self.t += 21;
                if !self.v[c].loopflag {
                    self.t += 44;
                    self.ie = 0x8000 | ibit;
                    self.audio(hw, 10);
                    self.t += 16;
                } else {
                    self.t += 21;
                    if self.v[c].loopoff == 0 {
                        self.t += 44;
                        self.ie = ibit;
                        self.audio(hw, 18);
                        self.t += 24;
                    } else {
                        self.t += 16 + 44;
                        self.v[c].e = 1;
                        self.ie = 0x8000 | ibit;
                        self.audio(hw, 10);
                        self.t += 16;
                    }
                }
            } else {
                self.t += 16;
            }
        }
    }
}

impl Replayer for RichardJosephRj {
    fn format(&self) -> &'static str {
        "Richard Joseph"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsub - 1, self.sub)
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.first_tick.take()
    }

    /// The host's change of subsong (spec 8.2): EndSound, the two volume settings, InitSound.
    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = if n < self.nsub { n } else { 0 };
        let now = hw.bus.clock.saturating_sub(self.origin);
        let t0 = self.end.max(now);
        self.t0 = t0;
        self.t = 0;
        self.end_sound(hw);
        let t1 = self.end + 236;
        self.init_sound(hw, t1, n + 1);
        self.sub = n;
        self.first_tick = Some(self.origin + self.end + TICK);
        hw.bus.clock = hw.bus.clock.max(self.origin + self.end);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock.saturating_sub(self.origin);
        self.t = 0;
        self.run_tick(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let (lay, start) = recognise(d)?;
    let img = d[HDR..].to_vec();
    let ib = base.wrapping_add(HDR as u32);
    let nsub = be32(d, HDR + 64).clamp(1, 255) as u16;
    // the adapter clears the first long of the image, which the emulated audio interrupt's empty
    // loop plays
    m.bus.write_bytes(ib, &[0, 0, 0, 0]);
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let mut r = RichardJosephRj {
        img,
        ib,
        lay,
        start,
        nsub,
        sub: 0,
        origin,
        v: Default::default(),
        ie: 0,
        ir: 0,
        f: [true; 4],
        rf: [true; 4],
        t0: 0,
        t: 0,
        end: 0,
        first_tick: None,
        songend: false,
    };
    // the host timeline (8.2): the module-change notification comes after the adapter's scan of
    // the start routine for the marker word 0x287B
    let a = r.init_song_addr() as usize;
    let mut s = 0u64;
    while a + 2 * (s as usize) + 2 <= r.img.len() {
        s += 1;
        if be16(&r.img, a + 2 * (s as usize - 1)) == 0x287B {
            break;
        }
    }
    let t0 = 257 + (893 + 21 * s) / 2 + 48 + 12 + 18 + 118 + 118;
    let mut hw = Hw { bus: &mut m.bus };
    r.init_sound(&mut hw, t0, 1);
    r.first_tick = Some(origin + r.end + TICK);
    let end_clock = origin + r.end;
    m.bus.clock = m.bus.clock.max(end_clock);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "richardjosephrj_tests.rs"]
mod tests;

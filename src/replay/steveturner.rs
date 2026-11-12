//! Steve Turner (`jpo.*`), the music driver Steve Turner wrote for Jason Page's
//! conversions of Paradroid 90, Simulcra and Ivan "Ironman" Stewart's Super Off Road.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's player and the structure of the
//! corpus files), and this player was written from it alone and checked against
//! those writes.
//!
//! A `jpo.*` file is the module data behind a 46-byte install stub. The stub is
//! decoded (never run) to recognise the format; the three addresses it stores give
//! the sequence, jump and sample tables, and the instrument records start right
//! after the stub. The engine is written from the behaviour: four channels, a
//! tick of 35,520 colour clocks, order lists and pattern events with pauses of
//! steps, instrument records with a four-phase volume envelope and a segment
//! program for the pitch and vibrato, a note glide, and a master volume.
//!
//! The channel records keep the layout of the specification, as bytes: the segment
//! copies of an instrument with three or more segments overlap the loop count and
//! the pending and delay fields, and the port keeps that overlap.
//!
//! Decisions where the specification leaves the choice to the port:
//! * Song end is announced in the tick in which a channel reads order byte 255.
//! * The subsong count is the data rule's (so `jpo.Paradroid_90` has 6, not the
//!   glue's 5). Subsongs are numbered from 0.
//! * A pending or delayed step is cleared whether or not its priority check passes.
//! * Returning from a pattern to the order list reads the next order byte at once,
//!   in the same tick.
//! * Order lists that never end a tick's work (a pattern that returns without a
//!   pause, over and over) are cut after 1,000 order bytes in a tick.
//! * Reads outside the file give 0; writes outside a channel record are dropped.
//! * The silent buffer is a zero longword allocated in chip memory.
//! * The write times are not compared: a channel's work takes 450 colour clocks.

use super::m68kdec::{decode, Ea, K};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Steve Turner", detect, new };

/// Colour clocks between ticks.
const TICK: u64 = 35520;
/// The time a channel's handlers are taken to run (rough: times are not compared).
const T_CH: u64 = 450;
const STEP0: usize = 46;

const PERIOD: [u16; 84] = [
    61156, 57723, 54484, 51425, 48540, 45814, 43244, 40817, 38525, 36363, 34322, 32396, 30578, 28862, 27242, 25713, 24270,
    22907, 21622, 20409, 19263, 18182, 17161, 16198, 15289, 14431, 13621, 12857, 12135, 11454, 10811, 10205, 9632, 9091,
    8581, 8099, 7645, 7216, 6811, 6429, 6068, 5727, 5406, 5103, 4816, 4546, 4291, 4050, 3823, 3608, 3406, 3215, 3034, 2864,
    2703, 2552, 2408, 2273, 2146, 2025, 1912, 1804, 1703, 1608, 1517, 1432, 1352, 1276, 1204, 1137, 1073, 1013, 956, 902,
    852, 804, 759, 716, 676, 638, 602, 569, 537, 507,
];

// ------------------------------------------------------------------ recognition

#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    seq: usize,
    jmp: usize,
    smp: usize,
}

/// The install stub: four stores of a 32-bit immediate to the same base register,
/// a constant loaded, a call and the return, ending at byte 46. Returns the four
/// stored addresses.
fn stub(d: &[u8]) -> Option<[u32; 4]> {
    let mut at = 0;
    let mut vals = [0u32; 4];
    let mut base = None;
    for v in &mut vals {
        let i = decode(d, at)?;
        let (Ea::Imm { val, .. }, Ea::Disp(r, _)) = (i.src, i.dst) else { return None };
        if i.k != K::Move || i.size != 4 || *base.get_or_insert(r) != r {
            return None;
        }
        *v = val;
        at = i.next();
    }
    let mut const_loaded = false;
    loop {
        let i = decode(d, at)?;
        at = i.next();
        match i.k {
            K::Moveq => const_loaded = true,
            K::Move if matches!(i.src, Ea::Imm { .. }) => const_loaded = true,
            K::Move if matches!(i.src, Ea::D(_)) && matches!(i.dst, Ea::D(_)) => {}
            K::Jsr if matches!(i.src, Ea::AbsL { .. }) || matches!(i.dst, Ea::AbsL { .. }) => break,
            _ => return None,
        }
        if at > STEP0 {
            return None;
        }
    }
    let r = decode(d, at)?;
    (const_loaded && r.k == K::Rts && r.next() == STEP0).then_some(vals)
}

fn layout(d: &[u8]) -> Option<Layout> {
    let v = stub(d)?;
    let l = v[0] as i64;
    let off = |a: u32| -> Option<usize> {
        let o = a as i64 - l + STEP0 as i64;
        (o >= STEP0 as i64 && (o as usize) < d.len()).then_some(o as usize)
    };
    let lay = Layout { seq: off(v[1])?, jmp: off(v[2])?, smp: off(v[3])? };
    (d.len() >= STEP0 + 48 && lay.seq + 12 <= d.len() && count(d, lay.seq) >= 1).then_some(lay)
}

/// The data rule: records from SEQ while priority is 0 and the speed below 16.
fn count(d: &[u8], seq: usize) -> usize {
    let mut n = 0;
    while seq + 12 * (n + 1) <= d.len() && d[seq + 12 * n] == 0 && d[seq + 12 * n + 1] < 16 {
        n += 1;
    }
    n
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

// ------------------------------------------------------------------ the engine

#[derive(Clone, Copy, PartialEq, Debug)]
enum Env {
    None,
    SOff,
    SOn,
    Len1,
    Hold,
    P2,
    P3,
    P4,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Seg {
    None,
    Set,
    Dur,
    Upd,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stream {
    None,
    Order,
    Wait,
}

/// A channel record (108 bytes of the specification's layout) and its handlers.
#[derive(Clone)]
struct Ch {
    b: [u8; 108],
    env: Env,
    seg: Seg,
    glide: bool,
    stream: Stream,
    ord: usize,
    pat: usize,
}

impl Ch {
    fn new() -> Ch {
        Ch { b: [0; 108], env: Env::None, seg: Seg::None, glide: false, stream: Stream::None, ord: 0, pat: 0 }
    }

    /// A channel after a change of subsong: the stop before the new start leaves its
    /// pending, current and step words at the "none" value $FFFF, as `reset` does. A
    /// voice whose song word is 0 then stays silent; at the first start the records are
    /// still zero and such a voice triggers step 0. Both seen in both originals (the
    /// Eagleplayers "Jason Page Old" of EP 2.06 and "SteveTurner" of UADE): Paradroid 90's
    /// first song plays channel 3, Ivan Stewart's songs 9-18 (fourth voice word 0),
    /// reached by a change of subsong, leave it silent.
    fn stopped() -> Ch {
        let mut c = Ch::new();
        c.p32(0x1A, 0x0000_FFFF);
        c.p32(0x1E, 0x0000_FFFF);
        c.p32(0x22, 0x0000_FFFF);
        c
    }
    fn g8(&self, o: usize) -> u8 {
        self.b.get(o).copied().unwrap_or(0)
    }
    fn g16(&self, o: usize) -> u16 {
        (self.g8(o) as u16) << 8 | self.g8(o + 1) as u16
    }
    fn p8(&mut self, o: usize, v: u8) {
        if let Some(x) = self.b.get_mut(o) {
            *x = v;
        }
    }
    fn p16(&mut self, o: usize, v: u16) {
        self.p8(o, (v >> 8) as u8);
        self.p8(o + 1, v as u8);
    }
    fn p32(&mut self, o: usize, v: u32) {
        self.p16(o, (v >> 16) as u16);
        self.p16(o + 2, v as u16);
    }
}

pub struct SteveTurner {
    d: Vec<u8>,
    lay: Layout,
    base: u32,
    silence: u32,
    songs: usize,
    sub: u16,
    start: Option<u16>,
    /// A change of subsong has been made (see `Ch::stopped`).
    changed: bool,
    songend: bool,
    master: u16,
    target: u16,
    gspeed: i16,
    pending: u16,
    song: u8,
    gprio: u8,
    speed: u8,
    counter: u8,
    ch: [Ch; 4],
}

impl SteveTurner {
    fn b(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// InitSound: a fresh start of subsong `n` (1-based song number).
    fn init(&mut self, n: u16, hw: &mut Hw) {
        let c = if self.changed { Ch::stopped() } else { Ch::new() };
        self.ch = [c.clone(), c.clone(), c.clone(), c];
        self.master = 0;
        self.song = 0;
        self.gprio = 0;
        self.speed = 0;
        self.counter = 0;
        self.target = 255;
        self.gspeed = 255;
        self.pending = n;
        self.master_glide(hw);
        self.pending_step(hw);
    }

    fn master_glide(&mut self, hw: &mut Hw) {
        let d = self.target.wrapping_sub(self.master);
        if d == 0 {
            return;
        }
        let s = self.gspeed;
        if s >= 0 {
            let step = if (d as i16) > s { s } else { d as i16 };
            self.master = self.master.wrapping_add(step as u16);
        } else {
            let n = d.wrapping_neg() as i16;
            if n <= s {
                self.master = self.master.wrapping_sub(n as u16);
                if self.master == 0 {
                    self.silence(hw);
                }
            } else {
                self.master = self.master.wrapping_sub(s as u16);
            }
        }
    }

    fn silence(&mut self, hw: &mut Hw) {
        self.song = 0;
        self.gprio = 0;
        self.pending = 0;
        for c in 0..4 {
            self.reset(c, hw);
        }
        for c in 0..4 {
            hw.vol(c, 0);
        }
    }

    fn reset(&mut self, c: usize, hw: &mut Hw) {
        let ch = &mut self.ch[c];
        ch.p32(0x1A, 0x0000_FFFF);
        ch.p32(0x1E, 0x0000_FFFF);
        ch.p32(0x22, 0x0000_FFFF);
        ch.env = Env::None;
        ch.seg = Seg::None;
        ch.glide = false;
        ch.stream = Stream::None;
        ch.p8(0x5C, 0);
        hw.dmacon(1 << c);
    }

    /// Step 5: a pending subsong start, or the pending steps.
    fn pending_step(&mut self, hw: &mut Hw) {
        if self.pending != 0 {
            let d = self.pending;
            self.pending = 0;
            self.start_song(d, hw);
            return;
        }
        for c in 0..4 {
            let w = self.ch[c].g16(0x1C) as i16;
            // The spec's "second branch" for a non-zero byte at 0x1A: the segment copies
            // of a 4-segment instrument put the priority there. Its effect is nothing.
            if w < 0 || self.ch[c].g8(0x1A) != 0 {
                continue;
            }
            let p = self.b(STEP0 + w as usize + 30);
            if p >= self.ch[c].g8(0x23) {
                self.trigger(c, w as u16, p, hw);
            }
            self.ch[c].p32(0x1A, 0x0000_FFFF);
        }
    }

    fn start_song(&mut self, d: u16, hw: &mut Hw) {
        let n = (d & 127).wrapping_sub(1) as usize;
        let rec = self.lay.seq + 12 * n;
        if self.b(rec) < self.gprio {
            return;
        }
        self.song = d as u8;
        self.gprio = self.b(rec);
        self.speed = self.b(rec + 1);
        self.counter = self.speed;
        for c in 0..4 {
            let w = self.w(rec + 4 + 2 * c);
            if w != 0 {
                self.ch[c].ord = (self.lay.seq as i64 + w as i16 as i64).max(0) as usize;
                self.reset(c, hw);
                self.ch[c].stream = Stream::Order;
            }
        }
    }

    fn song_end(&mut self) {
        for ch in &mut self.ch {
            ch.stream = Stream::None;
        }
        self.gprio = 0;
        self.pending = self.song as u16;
        self.song = 0;
        self.songend = true;
    }

    /// Section 9: starts the instrument record at `wo` (step index times 48).
    fn trigger(&mut self, c: usize, wo: u16, prio: u8, hw: &mut Hw) {
        let rec = STEP0 + wo as usize;
        let ch = &mut self.ch[c];
        ch.p16(0x24, wo);
        ch.p8(0x23, prio);
        ch.p8(0x22, (wo / 48 + 1) as u8);
        ch.p32(0x30, rec as u32);
        ch.env = Env::SOff;
        ch.seg = Seg::None;
        ch.glide = false;
        let si = self.b(rec + 31) as i8;
        if si >= 0 {
            let e = self.lay.smp + 4 * si as usize;
            let hd = self.lay.smp + be32(&self.d, e) as usize;
            hw.len(c, self.w(hd) >> 1);
            hw.loc(c, self.base.wrapping_add(hd as u32 + 2));
        }
        // segment setup
        let n = self.b(rec + 43) as usize;
        let h = self.b(rec + 44);
        let ch = &mut self.ch[c];
        ch.p8(0x48, h);
        ch.p8(0x4A, 0);
        ch.p8(0x44, 1);
        ch.p16(0x40, 0);
        ch.p16(0x42, 0);
        for k in 0..=n {
            let (s, dst) = (rec + 10 * k, 8 * k);
            if dst >= 108 {
                break;
            }
            let (w0, w2, b5) = (be16(&self.d, s), be16(&self.d, s + 2), self.d.get(s + 5).copied().unwrap_or(0));
            let ch = &mut self.ch[c];
            ch.p16(dst, w0);
            ch.p16(dst + 2, w0);
            ch.p16(dst + 4, w2);
            ch.p8(dst + 6, b5);
        }
        self.ch[c].seg = Seg::Set;
    }

    // -------------------------------------------------------------- the tick

    fn run_tick(&mut self, hw: &mut Hw) {
        self.counter = self.counter.wrapping_sub(1);
        for c in 0..4 {
            hw.wait(T_CH);
            // the delayed step
            let dl = self.ch[c].g8(0x1E).wrapping_sub(1);
            self.ch[c].p8(0x1E, dl);
            if dl == 0 {
                let w = self.ch[c].g16(0x20) as i16;
                if w >= 0 {
                    let p = self.ch[c].g8(0x1F);
                    if p >= self.ch[c].g8(0x23) {
                        self.trigger(c, w as u16, p, hw);
                    }
                    self.ch[c].p32(0x1E, 0x0000_FFFF);
                }
            }
            self.stream(c, hw);
            self.envelope(c, hw);
            self.segment(c, hw);
            self.glide(c);
        }
        if self.counter == 0 {
            self.counter = self.speed;
        }
        self.master_glide(hw);
        self.pending_step(hw);
    }

    fn stream(&mut self, c: usize, hw: &mut Hw) {
        match self.ch[c].stream {
            Stream::None => {}
            Stream::Order => self.order(c, hw),
            Stream::Wait => {
                let counter = self.counter;
                let ch = &mut self.ch[c];
                let go = if counter != 0 {
                    ch.g8(0x59) == 0
                } else {
                    let v = ch.g8(0x59).wrapping_sub(1);
                    ch.p8(0x59, v);
                    v == 0
                };
                if go {
                    let p = ch.pat;
                    if !self.events(c, p, hw) {
                        self.order(c, hw);
                    }
                }
            }
        }
    }

    /// Reads order bytes until a pattern pauses (or the channel ends).
    fn order(&mut self, c: usize, hw: &mut Hw) {
        for _ in 0..1000 {
            let o = self.ch[c].ord;
            let b = self.b(o);
            self.ch[c].ord = o + 1;
            match b {
                254 => {
                    self.ch[c].stream = Stream::None;
                    return;
                }
                255 => {
                    self.song_end();
                    return;
                }
                _ => {
                    let j = self.lay.jmp;
                    let start = (j as i64 + self.w(j + 2 * b as usize) as i16 as i64).max(0) as usize;
                    if self.events(c, start, hw) {
                        return;
                    }
                }
            }
        }
    }

    fn pause(&mut self, c: usize, p: usize) {
        let ch = &mut self.ch[c];
        ch.pat = p;
        let l = ch.g8(0x58);
        ch.p8(0x59, l);
        ch.stream = Stream::Wait;
    }

    /// Pattern events from `p`. True if the pattern paused, false if it returned to the order list.
    fn events(&mut self, c: usize, mut p: usize, hw: &mut Hw) -> bool {
        for _ in 0..100_000 {
            let b = self.b(p);
            p += 1;
            match b {
                0..=127 => {
                    self.ch[c].pat = p;
                    let (sp, so) = (self.ch[c].g8(0x5D), self.ch[c].g16(0x5E));
                    if sp >= self.ch[c].g8(0x23) {
                        self.trigger(c, so, sp, hw);
                    }
                    if (b as usize) < PERIOD.len() {
                        let np = PERIOD[b as usize];
                        let ch = &mut self.ch[c];
                        if ch.g8(0x5C) != 0 {
                            let old = ch.g16(0x5A);
                            ch.p16(0, old);
                            ch.p16(0x5A, np);
                            ch.glide = true;
                            ch.p16(2, np);
                        } else {
                            ch.p16(0x5A, np);
                            ch.p16(0, np);
                            ch.p16(2, np);
                            ch.glide = false;
                        }
                    }
                    self.pause(c, p);
                    return true;
                }
                128..=175 => self.ch[c].p8(0x58, b - 127),
                176..=207 => {
                    let wo = 48 * (b - 176) as usize;
                    let pr = self.b(STEP0 + wo + 30);
                    self.ch[c].pat = p;
                    if pr >= self.ch[c].g8(0x23) {
                        self.trigger(c, wo as u16, pr, hw);
                    }
                    self.pause(c, p);
                    return true;
                }
                208..=239 => {
                    let wo = 48 * (b - 208) as usize;
                    let pr = self.b(STEP0 + wo + 30);
                    self.ch[c].p16(0x5E, wo as u16);
                    self.ch[c].p8(0x5D, pr);
                }
                240..=248 => self.ch[c].p8(0x5C, b - 240),
                254 => {
                    self.ch[c].pat = p;
                    self.pause(c, p);
                    return true;
                }
                _ => return false,
            }
        }
        false
    }

    // -------------------------------------------------------------- envelope

    fn vol_out(&mut self, c: usize, hw: &mut Hw) {
        let master = self.master as i32;
        let ch = &mut self.ch[c];
        let d = (ch.g8(0x2C) as i8 as i16).wrapping_add(ch.g16(0x2E) as i16);
        let d = if d < 0 {
            0
        } else if d as i32 > master {
            master as i16
        } else {
            d
        };
        ch.p16(0x2E, d as u16);
        hw.vol(c, (d as u16) >> 2);
    }

    fn rec(&self, c: usize) -> usize {
        ((self.ch[c].g16(0x30) as usize) << 16) | self.ch[c].g16(0x32) as usize
    }

    fn restart_env(&mut self, c: usize) {
        let rec = self.rec(c);
        let (a, b) = (self.b(rec + 33), self.b(rec + 34));
        let ch = &mut self.ch[c];
        ch.p16(0x2A, 0);
        ch.p8(0x2B, a);
        ch.p8(0x2C, b);
        ch.env = Env::Len1;
    }

    fn dec_counter(&mut self, c: usize) -> bool {
        let ch = &mut self.ch[c];
        let v = ch.g16(0x2A).wrapping_sub(1);
        ch.p16(0x2A, v);
        v == 0
    }

    fn envelope(&mut self, c: usize, hw: &mut Hw) {
        let rec = self.rec(c);
        match self.ch[c].env {
            Env::None => {}
            Env::SOff => {
                hw.dmacon(1 << c);
                self.ch[c].env = Env::SOn;
            }
            Env::SOn => {
                hw.vol(c, 0);
                hw.dmacon(0x8000 | 1 << c);
                let l = self.b(rec + 32);
                self.ch[c].p8(0x18, l);
                self.ch[c].p8(0x60, l);
                if l != 0 {
                    self.ch[c].p16(0x2E, 0);
                } else {
                    let m = self.master;
                    self.ch[c].p16(0x2E, m);
                    hw.vol(c, m >> 2);
                    self.ch[c].p8(0x18, 1);
                }
                self.restart_env(c);
            }
            Env::Len1 => {
                if self.ch[c].g8(0x60) == 0 {
                    hw.len(c, 1);
                    hw.loc(c, self.silence);
                }
                self.ch[c].env = Env::Hold;
                self.hold(c, rec, hw);
            }
            Env::Hold => self.hold(c, rec, hw),
            Env::P2 => {
                self.vol_out(c, hw);
                if self.dec_counter(c) {
                    let (w38, b40, b41) = (self.w(rec + 38), self.b(rec + 40), self.b(rec + 41));
                    let ch = &mut self.ch[c];
                    ch.p16(0x2A, w38);
                    ch.p8(0x2C, b40);
                    ch.env = Env::P3;
                    ch.p8(0x2D, b41);
                    let n = ch.g8(0x2C).wrapping_neg();
                    ch.p8(0x2C, n);
                }
            }
            Env::P3 => {
                self.vol_out(c, hw);
                let (b41, b42) = (self.b(rec + 41), self.b(rec + 42));
                if self.dec_counter(c) {
                    let ch = &mut self.ch[c];
                    ch.p8(0x2C, b42);
                    ch.env = Env::P4;
                } else {
                    let ch = &mut self.ch[c];
                    let v = ch.g8(0x2D).wrapping_sub(1);
                    ch.p8(0x2D, v);
                    if v == 0 {
                        ch.p8(0x2D, b41);
                        let n = ch.g8(0x2C).wrapping_neg();
                        ch.p8(0x2C, n);
                    }
                }
            }
            Env::P4 => {
                self.vol_out(c, hw);
                if self.ch[c].g16(0x2E) == 0 {
                    self.env_end(c, rec, hw);
                }
            }
        }
    }

    fn hold(&mut self, c: usize, rec: usize, hw: &mut Hw) {
        self.vol_out(c, hw);
        if self.dec_counter(c) {
            let (a, b) = (self.b(rec + 35), self.b(rec + 36));
            let ch = &mut self.ch[c];
            ch.p8(0x2B, a);
            ch.p8(0x2C, b);
            ch.env = Env::P2;
        }
    }

    fn env_end(&mut self, c: usize, rec: usize, hw: &mut Hw) {
        let ch = &mut self.ch[c];
        let n = ch.g8(0x18).wrapping_sub(1);
        ch.p8(0x18, n);
        if n != 0 {
            ch.p16(0x2E, 0);
            self.restart_env(c);
        } else if self.b(rec + 47) != 0 {
            let wo = 48 * (self.b(rec + 47) as usize - 1);
            let pr = self.b(STEP0 + wo + 30);
            self.trigger(c, wo as u16, pr, hw);
        } else {
            hw.vol(c, 0);
            hw.dmacon(1 << c);
            let ch = &mut self.ch[c];
            ch.env = Env::None;
            ch.seg = Seg::None;
            ch.glide = false;
            ch.p32(0x22, 0x0000_FFFF);
        }
    }

    // -------------------------------------------------------------- segments

    fn segment(&mut self, c: usize, hw: &mut Hw) {
        let rec = self.rec(c);
        match self.ch[c].seg {
            Seg::None => {}
            Seg::Set => self.seg_set(c, rec, hw),
            Seg::Dur => self.seg_dur(c, rec, hw),
            Seg::Upd => self.seg_upd(c, rec, hw),
        }
    }

    fn seg_set(&mut self, c: usize, rec: usize, hw: &mut Hw) {
        let (d4, d3) = (self.ch[c].g16(0x40) as usize, self.ch[c].g16(0x42) as usize);
        let (a, b) = (self.b(rec + 4 + d3), self.b(rec + 6 + d3));
        let ch = &mut self.ch[c];
        ch.p8(0x46, a);
        let r = ch.g8(6 + d4);
        ch.p8(0x45, r);
        ch.p8(0x47, b);
        ch.seg = Seg::Dur;
        self.seg_dur(c, rec, hw);
    }

    fn seg_dur(&mut self, c: usize, rec: usize, hw: &mut Hw) {
        let d4 = self.ch[c].g16(0x40) as usize;
        let h = self.b(rec + 45);
        let lim = self.b(rec + 46) as i8;
        let sh = (self.b(rec + 37) & 63) as u32;
        let ch = &mut self.ch[c];
        if h != 0 {
            if ch.g8(0x48) != 0 {
                let v = ch.g8(0x48) - 1;
                ch.p8(0x48, v);
            } else {
                let v = ch.g16(d4).wrapping_add(ch.g8(0x4A) as i8 as i16 as u16);
                ch.p16(d4, v);
                let k = ch.g8(0x49).wrapping_sub(1);
                ch.p8(0x49, k);
                if k == 0 {
                    ch.p8(0x49, h);
                    let mut v = ch.g8(0x4A).wrapping_neg();
                    if v & 0x80 == 0 && (v as i8) < lim {
                        v = v.wrapping_add(1);
                    }
                    ch.p8(0x4A, v);
                }
            }
        }
        let p = if sh >= 16 { 0 } else { ch.g16(d4) >> sh };
        hw.per(c, p);
        let ch = &mut self.ch[c];
        let n = ch.g8(0x47).wrapping_sub(1);
        ch.p8(0x47, n);
        if n == 0 {
            ch.seg = Seg::Upd;
        }
    }

    fn seg_upd(&mut self, c: usize, rec: usize, hw: &mut Hw) {
        let (d4, d3) = (self.ch[c].g16(0x40) as usize, self.ch[c].g16(0x42) as usize);
        let (b6, b7, b8, nseg) = (self.b(rec + 6 + d3), self.b(rec + 7 + d3), self.b(rec + 8 + d3), self.b(rec + 43));
        let ch = &mut self.ch[c];
        let v = ch.g16(d4).wrapping_add(ch.g16(d4 + 4));
        ch.p16(d4, v);
        let s = ch.g16(d4 + 4).wrapping_add(b7 as i8 as i16 as u16);
        ch.p16(d4 + 4, s);
        let r = ch.g8(0x45).wrapping_sub(1);
        ch.p8(0x45, r);
        if r != 0 {
            ch.p8(0x47, b6);
            ch.seg = Seg::Dur;
            self.seg_dur(c, rec, hw);
            return;
        }
        if b8 & 1 != 0 {
            let v = ch.g16(d4 + 2);
            ch.p16(d4, v);
        }
        if b8 & 2 != 0 {
            let v = ch.g16(d4 + 4).wrapping_neg();
            ch.p16(d4 + 4, v);
        }
        if b8 & 4 != 0 && ch.g8(d4 + 6) > 8 {
            let v = ch.g8(d4 + 6) - 1;
            ch.p8(d4 + 6, v);
        }
        let k = ch.g8(0x46).wrapping_sub(1);
        ch.p8(0x46, k);
        if k != 0 {
            let r = ch.g8(d4 + 6);
            ch.p8(0x45, r);
            ch.p8(0x47, b6);
            ch.seg = Seg::Dur;
            self.seg_dur(c, rec, hw);
            return;
        }
        let (nd4, nd3) = if ch.g8(0x44) != nseg {
            let i = ch.g8(0x44).wrapping_add(1);
            ch.p8(0x44, i);
            (d4 + 8, d3 + 10)
        } else {
            ch.p8(0x44, 1);
            (0, 0)
        };
        ch.p16(0x40, nd4 as u16);
        ch.p16(0x42, nd3 as u16);
        self.seg_set(c, rec, hw);
    }

    /// The note glide, once per tick.
    fn glide(&mut self, c: usize) {
        let ch = &mut self.ch[c];
        if !ch.glide {
            return;
        }
        let t = ch.g16(0x5A);
        let d = t.wrapping_sub(ch.g16(0)) as i16;
        let sh = (ch.g8(0x5C) & 63) as u32;
        let r = if sh >= 16 { d >> 15 } else { d >> sh };
        if r == 0 {
            ch.p16(0, t);
            ch.glide = false;
        } else {
            let v = ch.g16(0).wrapping_add(r as u16);
            ch.p16(0, v);
        }
    }
}

impl Replayer for SteveTurner {
    fn format(&self) -> &'static str {
        "Steve Turner"
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
        (0, self.songs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n;
        self.start = Some(n + 1);
        self.changed = true;
        self.songend = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        if let Some(n) = self.start.take() {
            self.init(n, hw);
        }
        self.run_tick(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let silence = m.alloc(4);
    if silence == 0 {
        return None;
    }
    Some(Box::new(SteveTurner {
        d: d.to_vec(),
        lay,
        base,
        silence,
        songs: count(d, lay.seq),
        sub: 0,
        start: Some(1),
        changed: false,
        songend: false,
        master: 0,
        target: 255,
        gspeed: 255,
        pending: 0,
        song: 0,
        gprio: 0,
        speed: 0,
        counter: 0,
        ch: [Ch::new(), Ch::new(), Ch::new(), Ch::new()],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A test-only encoder for the few instruction forms the stub uses, built from
    /// their descriptions (mode and register fields), not from words.
    mod enc {
        const IMM: u16 = 7 << 3 | 4;
        pub fn move_l_imm_disp(v: u32, an: u16, disp: u16) -> Vec<u8> {
            let op = 0b0010 << 12 | an << 9 | 0b101 << 6 | IMM;
            [op.to_be_bytes().to_vec(), v.to_be_bytes().to_vec(), disp.to_be_bytes().to_vec()].concat()
        }
        pub fn move_w_imm_dn(v: u16, dn: u16) -> Vec<u8> {
            let op = 0b0011 << 12 | dn << 9 | IMM;
            [op.to_be_bytes().to_vec(), v.to_be_bytes().to_vec()].concat()
        }
        pub fn move_w_dn_dn(src: u16, dst: u16) -> Vec<u8> {
            (0b0011u16 << 12 | dst << 9 | src).to_be_bytes().to_vec()
        }
        pub fn jsr_abs_l(a: u32) -> Vec<u8> {
            let op: u16 = 0b0100_1110_10 << 6 | 7 << 3 | 1;
            [op.to_be_bytes().to_vec(), a.to_be_bytes().to_vec()].concat()
        }
        pub fn rts() -> Vec<u8> {
            (0b0100_1110_0111_0101u16).to_be_bytes().to_vec()
        }
    }

    const L: u32 = 0x1000;
    fn addr(off: usize) -> u32 {
        L + (off - STEP0) as u32
    }

    /// The 46-byte stub: four stores, a constant, a call and the return.
    fn stub_bytes(offs: [usize; 4], with_rts: bool) -> Vec<u8> {
        let mut s = Vec::new();
        for (k, o) in offs.iter().enumerate() {
            s.extend(enc::move_l_imm_disp(addr(*o), 5, 0x100 + 4 * k as u16));
        }
        s.extend(enc::move_w_imm_dn(255, 0));
        s.extend(enc::move_w_dn_dn(0, 1));
        s.extend(enc::jsr_abs_l(0x4000));
        if with_rts {
            s.extend(enc::rts());
        } else {
            s.extend(enc::move_w_imm_dn(1, 1));
        }
        s.truncate(STEP0);
        s
    }

    /// One subsong (speed 2, channel 0 only) whose track plays pattern 0 then ends;
    /// instrument 0 plays a 16-byte sample once. The pattern is at 170.
    fn module(pat: &[u8]) -> Vec<u8> {
        let mut d = stub_bytes([46, 142, 168, 178], true);
        assert_eq!(d.len(), STEP0);
        d.resize(STEP0 + 96, 0);
        let r = STEP0;
        d[r..r + 2].copy_from_slice(&0x1000u16.to_be_bytes());
        d[r + 4] = 1;
        d[r + 5] = 1;
        d[r + 6] = 3;
        d[r + 30] = 5;
        d[r + 31] = 0;
        d[r + 33] = 2;
        d[r + 35] = 2;
        d[r + 39] = 3;
        d[r + 40] = 10;
        d[r + 41] = 1;
        d[r + 43] = 1;
        // subsong table at 142: one record, then a terminator (priority 1)
        d.extend([0, 2, 0, 0, 0, 24, 0, 0, 0, 0, 0, 0]);
        d.extend([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        // track at 166: pattern 0, song end
        d.extend([0, 255]);
        // jump table at 168: pattern 0 at 170 (relative to the table)
        d.extend([0, 2]);
        // sample pointer table at 178: sample 0 header at 182 (offset 4)
        d.resize(178, 0);
        d.extend([0, 0, 0, 4]);
        d.extend([0, 16]);
        d.extend([1u8; 16]);
        assert_eq!(d.len(), 200);
        d[170..170 + pat.len()].copy_from_slice(pat);
        d
    }

    fn play(d: Vec<u8>, frames: usize) -> (bool, Vec<(u32, u16)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("jpo.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Steve Turner");
        let mut out = vec![0f32; 2 * 960 * frames];
        e.render(&mut out);
        let end = e.songend();
        (end, e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect())
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognises_the_stub_and_tables() {
        let d = module(&[176, 255]);
        let lay = layout(&d).unwrap();
        assert_eq!((lay.seq, lay.jmp, lay.smp), (142, 168, 178));
        assert_eq!(count(&d, lay.seq), 1);
        assert!(detect(&d));
        // no stub, a table outside the file, a short file, no return
        let mut e = d.clone();
        e[..STEP0].fill(0);
        assert!(!detect(&e));
        let mut e = d.clone();
        e[..STEP0].copy_from_slice(&stub_bytes([46, 142, 168, 9999], true));
        assert!(!detect(&e));
        assert!(!detect(&d[..100]));
        let mut e = d.clone();
        e[..STEP0].copy_from_slice(&stub_bytes([46, 142, 168, 178], false));
        assert!(!detect(&e));
    }

    /// A channel at the first start has no step pending word "none" (zeros, so a voice
    /// whose song word is 0 triggers step 0); after a change of subsong it has.
    #[test]
    fn channels_after_a_subsong_change_have_no_step() {
        let (n, s) = (Ch::new(), Ch::stopped());
        assert_eq!(n.g16(0x1C), 0);
        assert_eq!(s.g16(0x1C) as i16, -1);
        assert_eq!((s.g8(0x1A), s.g8(0x23)), (0, 0));
    }

    #[test]
    fn starts_the_instrument_then_the_envelope() {
        let (_, log) = play(module(&[129, 176, 255]), 4);
        // sample set (length in words), period of the segment
        assert_eq!(reg(&log, 0xDFF0A4)[0], 8);
        assert_eq!(reg(&log, 0xDFF0A6)[0], 0x1000);
        let dma = reg(&log, 0xDFF096);
        // the one-shot (loop count 0): volume 0 and DMA on, then the master volume
        assert!(dma.contains(&0x8001));
        let vol = reg(&log, 0xDFF0A8);
        assert_eq!(vol[..2], [0, 63]);
        // then the silent loop of one word
        assert!(reg(&log, 0xDFF0A4).contains(&1));
    }

    #[test]
    fn a_notes_period_comes_from_the_table() {
        // select instrument 0, a pause of 1, note 12
        let (_, log) = play(module(&[208, 128, 12, 255]), 2);
        assert_eq!(reg(&log, 0xDFF0A6)[0], 30578);
    }

    #[test]
    fn song_end_is_announced_when_the_order_byte_is_read_and_restarts() {
        let (end, log) = play(module(&[129, 176, 255]), 1);
        assert!(!end);
        assert_eq!(reg(&log, 0xDFF0A4).len(), 1);
        let (end, log) = play(module(&[129, 176, 255]), 6);
        assert!(end);
        // the replayer restarts the song itself: the sample is set again
        assert!(reg(&log, 0xDFF0A4).iter().filter(|&&l| l == 8).count() >= 2);
    }
}

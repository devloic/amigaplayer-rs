//! Paul Robotham (`dat.*` with a sample companion `*.ssd`), the music driver of a few
//! Amiga games (the corpus has `dat.intromus`, `dat.mdtest` and `dat.win`).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's player and the structure of the
//! three corpus files), and this player was written from it alone and checked
//! against those writes. The numeric table (the periods) is the specification's.
//!
//! A module is a header (channel, vibrato table, envelope table and sample counts),
//! the pointer tables, a sample table (offsets into the companion file) and a byte
//! stream per channel: notes, ties and commands, each note or tie followed by a
//! duration byte. The tick is 50 Hz. A note stops the channel's DMA and sets address,
//! length and period; the next tick starts the DMA and the one after installs the
//! loop; the volume follows an envelope table.
//!
//! Decisions where the specification leaves the choice to the port:
//! * The companion is `NAME.SSD` for `NAME.dat` (the name's last extension replaced),
//!   else `mdtest.ssd`; a module whose companion is missing is refused.
//! * Notes 60 to 126 use the last period of the table (the original reads past it).
//! * A command index without a defined effect has no argument; bytes from 0xA0 on
//!   are such commands. Commands 0x94 and 0x95 take one argument byte, which is
//!   ignored. An index (sample, vibrato table, envelope table) past its count is
//!   ignored.
//! * The specification's "silent note" (no-retrigger mode with a sample just selected)
//!   is not what the reference does: it starts the note as a normal one, clearing the
//!   selection; only without a selection is the note period-only. The port follows
//!   the reference.
//! * The loop stack is limited to 256 entries (further pushes are dropped).
//! * An envelope that loops back without end is cut after 256 steps (volume 0).
//! * With a decay on and a decay value of zero, the level byte is used.
//! * A song of one channel has no song end.
//! * The song end restarts the song at once. As the reference does, the restart
//!   leaves the periods, the restart values for Paula, the tempo, the global volume
//!   slide's rate and the pending DMA bits as they are (the specification has them
//!   cleared); the hardware is left as it is.
//! * Reads past the end of the module give zero (the channel ends).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Paul Robotham", detect, new };

/// Paula periods of the notes.
const PERIODS: [u16; 60] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 452, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240,
    226, 214, 202, 190, 180, 169, 160, 151, 142, 134, 127, 120, 113, 107, 101, 95, 90, 84, 80, 75, 71, 67, 63, 60, 56,
    53, 50, 47, 45, 42, 40, 37, 35, 33, 31, 30, 28,
];

const DURATION_DIVISOR: u32 = 10800;
const GUARD: u32 = 1 << 16;

/// Colour clocks of the original's code between the tick and the first channel,
/// and between channels.
const T_FIRST: u64 = 246;
const T_CHANNEL: u64 = 186;
/// The note's parse before its first write, between its writes, and before its tail.
const T_NOTE: u64 = 684;
const T_WRITE: u64 = 32;
const T_NOTE_TAIL: u64 = 370;
/// Before the volume write.
const T_VOL: u64 = 386;

struct Layout {
    c: usize,
    p: usize,
    q: usize,
    s: usize,
    smp: usize,
}

/// The header, the tables and the 0x3F run after the sample table, if all fit.
fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 8 {
        return None;
    }
    let (c, p, q, s) = (be16(d, 0) as usize, be16(d, 2) as usize, be16(d, 4) as usize, be16(d, 6) as usize);
    if !(1..=4).contains(&c) || p >= 256 || q == 0 || q >= 256 || s >= 256 {
        return None;
    }
    let smp = 8 + 4 * c + 4 * p + 4 * q;
    let end = smp + 12 * s;
    if end > d.len() {
        return None;
    }
    for i in 0..c {
        let v = be32(d, 8 + 4 * i);
        if v == 0 || v >= 65536 || v as usize >= d.len() {
            return None;
        }
    }
    for i in 0..p + q {
        let v = be32(d, 8 + 4 * c + 4 * i);
        if v == 0 || v >= 0x8000_0000 || v & 1 != 0 || v as usize >= d.len() {
            return None;
        }
    }
    if be32(d, 8 + 4 * c + 4 * p) as usize != end {
        return None;
    }
    if end + 254 > d.len() || (0..127).any(|i| be16(d, end + 2 * i) != 0x3F3F) {
        return None;
    }
    Some(Layout { c, p, q, s, smp })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

#[derive(Clone, Default)]
struct Chan {
    ptr: usize,
    counter: u16,
    acc: u16,
    vphase: u8,
    vspeed: u8,
    ephase: u8,
    vtab: usize,
    etab: usize,
    stack: Vec<(u8, usize)>,
    entry: usize,
    level: u8,
    decay: u8,
    dstep: u8,
    depth: u16,
    slide: u16,
    cur: u16,
    tgt: u16,
    raddr: u32,
    rlen: u16,
    rper: u16,
    /// bit 2 slide, 3 tie, 4 vibrato, 6 decay, 7 stopped
    f1: u8,
    /// bit 0 no-retrigger, 1 keep envelope phase, 2 keep vibrato phase
    f2: u8,
    pending: bool,
}

pub struct PaulRobotham {
    d: Vec<u8>,
    lay: Layout,
    title: String,
    /// The companion in guest memory, and the silence buffer.
    smp: u32,
    silence: u32,
    ch: [Chan; 4],
    tempo: u32,
    gvol: u8,
    gtarget: u8,
    gdir: i8,
    gcount: u8,
    gspeed: u8,
    /// per-channel bits: trigger pending, restart pending, DMA active
    da9: u8,
    da8: u8,
    dae: u8,
    songend: bool,
}

/// The companion names to try: `NAME.SSD` for `NAME.dat`, then `mdtest.ssd`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    if let Some(i) = file.rfind('.') {
        if file[i + 1..].eq_ignore_ascii_case("dat") {
            v.push(format!("{}.SSD", &file[..i]));
        }
    }
    v.push("mdtest.ssd".to_string());
    v
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut sample = None;
    for name in companion(&file) {
        if let Ok(s) = crate::deli::read_companion(m, &name).1 {
            sample = Some(s);
            break;
        }
    }
    let sample = sample?;
    let smp = m.alloc((sample.len() as u32).max(4));
    let silence = m.alloc(16);
    if smp == 0 || silence == 0 {
        return None;
    }
    m.bus.write_bytes(smp, &sample);
    let mut r = PaulRobotham {
        d: d.to_vec(),
        lay,
        title: file,
        smp,
        silence,
        ch: Default::default(),
        tempo: 1000,
        gvol: 63,
        gtarget: 63,
        gdir: 0,
        gcount: 0,
        gspeed: 1,
        da9: 0,
        da8: 0,
        dae: 0,
        songend: false,
    };
    r.init(false);
    let mut hw = Hw { bus: &mut m.bus };
    hw.dmacon(0x000F);
    Some(Box::new(r))
}

impl PaulRobotham {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn vib_table(&self, i: usize) -> Option<usize> {
        (i < self.lay.p).then(|| be32(&self.d, 8 + 4 * self.lay.c + 4 * i) as usize)
    }

    fn env_table(&self, i: usize) -> Option<usize> {
        (i < self.lay.q).then(|| be32(&self.d, 8 + 4 * self.lay.c + 4 * self.lay.p + 4 * i) as usize)
    }

    /// The state at the start of the song, and at its restart (`restart`): the
    /// reference keeps the periods and Paula's restart values, the tempo, the global
    /// volume slide's rate and the pending DMA bits through the restart.
    fn init(&mut self, restart: bool) {
        let vt = self.vib_table(0).unwrap_or(0);
        let et = self.env_table(0).unwrap_or(0);
        for k in 0..4 {
            let old = std::mem::take(&mut self.ch[k]);
            self.ch[k] = Chan {
                ptr: if k < self.lay.c { be32(&self.d, 8 + 4 * k) as usize } else { 0 },
                counter: 10,
                vspeed: 1,
                ephase: 3,
                vtab: vt,
                etab: et,
                entry: self.lay.smp,
                depth: 10000,
                f1: if k < self.lay.c { 0x20 } else { 0xA0 },
                cur: old.cur,
                tgt: old.tgt,
                raddr: old.raddr,
                rlen: old.rlen,
                rper: old.rper,
                ..Default::default()
            };
        }
        self.gvol = 63;
        self.gtarget = 63;
        if !restart {
            self.da9 = 0;
            self.da8 = 0;
            self.dae = 0;
        }
    }

    fn period_of(note: u8) -> u16 {
        PERIODS[(note as usize).min(PERIODS.len() - 1)]
    }

    /// Reads the duration byte at `*p` and sets the counter from it.
    fn duration(&mut self, k: usize, p: &mut usize, reset: bool) {
        let b = self.rb(*p) as u32;
        *p += 1;
        let d = (b & 0x1F) << (b >> 5);
        let c = &mut self.ch[k];
        let t = d * self.tempo + c.acc as u32;
        c.counter = (t / DURATION_DIVISOR) as u16;
        c.acc = (t % DURATION_DIVISOR) as u16;
        if reset {
            if c.f2 & 4 == 0 {
                c.vphase = 0;
            }
            if c.f2 & 2 == 0 {
                c.ephase = 3;
            }
        }
    }

    /// A command byte `b` (0x80 or more); `p` is after it. Returns the position
    /// after its arguments.
    fn command(&mut self, k: usize, b: u8, mut p: usize) -> usize {
        if b >= 0xA0 {
            return p;
        }
        let mut next = |s: &Self| {
            let v = s.rb(p);
            p += 1;
            v
        };
        match b & 0x1F {
            1 => {
                let n = next(self);
                if self.ch[k].stack.len() < 256 {
                    let pos = p;
                    self.ch[k].stack.push((n, pos));
                }
            }
            2 => {
                if let Some(top) = self.ch[k].stack.last_mut() {
                    top.0 = top.0.wrapping_sub(1);
                    if top.0 != 0 {
                        return top.1;
                    }
                    self.ch[k].stack.pop();
                }
            }
            7 => {
                let v = next(self);
                let c = &mut self.ch[k];
                if v == 0 {
                    c.f1 &= !0x10;
                } else {
                    c.depth = (10000 / v as u32) as u16;
                    c.f1 |= 0x10;
                }
            }
            8 => {
                let v = next(self);
                self.ch[k].vspeed = v;
            }
            9 => {
                let v = next(self);
                let c = &mut self.ch[k];
                if v != 0 {
                    c.slide = 4 * v as u16;
                    c.f1 |= 4;
                } else {
                    c.f1 &= !4;
                }
            }
            11 => {
                let v = next(self);
                if let Some(o) = self.vib_table(v as usize) {
                    self.ch[k].vtab = o;
                }
            }
            12 => {
                let v = next(self);
                if let Some(o) = self.env_table(v as usize) {
                    self.ch[k].etab = o;
                }
            }
            13 => {
                let v = next(self);
                self.ch[k].level = v;
            }
            14 => {
                let v = next(self);
                self.tempo = 16 * v as u32;
            }
            15 => {
                let v = next(self) as usize;
                if v < self.lay.s {
                    self.ch[k].entry = self.lay.smp + 12 * v;
                }
                self.ch[k].pending = true;
            }
            19 => {
                let v = next(self);
                self.ch[k].f2 = v;
            }
            20 | 21 => {
                next(self);
            }
            22 => {
                let t = next(self);
                let s = next(self);
                self.gtarget = t;
                self.gdir = if t > self.gvol { 1 } else { -1 };
                self.gspeed = s;
                if s == 0 {
                    self.gdir = 0;
                    self.gvol = t;
                }
            }
            23 => {
                let c = &mut self.ch[k];
                c.decay = c.level;
                c.f1 |= 0x40;
            }
            24 => {
                let v = next(self);
                self.ch[k].dstep = v;
            }
            _ => {}
        }
        p
    }

    /// The sample entry of channel `k`: (start, length in words, loop start, loop words).
    fn entry(&self, k: usize) -> (u32, u16, u32, u16) {
        let e = self.ch[k].entry;
        (be32(&self.d, e), be16(&self.d, e + 4), be32(&self.d, e + 6), be16(&self.d, e + 10))
    }

    /// The DMA-on or restart step of a tick without a new note.
    fn trigger(&mut self, k: usize, hw: &mut Hw) {
        let bit = 1u8 << k;
        if self.da9 & bit != 0 {
            self.da9 &= !bit;
            self.da8 |= bit;
            hw.dmacon(0x8200 | bit as u16);
            self.dae |= bit;
        } else if self.da8 & bit != 0 {
            self.da8 &= !bit;
            let c = &self.ch[k];
            let (a, l, p) = (c.raddr, c.rlen, c.rper);
            hw.loc(k, a);
            hw.len(k, l);
            hw.per(k, p);
        }
    }

    fn channel(&mut self, k: usize, hw: &mut Hw) {
        if self.ch[k].f1 & 0x80 != 0 {
            return;
        }
        {
            let c = &mut self.ch[k];
            c.vphase = c.vphase.wrapping_add(c.vspeed);
            c.ephase = c.ephase.wrapping_add(4);
        }
        if self.ch[k].counter != 0 {
            self.trigger(k, hw);
        } else if !self.events(k, hw) {
            return;
        }
        self.tail(k, hw);
    }

    /// Reads events until a note, tie or end; false at a channel end.
    fn events(&mut self, k: usize, hw: &mut Hw) -> bool {
        let bit = 1u8 << k;
        let mut p = self.ch[k].ptr;
        for _ in 0..GUARD {
            let b = self.rb(p);
            p += 1;
            if b >= 0x80 {
                p = self.command(k, b, p);
                self.ch[k].ptr = p;
                continue;
            }
            if b == 0 {
                hw.dmacon(bit as u16);
                self.dae &= !bit;
                self.ch[k].f1 |= 0x80;
                return false;
            }
            if b == 0x7F {
                self.ch[k].f1 |= 8;
                self.duration(k, &mut p, false);
                self.ch[k].ptr = p;
                self.trigger(k, hw);
                return true;
            }
            self.ch[k].f1 &= !8;
            let per = Self::period_of(b);
            let (start, len, lstart, llen) = self.entry(k);
            let f2 = self.ch[k].f2;
            let pending = self.ch[k].pending;
            let reg = 0xDFF0AA + 16 * k as u32;
            if f2 & 1 == 0 || pending {
                // a normal note (in the no-retrigger mode only when a sample was just selected)
                hw.wait(T_NOTE);
                self.ch[k].pending = false;
                hw.bus.write_word(reg, 0);
                hw.wait(T_WRITE);
                hw.dmacon(bit as u16);
                self.dae &= !bit;
                self.ch[k].f1 &= !0x40;
                self.ch[k].decay = 0;
                hw.wait(T_WRITE);
                hw.loc(k, self.smp + start);
                hw.len(k, len);
                hw.per(k, per);
                let c = &mut self.ch[k];
                c.tgt = per;
                (c.raddr, c.rlen) = if llen != 0 { (self.smp + lstart, llen) } else { (self.silence, 8) };
                c.rper = per;
            } else {
                // a period-only note
                hw.per(k, per);
                self.ch[k].tgt = per;
            }
            self.duration(k, &mut p, true);
            self.ch[k].ptr = p;
            hw.wait(T_NOTE_TAIL);
            if f2 & 1 == 0 || pending {
                self.da9 |= bit;
            }
            return true;
        }
        // a stream that never reaches a note ends the channel
        hw.dmacon(bit as u16);
        self.dae &= !bit;
        self.ch[k].f1 |= 0x80;
        false
    }

    fn tail(&mut self, k: usize, hw: &mut Hw) {
        let bit = 1u8 << k;
        {
            let c = &mut self.ch[k];
            if c.f1 & 4 != 0 {
                let dist = c.tgt as i32 - c.cur as i32;
                if dist.abs() <= c.slide as i32 {
                    c.cur = c.tgt;
                } else {
                    c.cur = (c.cur as i32 + dist.signum() * c.slide as i32) as u16;
                }
            } else {
                c.cur = c.tgt;
            }
        }
        let c = &self.ch[k];
        let mut per = c.cur;
        if c.f1 & 0x10 != 0 && c.depth != 0 {
            let v = self.rb(c.vtab + c.vphase as usize) as i8 as i32;
            per = (c.cur as i32 + c.cur as i32 * v / c.depth as i32) as u16;
        }
        hw.per(k, per);
        self.ch[k].counter = self.ch[k].counter.wrapping_sub(1);
        if self.dae & bit == 0 {
            return;
        }
        let c = &self.ch[k];
        hw.wait(T_VOL);
        if c.level == 0 {
            hw.vol(k, 0);
            return;
        }
        let mut ph = c.ephase;
        let mut env = None;
        for _ in 0..256 {
            let v = self.rb(c.etab + ph as usize);
            if v & 0x80 == 0 {
                env = Some(v & 63);
                break;
            }
            ph = ph.wrapping_sub(((-(v as i8 as i32) + 3) & 0xFC) as u8);
        }
        self.ch[k].ephase = ph;
        let c = &self.ch[k];
        let Some(e) = env else {
            hw.vol(k, 0);
            return;
        };
        let mut level = c.level & 63;
        if c.f1 & 0x40 != 0 && c.decay != 0 {
            let c = &mut self.ch[k];
            let n = c.decay as i8 as i32 - c.dstep as i8 as i32;
            c.decay = if n < 0 { 0 } else { n as u8 };
            level = c.decay & 63;
        }
        let mut v = e as u32 * level as u32 / 63;
        if self.gvol != 63 {
            v = (v * self.gvol as u32 / 63) & 63;
        }
        hw.vol(k, v as u16);
    }
}

impl Replayer for PaulRobotham {
    fn format(&self) -> &'static str {
        "Paul Robotham"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        hw.wait(T_FIRST);
        for k in 0..self.lay.c {
            self.channel(k, hw);
            hw.wait(T_CHANNEL);
        }
        // the global volume slide
        self.gcount = self.gcount.wrapping_add(1);
        if self.gcount >= self.gspeed {
            self.gcount = 0;
            if self.gdir != 0 {
                if self.gvol == self.gtarget {
                    self.gdir = 0;
                } else {
                    self.gvol = (self.gvol as i16 + self.gdir as i16) as u8;
                }
            }
        }
        if self.lay.c >= 2 && self.ch[0].ptr + 1 == be32(&self.d, 12) as usize {
            self.songend = true;
            self.init(true);
        }
    }
}

#[cfg(test)]
#[path = "paulrobotham_tests.rs"]
mod tests;

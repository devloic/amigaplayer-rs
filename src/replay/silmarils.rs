//! Silmarils (`mok.*`), the music of the Silmarils games: Colorado, Crystals of
//! Arborea, Ishar, Starblade, Storm Master, Le Fetiche Maya, Targhan, Wind Surf
//! Willy, Metal Mutant and Boston Bomb Club.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Silmarils player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A module is a resource container of the games' ALIS engine: a header, a small
//! bytecode script, and a directory of self-relative resources, sample resources
//! (optionally delta-packed) and music streams. The script is run by a reduced
//! interpreter to learn which stream a subsong plays, at which tempo, and which
//! samples the 32 instrument slots hold. The stream is a byte sequence of delays,
//! notes, instrument changes and releases on three channels, read at a tick of
//! about 216 Hz (CIA timer 0x0CD0) through a tempo counter. The samples' loops and
//! one-shot ends are handled by per-channel audio interrupt handlers, which the
//! host's Paula emulation raises (`audio_irq_ch`, and while the tick runs, as the
//! original's tick allows them). A module with two samples or fewer takes the
//! rest from a companion, the file named up to the first `_` (`mok.BostonBombClub`
//! for `mok.BostonBombClub_title`).
//!
//! The moments of the writes within a tick follow the reference player's measured
//! gaps (a wait of ten raster lines before the tick's DMA and interrupt enables,
//! the time an audio interrupt takes to be served), so that the audio interrupts
//! fall between the same writes.
//!
//! Decided where the specification is silent (from the reference runs, or for
//! cases no corpus file reaches): a request that an INTENA write enables is taken
//! after the write that follows when that one comes at once (a release's DMACON);
//! every subsong starts from the state the load passes left (slots, variables,
//! tempo, module bytes), as a fresh load of the reference does; a return-stack push
//! stores at the stack offset and then lowers it (the first push lands at VARS
//! 0x08); variable and stack accesses past the 256-byte block use more of it; a
//! note index outside the period table takes its nearest entry; a note on a channel
//! with no sample turns its DMA off only.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Silmarils", detect, new };

/// Paula periods, index = note + the slot's note offset.
const PERIODS: [u16; 128] = [
    5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728, 5728,
    5728, 5728, 5728, 5404, 5104, 4816, 4544, 4288, 4048, 3824, 3608, 3404, 3214, 3034, 2864, 2703, 2551, 2408, 2273,
    2145, 2225, 1911, 1804, 1703, 1607, 1517, 1432, 1351, 1276, 1204, 1136, 1072, 1012, 956, 902, 851, 804, 758, 716,
    676, 638, 602, 568, 536, 506, 478, 451, 426, 402, 379, 358, 338, 319, 301, 284, 268, 253, 239, 226, 213, 201, 190,
    179, 169, 159, 150, 142, 134, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
    127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
    127,
];

/// The sample depacker's deltas, by 4-bit code.
const DELTA: [i8; 16] = [-34, -21, -13, -8, -5, -3, -2, -1, 1, 2, 3, 5, 8, 13, 21, 34];

/// The CIA timer value of the tick.
const TIMER: u64 = 0x0CD0;

/// Script steps before a run is given up (a guard: no corpus script comes close).
const SCRIPT_GUARD: u32 = 200_000;
/// Stream events read in one tick before the tick gives up (a stream with no delay).
const STREAM_GUARD: u32 = 4096;

/// Per channel (3 has the original's wrong entries): INTENA clear, DMACON off,
/// DMACON on, INTENA set.
const INT_CLR: [u16; 4] = [0x0080, 0x0100, 0x0200, 0x0000];
const DMA_OFF: [u16; 4] = [0x0001, 0x0002, 0x0004, 0x0008];
const DMA_ON: [u16; 4] = [0x8201, 0x8202, 0x8204, 0x8080];
const INT_SET: [u16; 4] = [0x8080, 0x8100, 0x8200, 0x0DBA];

// The reference player's timing, in colour clocks, measured on the UADE runs: the
// moments of the writes decide between which writes Paula's interrupts fall.
/// Tick start to the first stream event, when the counter runs out.
const T_PARSE: u64 = 224;
/// Tick start to the raster wait, when it does not.
const T_IDLE: u64 = 191;
/// An instrument change (or a song end) to the next event.
const T_SET: u64 = 98;
/// A note's VOL write to the next event (plus 10 a channel).
const T_NOTE_END: u64 = 182;
/// A note's LC write to its LEN write: with a first-play end, and without.
const T_LEN_B: u64 = 20;
const T_LEN_SIZE: u64 = 34;
/// A release's start to its first write, and its last write to the next event.
const T_REL: u64 = 22;
const T_REL_END: u64 = 72;
/// A delay byte to the start of the raster wait.
const T_DELAY: u64 = 25;
/// The tenth line change to the flush's INTENA write.
const T_FLUSH: u64 = 36;
/// A request Paula raises reaches the processor this much later (the tick goes
/// on meanwhile); the server then looks at the requests this much later.
const T_IRQ_DELAY: u64 = 153;
const T_IRQ_RAISE: u64 = 150;
/// An INTENA write that enables a pending request to the server's look.
const T_IRQ_ENABLE: u64 = 282;
/// Each channel the server tests before the one it takes.
const T_IRQ_TEST: u64 = 7;
/// A handler's last write to the next one's, and to the return.
const T_IRQ_NEXT: u64 = 66;
const T_IRQ_EXIT: u64 = 68;
/// A handler's start to its first write: loop (DMA on), one-shot, stop; the loop's
/// LEN write to its LC write.
const T_IRQ_LOOP: u64 = 30;
const T_IRQ_ONESHOT: u64 = 15;
const T_IRQ_STOP: u64 = 15;
const T_IRQ_LOOP_LC: u64 = 14;

/// A note's PER write to its VOL write, by channel.
const T_VOL: [u64; 4] = [158, 174, 188, 202];
/// The raster wait: this many line changes, then the flush (its INTENA, then its
/// DMACON this much later).
const WAIT_LINES: u32 = 10;
const T_FLUSH_DMA: u64 = 18;
/// The order in which the interrupt server tests the channels.
const SERVER_ORDER: [usize; 4] = [1, 3, 0, 2];
/// The gaps between a handler's writes.
const T_IRQ_GAP: u64 = 8;
/// An INTENA write enabling a pending request to the processor taking it.
const T_IRQ_RECOGNISE: u64 = 12;

const LINE: u64 = crate::paula::LINE;

/// The header base for loading: past a 6-byte prefix when the header's 0x0016
/// word is at 0x0A.
fn load_base(d: &[u8]) -> usize {
    if be16(d, 0x0A) == 0x16 {
        6
    } else {
        0
    }
}

/// Header checks 2 and 3 at base `h`: the zero words, the two 0x0020 words and a
/// directory header whose first long is 0x58. Returns the directory header.
fn header_ok(d: &[u8], h: usize) -> Option<usize> {
    if d.len() < h + 0x18 || be16(d, h + 4) != 0x16 {
        return None;
    }
    if [6, 8, 0x0A, 0x0C, 0x0E].iter().any(|o| be16(d, h + o) != 0) {
        return None;
    }
    if be16(d, h + 0x12) != 0x20 || be16(d, h + 0x16) != 0x20 {
        return None;
    }
    let dir = be32(d, h + 0x0E) as usize;
    if dir & 1 != 0 || be32(d, h + dir) != 0x58 {
        return None;
    }
    Some(h + dir)
}

/// The resource offsets of the entry table at `dt`: up to the first entry that is
/// zero or has a non-zero top byte (`None` if an entry used is odd).
fn walk(d: &[u8], dt: usize) -> Option<Vec<usize>> {
    let mut v = Vec::new();
    let mut e = dt;
    while e + 4 <= d.len() && v.len() < 0x10000 {
        let x = be32(d, e);
        if x == 0 || x >> 24 != 0 {
            break;
        }
        if x & 1 != 0 {
            return None;
        }
        v.push(e + x as usize);
        e += 4;
    }
    Some(v)
}

fn is_music(d: &[u8], r: usize) -> bool {
    be16(d, r + 6) == 0xFF02
}

/// Recognition: the header base, the resources, and whether there are music
/// resources (the subsongs) and how many other ones.
fn scan(d: &[u8]) -> Option<(usize, usize, usize)> {
    let h = if be16(d, 4) == 0x16 {
        0
    } else if be16(d, 0x0A) == 0x16 {
        6
    } else {
        return None;
    };
    let dh = header_ok(d, h)?;
    let res = walk(d, dh + 0x58)?;
    let music = res.iter().filter(|&&r| is_music(d, r)).count();
    Some((h, music, res.len() - music))
}

fn detect(d: &[u8]) -> bool {
    scan(d).is_some_and(|(_, music, _)| music > 0)
}

/// Depacks the delta-packed samples in place, walking the directory found from the
/// file start (for a file with a prefix that finds no resources).
fn depack(d: &mut [u8]) {
    let dh = be32(d, 0x0E) as usize;
    let count = be16(d, dh + 0x10) as usize;
    let dt = dh + be32(d, dh + 0x0C) as usize;
    for i in 0..count {
        let e = dt + 4 * i;
        let r = e.wrapping_add(be32(d, e) as usize);
        let Some(&ty) = d.get(r) else { continue };
        if !(ty == 1 || ty == 2) || d.get(r + 6) != Some(&1) {
            continue;
        }
        let size = be32(d, r + 2);
        if size < 0x10 {
            continue;
        }
        let n = ((size - 0x10) / 2) as usize;
        let s = r + 0x10;
        if n == 0 || s + n > d.len() {
            continue;
        }
        let packed = d[s..s + n].to_vec();
        let mut acc = packed[0];
        let mut out = Vec::with_capacity(2 * n);
        out.push(acc);
        for &p in &packed[1..] {
            for code in [p >> 4, p & 15] {
                acc = acc.wrapping_add(DELTA[code as usize] as u8);
                out.push(acc);
            }
        }
        let k = out.len();
        if k >= 2 {
            out[k - 2] = 0;
            out[k - 1] = 0;
        }
        let end = (s + k).min(d.len());
        d[s..end].copy_from_slice(&out[..end - s]);
        d[r + 2..r + 6].copy_from_slice(&(size - 2).to_be_bytes());
    }
}

/// A loaded container: the module, or its companion.
#[derive(Clone)]
struct Cont {
    d: Vec<u8>,
    /// Guest address of `d[0]`.
    base: u32,
    h: usize,
    /// The entry table.
    dt: usize,
}

impl Cont {
    fn new(d: Vec<u8>, base: u32) -> Option<Cont> {
        let h = load_base(&d);
        let dh = header_ok(&d, h)?;
        let dt = dh + be32(&d, dh + 0x0C) as usize;
        Some(Cont { d, base, h, dt })
    }

    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// Resource `i` (no bounds check, as the original's lookup).
    fn res(&self, i: u16) -> usize {
        let e = self.dt + 4 * i as usize;
        e.wrapping_add(be32(&self.d, e) as usize)
    }

    /// The first script position: past an `A1 42` word.
    fn script(&self) -> usize {
        let p = self.h + 0x18;
        if be16(&self.d, p) == 0xA142 {
            p + 2
        } else {
            p
        }
    }
}

/// A sample: the container and its resource offset.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Smp {
    c: u8,
    r: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Slot {
    smp: Option<Smp>,
    off: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Irq {
    None,
    Loop(Option<Smp>),
    OneShot,
    Stop,
}

#[derive(Clone, Copy)]
struct Chan {
    slot: Slot,
    irq: Irq,
}

/// The script interpreter's state.
#[derive(Clone)]
struct Vm {
    vars: Vec<u8>,
    x: u16,
    y: u16,
    sp: i32,
}

/// How a script run ended: at `3F` or `42` (the position after it), or aborted.
type RunEnd = Option<usize>;

/// The state the load-time passes leave, restored at each subsong start (the
/// reference loads the module afresh for each).
#[derive(Clone)]
struct Loaded {
    d: Vec<u8>,
    slots: [Slot; 32],
    vm: Vm,
    tempo: u8,
}

pub struct Silmarils {
    c: Vec<Cont>,
    loaded: Loaded,
    title: String,
    nsongs: u16,
    sub: u16,
    started: bool,
    sel_base: u16,
    tf: bool,
    vm: Vm,
    slots: [Slot; 32],
    stream: Option<(u8, usize)>,
    tempo: u8,
    // the song
    silent: bool,
    start: usize,
    pos: usize,
    sc: u8,
    step: u16,
    counter: u16,
    cmd: u8,
    pend_dma: u16,
    pend_int: u16,
    ch: [Chan; 4],
    songend: bool,
    /// When a raised audio interrupt reaches the processor, and the server's time
    /// from then to its first look at the requests.
    irq_due: Option<(u64, u64)>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (_, nsongs, others) = scan(d)?;
    if nsongs == 0 {
        return None;
    }
    let mut md = d.to_vec();
    depack(&mut md);
    m.bus.write_bytes(base, &md);
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut c = vec![Cont::new(md, base)?];
    if others <= 2 {
        let comp = companion(&file)?;
        let cd = crate::deli::read_companion(m, &comp).1.ok()?;
        let h = load_base(&cd);
        header_ok(&cd, h)?;
        let mut cd = cd;
        depack(&mut cd);
        let cb = m.alloc((cd.len() as u32).max(4));
        if cb == 0 {
            return None;
        }
        m.bus.write_bytes(cb, &cd);
        c.push(Cont::new(cd, cb)?);
    }
    let vm = Vm { vars: vec![0; 0x200], x: 0, y: 0, sp: -0x54 };
    let mut s = Silmarils {
        loaded: Loaded { d: Vec::new(), slots: [Slot::default(); 32], vm: vm.clone(), tempo: 0 },
        c,
        title: file.rsplit(['/', ':']).next().unwrap_or(&file).to_string(),
        nsongs: nsongs as u16,
        sub: 0,
        started: false,
        sel_base: 0,
        tf: false,
        vm,
        slots: [Slot::default(); 32],
        stream: None,
        tempo: 0,
        silent: true,
        start: 0,
        pos: 0,
        sc: 0,
        step: 0,
        counter: 0,
        cmd: 0,
        pend_dma: 0,
        pend_int: 0,
        ch: [Chan { slot: Slot::default(), irq: Irq::None }; 4],
        songend: false,
        irq_due: None,
    };
    s.load()?;
    Some(Box::new(s))
}

/// The companion's name: the module's up to its first `_` after the `mok.` prefix.
fn companion(file: &str) -> Option<String> {
    let name = file.rsplit(['/', ':']).next().unwrap_or(file);
    let cut = name.find('_')?;
    Some(name[..cut].to_string())
}

impl Silmarils {
    /// The load-time passes: the probe run, a second one with selector base 1 when
    /// the script starts by loading a sample or the probe left all slots alike,
    /// then the tempo flag.
    fn load(&mut self) -> Option<()> {
        self.slots = [Slot::default(); 32];
        self.sel_base = 0;
        self.run_main(0)?;
        let first = self.slots[0].smp;
        if be16(&self.c[0].d, 0x18) == 0xCD00 || self.slots.iter().all(|s| s.smp == first) {
            self.sel_base = 1;
            self.slots = [Slot::default(); 32];
            self.run_main(0)?;
        }
        self.tf = self.slots.iter().any(|s| s.off != 0);
        if self.c.len() > 1 {
            self.sel_base = 1;
        }
        self.loaded = Loaded { d: self.c[0].d.clone(), slots: self.slots, vm: self.vm.clone(), tempo: self.tempo };
        Some(())
    }

    /// A run of the module's script for `sub`: VARS cleared, the selector set.
    fn run_main(&mut self, sub: u16) -> RunEnd {
        self.vm.vars[..0x80].fill(0);
        let sel = self.sel_base.wrapping_add(sub);
        self.vm.vars[0x68..0x6A].copy_from_slice(&sel.to_be_bytes());
        self.vm.sp = -0x54;
        let p = self.c[0].script();
        self.run(0, p)
    }

    // ---- the script interpreter

    fn var8(&self, n: u8) -> u8 {
        self.vm.vars[0x5C + n as usize]
    }

    fn var16(&self, n: u8) -> u16 {
        let i = 0x5C + n as usize;
        u16::from_be_bytes([self.vm.vars[i], self.vm.vars[i + 1]])
    }

    fn set16(&mut self, n: u8, v: u16) {
        let i = 0x5C + n as usize;
        self.vm.vars[i..i + 2].copy_from_slice(&v.to_be_bytes());
    }

    /// Runs the script of container `c` from `pc` until it ends or aborts.
    fn run(&mut self, c: u8, mut pc: usize) -> RunEnd {
        let ci = c as usize;
        for _ in 0..SCRIPT_GUARD {
            let op = self.fetch8(ci, &mut pc)?;
            match op {
                0x05..=0x07 | 0x0A => {
                    let rel = match op {
                        0x05 => self.fetch8(ci, &mut pc)? as i8 as i64,
                        0x06 => self.fetch16(ci, &mut pc)? as i16 as i64,
                        _ => {
                            let v = (self.fetch8(ci, &mut pc)? as u32) << 16 | self.fetch16(ci, &mut pc)? as u32;
                            ((v << 8) as i32 >> 8) as i64
                        }
                    };
                    if op != 0x0A {
                        let i = 0x5C + self.vm.sp;
                        if !(0..0x1FC).contains(&i) {
                            return None;
                        }
                        let i = i as usize;
                        self.vm.vars[i..i + 4].copy_from_slice(&(pc as u32).to_be_bytes());
                        self.vm.sp -= 4;
                    }
                    pc = jump(pc, rel)?;
                }
                0x11 => {
                    self.vm.sp += 4;
                    let i = 0x5C + self.vm.sp;
                    if !(0..0x1FC).contains(&i) {
                        return None;
                    }
                    pc = be32(&self.vm.vars, i as usize) as usize;
                }
                0x14 => {
                    let v = (self.fetch8(ci, &mut pc)? as u32) << 16 | self.fetch16(ci, &mut pc)? as u32;
                    if self.vm.x == 0 {
                        pc = jump(pc, ((v << 8) as i32 >> 8) as i64)?;
                    }
                }
                0x15 => {
                    let rel = self.fetch8(ci, &mut pc)? as i8 as i64;
                    if self.vm.x != 0 {
                        pc = jump(pc, rel)?;
                    }
                }
                0x1E => {
                    self.vm.y = self.vm.x;
                    self.eval(ci, &mut pc, true)?;
                    self.store(ci, &mut pc)?;
                }
                0x1F | 0xD5 => self.eval(ci, &mut pc, true)?,
                0x20 => {
                    self.vm.y = self.vm.x;
                    self.eval(ci, &mut pc, true)?;
                    if self.fetch8(ci, &mut pc)? != 0x14 {
                        return None;
                    }
                    let n = self.fetch8(ci, &mut pc)?;
                    let v = self.var16(n).wrapping_add(self.vm.x);
                    self.set16(n, v);
                }
                0x2E => {
                    self.eval(ci, &mut pc, false)?;
                    let count = self.fetch8(ci, &mut pc)? as usize + 1;
                    pc += pc & 1;
                    let x = self.vm.x as i16;
                    let mut i = 0;
                    let mut hit = None;
                    while i < count {
                        let v = self.fetch16(ci, &mut pc)? as i16;
                        let rel = self.fetch16(ci, &mut pc)? as i16 as i64;
                        i += 1;
                        if v == x {
                            hit = Some(jump(pc, rel)?);
                            break;
                        }
                        if x < v {
                            break;
                        }
                    }
                    pc = match hit {
                        Some(p) => p,
                        None => pc + 4 * (count - i),
                    };
                }
                0x2F => {
                    self.eval(ci, &mut pc, false)?;
                    let n = self.fetch8(ci, &mut pc)? as i32;
                    pc += pc & 1;
                    let base = self.fetch16(ci, &mut pc)?;
                    self.vm.x = self.vm.x.wrapping_add(base);
                    let x = self.vm.x as i16 as i32;
                    if x < 0 || x > n {
                        pc += 2 * n as usize + 2;
                    } else {
                        let e = pc + 2 * x as usize;
                        let rel = be16(&self.c[ci].d, e) as i16 as i64;
                        pc = jump(e + 2, rel)?;
                    }
                }
                0x3F => {
                    self.vm.vars[0x58] = 0;
                    return Some(pc);
                }
                0x40 => {
                    self.fetch16(ci, &mut pc)?;
                    self.vm.x = 0xFFFF;
                    self.store(ci, &mut pc)?;
                }
                0x42 => return Some(pc),
                0x95 => {
                    let mut e = [0u16; 6];
                    for v in e.iter_mut() {
                        self.eval(ci, &mut pc, false)?;
                        *v = self.vm.x;
                    }
                    let r = self.c[ci].res(e[0]);
                    if matches!(self.c[ci].rb(r), 0 | 3) {
                        self.stream = Some((c, r + 6));
                        self.tempo = e[2] as u8;
                    }
                }
                0x97 => {
                    self.eval(ci, &mut pc, true)?;
                    self.tempo = self.vm.x as u8;
                }
                0xA1 => {}
                0xCD => {
                    let mut e = [0u16; 3];
                    for v in e.iter_mut() {
                        self.eval(ci, &mut pc, true)?;
                        *v = self.vm.x;
                    }
                    let s = e[0] as usize & 31;
                    if (e[1] as i16) < 0 {
                        self.slots[s] = Slot::default();
                    } else {
                        let r = self.c[ci].res(e[1]);
                        if matches!(self.c[ci].rb(r), 1 | 2) {
                            self.slots[s] = Slot { smp: Some(Smp { c, r }), off: e[2] };
                        }
                    }
                }
                _ => return None,
            }
        }
        None
    }

    fn fetch8(&self, c: usize, pc: &mut usize) -> Option<u8> {
        let b = *self.c[c].d.get(*pc)?;
        *pc += 1;
        Some(b)
    }

    fn fetch16(&self, c: usize, pc: &mut usize) -> Option<u16> {
        let hi = self.fetch8(c, pc)?;
        let lo = self.fetch8(c, pc)?;
        Some(u16::from_be_bytes([hi, lo]))
    }

    /// An expression, its value left in X; with `copy`, X is first copied to Y.
    fn eval(&mut self, c: usize, pc: &mut usize, copy: bool) -> Option<()> {
        let op = self.fetch8(c, pc)?;
        if copy && op != 0x38 {
            self.vm.y = self.vm.x;
        }
        match op {
            0x00 => self.vm.x = self.fetch8(c, pc)? as i8 as i16 as u16,
            0x02 => self.vm.x = self.fetch16(c, pc)?,
            0x12 => {
                let n = self.fetch8(c, pc)?;
                self.vm.x = self.var8(n) as i8 as i16 as u16;
            }
            0x14 => {
                let n = self.fetch8(c, pc)?;
                self.vm.x = self.var16(n);
            }
            0x1E => {
                let o = self.fetch16(c, pc)? as i16 as isize;
                let a = (self.c[0].h as isize + o) as usize;
                self.vm.x = self.c[0].rb(a) as i8 as i16 as u16;
            }
            0x38 => loop {
                if *self.c[c].d.get(*pc)? == 0x3A {
                    *pc += 1;
                    break;
                }
                self.eval(c, pc, true)?;
            },
            0x4A | 0x4E | 0x56 | 0x58 => {
                self.eval(c, pc, true)?;
                let (y, x) = (self.vm.y, self.vm.x);
                self.vm.x = match op {
                    0x4A => if y == x { 0xFFFF } else { 0 },
                    0x4E => if (y as i16) <= (x as i16) { 0xFFFF } else { 0 },
                    0x56 => y.wrapping_add(x),
                    _ => y.wrapping_sub(x),
                };
            }
            _ => return None,
        }
        Some(())
    }

    /// Stores X at the destination that follows.
    fn store(&mut self, c: usize, pc: &mut usize) -> Option<()> {
        match self.fetch8(c, pc)? {
            0x12 => {
                let n = self.fetch8(c, pc)?;
                self.vm.vars[0x5C + n as usize] = self.vm.x as u8;
            }
            0x14 => {
                let n = self.fetch8(c, pc)?;
                self.set16(n, self.vm.x);
            }
            0x1E => {
                let o = self.fetch16(c, pc)? as i16 as isize;
                let a = (self.c[0].h as isize + o) as usize;
                if let Some(b) = self.c[0].d.get_mut(a) {
                    *b = self.vm.x as u8;
                }
            }
            _ => return None,
        }
        Some(())
    }

    // ---- the song

    /// The subsong start: the script runs (the companion's first, then the
    /// module's, which then continues where it ended), and the stream, tempo and
    /// patches are set up. Returns false when there is no stream.
    fn start_song(&mut self) -> bool {
        self.c[0].d.clone_from(&self.loaded.d);
        self.slots = self.loaded.slots;
        self.vm = self.loaded.vm.clone();
        self.tempo = self.loaded.tempo;
        self.stream = None;
        self.songend = false;
        let ok = if self.c.len() > 1 {
            self.vm.sp = -0x54;
            let p = self.c[1].script();
            self.run(1, p).is_some()
                && match self.run_main(self.sub) {
                    Some(end) => self.run(0, end).is_some(),
                    None => false,
                }
        } else {
            self.run_main(self.sub).is_some()
        };
        let Some((sc, mut start)) = self.stream.filter(|_| ok) else { return false };
        let d = &self.c[sc as usize].d;
        if self.sub == 2 && start >= 2 && be32(d, start - 2) == 0x03B0_FF02 {
            start += 0x3B0;
        }
        // the two streams that run off their end into the next resource
        for o in [0x3E78usize, 0x3410] {
            let w = be32(&self.c[0].d, o);
            if w == 0x9BFF_020A && o == 0x3E78 || w == 0xCFFF_020A && o == 0x3410 {
                self.c[0].d[o + 2] = 0;
            }
        }
        self.sc = sc;
        self.start = start;
        self.pos = start;
        self.step = ((self.tempo & 0x7F) as u16 * 8).wrapping_sub(if self.tf { 0 } else { 7 });
        self.counter = 0;
        self.cmd = 0;
        self.pend_dma = 0;
        self.pend_int = 0;
        self.ch = [Chan { slot: Slot::default(), irq: Irq::None }; 4];
        true
    }

    fn sb(&self, o: usize) -> u8 {
        self.c[self.sc as usize].rb(o)
    }

    /// The header values of a sample: SIZE, loop start A, first-play end B, and the
    /// guest address of its data.
    fn sample(&self, s: Smp) -> (u32, u32, u32, u32) {
        let c = &self.c[s.c as usize];
        (be32(&c.d, s.r + 2), be32(&c.d, s.r + 8), be32(&c.d, s.r + 0x0C), c.base.wrapping_add(s.r as u32 + 0x10))
    }

    // ---- timing and the audio interrupts

    /// Lets time run to `t` while the tick runs. An audio interrupt Paula raises on
    /// the way reaches the processor a little later (the tick's writes go on until
    /// then), which then serves it before the tick's next write.
    fn run_to(&mut self, hw: &mut Hw, t: u64) {
        for _ in 0..1000 {
            let now = hw.bus.clock;
            hw.bus.paula.sync(now, &hw.bus.ram);
            if self.irq_due.is_none() && hw.bus.paula.pending_audio_irqs() != 0 {
                self.irq_due = Some((now + T_IRQ_DELAY, T_IRQ_RAISE));
            }
            if let Some((d, entry)) = self.irq_due.filter(|&(d, _)| d <= t) {
                hw.bus.clock = now.max(d);
                self.irq_due = None;
                self.serve(hw, entry);
                continue;
            }
            if t <= now {
                return;
            }
            let r = hw.bus.paula.sync_to_irq(t, &hw.bus.ram);
            hw.bus.clock = r.max(now);
            if r >= t && hw.bus.paula.pending_audio_irqs() == 0 {
                hw.bus.clock = t;
                return;
            }
            if r >= t && self.irq_due.is_some() {
                hw.bus.clock = t;
                return;
            }
        }
    }

    /// Lets `dt` colour clocks pass in the tick.
    fn at(&mut self, hw: &mut Hw, dt: u64) {
        let t = hw.bus.clock + dt;
        self.run_to(hw, t);
    }

    /// A custom register write of the tick; an INTENA write that enables a pending
    /// request has it taken at once.
    fn w(&mut self, hw: &mut Hw, reg: u32, v: u16) {
        let now = hw.bus.clock;
        hw.bus.paula.sync(now, &hw.bus.ram);
        let before = hw.bus.paula.pending_audio_irqs();
        hw.bus.write_word(0xDFF000 + reg, v);
        if reg == 0x09A && hw.bus.paula.pending_audio_irqs() & !before != 0 {
            // taken after the write that follows, when that one comes at once
            let d = hw.bus.clock + T_IRQ_RECOGNISE;
            if self.irq_due.is_none_or(|(due, _)| due > d) {
                self.irq_due = Some((d, T_IRQ_ENABLE - T_IRQ_RECOGNISE));
            }
        }
    }

    /// The audio interrupt server, entered `entry` colour clocks before its first
    /// look at the requests: the pending channels in the order 1, 3, 0, 2 (each
    /// place in that order costs a test), until none is pending.
    fn serve(&mut self, hw: &mut Hw, entry: u64) {
        self.irq_due = None;
        let mut wait = entry;
        let mut any = false;
        for _ in 0..64 {
            hw.wait(wait);
            let now = hw.bus.clock;
            hw.bus.paula.sync(now, &hw.bus.ram);
            let p = hw.bus.paula.pending_audio_irqs();
            let Some(k) = SERVER_ORDER.iter().position(|&c| p & 0x80 << c != 0) else { break };
            hw.wait(k as u64 * T_IRQ_TEST);
            self.handler(SERVER_ORDER[k], hw);
            any = true;
            wait = T_IRQ_NEXT;
        }
        if any {
            hw.wait(T_IRQ_EXIT);
        }
    }

    fn handler(&mut self, ch: usize, hw: &mut Hw) {
        let ib = 0x80u16 << ch;
        let dma_on = hw.bus.paula.dmacon & (1 << ch) != 0;
        match self.ch[ch].irq {
            Irq::Loop(s) => {
                if dma_on {
                    hw.wait(T_IRQ_LOOP);
                    if let Some(s) = s {
                        let (size, a, b, addr) = self.sample(s);
                        let end = if b != 0 { b } else { size.wrapping_sub(0x10) };
                        hw.len(ch, (end.wrapping_sub(a) >> 1) as u16);
                        hw.wait(T_IRQ_LOOP_LC);
                        hw.loc(ch, addr.wrapping_add(a));
                        hw.wait(T_IRQ_GAP);
                    }
                    hw.bus.write_word(0xDFF09A, ib);
                    hw.wait(T_IRQ_GAP);
                }
            }
            Irq::OneShot => {
                hw.wait(T_IRQ_ONESHOT);
                if dma_on {
                    self.ch[ch].irq = Irq::Stop;
                }
            }
            Irq::Stop => {
                hw.wait(T_IRQ_STOP);
                hw.dmacon(1 << ch);
                hw.wait(T_IRQ_GAP);
                hw.bus.write_word(0xDFF09A, ib);
                hw.wait(T_IRQ_GAP);
            }
            Irq::None => {}
        }
        hw.bus.write_word(0xDFF09C, ib);
    }

    // ---- the tick

    fn tick_song(&mut self, hw: &mut Hw) {
        let t0 = hw.bus.clock;
        if self.counter > self.step {
            self.counter -= self.step;
            self.run_to(hw, t0 + T_IDLE);
        } else {
            self.counter = 0;
            self.run_to(hw, t0 + T_PARSE);
            self.parse(hw);
        }
        // the raster wait: a polling loop that counts line changes (an interrupt
        // taken meanwhile can hide some)
        let mut line = hw.bus.clock / LINE;
        for _ in 0..WAIT_LINES {
            self.run_to(hw, (line + 1) * LINE);
            line = hw.bus.clock / LINE;
        }
        self.at(hw, T_FLUSH);
        let (i, d) = (std::mem::take(&mut self.pend_int), std::mem::take(&mut self.pend_dma));
        self.w(hw, 0x09A, i);
        self.at(hw, T_FLUSH_DMA);
        self.w(hw, 0x096, d);
    }

    fn parse(&mut self, hw: &mut Hw) {
        for _ in 0..STREAM_GUARD {
            let b = self.sb(self.pos);
            let c;
            if (0x80..=0xFE).contains(&b) {
                self.pos += 1;
                self.counter = ((b - 0x80) as u16) << 8;
                self.at(hw, T_DELAY);
                return;
            } else if b == 0xFF {
                c = self.sb(self.pos + 1);
                self.pos += 2;
                self.cmd = c;
            } else {
                c = self.cmd;
            }
            let c = if c == 0xFF { 0 } else { c };
            let ch = (c >> 6) as usize;
            match c & 3 {
                0 => {
                    self.cmd = 0;
                    self.songend = true;
                    self.pos = self.start;
                    self.at(hw, T_SET);
                }
                2 => {
                    let s = self.sb(self.pos);
                    self.pos += 1;
                    if s < 32 {
                        self.ch[ch].slot = self.slots[s as usize];
                    }
                    self.at(hw, T_SET);
                }
                1 => {
                    let (n, v) = (self.sb(self.pos), self.sb(self.pos + 1));
                    self.pos += 2;
                    self.note(hw, ch, n, v);
                }
                _ => self.release(hw, ch),
            }
        }
    }

    fn note(&mut self, hw: &mut Hw, ch: usize, n: u8, v: u8) {
        let r = 0x0A0 + 0x10 * ch as u32;
        self.w(hw, 0x09A, INT_CLR[ch]);
        self.at(hw, 8);
        self.w(hw, 0x096, DMA_OFF[ch]);
        let slot = self.ch[ch].slot;
        let Some(s) = slot.smp.filter(|_| v != 0) else {
            // a note-off (or no sample): nothing more
            self.at(hw, T_NOTE_END);
            return;
        };
        let (size, a, b, addr) = self.sample(s);
        self.at(hw, 38);
        hw.loc(ch, addr);
        self.at(hw, if b != 0 { T_LEN_B } else { T_LEN_SIZE });
        self.w(hw, r + 4, ((if b != 0 { b } else { size.wrapping_sub(0x10) }) >> 1) as u16);
        let idx = (n as u16).wrapping_add(slot.off) as i16;
        self.at(hw, 22);
        self.w(hw, r + 6, PERIODS[idx.clamp(0, 127) as usize]);
        self.at(hw, T_VOL[ch]);
        self.w(hw, r + 8, (((v as u16) << 6) >> 6) & 0x7F);
        self.ch[ch].irq = if a == 0 { Irq::OneShot } else { Irq::Loop(Some(s)) };
        self.pend_dma |= DMA_ON[ch];
        self.pend_int |= INT_SET[ch];
        self.at(hw, T_NOTE_END + 10 * ch as u64);
    }

    fn release(&mut self, hw: &mut Hw, ch: usize) {
        let r = 0x0A0 + 0x10 * ch as u32;
        let Some(s) = self.ch[ch].slot.smp else {
            self.at(hw, T_SET);
            return;
        };
        let (size, _, b, addr) = self.sample(s);
        self.at(hw, T_REL);
        if b == 0 {
            self.w(hw, 0x096, DMA_OFF[ch]);
            self.at(hw, T_REL_END);
            return;
        }
        self.w(hw, 0x09A, INT_CLR[ch]);
        self.at(hw, 8);
        self.w(hw, 0x096, DMA_OFF[ch]);
        let d = size as i64 - 0x10 - b as i64;
        if d <= 0 || d >> 1 == 0 {
            self.at(hw, 44);
            self.w(hw, 0x096, DMA_OFF[ch]);
            self.at(hw, T_REL_END);
            return;
        }
        self.at(hw, 44);
        self.w(hw, r + 4, (d >> 1) as u16);
        self.at(hw, 12);
        hw.loc(ch, addr.wrapping_add(b));
        self.ch[ch].irq = Irq::OneShot;
        self.at(hw, 26);
        self.w(hw, 0x09A, INT_SET[ch]);
        self.at(hw, 8);
        self.w(hw, 0x096, DMA_ON[ch]);
        self.at(hw, T_REL_END);
    }
}

fn jump(pc: usize, rel: i64) -> Option<usize> {
    usize::try_from(pc as i64 + rel).ok()
}

impl Replayer for Silmarils {
    fn format(&self) -> &'static str {
        "Silmarils"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        (TIMER + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsongs - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.sub = n;
        self.started = false;
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.silent = !self.start_song();
            if self.silent {
                return;
            }
            hw.dmacon(0x000F);
            for ch in 0..4 {
                hw.vol(ch, 0);
            }
            for v in [0x8080, 0x8100, 0x8200, 0x8400] {
                hw.bus.write_word(0xDFF09A, v);
            }
        }
        if self.silent {
            return;
        }
        self.tick_song(hw);
    }

    /// Any channel's audio interrupt runs the player's server, which takes all the
    /// pending channels in its own order.
    fn audio_irq_ch(&mut self, _ch: usize, hw: &mut Hw) -> bool {
        let now = hw.bus.clock;
        hw.bus.paula.sync(now, &hw.bus.ram);
        if hw.bus.paula.pending_audio_irqs() != 0 {
            let (due, entry) = self.irq_due.take().unwrap_or((now + T_IRQ_DELAY, T_IRQ_RAISE));
            hw.wait(due.saturating_sub(now));
            self.serve(hw, entry);
        } else {
            self.irq_due = None;
        }
        true
    }
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

    /// A container with a script that picks one of two streams by the selector (a
    /// jump table), the tempo with it, and loads sample 0 into slot 0. Stream 1
    /// plays a note on channel 0, stream 2 one on channel 1; both then wait and
    /// end. The sample is a one-shot of 32 bytes; `extra` more empty samples follow
    /// (with fewer than three samples a module takes them from a companion).
    fn module_with(extra: usize) -> Vec<u8> {
        let mut d = vec![0u8; 0x140];
        put16(&mut d, 4, 0x16);
        put32(&mut d, 0x0E, 0x50);
        put16(&mut d, 0x12, 0x20);
        put16(&mut d, 0x16, 0x20);
        let script: &[u8] = &[
            0x2F, 0x14, 0x0C, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x11, // switch on SEL
            0x95, 0x00, 0x01, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // play 1, tempo 32
            0x0A, 0x00, 0x00, 0x0D, // jump to the common part
            0x95, 0x00, 0x02, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // play 2, tempo 64
            0xCD, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // slot 0 := sample 0, offset 0
            0x42, 0x42,
        ];
        d[0x18..0x18 + script.len()].copy_from_slice(script);
        // the directory
        put32(&mut d, 0x50, 0x58);
        put32(&mut d, 0x50 + 0x0C, 0x58);
        put16(&mut d, 0x50 + 0x10, 3 + extra as u16);
        let dt = 0x50 + 0x58;
        for (i, r) in [0xC0usize, 0xF0, 0x100, 0x120, 0x130].iter().take(3 + extra).enumerate() {
            put32(&mut d, dt + 4 * i, (r - (dt + 4 * i)) as u32);
        }
        // the sample
        d[0xC0] = 1;
        d[0xC1] = 0x0A;
        put32(&mut d, 0xC2, 0x10 + 32);
        for i in 0..32 {
            d[0xD0 + i] = (i * 8) as u8;
        }
        for r in [0x120usize, 0x130].into_iter().take(extra) {
            d[r] = 1;
            put32(&mut d, r + 2, 0x10);
        }
        // the streams (a stream starts with an instrument change on channel 0)
        put32(&mut d, 0xF2, 16);
        d[0xF6..0x100].copy_from_slice(&[0xFF, 0x02, 0x00, 0xFF, 0x01, 0x24, 0x30, 0x88, 0xFF, 0x00]);
        put32(&mut d, 0x102, 32);
        d[0x106..0x113].copy_from_slice(&[0xFF, 0x02, 0x00, 0xFF, 0x42, 0x00, 0xFF, 0x41, 0x24, 0x30, 0x88, 0xFF, 0x00]);
        d
    }

    fn module() -> Vec<u8> {
        module_with(2)
    }

    #[test]
    fn recognises_the_container() {
        let d = module();
        assert!(detect(&d));
        assert_eq!(scan(&d), Some((0, 2, 3)));
        // with a 6-byte prefix
        let mut p = vec![0u8; 6];
        p.extend_from_slice(&d);
        assert!(detect(&p));
        assert_eq!(load_base(&p), 6);
        // the 0x16 word, a non-zero header word, an odd directory offset, a directory
        // that does not start with 0x58, no music resource
        for (o, v) in [(4usize, 0x17u16), (8, 1), (0x10, 0x51), (0x52, 0x59)] {
            let mut e = d.clone();
            put16(&mut e, o, v);
            assert!(!detect(&e), "{o:x}");
        }
        let mut e = d.clone();
        put16(&mut e, 0xF6, 0xFF00);
        put16(&mut e, 0x106, 0xFF00);
        assert!(!detect(&e));
        assert!(!detect(&d[..0x20]));
    }

    #[test]
    fn companion_is_the_name_up_to_the_first_underscore() {
        assert_eq!(companion("/x/mok.BostonBombClub_title").as_deref(), Some("mok.BostonBombClub"));
        assert_eq!(companion("MOK.A_b_c").as_deref(), Some("MOK.A"));
        assert_eq!(companion("mok.Colorado"), None);
    }

    #[test]
    fn depacks_delta_samples() {
        let mut d = module();
        d[0xC6] = 1;
        // 32 bytes packed as 16: a start byte, then codes 8 (+1) and 15 (+34)
        d[0xD0] = 10;
        for i in 1..16 {
            d[0xD0 + i] = 0x8F;
        }
        let rest = d[0xE0..].to_vec();
        depack(&mut d);
        assert_eq!(be32(&d, 0xC2), 0x10 + 30);
        let mut acc = 10u8;
        let mut want = vec![acc];
        for _ in 1..16 {
            for delta in [1u8, 34] {
                acc = acc.wrapping_add(delta);
                want.push(acc);
            }
        }
        let n = want.len();
        want[n - 2] = 0;
        want[n - 1] = 0;
        assert_eq!(&d[0xD0..0xD0 + n], &want[..]);
        // past the depacked bytes the resource is as it was
        assert_eq!(&d[0xD0 + n..], &rest[n - 16..]);
    }

    fn run(sub: u16, ticks: usize) -> (Vec<(u32, u16)>, bool) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/mok-test/mok.test"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Silmarils");
        for _ in 0..sub {
            e.change_subsong(1).unwrap();
        }
        let mut out = vec![0f32; 2 * 960];
        let mut end = false;
        for _ in 0..ticks {
            e.render(&mut out);
            end |= e.songend();
        }
        (e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect(), end)
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_the_stream_the_script_picks() {
        let (log, _) = run(0, 3);
        // the start-up writes, in order
        let start = log.iter().position(|w| *w == (0xDFF096, 0x000F)).unwrap();
        assert_eq!(
            log[start..start + 9],
            [
                (0xDFF096, 0x000F),
                (0xDFF0A8, 0),
                (0xDFF0B8, 0),
                (0xDFF0C8, 0),
                (0xDFF0D8, 0),
                (0xDFF09A, 0x8080),
                (0xDFF09A, 0x8100),
                (0xDFF09A, 0x8200),
                (0xDFF09A, 0x8400)
            ]
        );
        // the note: the one-shot sample whole, the period of note 0x24, the velocity
        assert_eq!(reg(&log, 0xDFF0A4)[0], 16);
        assert_eq!(reg(&log, 0xDFF0A6)[0], PERIODS[0x24]);
        assert_eq!(reg(&log, 0xDFF0A8)[1], 0x30);
        // DMA and the interrupt are enabled at the tick's end
        assert!(reg(&log, 0xDFF096).contains(&0x8201));
        assert!(reg(&log, 0xDFF09A).contains(&0x8080));
        // nothing on channel 1
        assert!(reg(&log, 0xDFF0B4).is_empty());
        // the one-shot: DMA on at its start makes the handler the stopping one, which
        // turns the channel off at the end of the first play
        let dma = reg(&log, 0xDFF096);
        assert!(dma.iter().skip_while(|&&v| v != 0x8201).any(|&v| v == 0x0001));
    }

    #[test]
    fn the_selector_picks_the_second_stream_and_its_tempo() {
        let (log, _) = run(1, 3);
        assert_eq!(reg(&log, 0xDFF0B4)[0], 16);
        assert!(reg(&log, 0xDFF096).contains(&0x8202));
        assert!(reg(&log, 0xDFF0A4).is_empty());
    }

    #[test]
    fn song_end_after_the_delay() {
        // tempo 32, no note offsets: a step of 249 a tick, so the delay of 0x800
        // runs out in the tenth tick, which reads the end (a render is 4.3 ticks)
        let (_, end) = run(0, 1);
        assert!(!end);
        let (_, end) = run(0, 6);
        assert!(end);
    }

    #[test]
    fn a_script_that_aborts_does_not_load() {
        let mut d = module();
        d[0x18] = 0x01;
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        assert!(e.play_data(Path::new("/mok-test/mok.bad"), d).is_err() || e.playing.as_ref().unwrap().format != "Silmarils");
    }

    #[test]
    fn the_companion_is_loaded_and_its_samples_play() {
        // the module keeps its music only; the companion holds the sample, and its
        // script loads it into slot 0
        let comp = module();
        let mut d = module_with(0);
        // the module's own script: play stream 1 (no sample load); the run that
        // continues after it ends at once
        let script: &[u8] = &[0x95, 0x00, 0x01, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42];
        d[0x18..0x18 + script.len()].copy_from_slice(script);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.m.os.mem_files.insert(Path::new("/mok-test/mok.Game").to_path_buf(), comp);
        e.play_data(Path::new("/mok-test/mok.Game_title"), d).unwrap();
        let mut out = vec![0f32; 2 * 960 * 3];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        assert_eq!(reg(&log, 0xDFF0A4)[0], 16);
        // without the companion the module does not load
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        let r = e.play_data(Path::new("/mok-test/mok.Other_title"), module_with(0));
        assert!(r.is_err() || e.playing.as_ref().unwrap().format != "Silmarils");
    }
}

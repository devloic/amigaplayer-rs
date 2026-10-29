//! Core Design (`core.*`), the sound driver Core Design's programmers put in their games
//! (Action Fighter, Dynamite Dux, Monty Python's Flying Circus, Rick Dangerous, Saint and
//! Greavsie and the shared sound-effect modules), in three code families.
//!
//! Clean-room, written from a behavioural specification and the Paula register logs of
//! UADE's run of the Eagleplayer; no original code. A module is an AmigaDOS load file
//! with one code hunk that holds the driver's own copy of the replay code (which is not
//! run here), its tables, its song data and its 8-bit samples. The port finds the
//! tables by reading the driver's code structurally, with the decoder of `m68kdec`
//! (the subsong-start routine, the play routine's calls, the song-init and sequencer
//! routines' program-counter-relative address loads, the output routine's flag test and
//! the interrupt routine's shape), and refuses a module where that fails.
//!
//! The replayer is a tick (the play routine: an output step for the four channels, then
//! the sequencer of the three music tracks) and an audio interrupt routine, run when
//! Paula raises an enabled audio interrupt: the first output step after a note start
//! restarts the channel's DMA, and the interrupt that then comes (at the first line
//! after the DMA-on, or at once if its request is pending when it is enabled) loads
//! the channel's next block. The port takes the interrupts at the moments the repo's
//! Paula raises them, between its own writes.
//!
//! Decisions where the specification leaves the choice to the port:
//! * The host's volume and balance scaling is not applied (the defaults are the identity).
//! * The tick's writes are spaced by the median gaps of the specification's timing
//!   table; the interrupt routine's entry takes 190 colour clocks and its returns 40.
//!   The order of the writes does not depend on the spaces except where an interrupt
//!   falls near a write.
//! * A song that is not running because its end was reached restarts at once, as the
//!   specification says; the "stop" path (repeat 0, never taken with the host) is a
//!   plain stop of the four channels.
//! * Reads outside the image give 0.

use super::m68kdec::{decode, Ea, Insn, K};
use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Core Design", detect, new };

const HUNK_HEADER: u32 = 1011;
const HUNK_CODE: u32 = 1001;
const HUNK_RELOC32: u32 = 1004;
const HUNK_END: u32 = 1010;
const MAGIC: &[u8; 8] = b"S.PHIPPS";

const DMACON: u32 = 0xDFF096;
const INTENA: u32 = 0xDFF09A;
const INTREQ: u32 = 0xDFF09C;
const INTREQR: u32 = 0xDFF01E;

// ------------------------------------------------------------------ the container

/// The code hunk's data, if `d` is a load file of the shape the drivers have.
fn image(d: &[u8]) -> Option<&[u8]> {
    if d.len() < 100 || be32(d, 0) != HUNK_HEADER || be32(d, 4) != 0 || be32(d, 8) != 1 || be32(d, 12) != 0 || be32(d, 16) != 0 {
        return None;
    }
    if d[20] == 0 || be32(d, 24) != HUNK_CODE {
        return None;
    }
    let n = be32(d, 28) as usize;
    if n < 40 || be32(d, 20) & 0x3FFF_FFFF != n as u32 {
        return None;
    }
    let end = 32usize.checked_add(n.checked_mul(4)?)?;
    if end > d.len() {
        return None;
    }
    // the relocation table refers to the one hunk only
    let mut p = end;
    if be32(d, p) == HUNK_RELOC32 {
        p += 4;
        loop {
            let count = be32(d, p) as usize;
            p += 4;
            if count == 0 {
                break;
            }
            if be32(d, p) != 0 {
                return None;
            }
            p = p.checked_add(4 + 4 * count)?;
            if p > d.len() {
                return None;
            }
        }
    }
    if be32(d, p) != HUNK_END {
        return None;
    }
    let img = &d[32..end];
    // a "return -1" stub, the signature, and the header's routine addresses and counts
    let (a, b) = (decode(img, 0)?, decode(img, 2)?);
    if !(a.k == K::Moveq && a.data == -1 && a.dst == Ea::D(0) && b.k == K::Rts) || &img[4..12] != MAGIC {
        return None;
    }
    if (12..32).step_by(4).any(|o| be32(img, o) == 0) {
        return None;
    }
    Some(img)
}

// ------------------------------------------------------------------ the layout

#[derive(Clone, Copy, Debug, PartialEq)]
enum Family {
    /// Enabled flag bit 6, the interrupt routine walks the channels upward, reading
    /// INTREQR for each.
    A,
    /// Enabled flag bit 1, otherwise as A.
    R,
    /// As A, but the interrupt routine reads INTREQR once and walks downward.
    B,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sfx {
    /// The entry's channel selector; `skip0` keeps the round robin off channel 0.
    Selector { skip0: bool },
    /// Rick Dangerous: channel 3 for the host's call, the entry's field is the sustain.
    Channel3,
}

#[derive(Clone, Debug, PartialEq)]
struct Layout {
    family: Family,
    sfx: Sfx,
    /// An image byte that, when non-zero, stops the effects (Saint and Greavsie).
    disable: Option<usize>,
    subs: usize,
    sub: usize,
    seq: usize,
    pattab: usize,
    patbase: usize,
    per: usize,
    vib: usize,
    smp: usize,
    sil: usize,
    noteadd: u8,
}

/// The instructions from `from` to the first of kind `stop` (inclusive).
fn until(img: &[u8], from: usize, stop: K) -> Vec<Insn> {
    let mut v = Vec::new();
    let mut p = from;
    while v.len() < 4000 {
        let Some(i) = decode(img, p) else { break };
        p = i.next();
        let done = i.k == stop;
        v.push(i);
        if done {
            break;
        }
    }
    v
}

/// The instructions in `from..to`.
fn range(img: &[u8], from: usize, to: usize) -> Vec<Insn> {
    let mut v = Vec::new();
    let mut p = from;
    while p < to {
        let Some(i) = decode(img, p) else { break };
        p = i.next();
        v.push(i);
    }
    v
}

fn call_targets(v: &[Insn]) -> Vec<usize> {
    v.iter().filter(|i| i.k == K::Bsr).map(|i| i.target).collect()
}

/// An absolute address loaded into an address register.
fn abs_load(i: &Insn) -> Option<usize> {
    match (i.k, i.src, i.dst) {
        (K::Lea, Ea::AbsL { addr, .. }, Ea::A(_)) => Some(addr as usize),
        (K::Movea, Ea::Imm { val, .. }, Ea::A(_)) if i.size == 4 => Some(val as usize),
        _ => None,
    }
}

fn pc_loads(v: &[Insn]) -> Vec<usize> {
    v.iter()
        .filter_map(|i| match (i.k, i.src) {
            (K::Lea, Ea::Pc(t)) => Some(t),
            _ => None,
        })
        .collect()
}

/// The 32-bit immediates the code stores into a memory operand.
fn long_stores(v: &[Insn]) -> Vec<usize> {
    v.iter()
        .filter_map(|i| match (i.k, i.src, i.dst) {
            (K::Move, Ea::Imm { val, .. }, Ea::Disp(..)) if i.size == 4 => Some(val as usize),
            _ => None,
        })
        .collect()
}

fn locate(img: &[u8]) -> Option<Layout> {
    let h = |o: usize| be32(img, o) as usize;
    let (play, irq, start, subs, smp, smp_end) = (h(12), h(16), h(20), h(28), h(32), h(36));
    let n = img.len();
    if [play, irq, start, h(24)].iter().any(|&a| a < 68 || a >= n) || play <= start || subs == 0 || subs > 256 {
        return None;
    }
    if smp >= smp_end || smp_end > n || (smp_end - smp) % 14 != 0 {
        return None;
    }
    // the play routine: the output routine is its first call, the sequencer its last
    let calls = call_targets(&until(img, play, K::Rts));
    let (out_r, seq_r) = (*calls.first()?, *calls.last()?);
    if out_r >= n || seq_r >= n || out_r == seq_r {
        return None;
    }
    // the subsong-start routine runs up to the play routine
    let s = range(img, start, play);
    let loads: Vec<usize> = s.iter().filter_map(abs_load).collect();
    let (sub, chan) = (*loads.first()?, *loads.get(1)?);
    if sub >= n || subs * 8 + sub > n || chan <= seq_r || chan >= n {
        return None;
    }
    let init_r = *call_targets(&s).first()?;
    if init_r >= n {
        return None;
    }
    // the effect path: the branch of the "type is 1" test
    let cmp = s.iter().position(|i| i.k == K::Imm(6) && i.size == 2 && matches!(i.src, Ea::Imm { val: 1, .. }) && i.dst == Ea::Ind(0))?;
    let eff = s[cmp + 1..].iter().find(|i| i.k == K::Bcc && i.cond == 7)?.target;
    let disable = match decode(img, eff) {
        Some(Insn { k: K::Tst, size: 1, dst: Ea::AbsL { addr, .. }, .. }) => Some(addr as usize),
        _ => None,
    };
    let sfx = if s.iter().any(|i| i.k == K::Tst && i.size == 1 && i.dst == Ea::D(1)) {
        Sfx::Channel3
    } else {
        Sfx::Selector { skip0: s.iter().any(|i| i.k == K::Move && i.size == 1 && matches!(i.src, Ea::Imm { val: 1, .. }) && matches!(i.dst, Ea::D(_))) }
    };
    // song init: the program-counter-relative loads are the three track records, the
    // sequence start table, the sample table, ...
    let il = pc_loads(&until(img, init_r, K::Rts));
    if il.len() < 5 || il[4] != smp {
        return None;
    }
    let seq = il[3];
    // the sequencer, up to the channel records
    let sq = range(img, seq_r, chan);
    let sl = pc_loads(&sq);
    if sl.len() < 16 || sl[11] != sl[15] || sl[13] != smp || sl[8] != sl[12] {
        return None;
    }
    let (pattab, patbase, per, vib) = (sl[9], sl[10], sl[11], sl[14]);
    let adds: Vec<u8> = sq
        .iter()
        .filter_map(|i| match (i.k, i.src) {
            (K::Imm(3), Ea::Imm { val, .. }) if i.size == 1 => Some(val as u8),
            _ => None,
        })
        .collect();
    if adds.len() != 1 {
        return None;
    }
    // the silent block: stored by the output routine's release and by the interrupt routine
    let sil_out = long_stores(&range(img, out_r, per));
    let irqi = until(img, irq, K::Rte);
    let sil_irq = long_stores(&irqi);
    let sil = *sil_out.first()?;
    if sil_out.iter().chain(&sil_irq).any(|&v| v != sil) || sil_irq.is_empty() || sil + 256 > n || img[sil..sil + 256].iter().any(|&b| b != 0) {
        return None;
    }
    // the family
    let family = if irqi.iter().any(|i| i.k == K::Shift && !i.left && i.shk == 1 && i.data == 7) {
        Family::B
    } else {
        let bit = range(img, out_r, per).iter().find_map(|i| match (i.k, i.src, i.dst) {
            (K::Bit(0), Ea::Imm { val, .. }, Ea::Disp(_, 28)) => Some(val),
            _ => None,
        })?;
        match bit {
            6 => Family::A,
            1 => Family::R,
            _ => return None,
        }
    };
    if [seq, pattab, patbase, per, vib, sil].iter().any(|&a| a >= n) {
        return None;
    }
    Some(Layout { family, sfx, disable, subs, sub, seq, pattab, patbase, per, vib, smp, sil, noteadd: adds[0] })
}

fn detect(d: &[u8]) -> bool {
    image(d).is_some_and(|img| locate(img).is_some())
}

// ------------------------------------------------------------------ the player

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy, Debug, Default)]
struct Chan {
    per: u16,
    vol: u8,
    sustain: u16,
    /// The next block: module offset and length in words.
    lc: u32,
    len: u16,
    stage: Stage,
    entry: usize,
    flags: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct Track {
    seq_ptr: usize,
    pat_start: usize,
    pos: usize,
    entry: usize,
    repeat: u8,
    transpose: u8,
    ended: bool,
    dur: u8,
    active: u8,
    note: u8,
    vib_sel: u8,
    vib_ptr: usize,
    vib_delay: u8,
    vib_steps: u8,
    vib_flags: u8,
}

#[derive(Clone, Debug, Default)]
struct State {
    ch: [Chan; 4],
    tr: [Track; 3],
    playing: bool,
    repeat: u8,
    cur: u16,
    running: u8,
    rr: u8,
    map: [u8; 24],
    /// The DMA bits as the replayer last wrote them (what it reads back from DMACONR).
    dma: u16,
}

// The waits between writes, in colour clocks (the medians of the reference).
const T_PER: [u64; 4] = [44, 56, 68, 72];
const T_LCH: u64 = 10;
const T_LEN: u64 = 14;
const T_NEXT: u64 = 212;
const T_DMA: u64 = 46;
const T_INT: u64 = 18;
const T_DISABLED: u64 = 28;
const T_NS_DMA: u64 = 27;
const T_NS_NEXT: u64 = 380;
const T_NS_FIRST: u64 = 360;
const T_NS_ALONE: u64 = 640;
const T_ENTRY: u64 = 168;
const T_ENTRY_B: u64 = 110;
const T_SKIP: u64 = 26;
const T_SKIP_B: u64 = 14;
const T_WORK: u64 = 70;
const T_ACK_B: u64 = 60;
const T_LOOP_B: u64 = 52;
const T_EXIT: u64 = 40;
const T_READ: u64 = 26;
const T_ACK: u64 = 38;
const T_REQ: u64 = 8;

pub struct CoreDesign {
    lay: Layout,
    img: Vec<u8>,
    /// The guest address of image offset 0.
    base: u32,
    sub: u16,
    started: bool,
    songend: bool,
    st: State,
    /// The colour clocks to let pass before the next write.
    gap: u64,
}

impl CoreDesign {
    fn b(&self, o: usize) -> u8 {
        self.img.get(o).copied().unwrap_or(0)
    }

    fn w(&self, o: usize) -> u16 {
        be16(&self.img, o)
    }

    /// The word at a possibly negative offset.
    fn wi(&self, o: isize) -> u16 {
        if o < 0 {
            0
        } else {
            self.w(o as usize)
        }
    }

    fn per_at(&self, idx: i8) -> u16 {
        self.wi(self.lay.per as isize + 2 * idx as isize)
    }

    fn en_bit(&self) -> u8 {
        if self.lay.family == Family::R {
            0x02
        } else {
            0x40
        }
    }

    fn put(&mut self, hw: &mut Hw, reg: u32, v: u16) {
        let g = std::mem::take(&mut self.gap);
        self.adv(hw, g);
        hw.bus.write_word(reg, v);
    }

    /// Lets `clocks` pass, taking the audio interrupts Paula raises meanwhile: they run
    /// in the gap and delay what follows.
    fn adv(&mut self, hw: &mut Hw, clocks: u64) {
        let mut target = hw.bus.clock + clocks;
        while target > hw.bus.clock {
            let t = hw.bus.paula.sync_to_irq(target, &hw.bus.ram);
            hw.bus.clock = t;
            if t < target && hw.bus.paula.pending_audio_irqs() != 0 {
                let rest = target - t;
                self.handler(hw);
                target = hw.bus.clock + rest + T_EXIT;
            } else {
                hw.bus.clock = target;
                break;
            }
        }
    }

    /// An INTENA write that enables an interrupt that is already requested takes it at once.
    fn enable(&mut self, hw: &mut Hw, v: u16) {
        self.put(hw, INTENA, v);
        if v & 0x8000 != 0 && hw.bus.paula.pending_audio_irqs() != 0 {
            self.handler(hw);
            hw.wait(T_EXIT);
        }
    }

    fn guest(&self, off: u32) -> u32 {
        self.base.wrapping_add(off)
    }

    // ---- subsong start (5.1) and song init (5.2)

    fn start_subsong(&mut self, hw: &mut Hw, n: u16, r: u8, from_host: bool) {
        if !from_host {
            self.songend = true;
        }
        let e = self.lay.sub + 8 * n as usize;
        let (ty, a, b, c) = (self.w(e), self.w(e + 2), self.w(e + 4), self.w(e + 6));
        if ty != 1 {
            self.st.cur = n;
            self.st.repeat = r;
            self.song_init(hw, (a & 255) as usize);
            self.st.playing = true;
            return;
        }
        if self.st.playing || self.lay.disable.is_some_and(|o| self.b(o) != 0) {
            return;
        }
        let entry = self.lay.smp + 14 * a as usize;
        let (ch, sustain) = match self.lay.sfx {
            Sfx::Channel3 => {
                if r != 0 {
                    (3, c)
                } else {
                    let ch = (self.st.rr & 3) as usize;
                    self.st.rr = self.st.rr.wrapping_add(1);
                    if self.st.rr == 3 {
                        self.st.rr = 0;
                    }
                    (ch, c)
                }
            }
            Sfx::Selector { skip0 } => {
                let mut guard = 0;
                let r = r as usize & 3;
                let ch = match c {
                    0 => loop {
                        let ch = (self.st.rr & 3) as usize;
                        let mut next = (ch as u8 + 1) & 3;
                        if skip0 && next == 0 {
                            next = 1;
                        }
                        self.st.rr = self.st.rr & !3 | next;
                        guard += 1;
                        if self.st.rr >> (ch + 4) & 1 == 0 || guard > 8 {
                            break ch;
                        }
                    },
                    1 => {
                        self.st.rr |= 1 << (r + 4);
                        r
                    }
                    _ => {
                        self.st.rr &= !(1 << (r + 4));
                        r
                    }
                };
                (ch & 3, 0)
            }
        };
        self.note_start(hw, ch, entry, b, sustain);
        self.st.running = 0;
        self.st.playing = false;
    }

    fn song_init(&mut self, hw: &mut Hw, i: usize) {
        if self.lay.family == Family::R {
            self.put(hw, DMACON, 0x000E);
            self.gap = 10;
            self.put(hw, INTENA, 0x0700);
            self.st.dma &= !0xE;
            for c in 1..4 {
                self.st.ch[c].flags = 0;
            }
        } else {
            self.put(hw, DMACON, 0x000F);
            self.gap = 10;
            self.put(hw, INTENA, 0x0780);
            self.st.dma = 0;
            for c in 0..4 {
                self.st.ch[c].flags = 0;
            }
        }
        for t in 0..3 {
            let w = self.w(self.lay.seq + 6 * i + 2 * t) as i16 as isize;
            let tr = &mut self.st.tr[t];
            tr.seq_ptr = (self.lay.seq as isize + w).max(0) as usize;
            tr.transpose = 0;
            tr.vib_sel = 0;
            tr.ended = true;
            tr.repeat = 1;
            tr.dur = 1;
            tr.entry = self.lay.smp;
            tr.active = 255;
        }
        for k in 0..24 {
            self.st.map[k] = (14 * (k & 7)) as u8;
        }
        self.st.running = 255;
    }

    // ---- note start (6.4)

    fn note_start(&mut self, hw: &mut Hw, c: usize, entry: usize, period: u16, sustain: u16) {
        let bit = 1u16 << c;
        {
            let ch = &mut self.st.ch[c];
            ch.entry = entry;
            ch.per = period;
            ch.sustain = sustain;
            ch.vol = 0;
        }
        self.put(hw, 0xDFF0A8 + 16 * c as u32, 0);
        self.st.ch[c].stage = Stage::Attack;
        self.gap = T_NS_DMA;
        self.put(hw, DMACON, bit);
        self.st.dma &= !bit;
        self.gap = T_INT;
        self.put(hw, INTENA, 0x80 << c);
        let flags = self.en_bit() | 128 | self.b(entry + 5);
        let rec = be32(&self.img, entry + 6) as usize;
        let mut len = self.w(rec);
        if self.lay.family != Family::R && flags & 2 != 0 {
            len = self.w(entry + 10);
        }
        let ch = &mut self.st.ch[c];
        ch.flags = flags;
        ch.lc = rec as u32 + 2;
        ch.len = len;
        self.gap = T_NS_NEXT;
    }

    // ---- the tick

    fn play(&mut self, hw: &mut Hw) {
        // the tick interrupt's own work, up to the first output write
        self.gap = match self.lay.family {
            Family::A => 258,
            Family::B => 246,
            Family::R => 257,
        };
        self.output_all(hw);
        if !self.st.playing {
            return;
        }
        if self.st.running == 0 {
            if self.st.repeat == 0 {
                self.stop(hw);
                return;
            }
            self.start_subsong(hw, self.st.cur, 1, false);
        }
        if self.st.running == 0 {
            return;
        }
        self.st.running = self.st.tr.iter().fold(0, |a, t| a | t.active);
        if self.st.running == 0 {
            return;
        }
        for t in 0..3 {
            self.track(hw, t);
        }
    }

    fn stop(&mut self, hw: &mut Hw) {
        self.put(hw, DMACON, 0x000F);
        self.put(hw, INTENA, 0x0780);
        for c in 0..4 {
            self.put(hw, 0xDFF0A8 + 16 * c as u32, 0);
        }
        self.st.dma = 0;
        self.st.playing = false;
    }

    fn output_all(&mut self, hw: &mut Hw) {
        let mut wrote = false;
        for c in 0..4 {
            wrote |= self.output(hw, c);
        }
        self.gap = if wrote { T_NS_FIRST } else { T_NS_ALONE };
    }

    /// The output step (6.3) of channel `c`; whether it wrote.
    fn output(&mut self, hw: &mut Hw, c: usize) -> bool {
        if self.st.ch[c].flags & self.en_bit() == 0 {
            self.gap += T_DISABLED;
            return false;
        }
        let r = 0xDFF0A0 + 16 * c as u32;
        // each value is read as it is written: an interrupt in between may change it
        self.put(hw, r + 8, (self.st.ch[c].vol >> 1) as u16);
        self.gap = T_PER[c];
        self.put(hw, r + 6, self.st.ch[c].per);
        self.gap = T_LCH;
        let a = self.guest(self.st.ch[c].lc);
        self.put(hw, r, (a >> 16) as u16);
        let a = self.guest(self.st.ch[c].lc);
        self.put(hw, r + 2, a as u16);
        self.gap = T_LEN;
        self.put(hw, r + 4, self.st.ch[c].len);
        self.gap = T_NEXT;
        let bit = 1u16 << c;
        if self.st.dma & bit == 0 {
            self.st.ch[c].flags &= !0x80;
            self.gap = T_DMA;
            self.put(hw, DMACON, 0x8000 | bit);
            self.st.dma |= bit;
            self.gap = T_INT;
            self.enable(hw, 0x8000 | 0x80 << c);
            self.gap = T_NEXT - 20;
        }
        let ch = &mut self.st.ch[c];
        if ch.sustain != 0 && ch.sustain != 65535 {
            ch.sustain -= 1;
        }
        self.envelope(hw, c);
        true
    }

    fn envelope(&mut self, hw: &mut Hw, c: usize) {
        let e = self.st.ch[c].entry;
        let (e0, e1, e2, e3, e4) = (self.b(e), self.b(e + 1), self.b(e + 2), self.b(e + 3), self.b(e + 4));
        let ch = &mut self.st.ch[c];
        match ch.stage {
            Stage::Attack => {
                ch.vol = ch.vol.wrapping_add(e0);
                if ch.vol >= 128 || (e4 as i8) <= ch.vol as i8 {
                    ch.vol = e4;
                    ch.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                ch.vol = ch.vol.wrapping_add(e1);
                if ch.vol >= 128 || (e2 as i8) >= ch.vol as i8 {
                    ch.vol = e2;
                    ch.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                if ch.sustain == 0 {
                    ch.stage = Stage::Release;
                }
            }
            Stage::Release => {
                ch.vol = ch.vol.wrapping_add(e3);
                if ch.vol >= 128 && ch.flags & 1 == 0 {
                    ch.flags = 0;
                    let sil = self.guest(self.lay.sil as u32);
                    let r = 0xDFF0A0 + 16 * c as u32;
                    self.gap = 20;
                    self.put(hw, r + 4, 128);
                    self.put(hw, r, (sil >> 16) as u16);
                    self.put(hw, r + 2, sil as u16);
                }
            }
        }
    }

    // ---- the sequencer (6.2)

    fn track(&mut self, hw: &mut Hw, t: usize) {
        self.vibrato(t);
        let mut p = self.st.tr[t].pos;
        if self.st.tr[t].ended {
            if !self.seq_step(t) {
                return;
            }
            p = self.st.tr[t].pat_start;
        }
        self.pattern_step(hw, t, p);
    }

    /// Returns false when the track ended.
    fn seq_step(&mut self, t: usize) -> bool {
        let tr = &mut self.st.tr[t];
        tr.repeat = tr.repeat.wrapping_sub(1);
        if tr.repeat != 0 {
            tr.pos = tr.pat_start;
            tr.ended = false;
            return true;
        }
        tr.repeat = 1;
        loop {
            let p = self.st.tr[t].seq_ptr;
            let x = self.b(p);
            match x {
                0..=127 => {
                    let off = self.wi(self.lay.pattab as isize + 2 * x as isize) as i16 as isize;
                    let tr = &mut self.st.tr[t];
                    tr.ended = false;
                    tr.seq_ptr = p + 1;
                    tr.pat_start = (self.lay.patbase as isize + off).max(0) as usize;
                    return true;
                }
                128..=191 => {
                    let tr = &mut self.st.tr[t];
                    tr.repeat = x & 31;
                    tr.seq_ptr = p + 1;
                }
                192..=253 => {
                    let v = self.b(p + 1);
                    self.st.map[8 * t + (x & 7) as usize] = v;
                    self.st.tr[t].seq_ptr = p + 2;
                }
                254 => {
                    let v = self.b(p + 1);
                    let tr = &mut self.st.tr[t];
                    tr.transpose = v;
                    tr.seq_ptr = p + 2;
                }
                255 => {
                    self.st.tr[t].active = 0;
                    return false;
                }
            }
        }
    }

    /// Executes the command at `p`; returns the position after it.
    fn command(&mut self, t: usize, p: usize) -> usize {
        let x = self.b(p);
        match x {
            128..=136 => {
                let m = self.st.map[8 * t + (x & 7) as usize];
                self.st.tr[t].entry = self.lay.smp + m as usize;
                p + 1
            }
            137..=191 => {
                self.st.tr[t].vib_sel = x & 15;
                p + 1
            }
            194 => p + 1,
            255 => {
                self.st.tr[t].ended = true;
                p + 1
            }
            _ => p + 4,
        }
    }

    fn pattern_step(&mut self, hw: &mut Hw, t: usize, mut p: usize) {
        'again: loop {
            let tr = &mut self.st.tr[t];
            tr.dur = tr.dur.wrapping_sub(1);
            if tr.dur != 0 {
                if self.b(p) >= 128 {
                    p = self.command(t, p);
                }
                self.st.tr[t].pos = p;
                return;
            }
            while self.b(p) >= 128 {
                let x = self.b(p);
                p = self.command(t, p);
                if x == 255 {
                    self.st.tr[t].pos = p;
                    if !self.seq_step(t) {
                        return;
                    }
                    p = self.st.tr[t].pat_start;
                    continue 'again;
                }
            }
            self.note(hw, t, p);
            return;
        }
    }

    fn note(&mut self, hw: &mut Hw, t: usize, p: usize) {
        let n = self.b(p);
        if n == 127 {
            let tr = &mut self.st.tr[t];
            tr.dur = self.img.get(p + 1).copied().unwrap_or(0);
            tr.pos = p + 2;
            return;
        }
        let (period, d, next) = if n == 126 {
            (self.b(p + 1) as u16 + 256 * self.b(p + 2) as u16, self.b(p + 3), p + 4)
        } else {
            let tr = &mut self.st.tr[t];
            tr.note = n.wrapping_add(tr.transpose).wrapping_add(self.lay.noteadd);
            let note = tr.note as i8;
            (self.per_at(note), self.b(p + 1), p + 2)
        };
        let tr = &mut self.st.tr[t];
        tr.vib_flags = tr.vib_sel | 192;
        tr.dur = d;
        tr.pos = next;
        let entry = tr.entry;
        self.note_start(hw, t + 1, entry, period, d as u16);
    }

    fn vibrato(&mut self, t: usize) {
        let tr = self.st.tr[t];
        let mut f = tr.vib_flags;
        if f & 0x80 == 0 {
            return;
        }
        if f & 63 == 0 {
            self.st.tr[t].vib_flags = f & 0x7F;
            return;
        }
        if f & 0x40 == 0 {
            let tr = &mut self.st.tr[t];
            tr.vib_delay = tr.vib_delay.wrapping_sub(1);
            if tr.vib_delay != 0 {
                return;
            }
            tr.vib_steps = tr.vib_steps.wrapping_sub(1);
            let (steps, ptr, note) = (tr.vib_steps, tr.vib_ptr, tr.note);
            if steps != 0 {
                let s = self.b(ptr);
                let idx = ((s >> 3) & 31).wrapping_add(note) as i8;
                let per = self.per_at(idx);
                let tr = &mut self.st.tr[t];
                tr.vib_ptr += 1;
                tr.vib_delay = s & 7;
                self.st.ch[t + 1].per = per;
                return;
            }
        }
        let tb = self.lay.vib + 8 * (f & 31) as usize;
        let h = self.b(tb);
        if h & 0x80 == 0 && f & 0x40 == 0 {
            self.st.tr[t].vib_steps = 1;
            return;
        }
        f &= !0x40;
        let per = self.per_at(tr.note as i8);
        let tr = &mut self.st.tr[t];
        tr.vib_flags = f;
        tr.vib_delay = (h >> 3) & 7;
        tr.vib_steps = (h & 7) + 1;
        tr.vib_ptr = tb + 1;
        self.st.ch[t + 1].per = per;
    }

    // ---- the audio interrupt routine (6.5)

    fn requests(&self, hw: &mut Hw) -> u16 {
        (hw.bus.read_word(INTREQR) >> 7) & 0xF
    }

    fn handler(&mut self, hw: &mut Hw) {
        let fam = self.lay.family;
        let by_one = fam == Family::B;
        hw.wait(if by_one { T_ENTRY_B } else { T_ENTRY });
        let order: [usize; 4] = if by_one { [3, 2, 1, 0] } else { [0, 1, 2, 3] };
        // family B reads INTREQR once, the others for each channel
        let mut req = self.requests(hw);
        let mut first = true;
        for c in order {
            if !by_one {
                hw.wait(T_READ);
                req = self.requests(hw);
            }
            if req >> c & 1 == 0 {
                hw.wait(if by_one { T_SKIP_B } else { T_SKIP });
                continue;
            }
            let ch = self.st.ch[c];
            let (busy_clear, armed) = (ch.flags & 0x80 == 0, ch.flags & self.en_bit() != 0);
            if busy_clear && armed {
                self.st.ch[c].flags |= 0x80;
                hw.wait(T_WORK);
                let fl = self.st.ch[c].flags;
                let e = ch.entry;
                let silent = |s: &mut Self| {
                    s.st.ch[c].len = 128;
                    s.st.ch[c].lc = s.lay.sil as u32;
                };
                let looped = |s: &mut Self| {
                    let rec = be32(&s.img, e + 6) as isize;
                    let off = s.w(e + 10) as i16 as isize;
                    s.st.ch[c].lc = (rec + 2 + off).max(0) as u32;
                    s.st.ch[c].len = s.w(e + 12);
                };
                if fam == Family::R {
                    if fl & 1 != 0 {
                        silent(self);
                    } else {
                        looped(self);
                    }
                } else if fl & 2 != 0 {
                    looped(self);
                } else if fl & 1 != 0 {
                    silent(self);
                }
            }
            self.gap = if !by_one { T_ACK } else if first { T_ACK_B } else { T_LOOP_B };
            first = false;
            self.put_plain(hw, INTENA, 0x80 << c);
            self.gap = T_REQ;
            self.put_plain(hw, INTREQ, 0x80 << c);
        }
    }

    /// A write inside the interrupt routine: no nested interrupt.
    fn put_plain(&mut self, hw: &mut Hw, reg: u32, v: u16) {
        let g = std::mem::take(&mut self.gap);
        hw.wait(g);
        hw.bus.write_word(reg, v);
    }

    fn reset(&mut self) {
        self.st = State::default();
        self.started = false;
        self.songend = false;
        self.gap = 0;
    }
}

impl Replayer for CoreDesign {
    fn format(&self) -> &'static str {
        "Core Design"
    }

    fn title(&self) -> String {
        let o = be32(&self.img, 40) as usize;
        self.img.get(o..).map_or(String::new(), |s| {
            let e = s.iter().position(|&b| b == 0).unwrap_or(0).min(60);
            String::from_utf8_lossy(&s[..e]).trim().to_string()
        })
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.lay.subs as u16 - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (n as usize) < self.lay.subs {
            self.reset();
            self.sub = n;
        }
    }

    fn audio_irq_ch(&mut self, _ch: usize, hw: &mut Hw) -> bool {
        if self.started && hw.bus.paula.pending_audio_irqs() != 0 {
            self.handler(hw);
        }
        true
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.gap = 0;
            // the host's volume reset, the subsong start, the interrupt vectors
            self.put(hw, 0xDFF0A8, 0);
            self.gap = 10;
            self.start_subsong(hw, self.sub, 1, true);
            for c in 0..4 {
                self.gap = 180;
                self.enable(hw, 0x8000 | 0x80 << c);
            }
        }
        self.play(hw);
    }
}

fn new(d: &[u8], base: u32, _m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let img = image(d)?;
    let lay = locate(img)?;
    Some(Box::new(CoreDesign { lay, img: img.to_vec(), base: base + 32, sub: 0, started: false, songend: false, st: State::default(), gap: 0 }))
}

#[cfg(test)]
#[path = "coredesign_tests.rs"]
mod tests;

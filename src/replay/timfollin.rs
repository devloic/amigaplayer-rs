//! Tim Follin II (`tf.*`), the music driver Tim Follin (with Mike Delaney) wrote for
//! the Amiga conversions of Ghouls'n'Ghosts, Sly Spy - Secret Agent and Puzznic.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes and audio DMA fetches of UADE's Tim Follin
//! player and the structure of the corpus files), and this player was written from
//! it alone and checked against those writes.
//!
//! A `tf.*` file is a raw memory dump of the game's music data: a song table of four
//! sequence pointers a song, a table of instrument tables, song types, and the
//! sequences and instrument records (a sample length, a header, the sample) they
//! point at, all with absolute pointers to the game's original load address. The
//! replay routine is not in the file. Two builds: the game's music module with a
//! table of jumps in front (Puzznic) and an Atari-ST-style program header (Ghouls'n'
//! Ghosts, Sly Spy). Four voices, one Paula channel each, 50 Hz; byte-coded sequences
//! of notes and 19 commands (instrument, call / return / jump, repeat, envelope,
//! glide, trill, vibrato, transposition, fixed duration, retrigger flags, pulse-width
//! modulation, end of voice). The pulse-width modulation rewrites the bytes of the
//! sample Paula plays, so the samples are played from the module's copy in guest
//! memory and rewritten there.
//!
//! The player keeps its call stacks, the song number, the fade byte, the DMA masks,
//! the register addresses and the period table in one work area, and its stacks are
//! not bounds-checked: a song that calls without returning (Puzznic song 5) spills
//! into the next voice's stack and, after some six minutes, into the song number and
//! the fade byte. The port keeps that area with the same layout so that the spill
//! does what it did in the original, and ends the song when the fade it starts runs
//! out (the original crashes there).

use super::{be16, be32, Hw, Open, Replayer, WriteGaps};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Tim Follin II", detect, new };

/// The address the reference host loads a module at. Pointers are relocated for it
/// and every position (sequence, call stack entries) is kept in its address space,
/// so that the stack spill writes the same values into the song number and the fade
/// byte as in the reference runs; guest addresses are formed only for memory
/// accesses and Paula's location registers.
const VIRT: u32 = 0x10000;

/// The jump-table build's data block (file offset).
const JT_BASE: usize = 0x432;
/// Data block offsets.
const SONGS: u32 = 0x004;
const INSTABS: u32 = 0x104;
const TYPES: u32 = 0x144;
/// Instrument record: the sample follows the length word and a 0x30-byte header.
const SAMPLE: u32 = 0x32;
/// Song 0's sequence, the header build's anchor (instrument 4, note 0x32 for 2 ticks, end).
const ANCHOR: [u8; 6] = [0x81, 0x04, 0x32, 0x02, 0x8F, 0x00];

/// Work-area layout (specification, 7.1).
const W_SONG: usize = 0x44;
const W_TYPE: usize = 0x48;
const W_FADE: usize = 0x4B;
const W_OFF: usize = 0x4E;
const W_ON: usize = 0x56;
const W_REGS: usize = 0x5E;
const W_PERIODS: usize = 0xA0;
const W_SIZE: usize = W_PERIODS + 0x100;

/// The period table, entries 0..96 (specification, `tf-periods.tsv`). The words the
/// original reads past it (indices 97..127) are its own memory; no corpus run reads
/// them, and the port reads 0 there.
const PERIODS: [u16; 97] = [
    3822, 3608, 3405, 3214, 3034, 2863, 2703, 2551, 2408, 2273, 2145, 2025, 1911, 1804, 1703, 1607, 1517, 1432,
    1351, 1276, 1204, 1136, 1073, 1012, 956, 902, 851, 804, 758, 716, 676, 638, 602, 568, 536, 506, 478, 451, 426,
    402, 379, 358, 338, 319, 301, 284, 268, 253, 239, 225, 213, 201, 190, 179, 169, 159, 150, 142, 134, 127, 119,
    113, 106, 100, 95, 89, 84, 80, 75, 71, 67, 63, 60, 56, 53, 50, 47, 45, 42, 40, 38, 36, 34, 32, 30, 28, 27,
    25, 24, 22, 21, 20, 19, 18, 17, 16, 8,
];

/// Where the writes of a tick fall in the reference runs (ten files), by the previous write (see
/// `WriteGaps`): the voice work between two writes. After the DMA is switched on the routine waits
/// ten raster lines (see `play`).
static GAPS: WriteGaps = WriteGaps::new(
    &[
        (0, 4, 25), // LOC0 -> LEN0 x690
        (1, 5, 25), // LOC1 -> LEN1 x551
        (2, 6, 25), // LOC2 -> LEN2 x757
        (3, 7, 25), // LOC3 -> LEN3 x496
        (4, 1, 60), // LEN0 -> LOC1 x296
        (4, 2, 96), // LEN0 -> LOC2 x162
        (4, 3, 131), // LEN0 -> LOC3 x5
        (4, 12, 139), // LEN0 -> VOL0 x81
        (5, 2, 60), // LEN1 -> LOC2 x192
        (5, 3, 80), // LEN1 -> LOC3 x48
        (5, 12, 111), // LEN1 -> VOL0 x131
        (6, 3, 60), // LEN2 -> LOC3 x145
        (6, 12, 84), // LEN2 -> VOL0 x118
        (7, 12, 56), // LEN3 -> VOL0 x122
        (8, 13, 50), // PER0 -> VOL1 x1226
        (9, 14, 50), // PER1 -> VOL2 x915
        (10, 15, 50), // PER2 -> VOL3 x703
        (11, 0, 75), // PER3 -> LOC0 x131
        (11, 1, 110), // PER3 -> LOC1 x8
        (11, 2, 146), // PER3 -> LOC2 x90
        (11, 3, 181), // PER3 -> LOC3 x229
        (11, 20, 50), // PER3 -> DMA_ON x152
        (12, 8, 37), // VOL0 -> PER0 x1226
        (12, 13, 65), // VOL0 -> VOL1 x3734
        (13, 9, 37), // VOL1 -> PER1 x915
        (13, 14, 65), // VOL1 -> VOL2 x4045
        (14, 10, 37), // VOL2 -> PER2 x703
        (14, 15, 65), // VOL2 -> VOL3 x4257
        (15, 0, 89), // VOL3 -> LOC0 x67
        (15, 1, 125), // VOL3 -> LOC1 x55
        (15, 2, 160), // VOL3 -> LOC2 x162
        (15, 11, 37), // VOL3 -> PER3 x930
        (15, 20, 65), // VOL3 -> DMA_ON x300
        (16, 0, 55), // DMA_OFF -> LOC0 x218
        (16, 1, 83), // DMA_OFF -> LOC1 x106
        (16, 2, 110), // DMA_OFF -> LOC2 x92
        (16, 3, 138), // DMA_OFF -> LOC3 x36
        (20, 0, 0),
        (20, 1, 0),
        (20, 2, 0),
        (20, 3, 0),
    ],
    &[(0, 75), (1, 60), (2, 110), (3, 181), (4, 25), (5, 25), (6, 25), (7, 25), (8, 37), (9, 37), (10, 37), (11, 37), (12, 84), (13, 65), (14, 65), (15, 65), (20, 65)],
    &[(12, 985), (13, 985), (14, 985), (15, 985), (16, 2029)],
);

/// Colour clocks from a voice's end to its DMA off (the end-of-voice command).
const T_OFFV: u64 = 54;

/// Bytes a parse may read before the port gives the voice up (a sequence of
/// commands that never reaches a note).
const PARSE_LIMIT: usize = 1 << 16;

// ---------------------------------------------------------------- recognition

/// Where the data block is and how far the pointers are off (for `VIRT`); and the
/// number of subsongs as the reference host offers them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lay {
    base: usize,
    dx: u32,
    subs: u16,
}

fn layout(d: &[u8]) -> Option<Lay> {
    if d.len() < 0x40 {
        return None;
    }
    if be16(d, 0) == 0x4EF9 {
        // the jump-table build (specification, 2.1)
        if ![0, 6, 0xC, 0x12, 0x18, 0x1E].iter().all(|&o| be16(d, o) == 0x4EF9) {
            return None;
        }
        if be32(d, 0x2C).wrapping_sub(be32(d, 2)) != 0xA0 || d.len() < JT_BASE + 0x194 {
            return None;
        }
        let subs = (0..15).filter(|k| be16(d, 6 * k) == 0x4EF9).count() as u16;
        return Some(Lay { base: JT_BASE, dx: VIRT.wrapping_sub(be32(d, 2)), subs });
    }
    // the header build
    let at = if be16(d, 0) == 0x601A { 0x1C } else { 0 };
    if !matches!(be16(d, at), 0x1010 | 0x1012) {
        return None;
    }
    let b = at + 2;
    if d.len() < b + 0x194 {
        return None;
    }
    let s0 = be32(d, b + 4);
    if ![8, 0xC, 0x10].iter().all(|&o| be32(d, b + o) == s0) {
        return None;
    }
    let m = (b..d.len().saturating_sub(5)).step_by(2).find(|&o| d[o..o + 6] == ANCHOR)?;
    Some(Lay { base: b, dx: (VIRT + m as u32).wrapping_sub(s0), subs: 1 })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

// -------------------------------------------------------------------- engine

#[derive(Clone, Copy, Default)]
struct Voice {
    active: u8,
    seq: u32,
    lp: u32,
    loopcnt: u8,
    /// Offset of the call-stack pointer in the work area.
    sp: u32,
    insno: u8,
    ins: u32,
    flags: u8,
    dur: u8,
    fixdur: u8,
    transp: u8,
    skiptr: u8,
    note: u8,
    period: u16,
    vol: u8,
    env_on: u8,
    env_vol0: u8,
    env_sus: u8,
    env_att: u8,
    env_dec: u8,
    env_phase0: u8,
    env_step: u8,
    env_phase: u8,
    env_cnt: u8,
    glide_spd: u8,
    glide_tgt: u8,
    tr_int: u8,
    tr_ta: u8,
    tr_tb: u8,
    tr_cnt: u8,
    vib_delay: u8,
    vib_step: u8,
    vib_len: u8,
    vib_dir0: u8,
    vib_dcnt: u8,
    vib_cnt: u8,
    dir: u8,
    pwm_speed: u8,
    pwm_cnt: u8,
    pwm_pos: u8,
    pwm_dir: u8,
    /// This tick's writes, buffered until the pass over the voices is done.
    /// The restart is armed: the tick before the note ends (duration counter 1), bit 7 of the flags was set.
    arm: bool,
    o_vol: Option<u16>,
    o_per: Option<u16>,
    o_trig: Option<(u32, u16)>,
    o_loop: Option<(u32, u16)>,
}

pub struct TimFollin {
    lay: Lay,
    /// Where file offset 0 lies in guest memory, and the file's length.
    gbase: u32,
    len: u32,
    sub: u16,
    started: bool,
    /// The colour clock of the first tick, and how many have run.
    t_first: u64,
    n_ticks: u32,
    /// The fade ran out: the original crashes here; the port stops.
    dead: bool,
    songend: bool,
    work: [u8; W_SIZE],
    v: [Voice; 4],
    /// The voices whose sample restarts this tick (DMA off and on in one batch).
    mask: u8,
    #[cfg(test)]
    ticks: u32,
}

/// Every byte the replayer stores into the module (address for `VIRT`, byte, tick),
/// for the reference check of the audio DMA fetches (`fetch_dump`).
#[cfg(test)]
static STORES: std::sync::Mutex<Vec<(u32, u8, u32)>> = std::sync::Mutex::new(Vec::new());

fn work_area() -> [u8; W_SIZE] {
    let mut w = [0u8; W_SIZE];
    let put16 = |w: &mut [u8; W_SIZE], o: usize, v: u16| w[o..o + 2].copy_from_slice(&v.to_be_bytes());
    for ch in 0..4 {
        put16(&mut w, W_OFF + 2 * ch, 1 << ch);
        put16(&mut w, W_ON + 2 * ch, 0x8200 | 1 << ch);
        // PER, LEN, LC, VOL of each channel
        for (k, r) in [6u32, 4, 0, 8].iter().enumerate() {
            let a = 0xDFF0A0 + 16 * ch as u32 + r;
            w[W_REGS + 16 * k + 4 * ch..][..4].copy_from_slice(&a.to_be_bytes());
        }
    }
    put16(&mut w, 0x9E, 0x0FFF);
    for (i, &p) in PERIODS.iter().enumerate() {
        put16(&mut w, W_PERIODS + 2 * i, p);
    }
    w
}

impl TimFollin {
    /// The guest address of `VIRT`-space address `a`, if it lies in the module.
    fn guest(&self, a: u32) -> Option<u32> {
        let o = a.wrapping_sub(VIRT);
        (o < self.len).then(|| self.gbase.wrapping_add(o))
    }

    fn b(&self, hw: &Hw, a: u32) -> u8 {
        self.guest(a).map_or(0, |g| hw.bus.r8(g))
    }

    fn l(&self, hw: &Hw, a: u32) -> u32 {
        (0..4).fold(0, |x, k| x << 8 | self.b(hw, a.wrapping_add(k)) as u32)
    }

    /// A store into the module (outside it: dropped), with Paula first run up to now so
    /// that it fetches the old bytes up to this moment and the new ones after it.
    fn store(&mut self, hw: &mut Hw, a: u32, x: u8) {
        if let Some(g) = self.guest(a) {
            let b = &mut *hw.bus;
            b.paula.sync(b.clock, &b.ram);
            b.w8(g, x);
            #[cfg(test)]
            STORES.lock().unwrap().push((a, x, self.ticks));
        }
    }

    fn wl(&self, o: usize) -> u32 {
        self.work.get(o..o + 4).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn ww(&self, o: usize) -> u16 {
        self.work.get(o..o + 2).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]))
    }

    fn period(&self, n: u8) -> u16 {
        self.ww(W_PERIODS + ((2 * n as usize) & 0xFF))
    }

    // the registers, at the addresses of the work area's table
    fn reg(&self, hw: &mut Hw, k: usize, ch: usize, v: u16) {
        hw.bus.write_word(self.wl(W_REGS + 16 * k + 4 * ch), v);
    }

    fn per(&self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0, ch, v);
    }

    fn vol(&self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 3, ch, v);
    }

    /// The sample address and length (words) of instrument record `r`.
    fn sample_of(&self, hw: &Hw, r: u32) -> (u32, u16) {
        let lc = self.guest(r.wrapping_add(SAMPLE)).unwrap_or_else(|| r.wrapping_add(SAMPLE));
        (lc, (self.b(hw, r) as u16) << 8 | self.b(hw, r.wrapping_add(1)) as u16)
    }

    /// LC and LEN of channel `ch`.
    fn put_sample(&self, hw: &mut Hw, ch: usize, (lc, len): (u32, u16)) {
        let a = self.wl(W_REGS + 32 + 4 * ch);
        hw.space(WriteGaps::LOC, ch);
        hw.bus.write_word(a, (lc >> 16) as u16);
        hw.bus.write_word(a.wrapping_add(2), lc as u16);
        hw.space(WriteGaps::LEN, ch);
        self.reg(hw, 1, ch, len);
    }

    fn song(&self) -> u32 {
        (self.ww(W_SONG) & 15) as u32
    }

    /// Instrument `i` of the current song, adjusted.
    fn instrument(&self, hw: &Hw, i: u8) -> u32 {
        let base = VIRT + self.lay.base as u32;
        let tab = self.l(hw, base + INSTABS + 4 * self.song());
        self.l(hw, tab.wrapping_add(4 * i as u32)).wrapping_add(self.lay.dx)
    }

    /// InitSound for song `d` (specification, 4): no register writes.
    fn start(&mut self, hw: &Hw, d: u16) {
        let d = d & 15;
        self.v = Default::default();
        self.work[W_SONG..W_SONG + 2].copy_from_slice(&d.to_be_bytes());
        let base = VIRT + self.lay.base as u32;
        let ty = self.b(hw, base + TYPES + d as u32);
        self.work[W_TYPE] = ty;
        let nv = if ty == 4 { 4 } else { 3 };
        for k in 0..nv {
            let seq = self.l(hw, base + SONGS + 16 * d as u32 + 4 * k as u32);
            let v = &mut self.v[k];
            v.seq = seq;
            v.sp = 16 * k as u32;
            v.dur = 1;
            v.flags = 0xC4;
            v.arm = true;
            v.active = if d == 14 { 0 } else { 0xFF };
        }
    }

    fn push(&mut self, ch: usize, x: u32) {
        let o = self.v[ch].sp as usize;
        if let Some(s) = self.work.get_mut(o..o + 4) {
            s.copy_from_slice(&x.to_be_bytes());
        }
        self.v[ch].sp = self.v[ch].sp.wrapping_add(4);
    }

    fn pop(&mut self, ch: usize) -> u32 {
        self.v[ch].sp = self.v[ch].sp.wrapping_sub(4);
        self.wl(self.v[ch].sp as usize)
    }

    /// PWM, envelope and volume (specification, 5.3).
    fn pwm_env(&mut self, hw: &mut Hw, ch: usize) -> u16 {
        let v = self.v[ch];
        if (v.insno as i8) < 4 {
            let v = &mut self.v[ch];
            v.pwm_cnt = v.pwm_cnt.wrapping_sub(1);
            if v.pwm_cnt == 0 {
                v.pwm_cnt = v.pwm_speed;
                let x = if v.pwm_dir == 0 {
                    v.pwm_pos = v.pwm_pos.wrapping_add(1);
                    if v.pwm_pos == 0x1E {
                        v.pwm_dir = 0xFF;
                    }
                    0x80
                } else {
                    v.pwm_pos = v.pwm_pos.wrapping_sub(1);
                    if v.pwm_pos == 4 {
                        v.pwm_dir = 0;
                    }
                    0
                };
                let a = v.ins.wrapping_add(SAMPLE).wrapping_add(v.pwm_pos as u32);
                self.store(hw, a, x);
            }
        }
        let v = &mut self.v[ch];
        match v.env_phase {
            0 => {
                v.env_cnt = v.env_cnt.wrapping_sub(1);
                if v.env_cnt & 0x80 != 0 {
                    v.env_cnt = v.env_att;
                    v.vol = v.vol.wrapping_add(v.env_step);
                    if v.vol == 0x3F {
                        v.env_phase = 1;
                        v.env_cnt = v.env_dec;
                    }
                }
            }
            1 => {
                v.env_cnt = v.env_cnt.wrapping_sub(1);
                if v.env_cnt & 0x80 != 0 {
                    v.env_cnt = v.env_dec;
                    if v.vol == v.env_sus {
                        v.env_phase = 2;
                    } else {
                        v.vol = v.vol.wrapping_sub(v.env_step);
                        if v.vol & 0x80 != 0 {
                            v.env_phase = 2;
                        }
                    }
                }
            }
            _ => {}
        }
        let mut out = v.vol;
        let fade = self.work[W_FADE];
        if fade != 0 && (out as i8) >= ((fade >> 2) as i8) {
            out = fade >> 2;
        }
        out as u16
    }

    fn set_note(&mut self, _hw: &mut Hw, ch: usize, n: u8) {
        self.v[ch].note = n;
        self.v[ch].period = self.period(n & 0x7F);
        self.v[ch].o_per = Some(self.v[ch].period);
    }

    fn glide(&mut self, hw: &mut Hw, ch: usize) {
        let v = self.v[ch];
        if v.glide_spd == 0 || v.note == v.glide_tgt {
            return;
        }
        let n = if v.note < v.glide_tgt {
            let n = v.note.wrapping_add(v.glide_spd);
            if n < v.glide_tgt {
                n
            } else {
                v.glide_tgt
            }
        } else {
            let n = v.note.wrapping_sub(v.glide_spd);
            if n >= v.glide_tgt {
                n
            } else {
                v.glide_tgt
            }
        };
        self.set_note(hw, ch, n);
    }

    /// The pitch effects (specification, 5.4).
    fn pitch(&mut self, hw: &mut Hw, ch: usize) {
        let v = &mut self.v[ch];
        let vib = v.vib_delay != 0
            && (v.vib_dcnt == 0 || {
                v.vib_dcnt = v.vib_dcnt.wrapping_sub(1);
                v.vib_dcnt == 0
            });
        if vib {
            let s = v.vib_step as u16;
            v.period = if v.dir != 0 { v.period.wrapping_add(s) } else { v.period.wrapping_sub(s) };
            let p = v.period;
            v.vib_cnt = v.vib_cnt.wrapping_sub(1);
            if v.vib_cnt == 0 && v.vib_len != 0 {
                v.vib_cnt = v.vib_len.wrapping_mul(2);
                v.dir ^= 1;
            }
            self.v[ch].o_per = Some(p);
            self.glide(hw, ch);
        } else if v.tr_cnt != 0 {
            v.tr_cnt -= 1;
            if v.tr_cnt == 0 {
                v.dir ^= 1;
                let n = if v.dir != 0 {
                    v.tr_cnt = v.tr_ta;
                    v.note.wrapping_add(v.tr_int)
                } else {
                    v.tr_cnt = v.tr_tb;
                    v.note.wrapping_sub(v.tr_int)
                };
                self.set_note(hw, ch, n);
            } else {
                self.glide(hw, ch);
            }
        } else {
            self.glide(hw, ch);
        }
    }

    /// Reads the voice's sequence up to its next note (specification, 6).
    fn parse(&mut self, hw: &mut Hw, ch: usize) {
        let dx = self.lay.dx;
        let mut p = self.v[ch].seq;
        for _ in 0..PARSE_LIMIT {
            let c = self.b(hw, p);
            p = p.wrapping_add(1);
            if (1..0x80).contains(&c) {
                self.note(hw, ch, c, p);
                return;
            }
            let op = |k: u32| self.b(hw, p.wrapping_add(k));
            match c & 0x7F {
                0 => {}
                1 => {
                    let i = op(0);
                    p = p.wrapping_add(1);
                    let r = self.instrument(hw, i);
                    let v = &mut self.v[ch];
                    v.insno = i;
                    v.ins = r;
                }
                2 => {
                    p = p.wrapping_add(p & 1);
                    let t = self.l(hw, p);
                    p = p.wrapping_add(4);
                    self.push(ch, p);
                    p = t.wrapping_add(dx);
                }
                3 => p = self.pop(ch),
                4 => {
                    self.v[ch].loopcnt = op(0);
                    p = p.wrapping_add(1);
                    self.v[ch].lp = p;
                }
                5 => {
                    let v = &mut self.v[ch];
                    v.loopcnt = v.loopcnt.wrapping_sub(1);
                    if v.loopcnt != 0 {
                        p = v.lp;
                    }
                }
                6 => {
                    let (x, y, z) = (op(0), op(1), op(2));
                    p = p.wrapping_add(3);
                    let v = &mut self.v[ch];
                    v.env_vol0 = (x >> 4) * 4;
                    v.env_sus = (x & 15) * 4;
                    v.env_att = y >> 4;
                    v.env_dec = y & 15;
                    v.env_phase0 = z;
                }
                7 => {
                    self.v[ch].glide_spd = op(0);
                    p = p.wrapping_add(1);
                }
                8 => {
                    let (i, a, b) = (op(0), op(1), op(2));
                    p = p.wrapping_add(3);
                    let v = &mut self.v[ch];
                    (v.tr_int, v.tr_ta, v.tr_tb) = (i, a, b);
                }
                9 => {
                    let (d, s, l, r) = (op(0), op(1), op(2), op(3));
                    p = p.wrapping_add(4);
                    let v = &mut self.v[ch];
                    (v.vib_delay, v.vib_step, v.vib_len, v.vib_dir0) = (d, s, l, r);
                }
                10 | 11 | 12 | 13 | 16 => {
                    let x = op(0);
                    p = p.wrapping_add(1);
                    let v = &mut self.v[ch];
                    match c & 0x7F {
                        10 => v.transp = x,
                        11 => v.fixdur = x,
                        12 => v.env_on = x,
                        13 => v.flags = x,
                        _ => v.env_step = x,
                    }
                }
                14 => {
                    let s = op(0);
                    p = p.wrapping_add(1);
                    let v = &mut self.v[ch];
                    v.pwm_speed = s;
                    v.pwm_cnt = s;
                    v.pwm_dir = 0;
                    v.pwm_pos = 7;
                    let ins = v.ins;
                    if ins != 0 {
                        // the record header, not the sample (the original's quirk)
                        for k in 2..0x22 {
                            self.store(hw, ins.wrapping_add(k), if k < 0x0A { 0x80 } else { 0 });
                        }
                    }
                }
                15 => {
                    self.v[ch].active = 0;
                    hw.wait(T_OFFV);
                    hw.dmacon(self.ww(W_OFF + 2 * ch));
                    self.vol(hw, ch, 0);
                    return;
                }
                17 => self.v[ch].skiptr = 0xFF,
                18 => {
                    p = p.wrapping_add(p & 1);
                    p = self.l(hw, p).wrapping_add(dx);
                }
                _ => {
                    // not in the corpus (the original jumps through garbage): the voice stops
                    self.v[ch].active = 0;
                    return;
                }
            }
        }
        self.v[ch].active = 0;
    }

    /// A note (specification, 6.1); `p` is the position after its byte.
    fn note(&mut self, hw: &mut Hw, ch: usize, c: u8, mut p: u32) {
        let v = &mut self.v[ch];
        let mut n = if v.skiptr != 0 {
            v.skiptr = 0;
            c
        } else {
            c.wrapping_add(v.transp)
        };
        if v.glide_spd != 0 {
            v.glide_tgt = n;
            n = v.note;
        }
        v.note = n;
        let period = self.period(n & 0x7F);
        self.v[ch].period = period;
        // a voice flagged to restart (bit 7 when the note begins) starts the note's sample; every
        // note then sets the loop pointer: bit 6 names the instrument (low six bits), else the note's own
        if self.mask & 1 << ch != 0 {
            let t = self.sample_of(hw, self.v[ch].ins);
            self.v[ch].o_trig = Some(t);
        }
        let fl = self.v[ch].flags;
        let lr = if fl & 0x40 != 0 { self.instrument(hw, fl & 0x3F) } else { self.v[ch].ins };
        let lp = self.sample_of(hw, lr);
        self.v[ch].o_loop = Some(lp);
        self.v[ch].o_per = Some(period);
        let d = if self.v[ch].fixdur != 0 {
            self.v[ch].fixdur
        } else {
            let d = self.b(hw, p);
            p = p.wrapping_add(1);
            d
        };
        let v = &mut self.v[ch];
        v.dur = d;
        v.seq = p;
        v.tr_cnt = v.tr_ta;
        if v.vib_delay != 0 {
            v.vib_dcnt = v.vib_delay;
            v.vib_cnt = v.vib_len;
            v.dir = v.vib_dir0;
        } else {
            v.dir = 0;
        }
        if v.env_on != 0 {
            v.env_phase = v.env_phase0;
            v.env_cnt = if v.env_phase == 0 { v.env_att } else { v.env_dec };
            v.vol = v.env_vol0;
        }
    }

    /// The fade check (specification, 7.3): true when it ran out.
    fn fade(&mut self) -> bool {
        for _ in 0..2 {
            if self.work[W_FADE] == 0 {
                return false;
            }
            self.work[W_FADE] -= 1;
            if self.work[W_FADE] == 0 {
                return true;
            }
        }
        false
    }

    fn play(&mut self, hw: &mut Hw) {
        hw.gaps(&GAPS);
        if self.fade() {
            self.dead = true;
            self.songend = true;
            return;
        }
        // the voices whose note begins this tick and whose current flags say "restart": their DMA
        // goes off first and on again after everything is written
        self.mask = 0;
        for ch in 0..4 {
            let v = &mut self.v[ch];
            (v.o_vol, v.o_per, v.o_trig, v.o_loop) = (None, None, None, None);
            if v.active & 0x80 != 0 && v.dur.wrapping_sub(1) == 0 && v.arm {
                self.mask |= 1 << ch;
            }
        }
        let mask = self.mask;
        if mask != 0 {
            let m = (0..4).filter(|ch| mask & 1 << ch != 0).fold(0, |a, ch| a | self.ww(W_OFF + 2 * ch));
            hw.dmacon(m);
        }
        for ch in 0..4 {
            if self.v[ch].active & 0x80 == 0 {
                continue;
            }
            let vol = self.pwm_env(hw, ch);
            self.v[ch].o_vol = Some(vol);
            self.pitch(hw, ch);
            let v = &mut self.v[ch];
            v.dur = v.dur.wrapping_sub(1);
            if v.dur == 1 && v.flags & 0x80 != 0 {
                v.arm = true;
            }
            if v.dur == 0 {
                self.parse(hw, ch);
                self.v[ch].arm = false;
            }
        }
        // the sample starts of the restarted voices, then the volumes and periods (a note's loop
        // pointer follows its period), then the DMA back on
        for ch in 0..4 {
            if let Some(t) = self.v[ch].o_trig {
                self.put_sample(hw, ch, t);
            }
        }
        for ch in 0..4 {
            if let Some(x) = self.v[ch].o_vol {
                hw.space(WriteGaps::VOL, ch);
                self.vol(hw, ch, x);
            }
            if let Some(x) = self.v[ch].o_per {
                hw.space(WriteGaps::PER, ch);
                self.per(hw, ch, x);
            }
        }
        if self.v.iter().any(|v| v.o_trig.is_some()) {
            let m = (0..4).filter(|&ch| self.v[ch].o_trig.is_some()).fold(0, |a, ch| a | self.ww(W_OFF + 2 * ch));
            hw.dmacon(m | 0x8000);
            // ten raster lines; the loop writes then start at a fixed place in the line, by the
            // voice that comes first
            let first = (0..4).find(|&ch| self.v[ch].o_loop.is_some()).unwrap_or(0);
            hw.wait_lines(10, [45, 81, 116, 152][first]);
        }
        for ch in 0..4 {
            if let Some(t) = self.v[ch].o_loop {
                self.put_sample(hw, ch, t);
            }
        }
        // the original never reports an end; when every voice has ended the output stays
        // silent, and the port ends the song there
        if self.v.iter().all(|v| v.active & 0x80 == 0) {
            self.songend = true;
        }
    }
}

/// Colour clocks from the song's start to the host's timer start, beyond the first period (measured
/// in the reference runs): the first tick's writes follow 76 623 clocks after the song start in the
/// reference, whatever the module.
const TIMER_DELAY: u64 = 3654;
/// The second tick follows the first by this much less than a period: the reference's first music
/// tick comes 67 979 clocks after the first (the writes of the second tick of all ten corpus files).
const FIRST_GAP: u64 = 2961;

impl Replayer for TimFollin {
    fn format(&self) -> &'static str {
        "Tim Follin II"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn timer_delay(&self) -> u64 {
        TIMER_DELAY
    }

    fn next_tick_at(&self) -> Option<u64> {
        (self.n_ticks == 1).then(|| self.t_first + super::DT_TICK - FIRST_GAP)
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.lay.subs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        if (1..=self.lay.subs).contains(&n) {
            self.sub = n;
            self.started = false;
        }
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.n_ticks == 0 {
            self.t_first = hw.bus.clock;
        }
        self.n_ticks = self.n_ticks.saturating_add(1);
        if !self.started {
            // song 0 first, then the requested one, as the original's InitSound
            self.started = true;
            self.songend = false;
            self.dead = false;
            self.start(hw, 0);
            self.start(hw, self.sub);
        }
        if !self.dead {
            self.play(hw);
        }
        #[cfg(test)]
        {
            self.ticks += 1;
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = layout(d)?;
    // relocation (specification, 2.3 step 1), in the module in guest memory; the second
    // pointer list (step 2) is never read by the replay and is left alone
    let mut o = lay.base;
    while o + 4 <= d.len() {
        let g = base + o as u32;
        let x = m.bus.r32(g);
        if x >> 24 != 0 {
            break;
        }
        m.bus.w32(g, x.wrapping_add(lay.dx));
        o += 4;
    }
    #[cfg(test)]
    STORES.lock().unwrap().clear();
    // the original's init routine runs before the host starts its timer. In the reference runs the
    // first music tick of a header-build module (Ghouls'n'Ghosts, Sly Spy) comes 153,000 colour
    // clocks plus 12.9 a byte of the file after the port's clock (nine files, within 2 %: the routine
    // goes over the module), and the jump-table build's (Puzznic, one file) 155,000 after it
    let init = if lay.subs > 1 || be16(d, 0) == 0x4EF9 { 11_200 } else { 9_350 + d.len() as u64 * 129 / 10 };
    m.bus.clock += init;
    Some(Box::new(TimFollin {
        lay,
        gbase: base,
        len: d.len() as u32,
        sub: 1,
        started: false,
        t_first: 0,
        n_ticks: 0,
        dead: false,
        songend: false,
        work: work_area(),
        v: Default::default(),
        mask: 0,
        #[cfg(test)]
        ticks: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    fn p16(d: &mut [u8], o: usize, v: u16) {
        d[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }

    fn p32(d: &mut [u8], o: usize, v: u32) {
        d[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// The game address the synthetic modules' pointers are made for (file offset 0).
    const G: u32 = 0x4_0000;
    const B: usize = 0x1E;
    const SEQ0: usize = 0x200;
    const SEQ1: usize = 0x210;
    const SUB: usize = 0x300;
    const INS: usize = 0x400;
    const INSTAB: usize = 0x3C0;

    /// A header-build module: song 0 the anchor sequence, song 1 voice 0's sequence
    /// `seq` at 0x210 (voices 1-3 the anchor), a subsequence `sub` at 0x300, two
    /// instruments (0 at 0x400 with an 8-word sample, 1 at 0x500 with a 4-word one).
    fn module(seq: &[u8], sub: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8; 0x600];
        p16(&mut d, 0, 0x601A);
        p16(&mut d, 0x1C, 0x1010);
        p32(&mut d, B, 0x600);
        for k in 0..4 {
            p32(&mut d, B + 4 + 4 * k, G + SEQ0 as u32);
            p32(&mut d, B + 0x14 + 4 * k, G + if k == 0 { SEQ1 } else { SEQ0 } as u32);
        }
        for s in 0..16 {
            p32(&mut d, B + 0x104 + 4 * s, G + INSTAB as u32);
            d[B + 0x144 + s] = 4;
        }
        for i in 0..5 {
            p32(&mut d, INSTAB + 4 * i, G + INS as u32 + if i == 1 { 0x100 } else { 0 });
        }
        d[SEQ0..SEQ0 + 6].copy_from_slice(&ANCHOR);
        d[SEQ1..SEQ1 + seq.len()].copy_from_slice(seq);
        d[SUB..SUB + sub.len()].copy_from_slice(sub);
        p16(&mut d, INS, 8);
        p16(&mut d, INS + 0x100, 4);
        for k in 0..16 {
            d[INS + 0x32 + k] = 0x11;
        }
        d
    }

    fn play(d: Vec<u8>, ticks: usize) -> (Engine, Vec<(u32, u16, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("tf.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Tim Follin II");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        (e, log)
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    fn audio(log: &[(u32, u16, u64)]) -> Vec<(u32, u16)> {
        log.iter().filter(|w| w.0 == 0xDFF096 || (0xDFF0A0..0xDFF0E0).contains(&w.0)).map(|w| (w.0, w.1)).collect()
    }

    #[test]
    fn recognises_both_builds() {
        let d = module(&[0x8F], &[]);
        let lay = layout(&d).unwrap();
        assert_eq!(lay, Lay { base: B, dx: (VIRT + SEQ0 as u32).wrapping_sub(G + SEQ0 as u32), subs: 1 });
        // the voices of song 0 must share their sequence, and the anchor must be there
        let mut e = d.clone();
        p32(&mut e, B + 0x10, G);
        assert!(!detect(&e));
        let mut e = d.clone();
        e[SEQ0 + 3] = 3;
        assert!(!detect(&e));
        // jump-table build
        let mut j = vec![0u8; 0x800];
        for k in 0..8 {
            p16(&mut j, 6 * k, 0x4EF9);
        }
        p32(&mut j, 2, 0xFF00);
        p32(&mut j, 0x2C, 0xFFA0);
        assert_eq!(layout(&j), Some(Lay { base: JT_BASE, dx: 0x100, subs: 8 }));
        p32(&mut j, 0x2C, 0xFFA2);
        assert!(!detect(&j));
        assert!(!detect(b"FORM\0\0\0\0EMODEMIC"));
    }

    #[test]
    fn plays_notes_in_the_original_order() {
        // instrument 0, fixed duration off, note 0x10 for 3 ticks, note 0x20 for 2, end
        let (_, log) = play(module(&[0x81, 0, 0x10, 3, 0x20, 2, 0x8F], &[]), 8);
        let a = audio(&log);
        // tick 1: every voice's DMA off, then the sample starts of all four (voice 0's instrument 0, the
        // anchor voices' instrument 4), the volumes and periods, the DMA back on, and the loop pointers
        assert_eq!(a[0], (0xDFF096, 0x000F));
        assert_eq!(a[1].0, 0xDFF0A0);
        assert_eq!(a[3], (0xDFF0A4, 8));
        assert_eq!(a.iter().filter(|w| **w == (0xDFF096, 0x800F)).count(), 1);
        assert_eq!(reg(&log, 0xDFF0A6)[0], PERIODS[0x10]);
        assert_eq!(reg(&log, 0xDFF0A6), [PERIODS[0x10], PERIODS[0x20]]);
        // the end of the voice: DMA off, VOL 0
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x0001));
        assert_eq!(*reg(&log, 0xDFF0A8).last().unwrap(), 0);
    }

    #[test]
    fn calls_loops_transposes_and_ends() {
        // repeat 2 x (call sub), transpose 2, note 1 (+2), untransposed note 5, end
        let mut seq = vec![0x81, 0, 0x84, 2, 0x82];
        seq.resize(6, 0); // the call operand is aligned to an even address
        seq.extend_from_slice(&(G + SUB as u32).to_be_bytes());
        seq.extend_from_slice(&[0x85, 0x8A, 2, 0x01, 1, 0x91, 0x05, 1, 0x8F]);
        let (e, log) = play(module(&seq, &[0x08, 1, 0x83]), 10);
        assert_eq!(reg(&log, 0xDFF0A6), [PERIODS[8], PERIODS[8], PERIODS[3], PERIODS[5]]);
        assert!(e.songend());
    }

    #[test]
    fn pwm_rewrites_the_sample_in_guest_memory() {
        // instrument 0 (number below 4), PWM speed 1: one store a tick from position 8 up
        let (e, _) = play(module(&[0x81, 0, 0x8E, 1, 0x10, 0], &[]), 8);
        let (a, _) = e.m.deli.list[0];
        let ram = &e.m.bus.ram;
        let s = a as usize + INS + 0x32;
        assert_eq!(ram[s + 8..s + 12], [0x80, 0x80, 0x80, 0x80]);
        assert_eq!(ram[s + 7], 0x11);
        // the record header reset of command 14
        assert_eq!(ram[a as usize + INS + 2..a as usize + INS + 10], [0x80; 8]);
    }

    #[test]
    fn envelope_and_retrigger() {
        // envelope from 0x10, attack every tick by 8; loop pointer of instrument 1 after each note
        let seq = [0x81, 0, 0x86, 0x48, 0x00, 0, 0x90, 8, 0x8C, 1, 0x8D, 0x41, 0x10, 4, 0x8F];
        let (_, log) = play(module(&seq, &[]), 6);
        let vols = reg(&log, 0xDFF0A8);
        assert_eq!(vols[..4], [0, 0x18, 0x20, 0x28]);
        // the sample start (instrument 0), then the loop pointer: LEN of instrument 1; the DMA is
        // restarted by the first note only (bit 7 of the flags is clear afterwards)
        assert_eq!(reg(&log, 0xDFF0A4)[..2], [8, 4]);
        let ons: Vec<u16> = reg(&log, 0xDFF096).into_iter().filter(|&v| v & 0x8000 != 0).collect();
        assert_eq!(ons.len(), 1);
    }

    /// `TF_MODULE=file TF_TICKS=n TF_STORES=out [PK_SUBSONG=k] cargo test --release fetch_dump -- --ignored`:
    /// the replayer's stores into the module, `addr byte tick` (addresses as the reference
    /// host loads the module, at 0x10000), for the check of UADE's DMA fetch words.
    #[test]
    #[ignore]
    fn fetch_dump() {
        let (Ok(file), Ok(ticks), Ok(out)) = (std::env::var("TF_MODULE"), std::env::var("TF_TICKS"), std::env::var("TF_STORES"))
        else {
            return;
        };
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new(&file), std::fs::read(&file).unwrap()).unwrap();
        for _ in 0..std::env::var("PK_SUBSONG").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0) {
            e.change_subsong(1).unwrap();
        }
        let mut buf = vec![0f32; 2 * 960];
        for _ in 0..ticks.parse::<usize>().unwrap() {
            e.render(&mut buf);
        }
        let s: String = STORES.lock().unwrap().iter().map(|(a, x, t)| format!("{a:x} {x:x} {t}\n")).collect();
        std::fs::write(out, s).unwrap();
    }
}

//! Paul Tonge (`pat.*`; the Amiga music of a 1993 game by Paul Tonge, played by the Eagleplayer
//! "Paul Tonge": a thin wrapper around the game's own 68k driver).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes,
//! call timing and module data of the 68k reference player; no original code. A song file
//! `PAT.name` holds four position lists (one per Paula channel), patterns of note, length,
//! instrument and command bytes, and 12-byte instrument records; the samples are in a bank
//! file `SMP.name` (else `SMP.set`) beside it. Each tick every voice is stepped: the pattern is
//! read when its row counter runs out, an attack, decay and release envelope, an optional
//! pitch slide and a vibrato run on the other ticks. Every register write is placed at the
//! cycle count of the specification's ledger (the tick, the pattern read and the audio interrupt
//! handler that swaps the loop part in once Paula has latched the attack part).
//!
//! Two tables of the original driver are needed and are embedded: the volume curve (15 words) and
//! the period table (36 words, three octaves). The data is taken from the original player on the
//! project owner's decision. The driver has no bound check on either: a volume index of 15 or more
//! and a note index of 36 or more read the driver's own code behind the table, which is not
//! embedded; the port clamps such an index to the table's last entry (volume 64, period 113).
//!
//! Choices where the specification leaves the behaviour open: a pattern byte 0x9A..0x9F and the
//! unspecified command 0x98 end the voice (it goes inactive, no song-end mark); chord bytes
//! (0xA0..0xAF) take no argument bytes; a volume or note index past its table reads the table's
//! last entry (above); a pattern or list that runs off the known memory reads zero bytes; a pattern read of more than 4096 bytes in one
//! tick, or more than 8 position-list restarts in a row, ends the voice; a start request of 2 or
//! more stops the song without cost. Memory past the song file is the sample bank at the
//! offset the reference's host put it (0x800, 0x810 or 0xC08 from the song start by the song's
//! size, as the three real sets showed), so PAT.TUNE8's list without a terminator reads what the
//! reference reads up to the byte that is undefined there.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus as _;

pub const OPEN: Open = Open { name: "Paul Tonge", detect, new };

static VOLUMES: &[u8; 30] = include_bytes!("paultonge_volumes.bin");
static PERIODS: &[u8; 72] = include_bytes!("paultonge_periods.bin");

/// The delay of the engine's first tick that puts the first call of the play routine at colour
/// clock 98306 (the reference's value for a file name of nine characters in a directory path of
/// 25 characters, with its sample bank found under the name `SMP.<suffix>`).
const FIRST_TICK_DELAY: u64 = 27266;
/// Colour clock of the first call in the reference run for a nine-character file name whose
/// directory path (with its final slash) has 60 characters, without a bank of the song's own
/// name (the bank `SMP.set` is read after a failed open) and with it.
const FIRST_CALL_FALLBACK: i64 = 99198;
const FIRST_CALL_OWN_BANK: i64 = 98495;
/// The reference host's first call moves with the length of the path: 25.5 colour clocks per
/// character of the directory when the bank has to be looked for twice, 17 when it is found at
/// once, and 10.5 per character of the file name.
const DIR_REF_LEN: i64 = 60;
const NAME_REF_LEN: i64 = 9;
/// Colour clocks from a handler call to its read of the interrupt flags (any value from 40 to 100
/// fits the reference logs; the prologue before the first channel is 94).
const S_READ: u64 = 94;
const INTENA_SET: [u16; 4] = [0x8080, 0x8100, 0x8200, 0x8400];
const DMA_SET: [u16; 4] = [0x8201, 0x8202, 0x8204, 0x8208];
/// Paula channel 0..3's cycles from the end of the previous step to the first volume write.
const VOL1: [u64; 4] = [432, 480, 518, 553];
const VOL2: [u64; 4] = [137, 168, 199, 207];
const INTENA_COST: [u64; 4] = [241, 272, 303, 311];

/// The volume curve; an index past it (the original reads its code there) gives the last entry.
fn volume(i: usize) -> u16 {
    let i = i.min(VOLUMES.len() / 2 - 1);
    u16::from_be_bytes([VOLUMES[2 * i], VOLUMES[2 * i + 1]])
}

/// The period table; an index past it (the original reads its code there) gives the last entry.
fn period_of(i: usize) -> u16 {
    let i = i.min(PERIODS.len() / 2 - 1);
    u16::from_be_bytes([PERIODS[2 * i], PERIODS[2 * i + 1]])
}

// ---------------------------------------------------------------------------------
// Recognition (spec 1.2)

fn detect(d: &[u8]) -> bool {
    if d.len() < 16 || be16(d, 0) != 12 {
        return false;
    }
    for k in [2usize, 4, 6] {
        let o = be16(d, k) as usize;
        if o & 0x8000 != 0 || o & 1 != 0 {
            return false;
        }
        if o == 0 {
            continue;
        }
        if o + 2 > d.len() {
            return false;
        }
        let e = be16(d, o) as usize;
        if e == 0 || e & 0x8000 != 0 {
            return false;
        }
        match d.get(e - 1) {
            Some(0x80) | Some(0x8F) => {}
            _ => return false,
        }
    }
    true
}

/// The sample bank's name for a song file: `SMP.rest` for `PAT.rest`, else `SMP.set`.
fn bank_name(file: &str) -> String {
    let b = file.as_bytes();
    if b.len() >= 4 && b[..4].eq_ignore_ascii_case(b"pat.") {
        format!("SMP{}", &file[3..])
    } else {
        "SMP.set".to_string()
    }
}

// ---------------------------------------------------------------------------------
// State

#[derive(Clone, Default)]
struct Voice {
    active: bool,
    period: u16,
    level: u8,
    list: usize,
    pat: usize,
    f_slide: bool,
    f_ns: bool,
    f_dir: bool,
    f_sor: bool,
    row: u8,
    len: u8,
    note: u8,
    att_byte: u8,
    dec_byte: u8,
    att_steps: u8,
    att_size: u8,
    dec_steps: u8,
    dec_size: u8,
    ivol: u8,
    glide: i8,
    rel_pt: u8,
    rel_step: u8,
    rel_on: bool,
    att_cnt: u8,
    att_rl: u8,
    dec_cnt: u8,
    dec_rl: u8,
    rel_cnt: u8,
    rel_rl: u8,
    slide_step: u16,
    ns_off: u16,
    slide_cnt: u8,
    slide_rl: u8,
    slide_delay: u8,
    rate_cnt: u8,
    rate_rl: u8,
    vib_depth: u8,
    vib_cnt: u8,
    vib_period: u8,
    vib_delay: u8,
    trans: i8,
    att_ptr: u32,
    att_len: u16,
    loop_ptr: u32,
    loop_len: u16,
}

pub struct PaulTonge {
    song: Vec<u8>,
    bank: Vec<u8>,
    bank_addr: u32,
    bank_rel: usize,
    v: [Voice; 4],
    start_req: u8,
    playing: bool,
    stop_next: bool,
    speed: u8,
    speed_cnt: u8,
    master: u16,
    fade_speed: u8,
    fade_cnt: u8,
    int_acc: u16,
    dma_acc: u16,
    instr: usize,
    marks: [bool; 4],
    songend: bool,
    t0: u64,
    cy: u64,
}

enum End {
    /// A note, tie, rest or voice-off ended the read.
    Done,
    /// The song was stopped (the remaining voices are inactive).
    Stopped,
    /// The voice ended by an undefined byte.
    Dead,
}

impl PaulTonge {
    /// A byte of the guest memory the reference's driver sees, offsets from the song's start.
    fn byte(&self, o: usize) -> u8 {
        if let Some(&b) = self.song.get(o) {
            return b;
        }
        if o >= self.bank_rel {
            if let Some(&b) = self.bank.get(o - self.bank_rel) {
                return b;
            }
        }
        0
    }

    fn word(&self, o: usize) -> u16 {
        (self.byte(o) as u16) << 8 | self.byte(o + 1) as u16
    }

    fn at(&self, hw: &mut Hw) {
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    /// A host call: it takes no time but drops an odd half cycle.
    fn host(&mut self) {
        self.cy -= self.cy % 2;
    }

    fn put(&self, hw: &mut Hw, reg: u32, v: u16) {
        self.at(hw);
        hw.bus.write_word(reg, v);
    }

    fn sample_long(&self, o: usize) -> u32 {
        be32(&self.bank, o)
    }

    // -----------------------------------------------------------------------------
    // Song start and stop (5.1, 5.10)

    fn start(&mut self, hw: &mut Hw) {
        self.playing = true;
        self.fade_speed = 0;
        self.master = 16;
        self.cy += 223;
        for c in 0..4 {
            let off = be16(&self.song, 2 * c) as usize;
            if off != 0 {
                let v = &mut self.v[c];
                v.list = off + 2;
                v.f_slide = false;
                v.f_ns = false;
                v.f_dir = false;
                v.f_sor = false;
                v.vib_depth = 0;
                v.glide = 0;
                v.trans = 0;
                v.row = 1;
                v.active = true;
                v.pat = be16(&self.song, off) as usize;
                self.cy += 227;
            } else {
                self.cy += 67;
            }
        }
        self.instr = be16(&self.song, 8) as usize;
        self.speed = self.song.get(10).copied().unwrap_or(0);
        self.speed_cnt = 1;
        self.cy += 84;
        self.put(hw, 0xDFF09A, 0x0780);
        self.cy += 124;
        self.host();
        for _ in 0..3 {
            self.cy += 66;
            self.host();
        }
        self.cy += 108;
        self.put(hw, 0xDFF09E, 0x00FF);
        self.cy += 136;
    }

    fn stop(&mut self, hw: &mut Hw) {
        self.playing = false;
        self.start_req = 1;
        self.put(hw, 0xDFF096, 0x000F);
        for v in self.v.iter_mut() {
            v.active = false;
        }
        self.fade_speed = 0;
        self.host();
        self.songend = true;
    }

    // -----------------------------------------------------------------------------
    // Period, envelope, slide, vibrato (5.4 to 5.8)

    /// The period of the current note (5.4 step 2); returns the cost.
    fn compute_period(&mut self, c: usize) -> u64 {
        let v = &mut self.v[c];
        let i = ((v.note as i32 + v.trans as i32) & 0x7F) as usize;
        let mut p = period_of(i);
        let mut cost = 118;
        if v.f_ns {
            p = p.wrapping_add(v.ns_off);
            v.slide_cnt = v.slide_rl;
            cost += 56;
        }
        if v.f_sor {
            v.f_slide = false;
            cost += 20;
        }
        v.period = p;
        cost
    }

    fn envelope(&mut self, c: usize) {
        let v = &mut self.v[c];
        if v.att_steps > 0 {
            v.att_cnt = v.att_cnt.wrapping_sub(1);
            if v.att_cnt != 0 {
                self.cy += 80;
                return;
            }
            v.att_cnt = v.att_rl;
            v.att_steps -= 1;
            let s = v.att_size.wrapping_add(v.level);
            if s < 16 {
                v.level = s;
                self.cy += 169;
            } else {
                self.cy += 157;
            }
        } else if v.dec_steps > 0 {
            v.dec_cnt = v.dec_cnt.wrapping_sub(1);
            if v.dec_cnt != 0 {
                self.cy += 101;
                return;
            }
            v.dec_cnt = v.dec_rl;
            v.dec_steps -= 1;
            if v.level >= v.dec_size {
                v.level -= v.dec_size;
                self.cy += 182;
            } else {
                v.level = 0;
                self.cy += 186;
            }
        } else if v.rel_on {
            v.rel_cnt = v.rel_cnt.wrapping_sub(1);
            if v.rel_cnt != 0 {
                self.cy += 122;
                return;
            }
            v.rel_cnt = v.rel_rl;
            if v.level >= v.rel_step {
                v.level -= v.rel_step;
                self.cy += 187;
            } else {
                v.level = 0;
                self.cy += 191;
            }
        } else {
            self.cy += 97;
        }
    }

    /// One slide step (5.8); the cost.
    fn slide_step(&mut self, c: usize) -> u64 {
        let v = &mut self.v[c];
        if v.slide_cnt == 0 {
            return 37;
        }
        if v.slide_delay > 0 {
            v.slide_delay -= 1;
            return 74;
        }
        if v.rate_cnt > 0 {
            v.rate_cnt -= 1;
            return 95;
        }
        v.slide_cnt -= 1;
        v.rate_cnt = v.rate_rl;
        if v.slide_step & 0x8000 != 0 {
            v.period = v.period.wrapping_sub(v.slide_step & 0x7FFF);
            160
        } else {
            v.period = v.period.wrapping_add(v.slide_step);
            152
        }
    }

    fn vibrato(&mut self, c: usize) {
        let tie = self.byte(self.v[c].pat.wrapping_sub(1)) == 0x8C;
        let v = &mut self.v[c];
        let runs;
        if tie {
            self.cy += 43;
            runs = true;
        } else if v.len.wrapping_sub(v.row) >= v.vib_delay {
            self.cy += 88;
            runs = true;
        } else {
            self.cy += 104;
            runs = false;
        }
        if !runs {
            return;
        }
        if v.vib_depth == 0 {
            self.cy += 37;
            return;
        }
        v.vib_cnt = v.vib_cnt.wrapping_sub(1);
        let turn = v.vib_cnt == 0;
        if v.f_dir {
            v.period = v.period.wrapping_add(v.vib_depth as u16);
            if turn {
                v.f_dir = false;
                v.vib_cnt = v.vib_period;
            }
        } else {
            v.period = v.period.wrapping_sub(v.vib_depth as u16);
            if turn {
                v.f_dir = true;
                v.vib_cnt = v.vib_period;
            }
        }
        self.cy += if turn { 151 } else { 111 };
    }

    /// The envelope, the two slides and the vibrato of a voice's tick without a pattern read.
    fn maintain(&mut self, c: usize) {
        self.envelope(c);
        for ns in [true, false] {
            let on = if ns { self.v[c].f_ns } else { self.v[c].f_slide };
            if on {
                self.cy += 43;
                let s = self.slide_step(c);
                self.cy += s;
            } else {
                self.cy += 59;
            }
        }
        self.vibrato(c);
    }

    // -----------------------------------------------------------------------------
    // Pattern read (2.4, 5.3 to 5.5, 6.4)

    fn select_instrument(&mut self, c: usize, n: usize) {
        let rec = self.instr + 12 * n;
        let b = |i: usize| self.byte(rec + i);
        let s = b(0) as usize;
        let (attack, decay, delays, relb, dbyte) = (b(3), b(4), b(5), b(7), b(10));
        let (tr, iv, rp, vd, vdep) = (b(1) as i8, b(2), b(6), b(8), b(9));
        let e = 12 * s;
        let (ap, al, lp, ll) = (
            self.bank_addr.wrapping_add(self.sample_long(e)),
            be16(&self.bank, e + 4),
            self.bank_addr.wrapping_add(self.sample_long(e + 6)),
            be16(&self.bank, e + 10),
        );
        let v = &mut self.v[c];
        v.f_slide = false;
        v.f_ns = false;
        v.f_dir = false;
        v.f_sor = false;
        v.att_ptr = ap;
        v.att_len = al;
        v.loop_ptr = lp;
        v.loop_len = ll;
        v.trans = tr;
        v.ivol = iv;
        v.att_byte = attack;
        v.dec_byte = decay;
        v.att_cnt = delays >> 4;
        v.att_rl = delays >> 4;
        v.dec_cnt = delays & 15;
        v.dec_rl = delays & 15;
        v.rel_pt = rp;
        v.rel_cnt = relb >> 4;
        v.rel_rl = relb >> 4;
        v.rel_step = relb & 15;
        v.vib_delay = vd;
        v.vib_depth = vdep;
        v.vib_period = dbyte.wrapping_mul(2);
    }

    /// The position-list read (0x80, 0x90; 6.4); returns false when the voice must end.
    fn list_read(&mut self, c: usize) -> bool {
        for _ in 0..8 {
            self.cy += 85 + if c == 0 { 20 } else { 0 } + 17;
            let l = self.v[c].list;
            let w = self.word(l);
            self.v[c].list = l + 2;
            if w != 0 {
                self.cy += 155;
                self.v[c].pat = w as usize;
                return true;
            }
            self.marks[c] = true;
            let used = |i: usize| be16(&self.song, 2 * i) != 0;
            if (0..4).all(|i| !used(i) || self.marks[i]) {
                self.cy += [192, 229, 262, 285][c];
                self.host();
                self.songend = true;
                self.marks = [false; 4];
                self.cy += 44;
            } else {
                self.cy += [136, 173, 206, 229][c];
            }
            let l = self.v[c].list;
            self.v[c].list = self.word(l) as usize;
            self.cy += 213;
        }
        false
    }

    fn start_note(&mut self, hw: &mut Hw, c: usize, n: u8) {
        self.v[c].note = n;
        let pc = self.compute_period(c);
        let v = &mut self.v[c];
        v.att_steps = v.att_byte & 15;
        v.att_size = v.att_byte >> 4;
        v.dec_steps = v.dec_byte & 15;
        v.dec_size = v.dec_byte >> 4;
        v.level = v.ivol;
        v.row = v.len;
        v.rel_on = false;
        v.vib_cnt = v.vib_period >> 1;
        self.cy += 334 + pc;
        let (ap, al) = (self.v[c].att_ptr, self.v[c].att_len);
        self.at(hw);
        hw.loc(c, ap);
        self.cy += 40;
        self.at(hw);
        hw.len(c, al);
        self.int_acc |= INTENA_SET[c];
        self.dma_acc |= DMA_SET[c];
        self.cy += INTENA_COST[c];
        self.put(hw, 0xDFF09A, 0x0080 << c);
        self.cy += 40;
        self.put(hw, 0xDFF096, 1 << c);
        self.cy += 36;
        let r = self.int_acc & 0x0780;
        self.put(hw, 0xDFF09C, r);
        self.cy += 16;
    }

    fn read_pattern(&mut self, hw: &mut Hw, c: usize) -> End {
        for _ in 0..4096 {
            let p = self.v[c].pat;
            let b = self.byte(p);
            self.v[c].pat = p + 1;
            self.cy += 19;
            match b {
                0..=0x7F => {
                    self.start_note(hw, c, b);
                    return End::Done;
                }
                0xE0..=0xFF => {
                    self.v[c].len = b - 0xDF;
                    self.cy += 47;
                }
                0xC0..=0xDF => {
                    self.select_instrument(c, (b - 0xC0) as usize);
                    self.cy += 678;
                }
                0xB0..=0xBF => {
                    self.v[c].ivol = b - 0xB0;
                    self.cy += 83;
                }
                0xA0..=0xAF => self.cy += 178,
                0x80..=0x99 => {
                    self.cy += 110;
                    if let Some(e) = self.command(hw, c, b) {
                        return e;
                    }
                }
                _ => {
                    self.v[c].active = false;
                    return End::Dead;
                }
            }
        }
        self.v[c].active = false;
        End::Dead
    }

    fn arg(&mut self, c: usize) -> u8 {
        let p = self.v[c].pat;
        self.v[c].pat = p + 1;
        self.byte(p)
    }

    /// A command byte 0x80..0x99 after its common 110 cycles; `Some` ends the read.
    fn command(&mut self, hw: &mut Hw, c: usize, b: u8) -> Option<End> {
        match b {
            0x80 => {
                self.v[c].f_ns = false;
                if !self.list_read(c) {
                    self.v[c].active = false;
                    return Some(End::Dead);
                }
            }
            0x81 => {
                self.v[c].glide = 1;
                self.cy += 26;
            }
            0x82 => {
                self.v[c].glide = -1;
                self.cy += 26;
            }
            0x83 => {
                let p = self.arg(c);
                let r = self.arg(c);
                let v = &mut self.v[c];
                v.rel_pt = p;
                v.rel_cnt = r >> 4;
                v.rel_rl = r >> 4;
                v.rel_step = r & 15;
                self.cy += 100;
            }
            0x84 | 0x87 | 0x89 => {
                match b {
                    0x87 => self.v[c].f_sor = false,
                    0x89 => self.v[c].f_ns = false,
                    _ => {}
                }
                self.cy += 30;
            }
            0x85 | 0x86 => {
                let d = self.arg(c);
                let lo = self.arg(c);
                let hi = self.arg(c);
                let rate = self.arg(c);
                let n = self.arg(c);
                let v = &mut self.v[c];
                if b == 0x85 {
                    v.f_slide = true;
                } else {
                    v.f_sor = true;
                }
                v.slide_delay = d;
                v.slide_step = (hi as u16) << 8 | lo as u16;
                v.rate_cnt = rate;
                v.rate_rl = rate;
                v.slide_cnt = n;
                self.cy += if b == 0x85 { 126 } else { 136 };
            }
            0x88 => {
                let olo = self.arg(c);
                let ohi = self.arg(c);
                let lo = self.arg(c);
                let hi = self.arg(c);
                let n = self.arg(c);
                let v = &mut self.v[c];
                v.ns_off = (ohi as u16) << 8 | olo as u16;
                v.slide_step = (hi as u16) << 8 | lo as u16;
                v.slide_rl = n;
                v.slide_delay = 0;
                v.rate_cnt = 0;
                v.rate_rl = 0;
                v.f_ns = true;
                self.cy += 158;
            }
            0x8A => {
                let d = self.arg(c);
                let depth = self.arg(c);
                let half = self.arg(c);
                let v = &mut self.v[c];
                v.vib_delay = d;
                v.vib_depth = depth;
                v.vib_period = half.wrapping_mul(2);
                self.cy += 66;
            }
            0x8B => {
                self.master = self.arg(c) as u16;
                self.cy += 38;
            }
            0x8C => {
                let v = &mut self.v[c];
                v.row = v.len;
                self.cy += 52;
                return Some(End::Done);
            }
            0x8D => {
                let v = &mut self.v[c];
                v.level = 0;
                v.att_steps = 0;
                v.dec_steps = 0;
                v.row = v.len;
                self.cy += 100;
                return Some(End::Done);
            }
            0x8E => {
                self.v[c].trans = self.arg(c) as i8;
                self.cy += 26;
            }
            0x8F => {
                self.cy += 60;
                self.stop(hw);
                return Some(End::Stopped);
            }
            0x90 => {
                self.v[c].list = ((self.arg(c) as usize) << 8) | self.arg(c) as usize;
                self.v[c].f_ns = false;
                self.cy += 48;
                if !self.list_read(c) {
                    self.v[c].active = false;
                    return Some(End::Dead);
                }
            }
            0x91 => {
                let v = &mut self.v[c];
                v.active = false;
                v.level = 0;
                self.cy += 64;
                self.at(hw);
                hw.vol(c, 0);
                self.cy += 36;
                self.put(hw, 0xDFF096, 1 << c);
                self.cy += 16;
                return Some(End::Done);
            }
            0x92 => {
                self.fade_speed = self.arg(c);
                self.fade_cnt = self.arg(c);
                self.cy += 42;
            }
            0x93 => {
                let v = &mut self.v[c];
                if v.ivol < 15 {
                    v.ivol += 1;
                    self.cy += 53;
                } else {
                    self.cy += 27;
                }
            }
            0x94 => {
                let v = &mut self.v[c];
                if v.ivol > 0 {
                    v.ivol -= 1;
                    self.cy += 47;
                } else {
                    self.cy += 31;
                }
            }
            0x95 => {
                self.speed = self.arg(c);
                self.cy += 26;
            }
            0x96 | 0x97 => {
                self.cy += 40;
                self.at(hw);
                let x = hw.bus.read_byte(0xBFE001);
                hw.bus.write_byte(0xBFE001, if b == 0x97 { x & !2 } else { x | 2 });
                self.cy += 10;
            }
            0x99 => {
                self.stop_next = true;
                self.cy += 26;
            }
            _ => {
                // 0x98, a built-in tune: not specified; the voice ends
                self.v[c].active = false;
                return Some(End::Dead);
            }
        }
        None
    }

    // -----------------------------------------------------------------------------
    // One voice (5.2)

    fn voice(&mut self, hw: &mut Hw, c: usize, row_tick: bool) {
        self.cy += 26;
        if !self.v[c].active {
            self.cy += 37;
            return;
        }
        self.cy += 73;
        let per = self.v[c].period;
        self.at(hw);
        hw.per(c, per);
        let idx = ((self.v[c].level as u32 * self.master as u32) & 0xFFFF) >> 4;
        let val = volume(idx as usize) as u32;
        let out = (((val * 64) & 0xFFFF) >> 6) as u16;
        self.cy += VOL1[c];
        self.at(hw);
        hw.vol(c, out);
        self.cy += VOL2[c];
        self.at(hw);
        hw.vol(c, out);
        self.cy += 53;
        if !row_tick {
            self.maintain(c);
            self.cy += 16;
            return;
        }
        self.cy += 25;
        let v = &mut self.v[c];
        v.row = v.row.wrapping_sub(1);
        if v.row == 0 {
            match self.read_pattern(hw, c) {
                End::Done | End::Dead | End::Stopped => {}
            }
            return;
        }
        // glide
        if self.v[c].glide != 0 {
            let v = &mut self.v[c];
            v.note = v.note.wrapping_add(v.glide as u8);
            let pc = self.compute_period(c);
            self.cy += 77 + pc;
        } else {
            self.cy += 55;
        }
        // release check
        let next = self.byte(self.v[c].pat);
        let v = &mut self.v[c];
        if v.rel_pt == v.row {
            if next == 0x8C {
                self.cy += 88;
            } else {
                v.rel_on = true;
                v.glide = 0;
                v.f_slide = v.f_sor;
                self.cy += if v.f_sor { 185 } else { 165 };
            }
        } else {
            self.cy += 67;
        }
        self.maintain(c);
        self.cy += 16;
    }

    fn play_tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 443;
        let d = std::mem::take(&mut self.dma_acc);
        let i = std::mem::take(&mut self.int_acc);
        self.put(hw, 0xDFF096, d);
        self.cy += 36;
        self.put(hw, 0xDFF09C, i & 0x0780);
        self.cy += 24;
        self.put(hw, 0xDFF09A, i);
        if self.stop_next {
            self.stop_next = false;
            self.cy += 60;
            self.stop(hw);
        }
        if !self.playing && self.start_req != 0 {
            let r = self.start_req;
            self.start_req = 0;
            if r >= 2 {
                self.songend = true;
                self.playing = false;
                self.start_req = 1;
                self.cy += 202;
            } else {
                self.start(hw);
            }
        } else {
            self.cy += 202;
            if !self.playing {
                if !self.v.iter().any(|v| v.active) {
                    hw.bus.clock = self.t0 + self.cy / 2;
                    return;
                }
                self.speed_cnt = 1;
            }
        }
        self.speed_cnt = self.speed_cnt.wrapping_sub(1);
        let row_tick = self.speed_cnt == 0;
        for c in 0..4 {
            self.voice(hw, c, row_tick);
        }
        // fade (5.9)
        if self.fade_speed == 0 {
            self.cy += 55;
        } else {
            self.fade_cnt = self.fade_cnt.wrapping_sub(1);
            if self.fade_cnt == 0 {
                self.fade_cnt = self.fade_speed;
                self.master = self.master.wrapping_sub(1);
                self.cy += 125;
                if self.master == 0 {
                    self.cy += 60;
                    self.stop(hw);
                }
            } else {
                self.cy += 80;
            }
        }
        if self.speed_cnt == 0 {
            self.speed_cnt = self.speed;
            self.cy += 345;
        } else {
            self.cy += 325;
        }
        hw.bus.clock = self.t0 + self.cy / 2;
    }

    /// The audio interrupt handler (5.11, 6.5).
    fn handler(&mut self, ch: usize, hw: &mut Hw) {
        // the handler reads the request flags after its prologue, not at the call: a request that
        // Paula raises in between is served in this call (the reference's TUNE1 and TUNE6 show it)
        let c = hw.bus.clock + S_READ;
        hw.bus.paula.sync(c, &hw.bus.ram);
        let s = hw.bus.paula.intreq & hw.bus.paula.intena & 0x0780;
        if s & (0x80 << ch) == 0 {
            return;
        }
        self.t0 = hw.bus.clock;
        self.cy = 188;
        for c in 0..4 {
            if s & (0x80 << c) != 0 {
                self.cy += 47;
                let (lp, ll) = (self.v[c].loop_ptr, self.v[c].loop_len);
                self.at(hw);
                hw.loc(c, lp);
                self.cy += 20;
                self.at(hw);
                hw.len(c, ll);
            } else {
                self.cy += 19;
            }
        }
        self.cy += 16;
        self.put(hw, 0xDFF09C, s);
        self.cy += 16;
        self.put(hw, 0xDFF09A, s);
        self.cy += 148;
        hw.bus.clock = self.t0 + self.cy / 2;
    }
}

impl Replayer for PaulTonge {
    fn format(&self) -> &'static str {
        "Paul Tonge"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.play_tick(hw);
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn audio_irq_ch(&mut self, ch: usize, hw: &mut Hw) -> bool {
        self.handler(ch, hw);
        true
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let dir_len = m.bus.read_cstr(m.deli.dir_buf).len();
    let mut bank = crate::deli::read_companion(m, &bank_name(&file)).1;
    let own_bank = bank.is_ok();
    if bank.is_err() {
        bank = crate::deli::read_companion(m, "SMP.set").1;
    }
    let bank = bank.ok()?;
    if bank.len() < 12 {
        return None;
    }
    // the reference's host spends the loading time before the song starts: the song starts
    // (and its audio output with it) `first_delay` after the engine's origin, and the first tick
    // follows one standard period later, as in the reference
    m.bus.clock += first_tick_delay(d, dir_len, file.len(), own_bank);
    let bank_addr = m.alloc(bank.len() as u32 + 16);
    if bank_addr == 0 {
        return None;
    }
    m.bus.write_bytes(bank_addr, &bank);
    let bank_rel = if d.len() <= 0x400 {
        0x800
    } else if d.len() <= 0x800 {
        0x810
    } else {
        0xC08
    };
    let mut v: [Voice; 4] = Default::default();
    for x in v.iter_mut() {
        x.level = 16;
        x.len = 4;
    }
    Some(Box::new(PaulTonge {
        song: d.to_vec(),
        bank,
        bank_addr,
        bank_rel,
        v,
        start_req: 1,
        playing: false,
        stop_next: false,
        speed: 0,
        speed_cnt: 0,
        master: 0,
        fade_speed: 0,
        fade_cnt: 0,
        int_acc: 0,
        dma_acc: 0,
        instr: 0,
        marks: [false; 4],
        songend: false,
        t0: 0,
        cy: 0,
    }))
}

/// The first call of the play routine in the reference, which the host's file loading moves (by
/// the path and file name lengths, by whether the song's own bank was found, and by what its
/// loader does with the three later position lists of the file): as colour clocks after the
/// engine's standard first tick.
fn first_tick_delay(d: &[u8], dir_len: usize, name_len: usize, own_bank: bool) -> u64 {
    let (dl, nl) = (dir_len as i64 - DIR_REF_LEN, name_len as i64 - NAME_REF_LEN);
    // twice the colour clocks, so that the half clocks round up as the reference's do
    let half = if own_bank { 2 * FIRST_CALL_OWN_BANK + 34 * dl } else { 2 * FIRST_CALL_FALLBACK + 51 * dl } + 21 * nl;
    // (a bank of the song's own name: a path of odd length takes one half clock off)
    let half = if own_bank { half - (dir_len & 1) as i64 } else { half };
    let mut cc = (half + 1).div_euclid(2);
    // a list offset of 0 in the header (bits: lists 1, 2, 3) skips that list's check
    let off = |i: usize| if d.len() >= 8 { be16(d, 2 * i) as usize } else { 0 };
    let mask = (1..4).fold(0usize, |m, i| m | usize::from(off(i) == 0) << (i - 1));
    cc += [0, -37, -45, -72, -45, -81, -89, -111][mask];
    // every list that is there and whose first pattern follows the stop byte 0x8F (not 0x80)
    // costs 27 cycles more
    let stops = (1..4)
        .filter(|&i| {
            let o = off(i);
            o != 0 && o + 2 <= d.len() && {
                let e = be16(d, o) as usize;
                e >= 1 && e < d.len() && d[e - 1] == 0x8F
            }
        })
        .count() as i64;
    cc += 27 * stops / 2;
    (FIRST_TICK_DELAY as i64 + cc - 98306).max(0) as u64
}

#[cfg(test)]
#[path = "paultonge_tests.rs"]
mod tests;

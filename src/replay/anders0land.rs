//! Anders 0land (Anders "Zonix" 0land, 1993; the Psygnosis game Prime Mover; Eagleplayer
//! "Anders 0land", module prefix `hot.`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes of
//! the 68k reference player; no original code, nothing embedded (the 70-entry period table is
//! written from the specification's values). A module is three chunks: a driver chunk (`mpl`, never
//! run: the replay below uses only the tables of the others), the song data (`mdt`) and raw signed
//! 8-bit samples (`msm`). Four voices, a tick counter that steps voices 0 and 1 on its first tick
//! of a row and voices 2 and 3 on the second, order lists with transposes, patterns of rows with
//! durations, notes, ties, alternate instruments and command words (tempo, pitch slide, fade),
//! volume envelopes, vibrato. Every register write is placed at the clock of the original's code,
//! from the specification's cycle ledger.
//!
//! Choices where the specification is silent or says "undefined" (all safe; nothing can panic or
//! hang, every read outside the file gives zero and every loop is bounded by the tick):
//! - detection also demands a chunk size of at least 8 (the specification only checks the sum);
//! - pattern, instrument, sample, envelope and command numbers above the specified limits index
//!   the tables plainly (no 8-bit wrap); what lies there is read as it is, past the file as zero;
//!   so a pattern count of 200 plays whatever that data is;
//! - a note index above 69 (note plus transpose) takes the period 0x050, the last ordinary entry;
//! - the fade-start entry (a subsong number with bit 15 set) is not provided: the subsong number
//!   is clamped to the song's range;
//! - a song with no subsong table entries is taken to have one;
//! - the volume, balance and mute paths of the host are not modelled (the volume written is the
//!   envelope volume & 0x7F).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Anders 0land", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;
/// From the start block's last write to the first tick, beyond one period.
/// The engine zeroes the audio at the clock `new` leaves; the reference's audio starts this much
/// after its start block's last write (fitted to its WAV renders, whole output frames).
const ORIGIN: u64 = 3327;
const FIRST_EXTRA: u64 = 74267 - TICK - ORIGIN;

const PERIODS: [u16; 70] = [
    0xD60, 0xCA0, 0xBE8, 0xB40, 0xA98, 0xA00, 0x970, 0x8E8, 0x868, 0x7F0, 0x780, 0x714, 0x6B0, 0x650, 0x5F4, 0x5A0,
    0x54C, 0x500, 0x4B8, 0x474, 0x434, 0x3F8, 0x3C0, 0x38A, 0x358, 0x328, 0x2FA, 0x2D0, 0x2A6, 0x280, 0x25C, 0x23A,
    0x21A, 0x1FC, 0x1E0, 0x1C5, 0x1AC, 0x194, 0x17D, 0x168, 0x153, 0x140, 0x12E, 0x11D, 0x10D, 0x0FE, 0x0F0, 0x0E2,
    0x0D6, 0x0CA, 0x0BE, 0x0B4, 0x0AA, 0x0A0, 0x097, 0x08F, 0x087, 0x07F, 0x078, 0x072, 0x06B, 0x065, 0x05F, 0x055,
    0x050, 0x050, 0x050, 0x050, 0x050, 0x8000,
];

// Cost items, in 68000 cycles (specification section 9).
const ENTRY: u64 = 242;
const PLAY_PROLOGUE: u64 = 76;
const DISP_PHASE0: u64 = 130;
const DISP_PHASE1: u64 = 41;
const DISP_LATE: u64 = 20;
const VS_HEAD: u64 = 23;
const VS_BUSY: u64 = 25;
const VS_WAIT: u64 = 18;
const VS_TAIL: u64 = 43;
const EV_START: u64 = 143;
const EV_PATEND: u64 = 67;
const EV_RESTART: u64 = 60;
const EV_RESTART_END: u64 = 10;
const EV_STOPCHK: u64 = 17;
const EV_STOP: u64 = 100;
const EV_STOP_END: u64 = 10;
const EV_TRANSCHK: u64 = 19;
const EV_TRANS: u64 = 86;
const EV_NEWPAT: u64 = 176;
const DC_FLAG7: u64 = 19;
const DC_INSTR: u64 = 50;
const DC_FLAG6: u64 = 19;
const DC_CMD: u64 = 95;
const DC_CMD_FADE: u64 = 42;
const DC_CMD_T: u64 = 19;
const DC_CMD_TEMPO: u64 = 22;
const DC_CMD_S: u64 = 19;
const DC_CMD_DOWN: u64 = 36;
const DC_CMD_UP: u64 = 42;
const DC_FLAG5: u64 = 19;
const DC_SHORT: u64 = 48;
const DC_NOTE2: u64 = 48;
const DC_LEN_CHK: u64 = 19;
const DC_LEN_NOTE: u64 = 48;
const DC_LEN_NONOTE: u64 = 38;
const IN_CHK: u64 = 19;
const IN_ALT_CHK: u64 = 21;
const IN_ALT: u64 = 164;
const IN_PRI: u64 = 182;
const FIN: u64 = 51;
const FIN_C0: u64 = 17;
const FIN_C0_INC: u64 = 4;
const FIN_NOTE: u64 = 41;
const FIN_TIE: u64 = 16;
const FIN_PERIOD: u64 = 60;
const DM_HEAD: u64 = 37;
const DM_OFF: u64 = 62;
const DM_ON: u64 = 49;
const DM_SETUP: u64 = 60;
const DM_V_C1: u64 = 21;
const DM_V_C2: u64 = 21;
const DM_V_WR: u64 = 108;
const DM_V_NEXT: u64 = 29;
const RC: u64 = 51;
const RE_START: u64 = 60;
const RE_C_MASK: u64 = 23;
const RE_C_S7: u64 = 23;
const RE_C_S8: u64 = 23;
const RE_C_S6: u64 = 21;
const RE_VIB: u64 = 184;
const RE_PTR1: u64 = 156;
const RE_PTR2: u64 = 76;
const RE_ENV: u64 = 105;
const RE_ENV_SET: u64 = 14;
const RE_COPY: u64 = 48;
const RE_NEXT: u64 = 35;
const EN_HEAD: u64 = 44;
const EN_C1: u64 = 23;
const EN_C2: u64 = 23;
const EN_C3: u64 = 23;
const VB_CHK: u64 = 44;
const VB_DCHK: u64 = 23;
const VB_DEC: u64 = 28;
const VB_RUN: u64 = 105;
const VB_FLIP: u64 = 42;
const EN_CHK: u64 = 21;
const EN_CCHK: u64 = 23;
const EN_STEP: u64 = 64;
const EN_FETCH: u64 = 55;
const EN_FETCH_SET: u64 = 24;
const EN_FETCH_REST: u64 = 72;
const FD_CHK: u64 = 25;
const FD_RUN: u64 = 275;
const FD_END: u64 = 20;
const EN_NEXT: u64 = 35;
const FL_HEAD: u64 = 16;
const FL_CHK: u64 = 21;
const FL_PERIOD: u64 = 110;
const FL_NEXT: u64 = 37;
const SONGEND: [u64; 4] = [132, 153, 170, 177];
const SONGEND_RAISE: u64 = 68;
const LENSTORE: [u64; 4] = [79, 104, 129, 137];
const ADRSTORE: [u64; 4] = [95, 120, 145, 153];
const VOLSTORE: [u64; 4] = [226, 243, 260, 267];
const VOLSTORE_WRITE: [u64; 4] = [176, 193, 210, 227];
// Write offsets inside items.
const W_DM_OFF: u64 = 52;
const W_DM_ON: u64 = 24;
const W_DM_V_LEN: u64 = 46;
const W_DM_V_ADR: u64 = 108;
const W_RE_LEN: u64 = 138;
const W_RE_ADR: u64 = 58;
const W_FL_PERIOD: u64 = 78;

/// The three chunks' start offsets, when the file is a module (specification 1.1).
fn chunks(d: &[u8]) -> Option<[usize; 3]> {
    let mut rem = d.len() as u64;
    let mut pos = 0usize;
    let mut at = [0usize; 3];
    for (i, tag) in [b"mpl", b"mdt", b"msm"].iter().enumerate() {
        at[i] = pos;
        let head = d.get(pos..pos.checked_add(8)?)?;
        let size = u32::from_be_bytes([head[4], head[5], head[6], head[7]]) as u64;
        if size < 8 || size > rem {
            return None;
        }
        rem -= size;
        if &head[..3] != *tag || (i < 2 && size & 1 != 0) {
            return None;
        }
        pos = pos.checked_add(size as usize)?;
    }
    Some(at)
}

fn detect(d: &[u8]) -> bool {
    chunks(d).is_some()
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    order_idx: u8,
    restart: u8,
    /// Start of the order list in the file.
    order: usize,
    /// Position of the current pattern in the file, `None` for the empty pattern (a lone end byte).
    pat: Option<usize>,
    pos: usize,
    instr: u8,
    wave: u8,
    snap: u8,
    transpose: u8,
    dur: u8,
    target: u16,
    cur: u16,
    slide: u16,
    applied: u16,
    vib_delay: u8,
    vib_step: u8,
    vib_half: u8,
    vib_counter: u8,
    vib_acc: u16,
    vib_out: u8,
    env_idx: u8,
    env_step: u8,
    env_count: u8,
    vol: u8,
    newnote: bool,
    tie: bool,
    alt: bool,
    prevnote: bool,
    note_present: bool,
}

pub struct Anders0land {
    d: Vec<u8>,
    /// Guest address of the module.
    base: u32,
    /// Chunk 2 body and chunk 3 body in the file.
    song: usize,
    smp: usize,
    nsub: usize,
    sub: usize,
    speed: u8,
    t: u8,
    song_mask: u8,
    fade: bool,
    fade_ctr: u16,
    /// Song-end flags (bit set = voice has not yet ended).
    end_flags: u8,
    songend: bool,
    v: [Voice; 4],
    /// Clock of the tick's start and cycles since.
    t0: u64,
    cy: u64,
    /// Clock of the first tick after `set_subsong`.
    first_at: u64,
    /// The clock of the current subsong's CIA line.
    cia_t: u64,
}

impl Anders0land {
    fn b(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }
    fn l(&self, o: usize) -> u32 {
        be32(&self.d, o)
    }
    /// A header word of the song data.
    fn hw(&self, i: usize) -> usize {
        self.w(self.song + 2 * i) as usize
    }

    fn c(&mut self, n: u64) {
        self.cy += n;
    }
    /// Places the next write `off` cycles into an item that starts now.
    fn at(&self, hw: &mut Hw, off: u64) {
        hw.bus.clock = self.t0 + (self.cy + off) / 2;
    }

    fn period(&self, idx: u8) -> u16 {
        PERIODS.get(idx as usize).copied().unwrap_or(0x050)
    }

    /// Resets all state and starts subsong `n` (specification 4.1).
    fn init(&mut self, n: usize) {
        let st = self.song + self.hw(1) + 4 * n;
        let sub = self.song + self.hw(2) + 4 * n;
        self.speed = self.b(sub + 3);
        let active = self.b(sub);
        self.song_mask = self.b(sub + 2);
        let vt = self.song + self.hw(0);
        self.v = [Voice::default(); 4];
        for v in 0..4 {
            let x = &mut self.v[v];
            x.active = active & 1 << v != 0;
            x.restart = self.d.get(st + v).copied().unwrap_or(0);
            let rel = be32(&self.d, vt + 4 * v) as usize;
            self.v[v].order = vt.wrapping_add(rel);
        }
        self.fade = false;
        self.fade_ctr = 0;
        self.t = 0;
        self.end_flags = 15;
        self.songend = false;
        self.sub = n;
    }

    /// Specification 4.9: the song-end bookkeeping of voice `v`.
    fn song_end(&mut self, v: usize) {
        self.end_flags &= !(1 << v);
        self.c(SONGEND[v]);
        if self.end_flags == 0 {
            self.c(SONGEND_RAISE);
            // the host's time step: cycles to colour clocks (rounded down) plus 8
            self.cy = (self.cy / 2 + 8) * 2;
            self.end_flags = 15;
            self.songend = true;
        }
    }

    fn pat_byte(&self, v: usize, p: usize) -> u8 {
        match self.v[v].pat {
            Some(base) => self.b(base.wrapping_add(p)),
            None => 0xFF,
        }
    }

    /// Specification 4.4.
    fn voice_step(&mut self, v: usize) {
        self.c(VS_HEAD);
        if !self.v[v].active {
            self.c(VS_TAIL);
            return;
        }
        self.c(VS_BUSY);
        if self.v[v].dur != 0 {
            self.v[v].dur -= 1;
            self.c(VS_WAIT + VS_TAIL);
            return;
        }
        {
            let x = &mut self.v[v];
            x.slide = 0;
            x.note_present = false;
            x.newnote = true;
            x.tie = false;
        }
        self.c(EV_START);
        let mut f = self.pat_byte(v, self.v[v].pos);
        if f == 0xFF {
            self.c(EV_PATEND);
            let mut bb = self.b(self.v[v].order.wrapping_add(self.v[v].order_idx as usize));
            if bb == 0xFF {
                self.v[v].order_idx = self.v[v].restart;
                bb = self.b(self.v[v].order.wrapping_add(self.v[v].order_idx as usize));
                self.c(EV_RESTART);
                self.song_end(v);
                self.c(EV_RESTART_END);
            } else {
                self.c(EV_STOPCHK);
                if bb == 0xFE {
                    let x = &mut self.v[v];
                    x.active = false;
                    x.env_idx = 0;
                    x.newnote = true;
                    x.tie = false;
                    self.c(EV_STOP);
                    self.song_end(v);
                    self.c(EV_STOP_END);
                    self.c(VS_TAIL);
                    return;
                }
            }
            self.c(EV_TRANSCHK);
            if bb >= 0x80 {
                self.v[v].transpose = bb & 0x7F;
                self.v[v].order_idx = self.v[v].order_idx.wrapping_add(1);
                bb = self.b(self.v[v].order.wrapping_add(self.v[v].order_idx as usize));
                self.c(EV_TRANS);
            }
            self.v[v].order_idx = self.v[v].order_idx.wrapping_add(1);
            // pattern table: longs relative to the table itself
            let pt = self.song + self.hw(5);
            let off = self.l(pt.wrapping_add(4 * bb as usize)) as usize;
            let base = pt.wrapping_add(off);
            self.v[v].pat = Some(base);
            self.v[v].pos = 0;
            f = self.b(base);
            self.c(EV_NEWPAT);
        }
        let p = self.v[v].pos;
        self.c(DC_FLAG7);
        let mut x = 0u8;
        let mut len_rule = false;
        if f & 0x80 != 0 {
            x = self.pat_byte(v, p + 1);
            self.v[v].instr = x & 0x7F;
            self.c(DC_INSTR);
            len_rule = true;
        } else {
            self.c(DC_FLAG6);
            if f & 0x40 != 0 {
                x = self.pat_byte(v, p + 1);
                let cw = self.w(self.song + self.hw(8) + 2 * (x & 0x7F) as usize);
                self.c(DC_CMD);
                if cw & 0x4000 != 0 {
                    self.fade = true;
                    self.c(DC_CMD_FADE);
                } else {
                    self.c(DC_CMD_T);
                    if cw & 0x2000 != 0 {
                        self.speed = cw as u8;
                        self.c(DC_CMD_TEMPO);
                    } else {
                        self.c(DC_CMD_S);
                        if cw & 0x1000 == 0 {
                            self.v[v].slide = (cw & 0xFFF).wrapping_neg();
                            self.c(DC_CMD_DOWN);
                        } else {
                            self.v[v].slide = cw & 0xFFF;
                            self.c(DC_CMD_UP);
                        }
                    }
                }
                len_rule = true;
            } else {
                self.c(DC_FLAG5);
                if f & 0x20 != 0 {
                    self.v[v].note_present = false;
                    self.v[v].pos += 1;
                    self.c(DC_SHORT);
                } else {
                    self.v[v].note_present = true;
                    self.v[v].pos += 2;
                    self.c(DC_NOTE2);
                }
            }
        }
        if len_rule {
            self.c(DC_LEN_CHK);
            if x & 0x80 == 0 {
                self.v[v].pos += 3;
                self.v[v].note_present = true;
                self.c(DC_LEN_NOTE);
            } else {
                self.v[v].pos += 2;
                self.v[v].note_present = false;
                self.c(DC_LEN_NONOTE);
            }
        }
        // instrument load
        self.c(IN_CHK);
        let ins = self.song + self.hw(6) + 8 * self.v[v].instr as usize;
        if f & 0x20 != 0 {
            self.c(IN_ALT_CHK);
            if !self.v[v].alt {
                self.v[v].alt = true;
                self.v[v].env_idx = self.b(ins + 1) & 0x7F;
                self.v[v].wave = self.b(ins + 3);
                self.c(IN_ALT);
            }
        } else {
            self.v[v].alt = false;
            self.v[v].env_idx = self.b(ins) & 0x7F;
            self.v[v].wave = self.b(ins + 2);
            self.c(IN_PRI);
        }
        self.v[v].dur = f & 0x1F;
        self.c(FIN);
        if self.v[v].note_present {
            self.c(FIN_C0);
            let mut no = p + 1;
            if f & 0xC0 != 0 {
                no += 1;
                self.c(FIN_C0_INC);
            }
            let n = self.pat_byte(v, no);
            self.c(FIN_NOTE);
            if n & 0x80 != 0 {
                self.v[v].tie = true;
                self.c(FIN_TIE);
            }
            let idx = (n & 0x7F).wrapping_add(self.v[v].transpose);
            self.v[v].target = self.period(idx);
            self.c(FIN_PERIOD);
        }
        self.c(VS_TAIL);
    }

    fn flags(&self, f: impl Fn(&Voice) -> bool) -> u16 {
        self.v.iter().enumerate().fold(0, |m, (i, v)| m | (f(v) as u16) << i)
    }

    /// Specification 4.5.
    fn dma_step(&mut self, hw: &mut Hw, t: u8) {
        self.c(DM_HEAD);
        if self.speed.wrapping_sub(1) == t {
            let val = self.flags(|v| v.newnote) ^ (self.flags(|v| v.tie) & 15);
            self.at(hw, W_DM_OFF);
            hw.dmacon(val & 0x7FFF);
            self.c(DM_OFF);
            return;
        }
        self.at(hw, W_DM_ON);
        hw.dmacon(0x8000 | self.song_mask as u16);
        self.c(DM_ON);
        if t == 1 {
            self.c(DM_SETUP);
            for v in 0..4 {
                self.c(DM_V_C1);
                if self.song_mask & 1 << v != 0 {
                    self.c(DM_V_C2);
                    if self.v[v].prevnote {
                        let i = self.v[v].snap as usize;
                        let len = self.w(self.song + self.hw(12) + 2 * i);
                        let adr = self.l(self.song + self.hw(11) + 4 * i);
                        self.at(hw, W_DM_V_LEN);
                        hw.len(v, len);
                        self.at(hw, W_DM_V_ADR);
                        hw.loc(v, self.base.wrapping_add((self.smp as u32).wrapping_add(adr)));
                        self.c(DM_V_WR);
                    }
                }
                self.c(DM_V_NEXT);
            }
        }
    }

    /// Specification 4.6.
    fn row_end(&mut self, hw: &mut Hw) {
        self.c(RE_START);
        for v in 0..4 {
            self.c(RE_C_MASK);
            if self.song_mask & 1 << v == 0 {
                self.c(RE_NEXT);
                continue;
            }
            self.c(RE_C_S7);
            if !self.v[v].tie {
                self.c(RE_C_S8);
                if !self.v[v].newnote {
                    self.c(RE_NEXT);
                    continue;
                }
                self.c(RE_C_S6);
                if !self.v[v].alt {
                    let ins = self.song + self.hw(6) + 8 * self.v[v].instr as usize;
                    let (d4, s5, h6) = (self.b(ins + 4), self.b(ins + 5), self.b(ins + 6));
                    let x = &mut self.v[v];
                    x.vib_out = 0;
                    x.vib_delay = d4;
                    x.vib_step = s5;
                    x.vib_half = h6;
                    x.vib_counter = h6 / 2;
                    x.vib_acc = 0;
                    self.c(RE_VIB);
                }
                let i = self.v[v].wave as usize;
                let len = self.w(self.song + self.hw(10) + 2 * i);
                self.at(hw, W_RE_LEN);
                hw.len(v, len);
                self.c(RE_PTR1 + LENSTORE[v]);
                let adr = self.l(self.song + self.hw(9) + 4 * i);
                self.at(hw, W_RE_ADR);
                hw.loc(v, self.base.wrapping_add((self.smp as u32).wrapping_add(adr)));
                self.c(RE_PTR2 + ADRSTORE[v]);
                self.v[v].env_count = 0;
                let e = self.song + self.hw(7) + 4 * self.v[v].env_idx as usize;
                let first = self.b(e);
                self.c(RE_ENV);
                if first != 0xFF {
                    self.v[v].vol = first;
                    self.c(RE_ENV_SET);
                }
            }
            let x = &mut self.v[v];
            x.cur = x.target;
            x.applied = x.slide;
            self.c(RE_COPY);
            self.c(RE_NEXT);
        }
    }

    /// Specification 4.7.
    fn envelope_step(&mut self) {
        self.c(EN_HEAD);
        for v in 0..4 {
            self.c(EN_C1);
            if !self.v[v].active {
                self.c(EN_NEXT);
                continue;
            }
            self.c(EN_C2);
            if self.song_mask & 1 << v == 0 {
                self.c(EN_NEXT);
                continue;
            }
            self.c(EN_C3);
            if self.v[v].newnote {
                self.c(EN_NEXT);
                continue;
            }
            self.c(VB_CHK);
            if self.v[v].vib_delay != 0xFF {
                self.c(VB_DCHK);
                if self.v[v].vib_delay != 0 {
                    self.v[v].vib_delay -= 1;
                    self.c(VB_DEC);
                } else {
                    let x = &mut self.v[v];
                    x.vib_acc = x.vib_acc.wrapping_add(((x.vib_step as i8 as i16) * 16) as u16);
                    x.vib_out = (x.vib_acc >> 8) as u8;
                    x.vib_counter = x.vib_counter.wrapping_sub(1);
                    let flip = x.vib_counter == 0;
                    if flip {
                        x.vib_step = x.vib_step.wrapping_neg();
                        x.vib_counter = x.vib_half;
                    }
                    self.c(VB_RUN);
                    if flip {
                        self.c(VB_FLIP);
                    }
                }
            }
            self.c(EN_CHK);
            self.c(EN_CCHK);
            if self.v[v].env_count != 0 {
                let x = &mut self.v[v];
                x.vol = x.vol.wrapping_add(x.env_step);
                x.env_count -= 1;
                self.c(EN_STEP);
            } else {
                let e = self.song + self.hw(7) + 4 * self.v[v].env_idx as usize;
                let (ev, es, ec, en) = (self.b(e), self.b(e + 1), self.b(e + 2), self.b(e + 3));
                self.c(EN_FETCH);
                if ev != 0xFF {
                    self.v[v].vol = ev;
                    self.c(EN_FETCH_SET);
                }
                let x = &mut self.v[v];
                x.env_step = es;
                x.env_count = ec;
                x.env_idx = en;
                self.c(EN_FETCH_REST);
            }
            self.c(FD_CHK);
            if self.fade {
                let x = &mut self.v[v];
                let q = (x.vol / 64) as u32;
                x.vol = (x.vol as u32).wrapping_sub(q.wrapping_mul(self.fade_ctr as u32)) as u8;
                self.fade_ctr = self.fade_ctr.wrapping_add(1);
                self.c(FD_RUN);
                if self.fade_ctr == 0x40 {
                    self.fade = false;
                    self.c(FD_END);
                }
            }
            self.c(EN_NEXT);
        }
    }

    /// Specification 4.8.
    fn output_step(&mut self, hw: &mut Hw) {
        self.c(FL_HEAD);
        for v in 0..4 {
            self.c(FL_CHK);
            if self.song_mask & 1 << v == 0 {
                self.c(FL_NEXT);
                continue;
            }
            let x = &mut self.v[v];
            x.cur = x.cur.wrapping_add(x.applied);
            let p = x.cur.wrapping_add(x.vib_out as i8 as i16 as u16);
            let vol = (x.vol & 0x7F) as u16;
            self.at(hw, W_FL_PERIOD);
            hw.per(v, p);
            self.c(FL_PERIOD);
            self.at(hw, VOLSTORE_WRITE[v]);
            hw.vol(v, vol);
            self.c(VOLSTORE[v]);
            self.c(FL_NEXT);
        }
    }

    /// One call of the replay routine (specification 4.3).
    fn run(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 0;
        self.c(ENTRY + PLAY_PROLOGUE);
        let t = self.t;
        let pair = if t == 1 {
            self.c(DISP_PHASE1);
            Some(2)
        } else if t < 128 && t != 0 {
            self.c(DISP_LATE);
            None
        } else {
            self.c(DISP_PHASE0);
            for x in self.v.iter_mut() {
                x.prevnote = x.newnote;
                x.snap = x.wave;
                x.newnote = false;
                x.tie = false;
            }
            Some(0)
        };
        if let Some(p) = pair {
            self.voice_step(p);
            self.voice_step(p + 1);
        }
        self.dma_step(hw, t);
        self.t = t.wrapping_add(1);
        self.c(RC);
        if self.t == self.speed {
            self.t = 0;
            self.row_end(hw);
        }
        self.envelope_step();
        self.output_step(hw);
    }

    /// The host's end of a stepped-over subsong that started at `t`: DMA clear and four volumes
    /// zero, then its eight zero volumes (LOGS.md).
    fn song_end_block(&mut self, hw: &mut Hw, t: u64) {
        hw.bus.clock = t + 3391;
        hw.dmacon(0x000F);
        for v in 0..4 {
            hw.bus.clock = t + 3399 + 6 * v as u64;
            hw.vol(v, 0);
        }
        const AFTER: [u64; 8] = [3648, 3791, 3943, 4104, 4364, 4507, 4659, 4820];
        for (i, off) in AFTER.iter().enumerate() {
            hw.bus.clock = t + off;
            hw.vol(i & 3, 0);
        }
    }

    /// The host's start of a subsong: DMA clear, four volumes zero (specification 3). `t` is the
    /// CIA write's clock; leaves the clock at the last volume write.
    fn start_block(&mut self, hw: &mut Hw, t: u64) -> u64 {
        hw.bus.clock = t + 10;
        hw.dmacon(0x000F);
        for v in 0..4 {
            hw.bus.clock = t + 20 + 10 * v as u64;
            hw.vol(v, 0);
        }
        t + 50
    }
}

impl Replayer for Anders0land {
    fn format(&self) -> &'static str {
        "Anders 0land"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    /// The first tick follows the start block's last write by 74267 colour clocks.
    fn timer_delay(&self) -> u64 {
        FIRST_EXTRA
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.nsub as u16 - 1, self.sub as u16)
    }


    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let n = (n as usize).min(self.nsub - 1);
        let from = self.sub;
        self.init(n);
        // the host runs the start and the song end of every subsong it steps over (LOGS.md): the
        // song-end block sits 3391 colour clocks after the start, the next start 5501 after it
        let mut t = self.cia_t;
        for _ in from..n {
            self.song_end_block(hw, t);
            t += 5501;
            self.start_block(hw, t);
        }
        if n <= from {
            t = t.max(hw.bus.clock);
        }
        self.cia_t = t;
        let end = if n <= from { self.start_block(hw, t) } else { t + 50 };
        self.first_at = end + 74267;
        hw.bus.clock = hw.bus.clock.max(end + ORIGIN);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        Some(self.first_at)
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let at = chunks(d)?;
    let song = at[1] + 8;
    let mut s = Anders0land {
        d: d.to_vec(),
        base,
        song,
        smp: at[2] + 8,
        nsub: 1,
        sub: 0,
        speed: 0,
        t: 0,
        song_mask: 0,
        fade: false,
        fade_ctr: 0,
        end_flags: 15,
        songend: false,
        v: [Voice::default(); 4],
        t0: 0,
        cy: 0,
        first_at: 0,
        cia_t: 0,
    };
    s.nsub = ((s.hw(2).wrapping_sub(s.hw(1)) & 0xFFFF) / 4).clamp(1, 1024);
    s.init(0);
    // the host's volume set-up (eight zero volumes), then the start block, as the reference logs
    // the reference's log counts from the run's origin, START_PHASE before the engine's aligned
    // start; its first write falls 1594 colour clocks after that origin
    let t = m.bus.clock.saturating_sub(crate::engine::START_PHASE) + 1594;
    let tc = t + 1853;
    let mut hw = Hw { bus: &mut m.bus };
    const PRE: [u64; 8] = [0, 143, 295, 456, 716, 859, 1011, 1172];
    for (i, off) in PRE.iter().enumerate() {
        hw.bus.clock = t + off;
        hw.vol(i & 3, 0);
    }
    // the reference leaves the LED/filter bit set (the CIA line of its log)
    hw.bus.ciaa.pra |= 2;
    s.cia_t = tc;
    let end = s.start_block(&mut hw, tc);
    s.first_at = end + 74267;
    hw.bus.clock = end + ORIGIN;
    Some(Box::new(s))
}

#[cfg(test)]
#[path = "anders0land_tests.rs"]
mod tests;

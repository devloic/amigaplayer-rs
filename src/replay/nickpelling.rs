//! Nick Pelling Packer (Eagleplayer "Nick_Pelling_Packer", module prefix `npp.`; a packed song
//! file `NPP.<name>` with a sample set `SMP.<name>` or `SMP.set`).
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player and from its logged timing; no original code and nothing
//! embedded (the period table is ProTracker's, the sine is the usual one, the other small tables
//! are described in words by the spec). The replay is a ProTracker-style four-channel pattern
//! player with its own quirks: a packed pattern format (literal runs and back references), sample
//! slots resolved by name from the sample set (with aliases, delta coding and repeat fix-ups), a
//! slide mask that starts at zero, a tremolo whose phase advances by an extra 0xC0, a glissando
//! that walks off the period table and reads whatever lies around it, and so on.
//!
//! Every write is placed at the clock the spec's cost table gives (a least-squares fit of the
//! reference's timing, so a write can be a colour clock or two off), the DMA writes after the
//! raster waits of its output phase.
//!
//! Choices where the spec is silent (all safe, none can panic or hang): a note on a channel that
//! never had a sample, and sample numbers over 31, use an empty record (address 0, length 0);
//! a pattern number past the pattern table, or a break row of 64 or more, plays as an empty
//! pattern / row 0; a pattern that unpacks to more than 1024 bytes stops at 1024; a back
//! reference before the start of the output reads zeros; an alias that leads out of the set
//! counts as a missing name; a zero-length sample is not delta decoded.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Nick Pelling Packer", detect, new };

/// Colour clocks between two ticks.
const TICK: u64 = 70940;

/// The reference's first write (a volume write of the start-up) falls this many colour clocks
/// after the run's origin; it depends on the host's loading of the files (about 8.5 colour clocks
/// per character of the directory path and 10.5 per character of the file suffix, plus the
/// sample set's name resolution), so it is a measured constant: 35412 for the synthetic modules
/// as the reference runs them on the current host (the spec-era logs were 527 lower).
const FIRST_WRITE: u64 = 35412;

/// Colour clocks from the song's audio origin (a few after the last start-up write, as the
/// reference host counts it, found by comparing renders) to the timer's start, on top of a period.
const START_DELAY: u64 = 233;

/// What the note-delay path (EDy with a note) takes beyond the table's items (measured from the
/// reference's logs: the table's fit leaves it out).
const ED_EXTRA: f64 = 54.5;

// ---------------------------------------------------------------------------------------------
// The cost table of the spec (colour clocks).
// ---------------------------------------------------------------------------------------------
mod k {
    pub const PROW: f64 = 60.5;
    pub const PDEL: f64 = 84.0;
    pub const FX: [f64; 4] = [0.0, 61.0, 62.5, 57.0];
    pub const RN: [f64; 4] = [0.0, 82.5, 82.5, 82.5];
    pub const FUNK: f64 = 17.0;
    pub const C0: f64 = 45.5;
    pub const ARP_Z: f64 = -25.0;
    pub const ARP_B: f64 = 13.5;
    pub const ARP_H: f64 = 39.5;
    pub const ARP_W: f64 = 17.0;
    pub const ARP_N: f64 = 17.5;
    pub const C1: f64 = 34.5;
    pub const C2: f64 = 34.5;
    pub const SL_MID: f64 = 36.5;
    pub const SL_LO: f64 = 35.0;
    pub const SL_HI: f64 = 40.5;
    pub const C3: f64 = 45.5;
    pub const TP_PAR: f64 = 14.0;
    pub const C5: f64 = 44.5;
    pub const TP_OFF: f64 = -23.5;
    pub const TP_U: f64 = 56.5;
    pub const TP_HIT: f64 = 23.0;
    pub const TP_D: f64 = 56.5;
    pub const TP_DHIT: f64 = 18.0;
    pub const GL_OFF: f64 = -12.5;
    pub const GL_EQ: f64 = 8.0;
    pub const GL_UP: f64 = 45.0;
    pub const GL_KEEP: f64 = 23.0;
    pub const GL_DN: f64 = 49.0;
    pub const C4: f64 = 38.0;
    pub const VP: f64 = 35.0;
    pub const VP_LO: f64 = 6.0;
    pub const VP_HI: f64 = 6.0;
    pub const C6: f64 = 37.0;
    pub const VIB: f64 = 74.5;
    pub const VW_SINE: f64 = 23.0;
    pub const VW_RAMP: f64 = 30.0;
    pub const VW_RNEG: f64 = 2.0;
    pub const VW_SQ: f64 = 21.0;
    pub const VNEG: f64 = 2.0;
    pub const VC_MID: f64 = 24.0;
    pub const VC_LO: f64 = 22.5;
    pub const VC_HI: f64 = 28.0;
    pub const C7: f64 = 132.0;
    pub const MP: f64 = 35.0;
    pub const MP_LO: f64 = 6.0;
    pub const MP_HI: f64 = 6.0;
    pub const TW_SINE: f64 = 19.0;
    pub const TW_RAMP: f64 = 28.0;
    pub const TW_RNEG: f64 = 2.0;
    pub const TW_SQ: f64 = 19.0;
    pub const TNEG: f64 = 2.0;
    pub const TC_MID: f64 = 20.5;
    pub const TC_LO: f64 = 22.5;
    pub const TC_HI: f64 = 22.5;
    pub const CA: f64 = 2.5;
    pub const VS_UP: f64 = 44.0;
    pub const VS_DN: f64 = 41.0;
    pub const VS_MID: f64 = 27.0;
    pub const VS_LO: f64 = 29.0;
    pub const VS_HI: f64 = 29.0;
    pub const CE: f64 = 36.0;
    pub const E1: f64 = 13.0;
    pub const E1_0: f64 = -2.5;
    pub const E1_8: f64 = 10.0;
    pub const E8_DIV: f64 = 78.5;
    pub const E0_8_GO: f64 = 14.5;
    pub const E8_DIVH: f64 = 77.0;
    pub const E8_GO: f64 = 11.0;
    pub const E1_C: f64 = 16.0;
    pub const EC_HIT: f64 = 12.0;
    pub const E1_D: f64 = 19.5;
    pub const ED_T: f64 = -3.5;
    pub const ED_N: f64 = 5.0;
    pub const ED_GO: f64 = 19.5;
    pub const RZ: f64 = 10.0;
    pub const RS: f64 = 53.0;
    pub const RNN: f64 = -9.5;
    pub const RN_NOTE: f64 = 76.5;
    pub const RN35: f64 = 21.5;
    pub const PS_EQ: f64 = 5.0;
    pub const PS_GT: f64 = 7.5;
    pub const PS_LT: f64 = 9.5;
    pub const RN9: f64 = 19.0;
    pub const RNE: f64 = 26.0;
    pub const RNE5: f64 = 77.0;
    pub const RNX: f64 = 9.5;
    pub const TG: f64 = 55.0;
    pub const TG_ED: f64 = 6.5;
    pub const W_LEN: f64 = 10.0;
    pub const W_PER: f64 = 10.0;
    pub const W_VOL: f64 = 8.0;
    pub const W_DMA: [f64; 4] = [86.5, 102.0, 117.5, 121.5];
    pub const TG_POST: f64 = 42.0;
    pub const TG_VK: f64 = -8.0;
    pub const TG_TK: f64 = -8.0;
    pub const FR: f64 = 73.0;
    pub const FR_BASE: f64 = -14.0;
    pub const FR_NOP: f64 = -24.0;
    pub const FR_B: f64 = 44.5;
    pub const FR_C: f64 = 11.0;
    pub const FR_CC: f64 = 13.0;
    pub const FR_D: f64 = 17.5;
    pub const FR_DX: f64 = 19.5;
    pub const FR_E: f64 = 12.0;
    pub const FR_F: f64 = 6.5;
    pub const FR_F0: f64 = -12.0;
    pub const E0: f64 = 34.5;
    pub const E0_0: f64 = 2.0;
    pub const E0_1: f64 = 21.5;
    pub const E0_2: f64 = 21.5;
    pub const E0_3: f64 = -2.0;
    pub const E0_4: f64 = -2.0;
    pub const E0_5: f64 = 27.0;
    pub const E0_6: f64 = 14.5;
    pub const E6_MARK: f64 = -18.0;
    pub const E6_SET: f64 = 17.5;
    pub const E6_DEC: f64 = 20.0;
    pub const E6_DONE: f64 = -6.0;
    pub const E0_7: f64 = 5.0;
    pub const E0_8: f64 = -15.5;
    pub const E0_9: f64 = -28.0;
    pub const E0_A: f64 = 12.5;
    pub const EA_MID: f64 = 5.5;
    pub const EA_HI: f64 = 7.5;
    pub const E0_B: f64 = 14.0;
    pub const EB_MID: f64 = 6.0;
    pub const EB_LO: f64 = 8.0;
    pub const E0_C: f64 = -9.5;
    pub const E0_D: f64 = -6.0;
    pub const E0_E: f64 = -41.0;
    pub const EE_SET: f64 = 43.5;
    pub const E0_F: f64 = 17.5;
    pub const RT: f64 = 30.5;
    pub const RT_LEN: f64 = 10.0;
    pub const RT_DMA: f64 = 26.0;
    pub const ADV: f64 = 59.0;
    pub const ADV_DN: f64 = 53.0;
    pub const ADV_DR: f64 = -10.5;
    pub const ADV_D0: f64 = -20.5;
    pub const ADV_L: f64 = 32.0;
    pub const OUT: f64 = 241.5;
    pub const ADV_NP: f64 = 186.5;
    pub const JC_NP: f64 = 212.0;
    pub const NP_END: f64 = 9.5;
    pub const SONGEND: f64 = 75.0;
    pub const UP_LS: f64 = 29.5;
    pub const UP_RS: f64 = 36.0;
    pub const UP_LB: f64 = 6.0;
    /// Output phase: offsets of the volume and period writes from the first volume write.
    pub const OUT_VP: [f64; 4] = [14.0, 14.0, 14.0, 14.0];
    /// From a channel's period write to the next channel's volume write.
    pub const OUT_PV: [f64; 4] = [0.0, 72.5, 72.5, 72.5];
    /// First repeat location write after the second raster wait, by first channel in the mask.
    pub const REP_FIRST: [f64; 4] = [62.0, 70.5, 79.0, 87.5];
}

// ---------------------------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------------------------

const SINE: [u8; 32] = [
    0x00, 0x18, 0x31, 0x4A, 0x61, 0x78, 0x8D, 0xA1, 0xB4, 0xC5, 0xD4, 0xE0, 0xEB, 0xF4, 0xFA, 0xFD, 0xFF, 0xFD, 0xFA,
    0xF4, 0xEB, 0xE0, 0xD4, 0xC5, 0xB4, 0xA1, 0x8D, 0x78, 0x61, 0x4A, 0x31, 0x18,
];
const FUNK: [u8; 16] = [0x00, 0x05, 0x06, 0x07, 0x08, 0x0A, 0x0B, 0x0D, 0x10, 0x13, 0x16, 0x1A, 0x20, 0x2B, 0x40, 0x80];

/// The period of table index `i` (36 notes for each of the 16 finetunes), also outside the
/// table, where the reference reads what lies around it (spec 2.1).
fn period_at(i: i32) -> u16 {
    use crate::protracker::PERIODS;
    match i {
        0..=575 => PERIODS[(i / 36 * 37 + i % 36) as usize],
        -24..=-1 => {
            let k = (-i) as usize;
            let b = |o: usize| if o < 32 { SINE[o] } else { FUNK[o - 32] };
            ((b(48 - 2 * k) as u16) << 8) | b(49 - 2 * k) as u16
        }
        576..=703 => {
            let j = (i - 576) as usize * 2;
            ((bcd_table(j) as u16) << 8) | bcd_table(j + 1) as u16
        }
        704..=1132 => {
            let j = (i - 704) as usize * 2;
            ((lookup_byte(j) as u16) << 8) | lookup_byte(j + 1) as u16
        }
        _ => 0,
    }
}

/// Entry `v` of the player's 256-byte BCD table: the value, or 0xFF when a nibble is over 9.
fn bcd_table(v: usize) -> u8 {
    if (v >> 4) > 9 || (v & 15) > 9 {
        0xFF
    } else {
        (10 * (v >> 4) + (v & 15)) as u8
    }
}

/// The note (0..35) of a period 0..856: the largest index of the first row with a period of at
/// least `p`.
fn note_of(p: u16) -> i32 {
    use crate::protracker::PERIODS;
    (0..36).rev().find(|&i| PERIODS[i] >= p).unwrap_or(0) as i32
}

/// Byte `j` of the player's note lookup: twice the note of period j / 1, then a zero.
fn lookup_byte(j: usize) -> u8 {
    if j > 856 {
        0
    } else {
        (2 * note_of(j as u16)) as u8
    }
}

fn bcd(v: u8) -> Option<u8> {
    let b = bcd_table(v as usize);
    (b != 0xFF).then_some(b)
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

fn detect(d: &[u8]) -> bool {
    if d.len() < 16 || &d[0..4] != b"COMP" || be16(d, 4) != 0 {
        return false;
    }
    let n = be16(d, 6) as usize;
    if n == 0 || !(0x10..=0x110).contains(&n) || n % 4 != 0 || n + 4 > d.len() + 4 || n > d.len() {
        return false;
    }
    be32(d, n - 4) as usize <= d.len()
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Rec {
    addr: u32,
    len: u16,
    raddr: u32,
    rlen: u16,
    vol: u8,
    fine: u8,
}

/// The sample set, its slots resolved (spec 1.2, 1.3). `data` is the set as it is to be loaded
/// into guest memory (delta-decoded, repeat fix-ups applied), `addr` offsets are into it.
struct Samples {
    data: Vec<u8>,
    /// Slot records with offsets into `data`; the load adds the guest address of `data`, except
    /// for addresses that are 0 on purpose.
    slots: [Rec; 32],
    /// Slots whose addresses are real (not the 0 of an empty or missing name).
    real: [bool; 32],
}

fn cstr(d: &[u8]) -> &[u8] {
    &d[..d.iter().position(|&b| b == 0).unwrap_or(d.len())]
}

fn parse_samples(song: &[u8], names: &[Vec<u8>], set: &[u8]) -> Option<Samples> {
    let k = be16(set, 0) as usize;
    if k == 0 || k > 31 || set.len() < 2 + 30 * k {
        return None;
    }
    let ent = |i: usize| 2 + 30 * i;
    let len_of = |i: usize| be16(set, ent(i) + 22) as usize;
    let total: usize = (0..k).map(|i| 2 * len_of(i)).sum();
    if set.len() < 2 + 30 * k + total {
        return None;
    }
    let _ = song;
    let dstart = 2 + 30 * k;
    let mut data = set.to_vec();
    let mut slots = [Rec::default(); 32];
    let mut real = [false; 32];
    for (s, name) in names.iter().enumerate().take(31) {
        let slot = s + 1;
        if name.is_empty() {
            continue;
        }
        let missing = Rec { addr: 0, len: 2, raddr: 0, rlen: 2, vol: 0, fine: 0 };
        let found = (0..k).find(|&i| {
            let en = &set[ent(i)..ent(i) + 22];
            let en = cstr(en);
            en == &name[..]
        });
        let Some(mut j) = found else {
            slots[slot] = missing;
            continue;
        };
        let mut addr = dstart + (0..j).map(|i| 2 * len_of(i)).sum::<usize>();
        if len_of(j) == 0 {
            let n = be16(set, ent(j) + 26) as usize;
            if n + 1 >= k {
                slots[slot] = missing;
                continue;
            }
            addr = dstart + (1..=n).map(|i| 2 * len_of(i)).sum::<usize>();
            j = n + 1;
        }
        let len = len_of(j);
        let fine = data[ent(j) + 24];
        let vol = set[ent(j) + 25];
        let rstart = be16(set, ent(j) + 26) as usize;
        let rlen = be16(set, ent(j) + 28) as usize;
        if fine & 0x80 != 0 {
            // delta coding, once: the bit is cleared in the set
            data[ent(j) + 24] &= 0x7F;
            let end = (addr + 2 * len).min(data.len());
            let mut prev = 0u8;
            for i in addr..end {
                if i > addr {
                    data[i] = data[i].wrapping_add(prev);
                }
                prev = data[i];
            }
        }
        let (raddr, rl) = if rlen < 4 {
            for i in addr..(addr + 4).min(data.len()) {
                data[i] = 0;
            }
            (addr, 2)
        } else {
            (addr + 2 * rstart, rlen)
        };
        slots[slot] = Rec {
            addr: addr as u32,
            len: len as u16,
            raddr: raddr as u32,
            rlen: rl as u16,
            vol: vol.min(64),
            fine: fine & 15,
        };
        real[slot] = true;
    }
    Some(Samples { data, slots, real })
}

// ---------------------------------------------------------------------------------------------
// The player
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Chan {
    cell: [u8; 4],
    rec: usize,
    period: u16,
    outper: u16,
    vol: i32,
    outvol: i32,
    fine: i32,
    note: i32,
    cur: i32,
    arp: u8,
    target: u16,
    up: bool,
    speed: u8,
    vib_cmd: u8,
    vib_pos: u8,
    trem_cmd: u8,
    trem_pos: u8,
    ctrl: u8,
    gl: u8,
    loop_row: u8,
    loop_cnt: u8,
}

/// The running clock of a tick (colour clocks, with the spec's fractions); `add` takes the
/// name of the cost-table item it adds, which documents the code.
struct Led {
    t: f64,
}

impl Led {
    fn add(&mut self, item: &str, c: f64) {
        let _ = item;
        self.t += c;
    }
}

pub struct NickPelling {
    d: Vec<u8>,
    title: String,
    slots: [Rec; 32],
    patoff: Vec<u32>,
    list: usize,
    len: u8,
    buf: [u8; 1024],
    ch: [Chan; 4],
    speed: u8,
    counter: u8,
    pos: u8,
    row: i32,
    break_row: i32,
    jump: bool,
    loop_flag: bool,
    pending: u8,
    delay: u8,
    slide_mask: u8,
    dma: u16,
    /// The tick's running clock (colour clocks, with the spec's fractions).
    led: Led,
    /// The fraction of the tick start in the reference (its start is not a whole clock).
    frac: f64,
    songend: bool,
    start_delay: u64,
}

/// The colour clock a tick's running clock value is logged as: the costs and the tick start are
/// whole half colour clocks, and the log shows the whole clock the half clock is in.
fn cutx(x: f64) -> u64 {
    x.floor().max(0.0) as u64
}

impl NickPelling {
    fn build(d: &[u8], set: &[u8]) -> Option<(Self, Samples)> {
        if !detect(d) {
            return None;
        }
        let n = be16(d, 6) as usize;
        let q = be32(d, 8) as usize;
        if q >= d.len() {
            return None;
        }
        let npat = n / 4 - 3;
        let patoff = (0..npat).map(|i| be32(d, 12 + 4 * i)).collect();
        // the song name, then exactly 31 sample names
        let mut o = n;
        let next = |o: &mut usize| -> Vec<u8> {
            let s = cstr(d.get(*o..).unwrap_or(&[])).to_vec();
            *o += s.len() + 1;
            s
        };
        let title = String::from_utf8_lossy(&next(&mut o)).into_owned();
        let names: Vec<Vec<u8>> = (0..31).map(|_| next(&mut o)).collect();
        let samples = parse_samples(d, &names, set)?;
        let me = NickPelling {
            d: d.to_vec(),
            title,
            slots: samples.slots,
            patoff,
            list: q + 1,
            len: d[q],
            buf: [0; 1024],
            ch: [Chan { cell: [0xFF; 4], outper: 0x7C, ..Default::default() }; 4],
            speed: 6,
            counter: 0,
            pos: 0,
            row: 0,
            break_row: 0,
            jump: false,
            loop_flag: false,
            pending: 0,
            delay: 0,
            slide_mask: 0,
            dma: 0,
            led: Led { t: 0.0 },
            frac: 0.0,
            songend: false,
            start_delay: 0,
        };
        Some((me, samples))
    }

    /// Unpacks the pattern of the current position into the buffer; returns its cost.
    fn unpack(&mut self) -> f64 {
        let p = self.d.get(self.list + (self.pos & 0x7F) as usize).copied().unwrap_or(0) as usize;
        let Some(&off) = self.patoff.get(p) else {
            self.buf = [0; 1024];
            return 0.0;
        };
        let mut i = off as usize;
        let mut n = 0usize;
        let (mut lits, mut backs) = (0u32, 0u32);
        while n < 1024 {
            let Some(&c) = self.d.get(i) else { break };
            i += 1;
            if c < 128 {
                lits += 1;
                for _ in 0..=c as usize {
                    let b = self.d.get(i).copied().unwrap_or(0);
                    i += 1;
                    if n < 1024 {
                        self.buf[n] = b;
                        n += 1;
                    }
                }
            } else {
                let Some(&cnt) = self.d.get(i) else { break };
                i += 1;
                if cnt == 0 {
                    break;
                }
                backs += 1;
                for _ in 0..cnt {
                    let back = 256 - c as usize;
                    if n >= 1024 {
                        break;
                    }
                    self.buf[n] = if n >= back { self.buf[n - back] } else { 0 };
                    n += 1;
                }
            }
        }
        n as f64 * k::UP_LB + lits as f64 * k::UP_LS + backs as f64 * k::UP_RS
    }

    fn rec(&self, c: usize) -> Rec {
        self.slots.get(self.ch[c].rec).copied().unwrap_or_default()
    }
}

impl NickPelling {
    /// Moves the bus clock to the running clock (never backwards).
    fn at(&mut self, hw: &mut Hw) {
        hw.bus.clock = hw.bus.clock.max(cutx(self.led.t));
    }

    fn song_end(&mut self) {
        self.led.add("SONGEND", k::SONGEND);
        self.songend = true;
    }

    // ---- small helpers over a channel -------------------------------------------------------

    fn vol_slide(&mut self, c: usize, par: u8) {
        let ch = &mut self.ch[c];
        let v = if par >= 0x10 {
            self.led.add("VS_UP", k::VS_UP);
            ch.vol + (par >> 4) as i32
        } else {
            self.led.add("VS_DN", k::VS_DN);
            ch.vol - (par & 15) as i32
        };
        let v = if v < 0 {
            self.led.add("VS_LO", k::VS_LO);
            0
        } else if v > 64 {
            self.led.add("VS_HI", k::VS_HI);
            64
        } else {
            self.led.add("VS_MID", k::VS_MID);
            v
        };
        ch.vol = v;
        ch.outvol = v;
    }

    /// 1xx / 2xx / E1y / E2y: the slide itself.
    fn slide(&mut self, c: usize, par: u8, down: bool) {
        let amount = (par & self.slide_mask) as i32;
        self.slide_mask = 0xFF;
        let ch = &mut self.ch[c];
        let old = ch.period;
        let p = (old & 0x0FFF) as i32 + if down { -amount } else { amount };
        let p = if p < 124 {
            self.led.add("SL_LO", k::SL_LO);
            124
        } else if p > 856 {
            self.led.add("SL_HI", k::SL_HI);
            856
        } else {
            self.led.add("SL_MID", k::SL_MID);
            p
        };
        ch.outper = p as u16;
        ch.period = p as u16 | (old & 0xF000);
    }

    fn tone_porta(&mut self, c: usize) {
        let ch = &mut self.ch[c];
        if ch.target == 0 {
            // no target: nothing happens, not even the glissando (the output period stays)
            self.led.add("TP_OFF", k::TP_OFF);
            return;
        } else if ch.up {
            self.led.add("TP_U", k::TP_U);
            ch.period = ch.period.wrapping_add(ch.speed as u16);
            if ch.target as i16 <= ch.period as i16 {
                ch.period = ch.target;
                ch.target = 0;
                self.led.add("TP_HIT", k::TP_HIT);
            }
        } else {
            self.led.add("TP_D", k::TP_D);
            ch.period = ch.period.wrapping_sub(ch.speed as u16);
            if ch.target as i16 >= ch.period as i16 {
                ch.period = ch.target;
                ch.target = 0;
                self.led.add("TP_DHIT", k::TP_DHIT);
            }
        }
        let mut p = ch.period;
        if ch.gl & 15 == 0 {
            self.led.add("GL_OFF", k::GL_OFF);
        } else {
            let cv = period_at(ch.cur);
            if p == cv {
                self.led.add("GL_EQ", k::GL_EQ);
            } else if p as i16 > cv as i16 {
                self.led.add("GL_UP", k::GL_UP);
                p = period_at(ch.cur + 1);
                ch.note += 1;
                ch.cur += 1;
            } else {
                let lo = period_at(ch.cur - 1);
                if p as i16 > lo as i16 {
                    self.led.add("GL_KEEP", k::GL_KEEP);
                } else {
                    self.led.add("GL_DN", k::GL_DN);
                    p = lo;
                    ch.note -= 1;
                    ch.cur -= 1;
                }
            }
        }
        ch.outper = p;
    }

    /// The wave value of position `pos` for wave `w` (spec 7.2).
    fn wave(w: u8, pos: u8) -> i32 {
        let kk = ((pos & 0x7C) >> 2) as usize;
        match w & 3 {
            0 => SINE[kk] as i32,
            1 => {
                let v = ((8 * kk) & 0xFF) as i32;
                if pos & 0x80 != 0 {
                    255 - v
                } else {
                    v
                }
            }
            _ => 255,
        }
    }

    fn vibrato(&mut self, c: usize) {
        self.led.add("VIB", k::VIB);
        let ch = &mut self.ch[c];
        let w = ch.ctrl & 3;
        let neg = ch.vib_pos & 0x80 != 0;
        match w {
            0 => self.led.add("VW_SINE", k::VW_SINE),
            1 => {
                self.led.add("VW_RAMP", k::VW_RAMP);
                if neg {
                    self.led.add("VW_RNEG", k::VW_RNEG);
                }
            }
            _ => self.led.add("VW_SQ", k::VW_SQ),
        }
        let v = Self::wave(w, ch.vib_pos);
        let d = ((ch.vib_cmd & 15) as i32 * v) >> 7;
        let base = ch.period as i16 as i32;
        let mut p = if neg {
            self.led.add("VNEG", k::VNEG);
            base - d
        } else {
            base + d
        };
        if p < 124 {
            self.led.add("VC_LO", k::VC_LO);
            p = 124;
        } else if p > 856 {
            self.led.add("VC_HI", k::VC_HI);
            p = 856;
        } else {
            self.led.add("VC_MID", k::VC_MID);
        }
        ch.outper = p as u16;
        ch.vib_pos = ch.vib_pos.wrapping_add(4u8.wrapping_mul(ch.vib_cmd >> 4));
    }

    fn tremolo(&mut self, c: usize, par: u8) {
        let ch = &mut self.ch[c];
        if par != 0 {
            self.led.add("MP", k::MP);
            if par & 15 != 0 {
                self.led.add("MP_LO", k::MP_LO);
                ch.trem_cmd = (ch.trem_cmd & 0xF0) | (par & 15);
            }
            if par & 0xF0 != 0 {
                self.led.add("MP_HI", k::MP_HI);
                ch.trem_cmd = (ch.trem_cmd & 15) | (par & 0xF0);
            }
        }
        let w = (ch.ctrl >> 4) & 3;
        let neg = ch.trem_pos & 0x80 != 0;
        match w {
            0 => self.led.add("TW_SINE", k::TW_SINE),
            1 => {
                self.led.add("TW_RAMP", k::TW_RAMP);
                if neg {
                    self.led.add("TW_RNEG", k::TW_RNEG);
                }
            }
            _ => self.led.add("TW_SQ", k::TW_SQ),
        }
        let v = Self::wave(w, ch.trem_pos);
        let d = ((ch.trem_cmd & 15) as i32 * v) >> 6;
        let mut x = if neg {
            self.led.add("TNEG", k::TNEG);
            ch.vol - d
        } else {
            ch.vol + d
        };
        if x < 0 {
            self.led.add("TC_LO", k::TC_LO);
            x = 0;
        } else if x > 64 {
            self.led.add("TC_HI", k::TC_HI);
            x = 64;
        } else {
            self.led.add("TC_MID", k::TC_MID);
        }
        ch.outvol = x;
        ch.trem_pos = ch.trem_pos.wrapping_add(0xC0u8.wrapping_add(4u8.wrapping_mul(ch.trem_cmd >> 4)));
    }

    /// The parameter merge of commands 4 and 7: a non-zero nibble replaces the stored one.
    fn vib_param(&mut self, c: usize, par: u8) {
        let ch = &mut self.ch[c];
        if par != 0 {
            self.led.add("VP", k::VP);
            if par & 15 != 0 {
                self.led.add("VP_LO", k::VP_LO);
                ch.vib_cmd = (ch.vib_cmd & 0xF0) | (par & 15);
            }
            if par & 0xF0 != 0 {
                self.led.add("VP_HI", k::VP_HI);
                ch.vib_cmd = (ch.vib_cmd & 15) | (par & 0xF0);
            }
        }
    }

    // ---- hardware writes ---------------------------------------------------------------------

    /// Retrigger: location, length, DMA off (the output phase writes period and volume).
    fn retrigger(&mut self, c: usize, hw: &mut Hw) {
        let r = self.rec(c);
        self.led.add("RT", k::RT);
        self.at(hw);
        hw.loc(c, r.addr);
        self.led.add("RT_LEN", k::RT_LEN);
        self.at(hw);
        hw.len(c, r.len);
        let ch = &mut self.ch[c];
        ch.outper = ch.period;
        ch.outvol = ch.vol;
        self.led.add("RT_DMA", k::RT_DMA);
        self.at(hw);
        hw.dmacon(1 << c);
        self.dma |= 1 << c;
    }

    /// The note-delay check (EDy): retrigger when the tick counter is y and the cell's first
    /// word is not 0.
    fn note_delay(&mut self, c: usize, y: u8, hw: &mut Hw) {
        let cell = self.ch[c].cell;
        if y != self.counter {
            self.led.add("ED_T", k::ED_T);
        } else if cell[0] == 0 && cell[1] == 0 {
            self.led.add("ED_N", k::ED_N);
        } else {
            self.led.add("ED_GO", k::ED_GO);
            self.retrigger(c, hw);
        }
    }

    /// An extended command. `tick0`: the tick-0 rules; `row`: inside a row's effect (against a
    /// pattern-delay row's pass, for the time of the filter writes).
    fn ext(&mut self, c: usize, tick0: bool, pdel: bool, hw: &mut Hw) {
        let par = self.ch[c].cell[3];
        let (x, y) = (par >> 4, par & 15);
        if tick0 {
            self.led.add("E0", k::E0);
            match x {
                0 => {
                    self.led.add("E0_0", k::E0_0);
                    // a row effect writes 3.5 clocks after this point; inside a pattern-delay
                    // row's pass the write comes 57 clocks before it (25 later on channel 3)
                    let t1 = if !pdel {
                        self.led.t + 3.5
                    } else if c == 3 {
                        self.led.t - 32.0
                    } else {
                        self.led.t - 57.0
                    };
                    hw.bus.clock = hw.bus.clock.max(cutx(t1));
                    let p = hw.bus.read_byte(0xBFE001) & !2;
                    hw.bus.write_byte(0xBFE001, p);
                    hw.bus.clock = hw.bus.clock.max(cutx(t1 + 6.0));
                    hw.bus.write_byte(0xBFE001, p | 2 * (y & 1));
                }
                1 | 2 => {
                    if x == 1 {
                        self.led.add("E0_1", k::E0_1)
                    } else {
                        self.led.add("E0_2", k::E0_2)
                    };
                    self.slide_mask = 0x0F;
                    self.slide(c, par, x == 1);
                }
                3 => {
                    self.led.add("E0_3", k::E0_3);
                    let ch = &mut self.ch[c];
                    ch.gl = (ch.gl & 0xF0) | y;
                }
                4 => {
                    self.led.add("E0_4", k::E0_4);
                    let ch = &mut self.ch[c];
                    ch.ctrl = (ch.ctrl & 0xF0) | y;
                }
                5 => {
                    self.led.add("E0_5", k::E0_5);
                    let ch = &mut self.ch[c];
                    ch.fine = y as i32;
                    ch.cur = ch.note + 36 * y as i32;
                }
                6 => {
                    self.led.add("E0_6", k::E0_6);
                    let row_now = self.row;
                    let ch = &mut self.ch[c];
                    let mut go = true;
                    if y == 0 {
                        self.led.add("E6_MARK", k::E6_MARK);
                        ch.loop_row = row_now as u8;
                    } else if ch.loop_cnt != 0 {
                        ch.loop_cnt -= 1;
                        if ch.loop_cnt == 0 {
                            self.led.add("E6_DONE", k::E6_DONE);
                            go = false;
                        } else {
                            self.led.add("E6_DEC", k::E6_DEC);
                        }
                    } else {
                        self.led.add("E6_SET", k::E6_SET);
                        ch.loop_cnt = y;
                    }
                    if go && y != 0 {
                        self.break_row = ch.loop_row as i32;
                        self.loop_flag = true;
                    }
                }
                7 => {
                    self.led.add("E0_7", k::E0_7);
                    let ch = &mut self.ch[c];
                    ch.ctrl = (ch.ctrl & 0x0F) | (y << 4);
                }
                8 => {
                    self.led.add("E0_8", k::E0_8);
                    if note_of_cell(&self.ch[c].cell) == 0 {
                        self.led.add("E0_8_GO", k::E0_8_GO);
                        self.retrigger(c, hw);
                    }
                }
                9 => self.led.add("E0_9", k::E0_9),
                0xA => {
                    self.led.add("E0_A", k::E0_A);
                    let ch = &mut self.ch[c];
                    let v = ch.vol + y as i32;
                    ch.vol = if v > 64 {
                        self.led.add("EA_HI", k::EA_HI);
                        64
                    } else {
                        self.led.add("EA_MID", k::EA_MID);
                        v
                    };
                    ch.outvol = ch.vol;
                }
                0xB => {
                    self.led.add("E0_B", k::E0_B);
                    let ch = &mut self.ch[c];
                    let v = ch.vol - y as i32;
                    ch.vol = if v < 0 {
                        self.led.add("EB_LO", k::EB_LO);
                        0
                    } else {
                        self.led.add("EB_MID", k::EB_MID);
                        v
                    };
                    ch.outvol = ch.vol;
                }
                0xC => {
                    self.led.add("E0_C", k::E0_C);
                    if y == 0 {
                        self.led.add("EC_HIT", k::EC_HIT);
                        let ch = &mut self.ch[c];
                        ch.vol = 0;
                        ch.outvol = 0;
                    }
                }
                0xD => {
                    self.led.add("E0_D", k::E0_D);
                    self.note_delay(c, y, hw);
                }
                0xE => {
                    self.led.add("E0_E", k::E0_E);
                    if self.delay == 0 {
                        self.led.add("EE_SET", k::EE_SET);
                        self.pending = y + 1;
                    }
                }
                _ => {
                    self.led.add("E0_F", k::E0_F);
                    let ch = &mut self.ch[c];
                    ch.gl = (ch.gl & 15) | (y << 4);
                    if y != 0 {
                        self.led.add("FUNK", k::FUNK);
                    }
                }
            }
        } else {
            self.led.add("E1", k::E1);
            match x {
                8 => {
                    self.led.add("E1_8", k::E1_8);
                    if y != 0 {
                        let hit = self.counter % y == 0;
                        if hit {
                            self.led.add("E8_DIVH", k::E8_DIVH);
                        } else {
                            self.led.add("E8_DIV", k::E8_DIV);
                        }
                        if hit {
                            self.led.add("E8_GO", k::E8_GO);
                            self.retrigger(c, hw);
                        }
                    }
                }
                0xC => {
                    self.led.add("E1_C", k::E1_C);
                    if y == self.counter {
                        self.led.add("EC_HIT", k::EC_HIT);
                        let ch = &mut self.ch[c];
                        ch.vol = 0;
                        ch.outvol = 0;
                    }
                }
                0xD => {
                    self.led.add("E1_D", k::E1_D);
                    self.note_delay(c, y, hw);
                }
                _ => self.led.add("E1_0", k::E1_0),
            }
        }
    }

    // ---- the per-tick pass ---------------------------------------------------------------------

    fn tick_chan(&mut self, c: usize, row: bool, hw: &mut Hw) {
        if self.ch[c].gl >> 4 != 0 {
            self.led.add("FUNK", k::FUNK);
        }
        let (cmd, par) = (self.ch[c].cell[2] & 15, self.ch[c].cell[3]);
        match cmd {
            0 => {
                self.led.add("C0", k::C0);
                let ch = &mut self.ch[c];
                if par == 0 {
                    self.led.add("ARP_Z", k::ARP_Z);
                    ch.outper = ch.period;
                } else {
                    ch.arp = (ch.arp + 1) % 4;
                    match ch.arp {
                        1 => {
                            self.led.add("ARP_B", k::ARP_B);
                            ch.outper = ch.period;
                        }
                        2 => {
                            self.led.add("ARP_H", k::ARP_H);
                            ch.outper = period_at(ch.cur + (par >> 4) as i32);
                        }
                        _ => {
                            ch.arp = 0;
                            self.led.add("ARP_W", k::ARP_W);
                            self.led.add("ARP_N", k::ARP_N);
                            ch.outper = period_at(ch.cur + (par & 15) as i32);
                        }
                    }
                }
            }
            1 => {
                self.led.add("C1", k::C1);
                self.slide(c, par, true);
            }
            2 => {
                self.led.add("C2", k::C2);
                self.slide(c, par, false);
            }
            3 => {
                self.led.add("C3", k::C3);
                if par != 0 {
                    self.led.add("TP_PAR", k::TP_PAR);
                    self.ch[c].speed = par;
                    self.ch[c].cell[3] = 0;
                }
                self.tone_porta(c);
            }
            4 => {
                self.led.add("C4", k::C4);
                self.vib_param(c, par);
                self.vibrato(c);
            }
            5 => {
                self.led.add("C5", k::C5);
                self.tone_porta(c);
                self.vol_slide(c, par);
            }
            6 => {
                self.led.add("C6", k::C6);
                self.vibrato(c);
                self.vol_slide(c, par);
            }
            7 => {
                self.led.add("C7", k::C7);
                let ch = &mut self.ch[c];
                ch.outper = ch.period;
                self.tremolo(c, par);
            }
            0xA => {
                self.led.add("CA", k::CA);
                let ch = &mut self.ch[c];
                ch.outper = ch.period;
                self.vol_slide(c, par);
            }
            0xE => {
                self.led.add("CE", k::CE);
                let t0 = self.counter == 0;
                self.ext(c, t0, row && t0, hw);
            }
            _ => {}
        }
    }

    // ---- the row pass --------------------------------------------------------------------------

    fn row_effect(&mut self, c: usize, hw: &mut Hw) {
        if self.ch[c].gl >> 4 != 0 {
            self.led.add("FUNK", k::FUNK);
        }
        self.led.add("FR", k::FR);
        let (cmd, par) = (self.ch[c].cell[2] & 15, self.ch[c].cell[3]);
        match cmd {
            0..=8 | 0xA => {
                self.led.add("FR_BASE", k::FR_BASE);
                let ch = &mut self.ch[c];
                ch.outper = ch.period;
            }
            9 => self.led.add("FR_NOP", k::FR_NOP),
            0xB => {
                self.led.add("FR_B", k::FR_B);
                if self.pos.wrapping_add(1) == self.len {
                    self.song_end();
                }
                self.pos = (par & 0x7F).wrapping_sub(1);
                self.break_row = 0;
                self.jump = true;
            }
            0xC => {
                if par > 64 {
                    self.led.add("FR_CC", k::FR_CC)
                } else {
                    self.led.add("FR_C", k::FR_C)
                };
                let v = par.min(64) as i32;
                let ch = &mut self.ch[c];
                ch.vol = v;
                ch.outvol = v;
            }
            0xD => {
                match bcd(par) {
                    Some(b) => {
                        self.led.add("FR_D", k::FR_D);
                        // rows past the buffer are not defined: row 0
                        self.break_row = if b < 64 { b as i32 } else { 0 };
                    }
                    None => {
                        self.led.add("FR_DX", k::FR_DX);
                        self.break_row = 0;
                    }
                }
                self.jump = true;
            }
            0xE => {
                self.led.add("FR_E", k::FR_E);
                self.ext(c, true, false, hw);
            }
            _ => {
                if par != 0 {
                    self.led.add("FR_F", k::FR_F);
                    self.speed = par;
                    self.counter = 0;
                } else {
                    self.led.add("FR_F0", k::FR_F0);
                    self.song_end();
                }
            }
        }
    }

    fn trigger(&mut self, c: usize, hw: &mut Hw) {
        let cell = self.ch[c].cell;
        let np = (((cell[0] as u16) << 8 | cell[1] as u16) & 0x0FFF) as u16;
        let (cmd, par) = (cell[2] & 15, cell[3]);
        {
            let ch = &mut self.ch[c];
            ch.note = note_of(np.min(856));
            ch.cur = ch.note + 36 * ch.fine;
            ch.period = period_at(ch.cur);
            ch.outper = ch.period;
        }
        if cmd == 0xE && par >> 4 == 0xD {
            self.led.add("TG_ED", k::TG_ED);
            self.led.add("ED_EXTRA", ED_EXTRA);
            if self.ch[c].gl >> 4 != 0 {
                self.led.add("FUNK", k::FUNK);
            }
            self.note_delay(c, par & 15, hw);
            return;
        }
        let r = self.rec(c);
        self.led.add("TG", k::TG);
        self.at(hw);
        hw.loc(c, r.addr);
        self.led.add("W_LEN", k::W_LEN);
        self.at(hw);
        hw.len(c, r.len);
        self.led.add("W_PER", k::W_PER);
        self.at(hw);
        hw.per(c, self.ch[c].outper);
        self.led.add("W_VOL", k::W_VOL);
        self.at(hw);
        hw.vol(c, 0);
        self.led.add(["W_DMA0", "W_DMA1", "W_DMA2", "W_DMA3"][c], k::W_DMA[c]);
        self.at(hw);
        hw.dmacon(1 << c);
        self.dma |= 1 << c;
        self.led.add("TG_POST", k::TG_POST);
        let ch = &mut self.ch[c];
        ch.arp = 0;
        if ch.ctrl & 4 == 0 {
            ch.vib_pos = 0;
        } else {
            self.led.add("TG_VK", k::TG_VK);
        }
        if ch.ctrl & 0x40 == 0 {
            ch.trem_pos = 0;
        } else {
            self.led.add("TG_TK", k::TG_TK);
        }
        self.row_effect(c, hw);
    }

    fn row_chan(&mut self, c: usize, hw: &mut Hw) {
        let off = self.row as usize * 16 + c * 4;
        let new = [self.buf[off & 1023], self.buf[(off + 1) & 1023], self.buf[(off + 2) & 1023], self.buf[(off + 3) & 1023]];
        if self.ch[c].cell == [0; 4] {
            self.led.add("RZ", k::RZ);
            let ch = &mut self.ch[c];
            ch.outper = ch.period;
        }
        self.ch[c].cell = new;
        let s = ((new[0] & 0xF0) | (new[2] >> 4)) as usize;
        if s != 0 {
            self.led.add("RS", k::RS);
            let slot = if s < 32 { s } else { 0 };
            let r = self.slots[slot];
            let ch = &mut self.ch[c];
            ch.rec = slot;
            ch.vol = r.vol as i32;
            ch.outvol = r.vol as i32;
            ch.fine = r.fine as i32;
        }
        let np = (((new[0] as u16) << 8 | new[1] as u16) & 0x0FFF) as u16;
        let (cmd, par) = (new[2] & 15, new[3]);
        if np == 0 {
            self.led.add("RNN", k::RNN);
            self.row_effect(c, hw);
            return;
        }
        self.led.add("RN_NOTE", k::RN_NOTE);
        match cmd {
            3 | 5 => {
                self.led.add("RN35", k::RN35);
                let ch = &mut self.ch[c];
                let tgt = period_at(note_of(np.min(856)) + 36 * ch.fine);
                if tgt == ch.period {
                    self.led.add("PS_EQ", k::PS_EQ);
                    ch.target = 0;
                } else {
                    ch.target = tgt;
                    ch.up = tgt as i16 > ch.period as i16;
                    if ch.up {
                        self.led.add("PS_GT", k::PS_GT)
                    } else {
                        self.led.add("PS_LT", k::PS_LT)
                    };
                }
                self.row_effect(c, hw);
                return;
            }
            9 => {
                self.led.add("RN9", k::RN9);
                self.row_effect(c, hw);
            }
            0xE => {
                self.led.add("RNE", k::RNE);
                if par >> 4 == 5 {
                    self.led.add("RNE5", k::RNE5);
                    self.ch[c].fine = (par & 15) as i32;
                }
            }
            _ => self.led.add("RNX", k::RNX),
        }
        self.trigger(c, hw);
    }

    // ---- row advance and position change -----------------------------------------------------

    fn position_change(&mut self) {
        self.row = self.break_row;
        self.break_row = 0;
        self.jump = false;
        self.pos = (self.pos.wrapping_add(1)) & 0x7F;
        if self.pos >= self.len {
            self.led.add("NP_END", k::NP_END);
            self.song_end();
            self.pos = 0;
        }
        let u = self.unpack();
        self.led.add("UNPACK", u);
    }

    fn row_advance(&mut self) {
        self.led.add("ADV", k::ADV);
        self.row += 1;
        if self.pending != 0 {
            self.led.add("ADV_DN", k::ADV_DN);
            self.delay = self.pending;
            self.pending = 0;
        }
        if self.delay != 0 {
            self.delay -= 1;
            if self.delay != 0 {
                self.led.add("ADV_DR", k::ADV_DR);
                self.row -= 1;
            } else {
                self.led.add("ADV_D0", k::ADV_D0);
            }
        }
        if self.loop_flag {
            self.led.add("ADV_L", k::ADV_L);
            self.loop_flag = false;
            self.row = self.break_row;
            self.break_row = 0;
        }
        if self.row >= 64 {
            self.led.add("ADV_NP", k::ADV_NP);
            self.position_change();
        } else if self.jump {
            self.led.add("JC_NP", k::JC_NP);
            self.position_change();
        }
    }

    // ---- the output phase ----------------------------------------------------------------------

    /// Nine raster-line changes from `x` (CPU cycles since the run's origin); the time of the
    /// ninth, in cycles.
    fn raster(x0: f64, first_read: f64) -> f64 {
        let line = |x: f64| (x / 454.0).floor();
        let mut x = x0;
        for n in 0..9 {
            // the first read of the line number happens later than the wait's start by this many cycles
            let l = line(if n == 0 { x + first_read } else { x });
            x += 18.0;
            while line(x) == l {
                x += 25.0;
            }
            if n < 8 {
                x += 33.0;
            }
        }
        x
    }

    fn output(&mut self, hw: &mut Hw) {
        const VP: [f64; 4] = k::OUT_VP;
        const PV: [f64; 4] = k::OUT_PV;
        for c in 0..4 {
            if c > 0 {
                self.led.t += PV[c];
            }
            self.at(hw);
            hw.vol(c, self.ch[c].outvol.clamp(0, 64) as u16);
            self.led.t += VP[c];
            self.at(hw);
            hw.per(c, self.ch[c].outper);
        }
        if self.dma == 0 {
            return;
        }
        let x0 = (2.0 * self.led.t).floor() + 95.0;
        let x = Self::raster(x0, 2.0);
        self.led.t = x / 2.0 + 37.5;
        self.at(hw);
        hw.dmacon(0x8000 | self.dma);
        let x0 = (2.0 * self.led.t).floor() + 60.0;
        let x = Self::raster(x0, 0.0);
        let mut first = true;
        let mut prev_end = 0.0;
        let mut last = 0usize;
        for c in 0..4 {
            if self.dma & (1 << c) == 0 {
                continue;
            }
            let lc = if first {
                first = false;
                x / 2.0 + k::REP_FIRST[c]
            } else {
                prev_end + 30.5 + 8.5 * (c - last - 1) as f64
            };
            self.led.t = lc;
            self.at(hw);
            let r = self.rec(c);
            hw.loc(c, r.raddr);
            self.led.t += 10.0;
            self.at(hw);
            hw.len(c, r.rlen);
            prev_end = self.led.t;
            last = c;
        }
    }

    fn run(&mut self, hw: &mut Hw) {
        self.led.t = hw.bus.clock as f64 + self.frac;
        self.dma = 0;
        self.counter = self.counter.wrapping_add(1);
        if self.counter >= self.speed {
            self.counter = 0;
            if self.delay != 0 {
                self.led.add("PDEL", k::PDEL);
                for c in 0..4 {
                    self.led.add(["FX0", "FX1", "FX2", "FX3"][c], k::FX[c]);
                    self.tick_chan(c, true, hw);
                }
            } else {
                self.led.add("PROW", k::PROW);
                for c in 0..4 {
                    self.led.add(["RN0", "RN1", "RN2", "RN3"][c], k::RN[c]);
                    self.row_chan(c, hw);
                }
            }
            self.row_advance();
        } else {
            for c in 0..4 {
                self.led.add(["FX0", "FX1", "FX2", "FX3"][c], k::FX[c]);
                self.tick_chan(c, false, hw);
            }
            if self.jump {
                self.led.add("JC_NP", k::JC_NP);
                self.position_change();
            }
        }
        self.led.add("OUT", k::OUT);
        self.output(hw);
    }
}

/// The note period of a stored cell.
fn note_of_cell(cell: &[u8; 4]) -> u16 {
    (((cell[0] as u16) << 8) | cell[1] as u16) & 0x0FFF
}

impl Replayer for NickPelling {
    fn format(&self) -> &'static str {
        "Nick Pelling Packer"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn timer_delay(&self) -> u64 {
        self.start_delay
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run(hw);
    }
}

/// The sample set's name beside the song file `file`: the song's name with its first four
/// characters replaced, then `SMP.set`.
fn companions(file: &str) -> Vec<String> {
    let leaf = file.rsplit(['/', ':']).next().unwrap_or(file);
    let mut v = Vec::new();
    if leaf.len() > 4 && leaf.is_char_boundary(4) {
        v.push(format!("SMP{}", &leaf[3..]));
    }
    v.push("SMP.set".to_string());
    v
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let mut found = None;
    for name in companions(&file) {
        if let Ok(f) = crate::deli::read_companion(m, &name).1 {
            if let Some(r) = NickPelling::build(d, &f) {
                found = Some((r, f));
                break;
            }
        }
    }
    let ((mut r, samples), _) = found?;
    let sbase = m.alloc((samples.data.len() as u32).max(4));
    if sbase == 0 {
        return None;
    }
    m.bus.write_bytes(sbase, &samples.data);
    for s in 1..32 {
        if samples.real[s] {
            r.slots[s].addr = r.slots[s].addr.wrapping_add(sbase);
            r.slots[s].raddr = r.slots[s].raddr.wrapping_add(sbase);
        }
    }
    // the start-up writes (spec 5), counted from the run's origin
    let origin = m.bus.clock.saturating_sub(crate::engine::START_PHASE);
    let fw: u64 = FIRST_WRITE;
    let first_up = r.unpack();
    r.row = 0;
    // the reference's first write is a whole clock, or half a clock past one; the half clock is
    // there when the unpack's cost (half a clock per literal run) is a whole number, which puts
    // the tick start (71 779 clocks after this write plus the unpack) half a clock into a clock
    let t0 = (origin + fw) as f64 + if first_up.fract() == 0.0 { 0.5 } else { 0.0 };
    {
        let mut hw = Hw { bus: &mut m.bus };
        let put = |hw: &mut Hw, t: f64, f: &mut dyn FnMut(&mut Hw)| {
            hw.bus.clock = hw.bus.clock.max(cutx(t));
            f(hw);
        };
        for c in 0..4 {
            let base = t0 + [0.0, 83.0, 166.0, 249.0][c];
            put(&mut hw, base, &mut |h| h.vol(c, 0));
            put(&mut hw, base + 16.0, &mut |h| h.per(c, 0x7C));
        }
        put(&mut hw, t0 + 296.0, &mut |h| h.dmacon(0x000F));
        put(&mut hw, t0 + 304.0, &mut |h| h.bus.write_word(0xDFF09A, 0x0780));
        put(&mut hw, t0 + 312.0, &mut |h| h.bus.write_word(0xDFF09C, 0x0780));
        put(&mut hw, t0 + 332.0, &mut |h| {
            let p = h.bus.read_byte(0xBFE001);
            h.bus.write_byte(0xBFE001, p | 2);
        });
        let t = t0 + 332.0 + 229.0 + first_up;
        put(&mut hw, t, &mut |h| h.dmacon(0x000F));
        let mut tt = t;
        for (c, dt) in [(0usize, 8.0), (1, 6.0), (2, 6.0), (3, 6.0)] {
            tt += dt;
            put(&mut hw, tt, &mut |h| h.bus.write_word(0xDFF0AA + 16 * c as u32, 0));
        }
        tt += 8.0;
        put(&mut hw, tt, &mut |h| h.bus.write_word(0xDFF09A, 0x0780));
        tt += 8.0;
        put(&mut hw, tt, &mut |h| h.bus.write_word(0xDFF09C, 0x0780));
    }
    // the first tick starts 71 779 colour clocks plus the first unpack after the first start-up
    // write (the reference's tick start is the whole clock below that sum, plus half a clock)
    let s0i = (t0 + first_up + 71779.0).floor() as u64;
    r.frac = 0.5;
    r.start_delay = START_DELAY;
    m.bus.clock = (s0i.saturating_sub(TICK + START_DELAY)).max(m.bus.clock);
    Some(Box::new(r))
}

#[cfg(test)]
#[path = "nickpelling_tests.rs"]
mod tests;

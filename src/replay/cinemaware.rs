//! Cinemaware (`cin.*` with its samples in `Instruments/`), the MIDI-like music
//! driver of Cinemaware's Wings.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour, and this player was written from it alone and checked against the
//! Paula register writes of UADE's Cinemaware player.
//!
//! A module (`IBLK`) is pure data: instrument records selected by MIDI program
//! number (each with up to three key-ranged waves, a volume and a pitch envelope,
//! an LFO and a pitch-bend range), a table giving each MIDI channel a Paula voice
//! or dynamic allocation, and after `ASEQ` a time-stamped list of MIDI-style
//! events (note on and off, controllers, program change, pitch bend, end). The
//! waves are IFF 8SVX files in an `Instruments` folder beside the module, each
//! slot loaded as its own copy. Four voices, one Paula channel each; the period
//! comes from a pitch in 1/20 semitones and a rate per wave. The tick is the game's
//! 60 Hz CIA timer; at the end the song pauses 25 ticks and starts again.
//!
//! Timing: every write lands at the colour clock the original's would, from 68000
//! cycle costs per code path (`COST`, fitted to the measured write times of the
//! reference, black box) and the beam waits before a voice's DMA turns on; the song
//! starts where the original's init would end (`init_clocks`).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Cinemaware", detect, new };

/// Colour clocks of a tick: the CIA timer value 11822, (11822 + 1) * 5.
const TICK: u64 = (11822 + 1) * 5;

/// Pitch dividers of the 240 steps of an octave (1/20 semitone each).
const PITCH: [u16; 240] = [
    32768, 32862, 32957, 33053, 33148, 33244, 33340, 33437, 33533, 33630, 33728, 33825, 33923, 34021, 34120, 34218,
    34317, 34416, 34516, 34616, 34716, 34816, 34917, 35018, 35119, 35221, 35323, 35425, 35527, 35630, 35733, 35837,
    35940, 36044, 36148, 36253, 36358, 36463, 36568, 36674, 36780, 36887, 36993, 37100, 37208, 37315, 37423, 37531,
    37640, 37749, 37858, 37968, 38077, 38188, 38298, 38409, 38520, 38631, 38743, 38855, 38967, 39080, 39193, 39307,
    39420, 39534, 39649, 39763, 39878, 39994, 40109, 40225, 40342, 40458, 40575, 40693, 40810, 40928, 41047, 41166,
    41285, 41404, 41524, 41644, 41764, 41885, 42006, 42128, 42250, 42372, 42494, 42617, 42741, 42864, 42988, 43112,
    43237, 43362, 43488, 43613, 43740, 43866, 43993, 44120, 44248, 44376, 44504, 44633, 44762, 44891, 45021, 45151,
    45282, 45413, 45544, 45676, 45808, 45941, 46074, 46207, 46340, 46474, 46609, 46744, 46879, 47015, 47150, 47287,
    47424, 47561, 47698, 47836, 47975, 48113, 48253, 48392, 48532, 48672, 48813, 48954, 49096, 49238, 49380, 49523,
    49667, 49810, 49954, 50099, 50244, 50389, 50535, 50681, 50827, 50974, 51122, 51270, 51418, 51567, 51716, 51865,
    52015, 52166, 52317, 52468, 52620, 52772, 52925, 53078, 53231, 53385, 53540, 53694, 53850, 54006, 54162, 54318,
    54476, 54633, 54791, 54950, 55108, 55268, 55428, 55588, 55749, 55910, 56072, 56234, 56397, 56560, 56723, 56887,
    57052, 57217, 57382, 57548, 57715, 57882, 58049, 58217, 58385, 58554, 58724, 58894, 59064, 59235, 59406, 59578,
    59750, 59923, 60096, 60270, 60444, 60619, 60795, 60970, 61147, 61324, 61501, 61679, 61857, 62036, 62216, 62396,
    62576, 62757, 62939, 63121, 63303, 63486, 63670, 63854, 64039, 64224, 64410, 64596, 64783, 64970, 65158, 65347,
];

const REC: usize = 138;

/// The record count, if the file starts as a module does.
fn records(d: &[u8]) -> Option<usize> {
    (d.len() >= 6 && d[..4] == *b"IBLK" && (1..=128).contains(&d[4])).then(|| d[4] as usize)
}

/// Recognition: `IBLK`, 1-128 records, and `ASEQ` at an even distance (0..254)
/// from where the channel table ends.
fn detect(d: &[u8]) -> bool {
    let Some(n) = records(d) else { return false };
    let o = 22 + REC * n;
    (0..128).any(|k| d.get(o + 2 * k..o + 2 * k + 4) == Some(b"ASEQ"))
}

#[derive(Clone, Copy)]
struct Voice {
    chan: i8,
    note: i8,
    inst: u8,
    vel: u8,
    rate: u16,
    /// Bit 1: key held, bit 0: held by the sustain pedal.
    flags: u8,
    pidx: i8,
    penv: u16,
    vidx: i8,
    lvl: i16,
    lfo: i16,
    lfos: i16,
    age: u8,
    lcnt: u8,
    lpos: u32,
    llen: u16,
}

const G: u8 = 2;
const S: u8 = 1;

impl Default for Voice {
    fn default() -> Voice {
        Voice {
            chan: -1,
            note: 0,
            inst: 0,
            vel: 0,
            rate: 0,
            flags: 0,
            pidx: 0,
            penv: 0,
            vidx: 0,
            lvl: 0,
            lfo: 0,
            lfos: 0,
            age: 0,
            lcnt: 0,
            lpos: 0,
            llen: 0,
        }
    }
}

pub struct Cinemaware {
    d: Vec<u8>,
    title: String,
    /// The record of each program number.
    rec: [Option<usize>; 128],
    /// The guest address of each record's wave slots' sample data.
    sample: Vec<[u32; 3]>,
    /// Four zero bytes, the loop of a wave without one.
    silent: u32,
    ctab: [u8; 16],
    first: usize,
    pos: usize,
    wait: i16,
    pause: u16,
    prog: [u8; 16],
    bend: [u8; 16],
    modw: [u8; 16],
    cvol: [u8; 16],
    ped: [u8; 16],
    v: [Voice; 4],
    songend: bool,
    /// The tick's start (colour clock) and the original's 68000 cycles since then.
    t0: u64,
    cy: i64,
}

/// The offset of a slot's sample data in its 8SVX file: 8 bytes after the first
/// `BODY`, searched from offset 8 in steps of 2.
fn body(f: &[u8]) -> Option<usize> {
    (8..f.len().saturating_sub(3)).step_by(2).find(|&o| f[o..o + 4] == *b"BODY").map(|o| o + 8)
}

/// The colour clocks the original's check and init take before the song starts, from their
/// measured 68000 cycle costs (each stretch between two host calls rounds its cycles/2 down):
/// the check (the distance from the channel table to `ASEQ`), the instrument loads in record
/// order (the directory string is copied per file), the sample setup in program order, the scan
/// of the sequence up to its end row, then three constant calls.
fn init_clocks(d: &[u8], n: usize, aseq: usize, dirlen: usize, rec: &[Option<usize>; 128]) -> u64 {
    let ct = 6 + REC * n;
    let k = (aseq as i64 - ct as i64 - 18) / 2;
    let mut cc = (316 + 52 * k) / 2;
    let file = |r: usize, j: usize| be32(d, 6 + REC * r + 0x3C + 26 * j + 6) != 0;
    // Instrument loads: every slot in record order.
    let slots: Vec<(usize, usize)> = (0..n).flat_map(|r| (0..3).map(move |j| (r, j))).filter(|&(r, j)| file(r, j)).collect();
    let Some(&(r0, j0)) = slots.first() else { return 0 };
    cc += (159 + 43 * (3 * r0 + j0) as i64) / 2;
    for (q, &(r, j)) in slots.iter().enumerate() {
        let w = 6 + REC * r + 0x3C + 26 * j;
        let len = d[w..w + 6].iter().rposition(|&b| b != b' ' && b != 0).map_or(0, |p| p + 1);
        cc += (580 + 17 * dirlen as i64 + 59 * (6 - len as i64)) / 2;
        cc += match slots.get(q + 1) {
            Some(&(r2, j2)) => (150 + 43 * (3 * (r2 - r) + j2 - j - 1) as i64 + 26 * (r2 - r) as i64) / 2,
            None => (81 + 43 * (3 * (n - r) - j - 1) as i64) / 2,
        };
    }
    // Sample setup: the programs' records in program order.
    let ps: Vec<(i64, i64)> =
        (0..128).filter_map(|p| rec[p].map(|r| (p, r))).flat_map(|(p, r)| (0..3).filter(move |&j| file(r, j)).map(move |j| (p as i64, j as i64))).collect();
    let Some(&(p0, s0)) = ps.first() else { return 0 };
    cc += 18 + (10297 + 153 * n as i64 + 80 * p0 + 120 * s0) / 2;
    for w in ps.windows(2) {
        let ((pa, ja), (pb, jb)) = (w[0], w[1]);
        cc += if pa == pb { (2538 + 120 * (jb - ja - 1)) / 2 } else { (2542 + 80 * (pb - pa) + 120 * (2 - ja + jb)) / 2 };
    }
    let (pl, jl) = ps[ps.len() - 1];
    let mut rows = 0i64;
    let mut o = aseq + 4;
    while o + 5 <= d.len() {
        rows += if d[o + 4] != 0 { 44 } else if d[o + 3] == 0x2F { 94 } else { 69 };
        if d[o + 2] >> 4 == 1 {
            break;
        }
        o += 5;
    }
    cc += (3174 + 80 * (127 - pl) + 120 * (2 - jl) + rows + 47 * k) / 2;
    (cc + 8 + 740 + 740 + 5161) as u64
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let n = records(d)?;
    let c = 6 + REC * n;
    let aseq = (c..d.len().saturating_sub(3)).step_by(2).find(|&o| d[o..o + 4] == *b"ASEQ")?;
    let mut rec = [None; 128];
    let mut sample = vec![[0u32; 3]; n];
    for (i, s) in sample.iter_mut().enumerate() {
        let r = 6 + REC * i;
        rec[(d[r] & 0x7F) as usize] = Some(i);
        for (j, a) in s.iter_mut().enumerate() {
            let w = r + 0x3C + 26 * j;
            if be32(d, w + 6) == 0 {
                continue;
            }
            let name = String::from_utf8_lossy(&d[w..w + 6]).trim_end_matches([' ', '\0']).to_string();
            let f = crate::deli::read_companion(m, &format!("Instruments/{name}")).1.ok()?;
            let o = body(&f)?;
            let g = m.alloc(f.len() as u32);
            if g == 0 {
                return None;
            }
            m.bus.write_bytes(g, &f);
            *a = g + o as u32;
        }
    }
    let silent = m.alloc(4);
    if silent == 0 {
        return None;
    }
    let mut ctab = [0u8; 16];
    ctab.copy_from_slice(&d[c..c + 16]);
    let mut r = Cinemaware {
        d: d.to_vec(),
        title: m.bus.read_cstr(m.deli.file_buf),
        rec: rec.map(|x| x.map(|i| 6 + REC * i)),
        sample,
        silent,
        ctab,
        first: aseq + 4,
        pos: 0,
        wait: 0,
        pause: 0,
        prog: [0; 16],
        bend: [0; 16],
        modw: [0; 16],
        cvol: [0; 16],
        ped: [0; 16],
        v: Default::default(),
        songend: false,
        t0: 0,
        cy: 0,
    };
    let dirlen = m.bus.read_cstr(m.deli.dir_buf).len();
    m.bus.clock += init_clocks(d, n, aseq, dirlen, &rec);
    r.reset();
    let mut hw = Hw { bus: &mut m.bus };
    for ch in 0..4 {
        hw.vol(ch, 0);
    }
    Some(Box::new(r))
}

/// The original's code paths, each with its 68000 cycles in `COST`: the tick's writes are direct
/// Paula writes, each at the tick's start plus half the cycles its code took to reach it.
mod k {
    pub const TICK: usize = 0;
    pub const PAUSE: usize = 1;
    pub const ROW: usize = 2;
    pub const EV_END: usize = 3;
    pub const EV_OFF: usize = 4;
    pub const EV_ON0: usize = 5;
    pub const EV_ON: usize = 6;
    pub const EV_CTL1: usize = 7;
    pub const EV_CTL7: usize = 8;
    pub const EV_CTL40: usize = 9;
    pub const EV_CTLX: usize = 10;
    pub const EV_PROG: usize = 11;
    pub const EV_BEND: usize = 12;
    pub const EV_X: usize = 13;
    pub const ROW_NEXT: usize = 14;
    pub const EOF: usize = 15;
    pub const DEC: usize = 16;
    pub const NOFF_V: usize = 17;
    pub const NOFF_REL: usize = 18;
    pub const REL_NOREC: usize = 19;
    pub const PED_V: usize = 20;
    pub const PED_HELD: usize = 21;
    pub const PED_REL: usize = 22;
    pub const PED_NONE: usize = 23;
    pub const ON_NOREC: usize = 24;
    pub const ON_DYN: usize = 25;
    pub const SC: usize = 26;
    pub const SC_FREE: usize = 27;
    pub const SC_NG: usize = 28;
    pub const SC_NG_SAME: usize = 29;
    pub const SC_CH: usize = 30;
    pub const SC_CH_NOTE: usize = 31;
    pub const SC_NOTE: usize = 32;
    pub const SC_AGE: usize = 33;
    pub const SC_BEST: usize = 34;
    pub const ON_NOBEST: usize = 35;
    pub const WS_MISS: usize = 36;
    pub const WS_HIT: usize = 37;
    pub const ON_NOWAVE: usize = 38;
    pub const ON_NOSMP: usize = 39;
    pub const ON_PER: usize = 40;
    pub const ON_DMA: usize = 41;
    pub const ON_LOOP: usize = 42;
    pub const ON_NOLOOP: usize = 43;
    pub const ON_PED: usize = 44;
    pub const ON_VOL: usize = 45;
    pub const ON_LOC: usize = 46;
    pub const ON_LEN: usize = 47;
    pub const ON_END: usize = 48;
    pub const V_IDLE: usize = 49;
    pub const V_NOREC: usize = 50;
    pub const V: usize = 51;
    pub const AGE: usize = 52;
    pub const RR1: usize = 53;
    pub const VUP: usize = 54;
    pub const VUP_REACH: usize = 55;
    pub const VDN: usize = 56;
    pub const VDN_REACH: usize = 57;
    pub const VOFF: usize = 58;
    pub const VSUS: usize = 59;
    pub const V8: usize = 60;
    pub const P: usize = 61;
    pub const PUP: usize = 62;
    pub const PUP_REACH: usize = 63;
    pub const PDN: usize = 64;
    pub const PDN_REACH: usize = 65;
    pub const PSUS: usize = 66;
    pub const LFO_LO: usize = 67;
    pub const LFO_HI: usize = 68;
    pub const PB_FF: usize = 69;
    pub const DIV0: usize = 70;
    pub const V_PER: usize = 71;
    pub const LCNT: usize = 72;
    pub const LOOP_LOC: usize = 73;
    pub const LOOP_LEN: usize = 74;
    pub const V_VOL: usize = 75;
    pub const VV_CLAMP: usize = 76;
    pub const V_WAIT: usize = 77;
    pub const V_DMA_ON: usize = 78;
    pub const V_DMA_OFF: usize = 79;
    pub const END_RESET: usize = 80;
    pub const SC_NOTBEST: usize = 81;
    // 82..=101: unused (zero cost).
    pub const PER_BIT: usize = 102;
    pub const WAIT_V: usize = 103;
    pub const LOOP_END_V: usize = 107;
    pub const HELD_V: usize = 111;
    pub const HELD_P: usize = 112;
    pub const V_OFF_END: usize = 113;
    /// a note off's test of a voice: note and channel (`NOFF_NC + 2 * note equal + channel equal`)
    pub const NOFF_NC: usize = 114;
    pub const NOFF_NOTG: usize = 118;
    /// a write by its site (12) and voice: `WS + 4 * site + voice`
    pub const WS: usize = 119;
    pub const SC_NG_CH: usize = 167;
    pub const PED_HELD_UP: usize = 168;
    pub const PED_UP: usize = 169;
    pub const PED_CH: usize = 170;
    pub const N: usize = 171;
}

const COST: [i32; k::N] = [0, 0, 978, 0, -1, 30, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 415, 192, 400, 0, 305, 60, 416, 0, 0, 1, 339, 28, 69, 8, 57, 12, 12, 40, 32, 0, 359, 0, 0, 0, 1148, 34, 1130, 0, 10, 1526, 616, 713, 111, 136, 0, 2850, 16, 16, 0, 36, 15, 36, 32, 16, 0, 0, 0, 0, 0, 299, 16, 0, 0, -2, 0, 0, 37, 178, 193, 724, 16, 157, 79, 103, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 10, 10, 10, 0, 85, 54, 23, 15, 45, 0, 10, 0, 0, 41, 66, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 31, 62, 70, 0, 0, 0, 0, 0, 31, 62, 70, 0, 31, 62, 70, 0, 54, 108, 139, 10, 10, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 37, -10, 25, 46];

impl Cinemaware {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    /// The cycles of a code path.
    fn k(&mut self, l: usize) {
        self.cy += COST[l] as i64;
    }

    /// Before a write of register `kind` of voice `v`.
    fn w(&mut self, hw: &mut Hw, kind: usize, v: usize) {
        self.k(k::WS + 4 * kind + v);
        self.at(hw);
    }

    /// The clock of the next write (or beam read): the tick's start plus half the cycles.
    fn at(&mut self, hw: &mut Hw) {
        let t = self.t0 + self.cy.max(0) as u64 / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
    }

    /// The original's wait for nine changes of the beam's line: it reads the line, then reads
    /// again every 25 cycles until it differs, nine times.
    fn wait_lines(&mut self, hw: &mut Hw) {
        use crate::paula::LINE;
        self.at(hw);
        let mut c = self.cy.max(0) as u64;
        for n in 0..9 {
            let l = (self.t0 + c / 2) / LINE;
            c += 16;
            while (self.t0 + c / 2) / LINE == l {
                c += 25;
            }
            if n < 8 {
                c += 35;
            }
        }
        self.cy = c as i64;
    }

    /// The song from its first row, as at init and at every restart.
    fn reset(&mut self) {
        self.pos = self.first;
        self.wait = be16(&self.d, self.pos) as i16;
        for v in self.v.iter_mut() {
            v.chan = -1;
            v.flags = 0;
        }
        for ch in 0..16 {
            self.prog[ch] = ch as u8;
            self.bend[ch] = 0x40;
            self.modw[ch] = 0;
            self.cvol[ch] = 0x7F;
            self.ped[ch] = 0;
        }
    }

    /// Executes one row; true if it ended the song.
    fn event(&mut self, hw: &mut Hw) -> bool {
        let p = self.pos;
        let (st, d1, d2) = (self.rb(p + 2), self.rb(p + 3), self.rb(p + 4));
        let ch = (st & 15) as usize;
        match st >> 4 {
            0x1 => {
                self.k(k::EV_END);
                self.songend = true;
                self.pause = 25;
                self.reset();
                self.k(k::END_RESET);
                return true;
            }
            0x8 => {
                self.k(k::EV_OFF);
                self.note_off(ch, d1)
            }
            0x9 if d2 == 0 => {
                self.k(k::EV_ON0);
                self.note_off(ch, d1)
            }
            0x9 => {
                self.k(k::EV_ON);
                self.note_on(self.prog[ch], d1, d2, ch, hw)
            }
            0xB => match d1 {
                1 => {
                    self.k(k::EV_CTL1);
                    self.modw[ch] = d2
                }
                7 => {
                    self.k(k::EV_CTL7);
                    self.cvol[ch] = d2
                }
                0x40 => {
                    self.k(k::EV_CTL40);
                    self.ped[ch] = d2;
                    self.pedal(ch, d2);
                }
                _ => self.k(k::EV_CTLX),
            },
            0xC => {
                self.k(k::EV_PROG);
                self.prog[ch] = d1
            }
            0xE => {
                self.k(k::EV_BEND);
                self.bend[ch] = d2
            }
            _ => self.k(k::EV_X),
        }
        false
    }

    fn note_on(&mut self, prog: u8, note: u8, vel: u8, ch: usize, hw: &mut Hw) {
        let Some(r) = self.rec.get(prog as usize).copied().flatten() else {
            self.k(k::ON_NOREC);
            return;
        };
        let mut v = self.ctab[ch] as usize;
        if v > 3 {
            self.k(k::ON_DYN);
            let (mut best, mut max, mut minage) = (None, 0u8, 0xFFu8);
            for i in 0..4 {
                let x = self.v[i];
                self.k(k::SC);
                let mut score = 0u8;
                if x.chan < 0 {
                    self.k(k::SC_FREE);
                    score |= 0x10;
                }
                if x.flags & G == 0 {
                    self.k(k::SC_NG);
                    score |= 0x08;
                    if x.chan as i32 == ch as i32 {
                        self.k(k::SC_NG_CH);
                    }
                    if x.chan as i32 == ch as i32 && self.ctab[15] == 0 {
                        self.k(k::SC_NG_SAME);
                        score |= 0x30;
                    }
                }
                if x.chan as i32 == ch as i32 {
                    self.k(k::SC_CH);
                    score |= 0x04;
                    if x.note as u8 == note {
                        self.k(k::SC_CH_NOTE);
                        score |= 0x08;
                    }
                }
                if x.note as u8 == note {
                    self.k(k::SC_NOTE);
                    score |= 0x02;
                }
                if x.age < minage {
                    self.k(k::SC_AGE);
                    minage = x.age;
                    score |= 0x01;
                }
                if score > max {
                    self.k(k::SC_BEST);
                    max = score;
                    best = Some(i);
                } else {
                    self.k(k::SC_NOTBEST);
                }
            }
            let Some(b) = best else {
                self.k(k::ON_NOBEST);
                return;
            };
            v = b;
        }
        let mut found = None;
        for j in 0..3 {
            let w = r + 0x3C + 26 * j;
            let lo = self.rb(w + 10) as i8 as i16 as u16;
            let hi = self.rb(w + 11) as i8 as i16 as u16;
            if lo <= note as u16 && note as u16 <= hi {
                self.k(k::WS_HIT);
                found = Some(j);
                break;
            }
            self.k(k::WS_MISS);
        }
        let Some(j) = found else {
            self.k(k::ON_NOWAVE);
            return;
        };
        let w = r + 0x3C + 26 * j;
        if be32(&self.d, w + 6) == 0 {
            self.k(k::ON_NOSMP);
            return;
        }
        let rate = be16(&self.d, w + 12);
        let (start, lp, end) = (be32(&self.d, w + 14) as i32, be32(&self.d, w + 18) as i32, be32(&self.d, w + 22) as i32);
        let smp = self.sample[(r - 6) / REC][j];
        let lfos = self.rb(r + 0x16) as i16;
        self.v[v].chan = -1;
        self.k(k::ON_PER);
        self.w(hw, 0, v);
        hw.per(v, 0x0080);
        self.k(k::ON_DMA);
        self.w(hw, 1, v);
        hw.dmacon(1 << v);
        let held = if self.ped[ch] != 0 {
            self.k(k::ON_PED);
            G | S
        } else {
            G
        };
        let (lpos, llen) = if lp >= 0 {
            self.k(k::ON_LOOP);
            (smp.wrapping_add((lp / 2 * 2) as u32), (end.wrapping_sub(lp) / 2) as u16)
        } else {
            self.k(k::ON_NOLOOP);
            (self.silent, 2)
        };
        let x = &mut self.v[v];
        x.lpos = lpos;
        x.llen = llen;
        x.lcnt = 2;
        x.note = note as i8;
        x.inst = prog;
        x.vel = vel;
        x.rate = rate;
        x.pidx = 0;
        x.penv = 0x80;
        x.vidx = 0;
        x.lvl = 0;
        x.age = 0xFF;
        x.lfo = 0;
        x.lfos = lfos;
        x.flags = held;
        x.chan = ch as i8;
        self.k(k::ON_VOL);
        self.w(hw, 2, v);
        hw.vol(v, 0);
        self.k(k::ON_LOC);
        self.w(hw, 3, v);
        hw.loc(v, smp.wrapping_add((start / 2 * 2) as u32));
        self.k(k::ON_LEN);
        self.w(hw, 4, v);
        hw.len(v, (end.wrapping_sub(start) / 2) as u16);
        self.k(k::ON_END);
    }

    fn release(&mut self, v: usize) {
        let Some(r) = self.rec[(self.v[v].inst & 0x7F) as usize] else {
            self.k(k::REL_NOREC);
            return;
        };
        let (vi, pi) = (self.rb(r + 0x2A) as i8, self.rb(r + 0x18) as i8);
        let x = &mut self.v[v];
        x.vidx = vi;
        x.pidx = pi;
        x.flags &= S;
    }

    fn note_off(&mut self, ch: usize, n: u8) {
        for v in 0..4 {
            let x = self.v[v];
            self.k(k::NOFF_V);
            let (ne, ce) = (x.note as u8 == n, x.chan as i32 == ch as i32);
            self.k(k::NOFF_NC + 2 * ne as usize + ce as usize);
            if ne && ce {
                if x.flags & G != 0 {
                    self.k(k::NOFF_REL);
                    self.release(v);
                } else {
                    self.k(k::NOFF_NOTG);
                }
            }
        }
    }

    fn pedal(&mut self, ch: usize, val: u8) {
        for v in 0..4 {
            let x = self.v[v];
            self.k(k::PED_V);
            if x.chan as i32 != ch as i32 {
                continue;
            }
            self.k(k::PED_CH);
            if x.flags & G != 0 {
                self.k(k::PED_HELD);
                if val == 0 {
                    self.k(k::PED_HELD_UP);
                }
                self.v[v].flags = if val != 0 { G | S } else { G };
            } else if val == 0 && {
                self.k(k::PED_UP);
                true
            } && x.flags & S != 0
            {
                self.k(k::PED_REL);
                self.release(v);
                self.v[v].flags = 0;
            } else {
                self.k(k::PED_NONE);
            }
        }
    }

    /// The period of voice `v` on MIDI channel `ch` with record `r`.
    fn period(&mut self, v: usize, ch: usize, r: usize) -> u16 {
        let x = self.v[v];
        let tr = self.rb(r + 0x12) as i8 as i16;
        let pb = self.rb(r + 0x17);
        let mut d: i16 = if pb == 0xFF {
            self.k(k::PB_FF);
            tr.wrapping_add(60).wrapping_mul(20)
        } else {
            (x.note as i16).wrapping_add(tr).wrapping_mul(20)
        };
        d = d.wrapping_add(x.lfo / 5);
        d = d.wrapping_add((x.penv.wrapping_sub(0x80) << 2) as i16);
        let b = (self.bend[ch] as u16).wrapping_sub(0x40);
        let t = b.wrapping_mul(pb as u16).wrapping_mul(20);
        d = d.wrapping_add((t as i16) >> 6);
        let q = d.wrapping_div(240);
        let rem = d.wrapping_sub(q.wrapping_mul(240));
        // a negative pitch reads before the table in the original (not reached by
        // any module): the first step here
        let t = PITCH.get(rem.max(0) as usize).copied().unwrap_or(0) as u32;
        let mut div = t.checked_shl((q & 63) as u32).unwrap_or(0);
        if div == 0 {
            self.k(k::DIV0);
            div = 1;
        }
        (((x.rate as u32) << 20) as i32).wrapping_div(div as i32) as u16
    }

    fn voice(&mut self, v: usize, hw: &mut Hw) {
        let x = self.v[v];
        if x.chan < 0 {
            self.k(k::V_IDLE);
            return;
        }
        let ch = x.chan as usize & 15;
        let Some(r) = self.rec[(x.inst & 0x7F) as usize] else {
            self.k(k::V_NOREC);
            return;
        };
        self.k(k::V);
        let held = x.flags & G != 0;
        let rb = |o: usize, i: i8| self.d.get((r + o).wrapping_add_signed(i as isize)).copied().unwrap_or(0);
        let (vr, vt) = (rb(0x34, x.vidx), rb(0x2C, x.vidx));
        let (pt, pr) = (rb(0x1A, x.pidx), rb(0x22, x.pidx));
        let (vsus, psus, depth, step) = (self.rb(r + 0x2B) as i8, self.rb(r + 0x19) as i8, self.rb(r + 0x15), self.rb(r + 0x16));
        let mut x = self.v[v];
        if x.age != 0 {
            self.k(k::AGE);
            x.age -= 1;
        }
        // the volume envelope
        if held {
            self.k(k::HELD_V);
        }
        let mut rr = vr as i16;
        if !held && rr == 0 {
            self.k(k::RR1);
            rr = 1;
        }
        let t = vt as i16;
        if t > x.lvl {
            self.k(k::VUP);
            x.lvl = x.lvl.wrapping_add(rr);
            if t <= x.lvl {
                self.k(k::VUP_REACH);
                x.lvl = t;
                x.vidx = x.vidx.wrapping_add(1);
            }
        } else {
            self.k(k::VDN);
            x.lvl = x.lvl.wrapping_sub(rr);
            if t >= x.lvl {
                self.k(k::VDN_REACH);
                x.lvl = t;
                x.vidx = x.vidx.wrapping_add(1);
            }
            if x.lvl < 1 {
                self.k(k::VOFF);
                x.chan = -1;
                x.flags &= S;
            }
        }
        if held && x.vidx > vsus {
            self.k(k::VSUS);
            x.vidx = 0;
        }
        if x.vidx > 7 {
            self.k(k::V8);
            x.lvl = 0;
            x.chan = -1;
        }
        // the pitch envelope
        if x.pidx < 8 {
            self.k(k::P);
            let (t, r) = (pt as u16, pr as u16);
            if t > x.penv {
                self.k(k::PUP);
                x.penv = x.penv.wrapping_add(r);
                if t <= x.penv {
                    self.k(k::PUP_REACH);
                    x.penv = t;
                    x.pidx = x.pidx.wrapping_add(1);
                }
            } else {
                self.k(k::PDN);
                x.penv = x.penv.wrapping_sub(r);
                if t >= x.penv {
                    self.k(k::PDN_REACH);
                    x.penv = t;
                    x.pidx = x.pidx.wrapping_add(1);
                }
            }
            if held {
                self.k(k::HELD_P);
            }
            if held && x.pidx > psus {
                self.k(k::PSUS);
                x.pidx = 0;
            }
        }
        // the LFO
        x.lfo = x.lfo.wrapping_add(x.lfos);
        if x.lfo < 0 {
            self.k(k::LFO_LO);
            x.lfo = 0;
            x.lfos = step as i16;
        }
        if x.lfo > depth as i16 {
            self.k(k::LFO_HI);
            x.lfo = depth as i16;
            x.lfos = -(step as i16);
        }
        self.v[v] = x;
        let per = self.period(v, ch, r);
        self.k(k::V_PER);
        for _ in 0..per.count_ones() {
            self.k(k::PER_BIT);
        }
        self.w(hw, 5, v);
        hw.per(v, per);
        if self.v[v].lcnt != 0 {
            self.k(k::LCNT);
            self.v[v].lcnt -= 1;
            if self.v[v].lcnt == 0 {
                self.k(k::LOOP_LOC);
                self.w(hw, 6, v);
                hw.loc(v, self.v[v].lpos);
                self.k(k::LOOP_LEN);
                self.w(hw, 7, v);
                hw.len(v, self.v[v].llen);
                self.k(k::LOOP_END_V + v);
            }
        }
        let x = self.v[v];
        let mut vv = x.vel as u16 + self.rb(r + 0x14) as u16;
        if vv > 0x7F {
            self.k(k::VV_CLAMP);
            vv = 0x7F;
        }
        let a = (self.cvol[ch] as u16).wrapping_mul(vv) >> 7;
        let vol = (a.wrapping_mul(x.lvl as u16) >> 9) & 0x7F;
        self.k(k::V_VOL);
        self.w(hw, 8, v);
        hw.vol(v, vol);
        if self.v[v].chan < 0 {
            self.k(k::V_DMA_OFF);
            self.w(hw, 9, v);
            hw.dmacon(1 << v);
            self.k(k::V_OFF_END);
        } else {
            self.k(k::V_WAIT);
            self.k(k::WAIT_V + v);
            self.wait_lines(hw);
            self.k(k::V_DMA_ON);
            self.w(hw, 10, v);
            hw.dmacon(0x8200 | 1 << v);
        }
    }
}

impl Replayer for Cinemaware {
    fn format(&self) -> &'static str {
        "Cinemaware"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cy = 0;
        if self.pause != 0 {
            self.k(k::PAUSE);
            self.w(hw, 11, 0);
            hw.dmacon(0x000F);
            self.pause -= 1;
            return;
        }
        self.k(k::TICK);
        while self.wait <= 0 {
            self.k(k::ROW);
            if self.event(hw) {
                return;
            }
            self.pos += 5;
            if self.pos + 5 > self.d.len() {
                // a list without an end row: the end of the file ends it
                self.k(k::EOF);
                self.songend = true;
                self.pause = 25;
                self.reset();
                return;
            }
            self.k(k::ROW_NEXT);
            self.wait = be16(&self.d, self.pos) as i16;
        }
        self.k(k::DEC);
        self.wait -= 1;
        for v in 0..4 {
            self.voice(v, hw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// One record (id 0, wave "Wave" for every note, rate 428, 16 bytes looped from
    /// 8), a channel table with MIDI channel 0 on voice 2, and the rows: note 60 on,
    /// 3 ticks later note off, then the end.
    fn song() -> Vec<u8> {
        let mut d = b"IBLK\x01\x01".to_vec();
        let mut r = vec![0u8; REC];
        // the volume envelope: up to 64 at once, then sustain
        r[0x2B] = 0;
        r[0x2C] = 64;
        r[0x34] = 64;
        r[0x1A..0x22].fill(0x80);
        let w = 0x3C;
        r[w..w + 6].copy_from_slice(b"Wave  ");
        r[w + 6..w + 10].copy_from_slice(&1u32.to_be_bytes());
        r[w + 10] = 0;
        r[w + 11] = 127;
        r[w + 12..w + 14].copy_from_slice(&428u16.to_be_bytes());
        r[w + 14..w + 18].copy_from_slice(&0u32.to_be_bytes());
        r[w + 18..w + 22].copy_from_slice(&8u32.to_be_bytes());
        r[w + 22..w + 26].copy_from_slice(&24u32.to_be_bytes());
        for j in 1..3 {
            r[w + 26 * j..w + 26 * j + 6].copy_from_slice(b"NoWave");
        }
        d.extend(&r);
        let mut ctab = [4u8; 16];
        ctab[0] = 2;
        d.extend(ctab);
        d.extend(b"ASEQ");
        for row in [[0, 1, 0x90, 60, 100], [0, 3, 0x80, 60, 0], [0, 0, 0x10, 0x2F, 0]] {
            d.extend(row);
        }
        d
    }

    fn wave() -> Vec<u8> {
        let mut f = b"FORM\0\0\0\x2c8SVXVHDR\0\0\0\x14".to_vec();
        f.resize(f.len() + 0x14, 0);
        f.extend(b"BODY\0\0\0\x18");
        f.extend([0x40u8; 24]);
        f
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        let mut e = d.clone();
        e[4] = 0;
        assert!(!detect(&e));
        let mut e = d.clone();
        e[22 + REC] = b'X';
        assert!(!detect(&e));
        assert_eq!(body(&wave()), Some(48));
    }

    #[test]
    fn pitch_of_middle_c() {
        // note 60, rate 428: pitch 1200 = 5 octaves, divider 32768 << 5
        let dir = Path::new("/cin-test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("Instruments/Wave"), wave());
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&dir.join("cin.test"), song()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Cinemaware");
        let mut out = vec![0f32; 2 * 800 * 40];
        e.render(&mut out);
        assert!(e.songend());
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // voice 2: muted (period 0x80, DMA off), the start of 12 words, then the
        // loop of 8 words at byte 8 two ticks later
        assert_eq!(reg(0xDFF0C6)[..2], [0x80, ((428u32 << 20) / (32768 << 5)) as u16]);
        assert_eq!(reg(0xDFF0C4)[..2], [12, 8]);
        let lc = reg(0xDFF0C2);
        assert_eq!(lc[1].wrapping_sub(lc[0]), 8);
        let dma = reg(0xDFF096);
        assert!(dma.windows(2).any(|w| w == [0x0004, 0x8204]));
        // full volume: 127 * 100 >> 7 = 99, * 64 >> 9 = 12
        assert_eq!(reg(0xDFF0C8)[..3], [0, 0, 12]);
        // the end: 25 ticks of DMA off
        assert!(dma.windows(25).any(|w| w.iter().all(|&x| x == 0x000F)));
    }
}

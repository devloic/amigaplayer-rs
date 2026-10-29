//! Synth Pack (`osp.*` with an instrument companion `smp.*` or `smp.set`), Karsten
//! Obarski's music system of Centerbase.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour, and this player was written from it alone and checked against the
//! Paula register writes of UADE's Synth Pack player.
//!
//! A song file (`OBISYNTHPACK`) holds 64 positions, 256 track sets of four slot
//! entries (pattern, instrument offset, transposition, speed) and the patterns:
//! lists of six-byte events, each at its row. The instruments are in a companion
//! file shared by the songs: synthetic waveforms and samples, each with an ADSR
//! envelope, tremolo, vibrato, pulse-width modulation (a window moved over a
//! 512-byte waveform) and portamento. Four voices, one Paula channel each (the
//! voices play channels 0, 1, 3, 2); a note is set up one tick after its row, its
//! DMA goes on at the start of the next tick and its loop is queued then. 50 Hz.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Synth Pack", detect, new };

const MAGIC: &[u8; 12] = b"OBISYNTHPACK";

/// Paula periods of notes 0..71.
const PERIODS: [u16; 72] = [
    3424, 3232, 3048, 2880, 2712, 2560, 2416, 2280, 2152, 2032, 1920, 1812, 1712, 1616, 1524, 1440, 1356, 1280, 1208,
    1140, 1076, 1016, 960, 904, 856, 808, 760, 720, 680, 640, 604, 572, 540, 508, 480, 452, 428, 404, 380, 360, 340,
    320, 302, 286, 270, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113, 107, 101, 95, 90,
    85, 80, 75, 71, 67, 63, 60, 56,
];

/// The six modulation waves (128 signed bytes each): ramp up, ramp down, triangle,
/// sine, square, noise.
const WAVES: [[i8; 128]; 6] = [
    [
        -127, -125, -123, -121, -119, -117, -115, -113, -111, -109, -107, -105, -103, -101, -99, -97, -95, -93, -91,
        -89, -87, -85, -83, -81, -79, -77, -75, -73, -71, -69, -67, -65, -63, -61, -59, -57, -55, -53, -51, -49, -47,
        -45, -43, -41, -39, -37, -35, -33, -31, -29, -27, -25, -23, -21, -19, -17, -15, -13, -11, -9, -7, -5, -3, -1,
        1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23, 25, 27, 29, 31, 33, 35, 37, 39, 41, 43, 45, 47, 49, 51, 53, 55, 57,
        59, 61, 63, 65, 67, 69, 71, 73, 75, 77, 79, 81, 83, 85, 87, 89, 91, 93, 95, 97, 99, 101, 103, 105, 107, 109,
        111, 113, 115, 117, 119, 121, 123, 125, 127,
    ],
    [
        127, 125, 123, 121, 119, 117, 115, 113, 111, 109, 107, 105, 103, 101, 99, 97, 95, 93, 91, 89, 87, 85, 83, 81,
        79, 77, 75, 73, 71, 69, 67, 65, 63, 61, 59, 57, 55, 53, 51, 49, 47, 45, 43, 41, 39, 37, 35, 33, 31, 29, 27, 25,
        23, 21, 19, 17, 15, 13, 11, 9, 7, 5, 3, 1, -1, -3, -5, -7, -9, -11, -13, -15, -17, -19, -21, -23, -25, -27,
        -29, -31, -33, -35, -37, -39, -41, -43, -45, -47, -49, -51, -53, -55, -57, -59, -61, -63, -65, -67, -69, -71,
        -73, -75, -77, -79, -81, -83, -85, -87, -89, -91, -93, -95, -97, -99, -101, -103, -105, -107, -109, -111,
        -113, -115, -117, -119, -121, -123, -125, -127,
    ],
    [
        127, 123, 119, 115, 111, 107, 103, 99, 95, 91, 87, 83, 79, 75, 71, 67, 63, 59, 55, 51, 47, 43, 39, 35, 31, 27,
        23, 19, 15, 11, 7, 3, -1, -5, -9, -13, -17, -21, -25, -29, -33, -37, -41, -45, -49, -53, -57, -61, -65, -69,
        -73, -77, -81, -85, -89, -93, -97, -101, -105, -109, -113, -117, -121, -125, -125, -121, -117, -113, -109,
        -105, -101, -97, -93, -89, -85, -81, -77, -73, -69, -65, -61, -57, -53, -49, -45, -41, -37, -33, -29, -25,
        -21, -17, -13, -9, -5, -1, 3, 7, 11, 15, 19, 23, 27, 31, 35, 39, 43, 47, 51, 55, 59, 63, 67, 71, 75, 79, 83,
        87, 91, 95, 99, 103, 107, 111, 115, 119, 123, 127,
    ],
    [
        0, 6, 12, 18, 24, 31, 37, 43, 48, 54, 60, 65, 71, 76, 81, 85, 90, 94, 98, 102, 106, 109, 112, 115, 118, 120,
        122, 124, 125, 126, 127, 127, 127, 127, 127, 126, 125, 124, 122, 120, 118, 115, 112, 109, 106, 102, 98, 94, 90,
        85, 81, 76, 71, 65, 60, 54, 48, 43, 37, 31, 24, 18, 12, 6, 0, -7, -13, -19, -25, -32, -38, -44, -49, -55, -61,
        -66, -72, -77, -82, -86, -91, -95, -99, -103, -107, -110, -113, -116, -119, -121, -123, -125, -126, -127, -128,
        -128, -128, -128, -128, -127, -126, -125, -123, -121, -119, -116, -113, -110, -107, -103, -99, -95, -91, -86,
        -82, -77, -72, -66, -61, -55, -49, -44, -38, -32, -25, -19, -13, -7,
    ],
    [
        -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127,
        -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127,
        -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, -127,
        -127, -127, -127, -127, -127, -127, -127, -127, -127, -127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
        127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
        127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
        127, 127, 127, 127, 127, 127, 127, 127, 127, 127,
    ],
    [
        -118, 11, -34, -97, 13, -17, -95, 62, 50, 110, -61, -37, -77, 10, 47, 89, -97, -83, 46, -3, -62, 83, -78, -2,
        99, 105, 48, -102, -116, -22, 93, 126, 11, -50, -2, 127, -82, -109, 29, -61, -119, 18, 27, 67, -22, -2, -102,
        47, 14, 124, -66, -112, -17, 32, 27, -106, 75, 3, -102, -99, -56, 110, 124, 30, -83, -78, -61, 2, 46, 83, -98,
        -89, 45, -9, -79, 38, 35, 119, -46, -62, -94, 19, 14, 96, -34, -12, -113, 68, 115, -6, 35, 103, -14, -16, -95,
        3, 13, 83, -4, -25, -118, 55, 31, 102, -81, -102, 28, -54, -120, 27, 27, 75, -22, -26, -70, 23, 14, 103, -35,
        -5, -116, 75, 120, -3, 40, 104, -7, 24,
    ],
];

/// The arpeggio programs: 0 no offset, 1 the low nibble, 2 the high nibble.
const ARP: [&[u8]; 8] = [
    &[0, 2],
    &[0, 2, 0, 1],
    &[0, 2, 1],
    &[0, 2, 1, 2],
    &[0, 0, 2, 2, 1, 1],
    &[0, 0, 2, 2, 1, 1, 2, 2],
    &[0, 0, 0, 2, 2, 2, 1, 1, 1],
    &[0, 0, 0, 2, 2, 2, 1, 1, 1, 2, 2, 2],
];

/// Slot k plays Paula channel CHANNEL[k].
const CHANNEL: [usize; 4] = [0, 1, 3, 2];

/// Recognition: the text, then the word 0x0100 at 0x50C (this format version).
fn detect(d: &[u8]) -> bool {
    d.len() >= 0x60C && d[..12] == *MAGIC && be16(d, 0x50C) == 0x0100
}

/// The companion names to try: `SMP.` for a leading `OSP.` (any case), else the
/// name less its last character, then `SMP.set`.
fn companion(file: &str) -> Vec<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let first = match file.get(..4) {
        Some(p) if p.eq_ignore_ascii_case("osp.") => format!("SMP.{}", &file[4..]),
        _ => {
            let mut s = file.to_string();
            s.pop();
            s
        }
    };
    vec![first, "SMP.set".to_string()]
}

/// The number of subsongs as the host is told it.
fn count_subsongs(d: &[u8]) -> u16 {
    let pos = |i: usize| be32(d, 0x0C + 4 * i);
    if be16(d, 0x0C) == 0x2000 && d.get(0x0E) == Some(&0xFE) {
        return 7;
    }
    let mut a = 0u16;
    for i in 0..64 {
        let p = pos(i);
        if p == 0 {
            break;
        }
        if p & 0xFFFF == 0x00F0 {
            a += 1;
        }
    }
    let mut b = 0u16;
    for i in 0..64 {
        let p = pos(i);
        if p == 0 {
            if i + 1 >= 64 || pos(i + 1) == 0 {
                break;
            }
            continue;
        }
        if matches!(p >> 8 & 0xFF, 0xFE | 0xFC) {
            b += 1;
        }
    }
    a.max(b).max(1)
}

/// The start position of subsong `n`: the n-th position whose mask has any of bits
/// 4-7, else position 0.
fn subsong_start(d: &[u8], n: u16) -> u8 {
    (0..64usize)
        .filter(|&i| d.get(0x0C + 4 * i + 3).is_some_and(|m| m & 0xF0 != 0))
        .nth(n as usize)
        .unwrap_or(0) as u8
}

#[derive(Clone, Copy, Default)]
struct Modu {
    dly: u16,
    spd: u16,
    pha: u16,
    /// The wave number; none before the first note.
    wave: Option<u8>,
}

#[derive(Clone, Copy)]
struct Voice {
    key: u8,
    inst: u8,
    arp: u8,
    prm: u8,
    base: u16,
    astep: u8,
    per: u16,
    outper: u16,
    st: u8,
    ph: u8,
    cnt16: u16,
    lvl: u16,
    out: u16,
    vel: u16,
    port: u16,
    pat: usize,
    m: [Modu; 3],
}

impl Default for Voice {
    fn default() -> Voice {
        Voice {
            key: 0xFF,
            inst: 0,
            arp: 0,
            prm: 0,
            base: 0,
            astep: 0,
            per: 0,
            outper: 0,
            st: 0,
            ph: 4,
            cnt16: 0,
            lvl: 0,
            out: 0,
            vel: 0,
            port: 0,
            pat: 0,
            m: Default::default(),
        }
    }
}

pub struct SynthPack {
    d: Vec<u8>,
    /// The instrument file, mirrored in guest memory at `base`.
    smp: Vec<u8>,
    base: u32,
    title: String,
    subsongs: u16,
    sub: u16,
    pos: u8,
    row: u16,
    rep: u8,
    speed: u16,
    cnt: u16,
    mark: i16,
    run: bool,
    nextdma: u16,
    v: [Voice; 4],
    songend: bool,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let smp = companion(&file).iter().find_map(|n| crate::deli::read_companion(m, n).1.ok())?;
    if smp.len() < 12 || smp[..12] != *MAGIC {
        return None;
    }
    let base = m.alloc((smp.len() as u32 + 0x400).max(4));
    if base == 0 {
        return None;
    }
    m.bus.write_bytes(base, &smp);
    let mut r = SynthPack {
        d: d.to_vec(),
        smp,
        base,
        title: file,
        subsongs: count_subsongs(d),
        sub: 0,
        pos: 0,
        row: 0,
        rep: 0,
        speed: 6,
        cnt: 1,
        mark: -1,
        run: true,
        nextdma: 0,
        v: Default::default(),
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    // the audio filter off
    let x = hw.bus.read_byte(0xBFE001);
    hw.bus.write_byte(0xBFE001, x | 2);
    r.start(0, &mut hw);
    Some(Box::new(r))
}

impl SynthPack {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn sb(&self, o: usize) -> u8 {
        self.smp.get(o).copied().unwrap_or(0)
    }

    fn sw(&self, o: usize) -> u16 {
        be16(&self.smp, o)
    }

    /// The offset of instrument `i` in the companion.
    fn ins(&self, i: u8) -> usize {
        be32(&self.smp, 0x0C + 4 * i as usize) as usize
    }

    fn period(&self, note: u16, inst: u8) -> u16 {
        let ft = self.sb(self.ins(inst) + 1) as i8 as i16 as u16;
        PERIODS.get(note as usize).copied().unwrap_or(0).wrapping_sub(ft)
    }

    /// The song from the start position of subsong `n`, with the start-up writes.
    fn start(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.pos = subsong_start(&self.d, n);
        self.row = 0;
        self.rep = 0;
        self.speed = 6;
        self.cnt = 1;
        self.mark = -1;
        self.run = true;
        self.nextdma = 0;
        self.v = Default::default();
        self.songend = false;
        for ch in CHANNEL {
            hw.len(ch, 1);
            hw.vol(ch, 0);
        }
        hw.dmacon(0x800F);
    }

    /// A modulator's wave byte (unsigned), or None for no output.
    fn modulate(&mut self, k: usize, j: usize, a: usize) -> Option<u8> {
        let o = a + [0x15, 0x1A, 0x1F][j];
        let (shift, flag) = (self.sb(o + 2), self.sb(o + 4));
        let v = &mut self.v[k];
        let m = &mut v.m[j];
        if flag != 0 && v.key == 0 {
            return None;
        }
        if m.dly >> 8 != 0 {
            m.dly = m.dly.wrapping_sub(0x40);
            return None;
        }
        m.pha = m.pha.wrapping_add(m.spd);
        if shift == 8 {
            return None;
        }
        let i = (m.pha >> 8 & 0x7F) as usize;
        Some(m.wave.and_then(|w| WAVES.get(w as usize)).map_or(0, |w| w[i] as u8))
    }

    fn adsr(&mut self, k: usize, a: usize) {
        let e: [u8; 5] = std::array::from_fn(|i| self.sb(a + 0x10 + i));
        let (al, asp, dsp, sl, rsp) = (e[0], e[1], e[2], e[3], e[4]);
        let v = &mut self.v[k];
        let low = |l: u16| l as u8 as i8;
        match v.ph {
            0 => {
                v.cnt16 = v.cnt16.wrapping_sub(2);
                if (v.cnt16 as i16) >= 0 {
                    return;
                }
                v.cnt16 = asp as u16;
                if asp != 0 && low(v.lvl) < al as i8 {
                    v.lvl = v.lvl.wrapping_add(8);
                } else {
                    v.ph = 1;
                    v.cnt16 = dsp as u16;
                    v.lvl = al as u16;
                }
            }
            1 => {
                v.cnt16 = v.cnt16.wrapping_sub(2);
                if (v.cnt16 as i16) >= 0 {
                    return;
                }
                v.cnt16 = dsp as u16;
                if dsp != 0 && low(v.lvl) > sl as i8 {
                    v.lvl = v.lvl.wrapping_sub(8);
                } else {
                    v.ph = 2;
                    v.lvl = sl as u16;
                }
            }
            2 => {
                if v.key == 0xFF {
                    v.ph = 3;
                    v.cnt16 = rsp as u16;
                }
            }
            3 => {
                v.cnt16 = v.cnt16.wrapping_sub(2);
                if (v.cnt16 as i16) >= 0 {
                    return;
                }
                v.cnt16 = rsp as u16;
                if rsp != 0 && (v.lvl as i16) > 0 {
                    v.lvl = v.lvl.wrapping_sub(8);
                } else {
                    v.ph = 4;
                    v.lvl = 0;
                }
            }
            _ => {}
        }
    }

    /// 6.1: the envelopes and modulators, then the registers of every voice.
    fn update(&mut self, hw: &mut Hw) {
        // the original's time to these writes (Paula must fetch a started sample's
        // start before the loop part is queued)
        hw.wait(180);
        if self.nextdma != 0 {
            hw.dmacon(self.nextdma | 0x8000);
        }
        hw.wait(450);
        let mut acc = 0u16;
        for k in 0..4 {
            if k != 0 {
                hw.wait(520);
            }
            let ch = CHANNEL[k];
            let a = self.ins(self.v[k].inst);
            self.adsr(k, a);
            // tremolo
            let w = self.modulate(k, 0, a);
            let shift = (self.sb(a + 0x17) & 63) as u32;
            let v = &mut self.v[k];
            v.out = match w {
                None => v.lvl,
                Some(w) => {
                    let d = shr(((w ^ 0x80) >> 1) as u32, shift) as u16;
                    if v.lvl == 0 {
                        0
                    } else {
                        let o = v.lvl.wrapping_sub(d);
                        if (o as i16) < 0 { 0 } else { o }
                    }
                }
            };
            // vibrato
            let w = self.modulate(k, 1, a);
            let shift = (self.sb(a + 0x1C) & 63) as u32;
            let d = w.map_or(0, |w| ((w as i8 as i32) >> shift.min(31)) as u16);
            let v = &mut self.v[k];
            v.outper = v.per.wrapping_add(d);
            // pulse width
            if (self.sw(a + 2) as i16) <= 0x80 {
                let w = self.modulate(k, 2, a);
                let shift = (self.sb(a + 0x21) & 63) as u32;
                let mut d = w.map_or(0, |w| shr(((!(w ^ 0x80)) as u32 & 0xFF) >> 1, shift));
                let mut b = a + 0x30;
                if d & 1 != 0 {
                    d -= 1;
                    b = a + 0x130;
                }
                hw.loc(ch, self.base.wrapping_add(b as u32).wrapping_add(d));
            }
            // portamento
            let (hstep, target, rstep) = (self.sw(a + 0x0A), self.sw(a + 0x0C), self.sw(a + 0x0E));
            let v = &mut self.v[k];
            if v.key == 0 {
                if v.port != 0 {
                    v.port = if (v.port as i16) > 0 { v.port.wrapping_sub(hstep) } else { v.port.wrapping_add(hstep) };
                }
            } else if v.port != target {
                v.port = if (v.port.wrapping_sub(target) as i16) > 0 {
                    v.port.wrapping_sub(rstep)
                } else {
                    v.port.wrapping_add(rstep)
                };
            }
            v.outper = v.outper.wrapping_sub(v.port);
            let vol = (v.out.wrapping_mul(v.vel) >> 6) & 0x7F;
            let per = v.outper;
            hw.vol(ch, vol);
            hw.per(ch, per);
            let looped = be32(&self.smp, a + 4) != 0;
            let (ls, le) = (self.sw(a + 4), self.sw(a + 6));
            let v = &mut self.v[k];
            if v.st == 0xFE {
                v.st = 0xFD;
                if !looped {
                    hw.len(ch, 1);
                } else {
                    hw.len(ch, le.wrapping_sub(ls) >> 1);
                    hw.loc(ch, self.base.wrapping_add(a as u32).wrapping_add(0x30u16.wrapping_add(ls) as u32));
                }
            }
            let v = &mut self.v[k];
            if v.st == 0xFF {
                v.st = 0xFE;
                acc |= 1 << ch;
                hw.loc(ch, self.base.wrapping_add(a as u32 + 0x30));
                hw.len(ch, if looped { le } else { self.sw(a + 2) } >> 1);
            }
        }
        hw.dmacon(acc);
        self.nextdma = acc;
    }

    fn arpeggio(&mut self) {
        for k in 0..4 {
            let v = self.v[k];
            let prog = ARP[(v.arp & 7) as usize];
            let mut s = v.astep as usize;
            let mut astep = v.astep.wrapping_add(1);
            if s >= prog.len() {
                astep = 0;
                s = 0;
            }
            self.v[k].astep = astep;
            let off = match prog[s] {
                0 => 0,
                1 => v.prm & 15,
                _ => v.prm >> 4,
            };
            if prog[s] != 0 && off == 0 {
                continue;
            }
            self.v[k].per = self.period(v.base.wrapping_add(off as u16), v.inst);
        }
    }

    fn note_off(&mut self, k: usize) {
        self.v[k].key = 0xFF;
        self.v[k].ph = 2;
    }

    fn row(&mut self) {
        let p = 0x0C + 4 * self.pos as usize;
        let (rows, set, cmd, mask) = (self.rb(p), self.rb(p + 1), self.rb(p + 2), self.rb(p + 3));
        let s = 0x10C + 16 * set as usize;
        self.speed = self.rb(s + 3) as u16;
        for k in 0..4 {
            let e = s + 4 * k;
            let (pn, insoff, tr) = (self.rb(e), self.rb(e + 1), self.rb(e + 2));
            if self.row == 0 {
                self.v[k].pat = 0x50C + be16(&self.d, 0x50C + 2 * pn as usize) as usize;
            }
            let ev = self.v[k].pat;
            let b = |i: usize| self.rb(ev + i);
            let (note, vol, ins, arp, erow, prm) = (b(0), b(1), b(2), b(3), b(4), b(5));
            if erow as u16 == self.row {
                self.v[k].pat += 6;
            }
            if mask & 1 << k == 0 {
                if self.row == 0 {
                    self.note_off(k);
                }
                continue;
            }
            if erow as u16 != self.row {
                continue;
            }
            if vol == 0 {
                self.note_off(k);
                continue;
            }
            let i = (ins.wrapping_sub(1) as i8).wrapping_add(insoff as i8).max(0) as u8;
            let a = self.ins(i);
            let base = (note.wrapping_add(self.sb(a)) as u16).wrapping_add(tr as i8 as i16 as u16);
            let mods: [Modu; 3] = std::array::from_fn(|j| {
                let o = a + [0x15, 0x1A, 0x1F][j];
                Modu { dly: (self.sb(o) as u16) << 8, spd: (self.sb(o + 1) as u16) << 4, pha: 0, wave: Some(self.sb(o + 3)) }
            });
            let per = self.period(base, i);
            let port = self.sw(a + 8);
            let v = &mut self.v[k];
            v.inst = i;
            v.vel = (vol as i8 as i16 as u16).wrapping_mul(64) >> 6;
            v.base = base;
            v.arp = arp;
            v.prm = prm;
            v.per = per;
            v.outper = per;
            v.key = 0;
            v.ph = 0;
            v.cnt16 = 0;
            v.lvl = 0;
            v.out = 0;
            v.m = mods;
            v.port = port;
            v.st = 0xFF;
            v.astep = 0;
        }
        self.row = self.row.wrapping_add(1);
        if rows as u16 > self.row {
            return;
        }
        self.row = 0;
        match cmd {
            0..=0x7F => {
                if cmd > self.rep {
                    self.rep += 1;
                } else {
                    self.rep = 0;
                    self.pos = (self.pos + 1) & 63;
                }
            }
            0xFF => self.songend = true,
            0xFE => {
                self.run = false;
                for k in 0..4 {
                    self.note_off(k);
                }
                self.songend = true;
            }
            0xFD => {
                self.mark = self.pos as i16;
                self.rep = 0;
                self.pos = (self.pos + 1) & 63;
            }
            0xFC => {
                self.rep = 0;
                if self.mark < 0 {
                    self.pos = (self.pos + 1) & 63;
                } else {
                    self.pos = self.mark as u8;
                    self.songend = true;
                }
            }
            _ => {}
        }
    }
}

/// A logical shift right by `n` (0..63): 0 from 32 on.
fn shr(x: u32, n: u32) -> u32 {
    x.checked_shr(n).unwrap_or(0)
}

impl Replayer for SynthPack {
    fn format(&self) -> &'static str {
        "Synth Pack"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.start(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.update(hw);
        if !self.run {
            return;
        }
        self.cnt = self.cnt.wrapping_add(1);
        if self.speed > self.cnt {
            self.arpeggio();
            return;
        }
        self.cnt = 0;
        self.arpeggio();
        self.row();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A song of one position (4 rows, set 0, played once, then stop with FE): slot 0
    /// plays instrument 1 (a looped 64-byte sample) at note 12 on row 0 and a note
    /// off on row 2.
    fn song() -> Vec<u8> {
        let mut d = vec![0u8; 0x60C + 12];
        d[..12].copy_from_slice(MAGIC);
        d[0x0C..0x10].copy_from_slice(&[4, 0, 0xFE, 0x01]);
        // set 0, slot 0: pattern 0, speed 1
        d[0x10C..0x110].copy_from_slice(&[0, 0, 0, 1]);
        d[0x50C..0x50E].copy_from_slice(&0x0100u16.to_be_bytes());
        // pattern 0 at 0x50C + 0x100
        d[0x60C..0x618].copy_from_slice(&[12, 64, 2, 0, 0, 0, 12, 0, 0, 0, 2, 0]);
        d
    }

    fn instruments() -> Vec<u8> {
        let mut s = MAGIC.to_vec();
        s.extend((0..4).flat_map(|i: u32| (0x1C + 0x70 * (i.min(1))).to_be_bytes()));
        // instrument 0 (at 0x1C) and 1 (at 0x8C): 64 bytes, loop 16..48, flat
        // envelope at 64 (attack level 64, speeds 0, sustain 64)
        for a in [0x1C, 0x8C] {
            s.resize(a + 0x70, 0);
            // a length over 0x80: no pulse width modulation
            s[a + 2..a + 4].copy_from_slice(&0x200u16.to_be_bytes());
            s[a + 4..a + 8].copy_from_slice(&[0, 16, 0, 48]);
            s[a + 0x10] = 64;
            s[a + 0x13] = 64;
            s[a + 0x17] = 8;
            s[a + 0x1C] = 8;
            s[a + 0x21] = 8;
        }
        s
    }

    #[test]
    fn recognises_by_content() {
        let d = song();
        assert!(detect(&d));
        let mut e = d.clone();
        e[0x50C] = 0x24;
        assert!(!detect(&e));
        assert!(!detect(&d[..0x600]));
        assert_eq!(companion("OSP.act_1"), ["SMP.act_1", "SMP.set"]);
        assert_eq!(companion("song"), ["son", "SMP.set"]);
        assert_eq!(count_subsongs(&d), 1);
    }

    #[test]
    fn subsong_markers() {
        let mut d = song();
        for (i, p) in [[0, 0, 0, 0xF0], [0x40, 0, 0xFF, 0x0F], [0, 0, 0, 0xF0], [0x40, 1, 0xFF, 0x0F]].iter().enumerate() {
            d[0x0C + 4 * i..0x10 + 4 * i].copy_from_slice(p);
        }
        assert_eq!(count_subsongs(&d), 2);
        assert_eq!(subsong_start(&d, 1), 2);
        assert_eq!(subsong_start(&d, 5), 0);
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note() {
        let dir = Path::new("/osp-test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("smp.set"), instruments());
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&dir.join("osp.test"), song()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Synth Pack");
        let mut out = vec![0f32; 2 * 960 * 12];
        e.render(&mut out);
        assert!(e.songend());
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // start-up: LEN 1 and VOL 0, then DMA on for all four
        assert_eq!(reg(&log, 0xDFF0A4)[0], 1);
        let dma = reg(&log, 0xDFF096);
        assert!(dma.contains(&0x800F));
        // the note: sample start (32 words to the loop end), DMA off, on, then the loop
        assert_eq!(reg(&log, 0xDFF0A4)[1..3], [24, 16]);
        assert!(dma.windows(2).any(|w| w == [1, 0x8001]));
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1].wrapping_sub(lc[0]), 16);
        // the period of note 12, then the volume of the flat envelope
        assert!(reg(&log, 0xDFF0A6).contains(&1712));
        assert!(reg(&log, 0xDFF0A8).contains(&64));
    }
}

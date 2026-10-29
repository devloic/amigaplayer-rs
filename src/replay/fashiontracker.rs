//! Fashion Tracker (`ex.*`), the music driver of a few Amiga games (Mini Golf Plus
//! among them).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Fashion Tracker Eagleplayer, in
//! its direct-write build, and the structure of the corpus files), and this player
//! was written from it alone and checked against those writes.
//!
//! A module is a raw memory image of the game's replay routine with its tables and
//! samples; the code is not run. Its tables are found by reading the first 1000
//! bytes structurally with the 68000 decoder: four instruction shapes (a long move
//! from an absolute address to a displaced address register, a long move from an
//! address register to an absolute address, a multiply by 1024 into d0, a word
//! compare with 1024 against an absolute address) each carry absolute addresses at
//! fixed distances, which, less the image's origin, are file offsets.
//!
//! The engine: 50 Hz, six ticks to a row. Each row step plays the next row of the
//! 64-row pattern (the first row of a pattern is skipped, as the original does);
//! each of the four cells is a period, a sample nibble and an effect. A note
//! switches the channel's DMA off, writes address, length, period and volume,
//! waits nine raster lines, switches DMA on, waits again and, for a one-shot sample,
//! points the channel at a short silent block. Effect 1 is an arpeggio over the
//! 36-entry period table in the other ticks of the row.
//!
//! Decisions where the specification leaves the choice to the port:
//! * The build is the direct-write one (the reference's); the amplifier build is
//!   not modelled.
//! * Recognition is by structure (exactly one of each of the four anchors in the
//!   first 1000 bytes, every table and pattern inside the file); the original's
//!   five fixed compares are not reproduced.
//! * The silent block is eight zero bytes allocated by the port.
//! * The host's start-up writes (each channel's volume 0, address 0x12D2,
//!   length 1 and period 150; then the four volumes 0 again) come in the
//!   first tick.
//! * An arpeggio on a period that is not in the table writes nothing; an index past
//!   the table end is clamped to the last period.
//! * Reads outside the file give 0.
//! * The raster waits are fixed at 2096 colour clocks after the volume write and 2052
//!   after the DMA-on (the reference's, to within a raster line).

use super::m68kdec::{decode, Ea, K};
use super::{be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Fashion Tracker", detect, new };

const PERIODS: [u16; 36] = [
    856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202,
    190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// The delay from a tick's start to its first write, and the raster waits.
const T_ENTRY: u64 = 740;
const T_WAIT1: u64 = 2096;
const T_WAIT2: u64 = 2052;
const T_WRITE: u64 = 12;
/// The host's start-up channel block (a low chip address).
const STARTUP_LC: u32 = 0x12D2;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    origin: u32,
    len_t: usize,
    addr_t: usize,
    rep_t: usize,
    nib_t: usize,
    pat: usize,
    song: usize,
    song_len: usize,
}

fn scan(d: &[u8]) -> Option<Layout> {
    let lim = d.len().min(1000);
    let mut origin = None;
    let (mut tab, mut mul, mut cmp) = (Vec::new(), Vec::new(), Vec::new());
    for at in (0..lim.saturating_sub(1)).step_by(2) {
        let Some(i) = decode(d, at) else { continue };
        if i.size == 4 && i.k == K::Move {
            if origin.is_none() && matches!(i.src, Ea::AbsL { .. }) && matches!(i.dst, Ea::Disp(..)) && at >= 4 {
                origin = Some(be32(d, at - 4));
            }
            if matches!(i.src, Ea::Ind(_)) && matches!(i.dst, Ea::AbsL { .. }) {
                tab.push(at);
            }
        }
        if i.k == K::Mulu && matches!(i.src, Ea::Imm { val: 1024, .. }) && i.dst == Ea::D(0) {
            mul.push(at);
        }
        if i.k == K::Imm(6) && i.size == 2 && matches!(i.src, Ea::Imm { val: 1024, .. }) && matches!(i.dst, Ea::AbsL { .. }) {
            cmp.push(at);
        }
    }
    if tab.len() != 1 || mul.len() != 1 || cmp.len() != 1 {
        return None;
    }
    let origin = origin?;
    let (a, m, c) = (tab[0], mul[0], cmp[0]);
    if a < 30 || a + 12 > d.len() || m + 10 > d.len() || c + 38 > d.len() {
        return None;
    }
    let off = |v: u32| -> Option<usize> {
        let o = v.checked_sub(origin)? as usize;
        (o < d.len()).then_some(o)
    };
    let l = Layout {
        origin,
        len_t: off(be32(d, a + 8))?,
        addr_t: off(be32(d, a - 6))?,
        rep_t: off(be32(d, a + 24))?,
        nib_t: off(be32(d, a - 30))?,
        pat: off(be32(d, m + 6))?,
        song: off(be32(d, c + 12))?,
        song_len: be32(d, c + 34) as usize,
    };
    // the tables lie in order, and the song and its patterns are inside the file
    if l.song_len == 0 || l.song_len > 256 || l.song + l.song_len > d.len() || l.addr_t <= l.len_t || (l.addr_t - l.len_t) % 4 != 0 {
        return None;
    }
    let n = (l.addr_t - l.len_t) / 4;
    if n == 0 || n > 256 || l.len_t + 4 * n > d.len() || l.addr_t + 4 * n > d.len() || l.rep_t + 4 * n > d.len() || l.nib_t + 64 > d.len() {
        return None;
    }
    for p in 0..l.song_len {
        if l.pat + (d[l.song + p] as usize + 1) * 1024 > d.len() {
            return None;
        }
    }
    Some(l)
}

fn detect(d: &[u8]) -> bool {
    scan(d).is_some()
}

struct Ex {
    d: Vec<u8>,
    base: u32,
    silent: u32,
    l: Layout,
    t: u32,
    fa: usize,
    lfc: usize,
    p: usize,
    s: [usize; 4],
    a: [u16; 4],
    started: bool,
    songend: bool,
}

impl Ex {
    fn byte(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn word(&self, o: usize) -> u16 {
        (self.byte(o) as u16) << 8 | self.byte(o + 1) as u16
    }
    fn long(&self, o: usize) -> u32 {
        (self.word(o) as u32) << 16 | self.word(o + 2) as u32
    }

    fn startup(&mut self, hw: &mut Hw) {
        for c in 0..4 {
            hw.vol(c, 0);
            hw.wait(T_WRITE);
            hw.loc(c, STARTUP_LC);
            hw.len(c, 1);
            hw.per(c, 150);
            hw.wait(T_WRITE);
        }
        // the host's volume routine, once more, when the interrupt is enabled
        for c in 0..4 {
            hw.vol(c, 0);
            hw.wait(T_WRITE);
        }
    }

    fn row(&mut self, hw: &mut Hw) {
        if self.fa == 1024 {
            let old = self.lfc;
            self.lfc += 1;
            if self.lfc == self.l.song_len {
                self.lfc = 0;
                self.songend = true;
            }
            let pat = self.byte(self.l.song + old) as usize;
            self.p = self.l.pat + pat * 1024;
            self.fa = 0;
        }
        self.p += 16;
        self.fa += 16;
        self.t = 0;
        hw.wait(T_ENTRY);
        for c in 0..4 {
            let cell = self.p + 4 * c;
            let period = self.word(cell);
            if period == 0 {
                continue;
            }
            let nib = (self.byte(cell + 2) >> 4) as usize;
            let off = if nib == 0 {
                self.s[c]
            } else {
                self.s[c] = nib * 4;
                nib * 4
            };
            let si = self.word(self.l.nib_t + off) as usize;
            let v = self.word(self.l.nib_t + off + 2) & 127;
            self.a[c] = period;
            let looped = self.byte(self.l.rep_t + 4 * si + 3) == 1;
            let len = self.word(self.l.len_t + 4 * si + 2);
            let addr = self.base.wrapping_add(self.long(self.l.addr_t + 4 * si).wrapping_sub(self.l.origin));
            hw.dmacon(1 << c);
            hw.wait(24);
            hw.loc(c, addr);
            hw.wait(14);
            hw.len(c, len);
            hw.wait(10);
            hw.per(c, period);
            hw.wait(200);
            hw.vol(c, v);
            hw.wait(T_WAIT1);
            hw.dmacon(0x8000 | 1 << c);
            hw.wait(T_WAIT2);
            if !looped {
                hw.loc(c, self.silent);
                hw.wait(T_WRITE);
                hw.len(c, 4);
            }
        }
    }

    fn effects(&mut self, hw: &mut Hw) {
        for c in 0..4 {
            let cell = self.p + 4 * c;
            let period = self.word(cell);
            let base = if period != 0 { period } else { self.a[c] };
            if self.byte(cell + 2) & 15 != 1 {
                continue;
            }
            let par = self.byte(cell + 3);
            let x = match self.t {
                3 => {
                    hw.per(c, base);
                    continue;
                }
                1 | 4 => par >> 4,
                _ => par & 15,
            } as usize;
            if let Some(i) = PERIODS.iter().position(|&p| p == base) {
                hw.per(c, PERIODS[(i + x).min(35)]);
            }
        }
    }
}

impl Replayer for Ex {
    fn format(&self) -> &'static str {
        "Fashion Tracker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.startup(hw);
        }
        self.t += 1;
        if self.t == 6 {
            self.row(hw);
        } else {
            self.effects(hw);
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = scan(d)?;
    let silent = m.alloc(8);
    Some(Box::new(Ex {
        d: d.to_vec(),
        base,
        silent,
        l,
        t: 5,
        fa: 1024,
        lfc: 0,
        p: 0,
        s: [0; 4],
        a: [0; 4],
        started: false,
        songend: false,
    }))
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


    /// A test-only encoder from instruction descriptions.
    mod enc {
        const ABS_L: u16 = 7 << 3 | 1;
        const IMM: u16 = 7 << 3 | 4;
        fn l(v: u32) -> [u16; 2] {
            [(v >> 16) as u16, v as u16]
        }
        /// `move.l abs.l,d16(an)`.
        pub fn move_l_abs_disp(addr: u32, an: u16, disp: u16) -> Vec<u16> {
            [&[2 << 12 | an << 9 | 5 << 6 | ABS_L][..], &l(addr), &[disp]].concat()
        }
        /// `move.l (an),abs.l`.
        pub fn move_l_ind_abs(an: u16, addr: u32) -> Vec<u16> {
            [&[2 << 12 | 1 << 9 | 7 << 6 | 2 << 3 | an][..], &l(addr)].concat()
        }
        /// `movea.l #imm,an`.
        pub fn movea_l_imm(an: u16, imm: u32) -> Vec<u16> {
            [&[2 << 12 | an << 9 | 1 << 6 | IMM][..], &l(imm)].concat()
        }
        /// `mulu.w #imm,dn`.
        pub fn mulu_w_imm(dn: u16, imm: u16) -> Vec<u16> {
            vec![0xC << 12 | dn << 9 | 3 << 6 | IMM, imm]
        }
        /// `cmpi.w #imm,abs.l`.
        pub fn cmpi_w_abs(imm: u16, addr: u32) -> Vec<u16> {
            [&[6 << 9 | 1 << 6 | ABS_L, imm][..], &l(addr)].concat()
        }
        pub fn nop() -> Vec<u16> {
            vec![0x4E << 8 | 0x71]
        }
    }

    fn put_insn(d: &mut [u8], o: usize, ws: Vec<u16>) {
        for (k, w) in ws.iter().enumerate() {
            put16(d, o + 2 * k, *w);
        }
    }

    #[test]
    fn encoder_round_trip() {
        let mut d = vec![0u8; 32];
        let mut ins = |ws: Vec<u16>| {
            put_insn(&mut d, 0, ws);
            decode(&d, 0).unwrap()
        };
        let i = ins(enc::move_l_abs_disp(0x1234, 1, 8));
        assert_eq!((i.k, i.size, i.len, i.src, i.dst), (K::Move, 4, 8, Ea::AbsL { addr: 0x1234, pos: 2 }, Ea::Disp(1, 8)));
        let i = ins(enc::move_l_ind_abs(1, 0x2000));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 4, Ea::Ind(1), Ea::AbsL { addr: 0x2000, pos: 2 }));
        let i = ins(enc::movea_l_imm(1, 0x3000));
        assert_eq!((i.k, i.dst, i.src), (K::Movea, Ea::A(1), Ea::Imm { val: 0x3000, pos: 2 }));
        let i = ins(enc::mulu_w_imm(0, 1024));
        assert_eq!((i.k, i.src, i.dst), (K::Mulu, Ea::Imm { val: 1024, pos: 2 }, Ea::D(0)));
        let i = ins(enc::cmpi_w_abs(1024, 0x4000));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Imm(6), 2, Ea::Imm { val: 1024, pos: 2 }, Ea::AbsL { addr: 0x4000, pos: 4 }));
        assert_eq!(ins(enc::nop()).k, K::Nop);
    }

    const ORIGIN: u32 = 0x40000;
    const LEN_T: usize = 0x200;
    const ADDR_T: usize = 0x208;
    const REP_T: usize = 0x210;
    const NIB_T: usize = 0x220;
    const SONG: usize = 0x260;
    const PAT: usize = 0x400;
    const SMP: usize = 0x1400;

    /// A synthetic image: the four anchor shapes at the start, two samples (0 loops,
    /// 1 is one-shot), two patterns, a song of two positions.
    fn module() -> Vec<u8> {
        let mut d = vec![0u8; 0x1500];
        let a = |o: usize| ORIGIN + o as u32;
        // the origin, then `move.l abs,8(a1)` at 4
        put32(&mut d, 0, ORIGIN);
        put_insn(&mut d, 4, enc::move_l_abs_disp(a(0x300), 1, 8));
        // the table anchor at 0x60: A-30 nibble map, A-6 address, A+8 length, A+24 repeat
        put32(&mut d, 0x60 - 30, a(NIB_T));
        put32(&mut d, 0x60 - 6, a(ADDR_T));
        put_insn(&mut d, 0x60, enc::move_l_ind_abs(1, a(0x310)));
        put_insn(&mut d, 0x66, enc::movea_l_imm(1, a(LEN_T)));
        put32(&mut d, 0x60 + 24, a(REP_T));
        // mulu #1024,d0 at 0x90, pattern base at A+6
        put_insn(&mut d, 0x90, enc::mulu_w_imm(0, 1024));
        put32(&mut d, 0x96, a(PAT));
        // cmpi.w #1024,abs at 0xB0: song table at A+12, song length at A+34
        put_insn(&mut d, 0xB0, enc::cmpi_w_abs(1024, a(0x320)));
        put32(&mut d, 0xB0 + 12, a(SONG));
        put32(&mut d, 0xB0 + 34, 2);
        // samples
        put32(&mut d, LEN_T, 8);
        put32(&mut d, LEN_T + 4, 4);
        put32(&mut d, ADDR_T, a(SMP));
        put32(&mut d, ADDR_T + 4, a(SMP + 16));
        d[REP_T + 3] = 1;
        // nibble 1: sample 0, volume 40; nibble 2: sample 1, volume 200 (7 bits: 72)
        put16(&mut d, NIB_T + 4, 0);
        put16(&mut d, NIB_T + 6, 40);
        put16(&mut d, NIB_T + 8, 1);
        put16(&mut d, NIB_T + 10, 200);
        d[SONG] = 0;
        d[SONG + 1] = 1;
        let cell = |d: &mut Vec<u8>, p: usize, row: usize, c: usize, per: u16, b2: u8, b3: u8| {
            let o = PAT + p * 1024 + row * 16 + c * 4;
            put16(d, o, per);
            d[o + 2] = b2;
            d[o + 3] = b3;
        };
        cell(&mut d, 0, 0, 0, 856, 0x10, 0);
        cell(&mut d, 0, 1, 0, 428, 0x10, 0);
        cell(&mut d, 0, 1, 1, 339, 0x21, 0x37);
        cell(&mut d, 0, 2, 0, 360, 0x00, 0);
        cell(&mut d, 1, 1, 3, 226, 0x20, 0);
        d
    }

    fn run(d: Vec<u8>, ticks: usize) -> Vec<(u32, u16)> {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/bye-test/ex.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Fashion Tracker");
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect()
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn recognition() {
        let d = module();
        assert!(detect(&d));
        assert_eq!(scan(&d).unwrap().song_len, 2);
        let mut e = d.clone();
        put_insn(&mut e, 0x90, enc::nop());
        assert!(!detect(&e));
        let mut e = d.clone();
        put32(&mut e, 0xB0 + 34, 0);
        assert!(!detect(&e));
        let mut e = d.clone();
        e[SONG] = 200;
        assert!(!detect(&e));
        assert!(!detect(&d[..0x100]));
        assert!(!detect(&[0u8; 2000]));
    }

    #[test]
    fn notes_and_effects() {
        let log = run(module(), 40);
        // row 0 is skipped: channel 0's first period is row 1's 428, then row 2's 360
        let p0 = reg(&log, 0xDFF0A6);
        assert_eq!(&p0[..3], &[150, 428, 360]);
        // channel 1: volume masked to 7 bits; one-shot, so a silent block follows
        assert_eq!(reg(&log, 0xDFF0B8), vec![0, 0, 72]);
        assert_eq!(reg(&log, 0xDFF0B4), vec![1, 4, 4]);
        // channel 0 loops: no silent block after it
        assert_eq!(reg(&log, 0xDFF0A4), vec![1, 8, 8]);
        // arpeggio 0x37 on 339 (index 16): ticks 1,4 +3 (285), 2,5 +7 (226), 3 the base
        let p1 = reg(&log, 0xDFF0B6);
        assert_eq!(&p1[..7], &[150, 339, 285, 226, 339, 285, 226]);
        // the DMA switches: off, on in channel order
        let dma: Vec<u16> = reg(&log, 0xDFF096).into_iter().filter(|v| matches!(*v, 1 | 2 | 0x8001 | 0x8002)).collect();
        assert_eq!(&dma[..4], &[1, 0x8001, 2, 0x8002]);
    }

    #[test]
    fn song_end() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.play_data(Path::new("/bye-test/ex.test"), module()).unwrap();
        let mut buf = vec![0f32; 2 * 960];
        let mut end = None;
        for t in 0..400 {
            e.render(&mut buf);
            if end.is_none() && e.songend() {
                end = Some(t);
            }
        }
        assert!(end.is_some());
    }
}

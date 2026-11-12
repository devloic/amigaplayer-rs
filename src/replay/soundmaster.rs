//! Sound Master (`sm.*`), Michiel Soede's music system (1990-94), in the songs of
//! Doofus and Future Shock.
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour (the Paula register writes of UADE's Sound Master player and the
//! structure of the corpus files), and this player was written from it alone and
//! checked against those writes.
//!
//! A module is a raw memory image: the replay routine compiled together with one
//! song and its samples. The routine is not run. Its data is located structurally
//! (the base of its variables, `G`, from the init routine's first PC-relative LEA;
//! the period table from the LEA after the note masking), and the three builds of
//! the routine in the corpus are written from their behaviour: four voices, one
//! Paula channel each, a 50 Hz tick, a step table of pattern-list ranges with a
//! transposition and a fade byte, patterns of (note, parameter) rows, 64 instruments
//! with an attack/release envelope, a vibrato and an arpeggio of notes and samples,
//! portamento, ties and master-volume fades. Build A (Doofus) has its tables behind
//! an offset table; builds B (Future Shock) and C (an unused Doofus song) at fixed
//! places, C with a sample sweep.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Sound Master", detect, new };

#[derive(Clone, Copy, PartialEq, Debug)]
enum Build {
    A,
    B,
    C,
    /// Version 3: build A's tables, a tick of build B/C's kind.
    D,
}

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

// ---------------------------------------------------------------------------------
// Recognition and structural location

/// What the structural scan of a module finds.
#[derive(Clone, Copy, Debug)]
struct Lay {
    build: Build,
    /// The routine's variable base.
    g: usize,
    /// The period table: 64 words (B) or 46 (A, C).
    periods: usize,
    /// Build A family: the shift of the header fields (0 in A) and the offset of the step
    /// table from `g`.
    ho: isize,
    tb: usize,
}

/// The first word `w` at an even offset in `from..to`.
fn find(d: &[u8], from: usize, to: usize, pat: &[u16]) -> Option<usize> {
    let to = to.min(d.len().saturating_sub(2 * pat.len()));
    (from..to).step_by(2).find(|&o| pat.iter().enumerate().all(|(i, &w)| be16(d, o + 2 * i) == w))
}

/// The target of the PC-relative displacement word at `o`.
fn pcrel(d: &[u8], o: usize) -> Option<usize> {
    usize::try_from(o as i64 + be16(d, o) as i16 as i64).ok()
}

fn scan(d: &[u8]) -> Option<Lay> {
    if d.len() < 0x200 || be16(d, 0) != 0x6000 || be16(d, 4) != 0x6000 || be16(d, 8) != 0x6000 {
        return None;
    }
    let (d2, d6) = (be16(d, 2) as i16, be16(d, 6) as i16);
    if d2 <= 0 || d6 <= 0 || d2 % 2 != 0 || d6 % 2 != 0 {
        return None;
    }
    // the init routine: its LEA d16(PC),A3 within 30 bytes, then the RTS that ends
    // it, after the audio filter's address operand
    let init = 2 + d2 as usize;
    let lea = find(d, init, init + 30, &[0x47FA])?;
    let rts = find(d, lea, d.len(), &[0x4E75])?;
    let mut end = rts + 2;
    if end >= 8 && be32(d, end - 8) == 0x177C_0000 {
        end -= 6;
    }
    if end < 6 || be32(d, end - 6) != 0x00BF_E001 {
        return None;
    }
    // build A and its relatives: the init routine sets up the step table's address (LEA tb(A3),A0)
    // and adds the sample table's offset (ADDA.L yy(A3),A0), the fields of the header being
    // `ho` further than build A's, 0xEE being the sample table's
    let a_fam = (lea..lea + 0x100).step_by(2).find_map(|o| {
        (be16(d, o) == 0x41EB && be16(d, o + 4) == 0xD1EB && be16(d, o + 8) == 0x2748)
            .then(|| (be16(d, o + 2) as usize, be16(d, o + 6) as isize - 0xEE))
    });
    let (mut ho, mut tb) = (0, 0xF4);
    let build = if let Some((t, h)) = a_fam {
        (ho, tb) = (h, t);
        // version 3: a routine with 0x3C-byte voice records
        if init == 0x0C && find(d, 0, 0x200, &[0xDBFC, 0x0000, 0x003C]).is_some() { Build::D } else { Build::A }
    } else if init == 0x0C {
        if find(d, 0, 0x200, &[0xDBFC, 0x0000, 0x0038]).is_some() { Build::C } else { Build::B }
    } else {
        return None;
    };
    let g = pcrel(d, lea + 2)?;
    let periods = match build {
        Build::B => pcrel(d, find(d, 0, d.len(), &[0x0200, 0x003F, 0xE300, 0x41FA])? + 8)?,
        _ => {
            let cmp = find(d, 0, d.len(), &[0x0C00, 0x002E])?;
            pcrel(d, find(d, cmp, d.len(), &[0xE300, 0x41FA])? + 4)?
        }
    };
    let n = if build == Build::B { 64 } else { 46 };
    if g < 0x80 || g + 0x1200 > d.len() || periods + 2 * n > d.len() {
        return None;
    }
    Some(Lay { build, g, periods, ho, tb })
}

fn detect(d: &[u8]) -> bool {
    scan(d).is_some()
}

// ---------------------------------------------------------------------------------
// The player

#[derive(Clone, Copy, Default)]
struct Voice {
    trk: usize,
    bit: u16,
    lev: u16,
    att: u16,
    peak: u16,
    rel: u8,
    floor: u16,
    cur: u16,
    tgt: u16,
    pst: u16,
    rowp: usize,
    rlen: u16,
    /// Module offset of the loop the voice's registers get.
    rptr: u32,
    note: u8,
    par: u8,
    ins: u8,
    cnote: u8,
    vvol: u8,
    vdelay: u8,
    vdepth: u8,
    vinc: u8,
    vlen: u8,
    vcnt: u8,
    vdir: u8,
    apos: u8,
    aend: u8,
    aloop: u8,
    afix: u8,
    ptrans: u8,
    iofs: u8,
    tune: u8,
    // build C: the sample sweep
    swstep: u16,
    swlen: u8,
    swcnt: u8,
    swmask: u8,
    // build A: the voice's master volume and the sound-effect request (never set by songs)
    master: u8,
    sfxpar: u8,
    sfxnote: u8,
}

struct Sm {
    img: Vec<u8>,
    base: u32,
    title: String,
    build: Build,
    g: usize,
    /// Offset of the fixed-layout tables beyond 0xD0 (C: 0x10 further).
    c: usize,
    /// Build A family: header shift, step table offset.
    ho: isize,
    tb: usize,
    periods: Vec<u16>,
    v: [Voice; 4],
    started: bool,
    songend: bool,
    // sequencer
    tick: u8,
    row: u8,
    pos: u8,
    step: u8,
    fctl: u8,
    fcnt: u8,
    flock: u8,
    dmaon: u16,
    pend: u16,
    pendn: u16,
    pos0: u8,
    pos_n: u8,
    strans: u8,
    master: u8,
    brk: bool,
    speed: u8,
    speed0: u8,
    patlen: u8,
    loopstep: u8,
    endstep: u8,
    /// Build A: the play request word from the host (31: play the song).
    mode: u8,
    /// Build C: the last arpeggio value of the tick (a register of the original).
    lastarp: u8,
}

// Colour clocks between the original's writes, so the DMA off and on writes of a
// note start are as far apart as in the reference.
const WAIT_LOOP: u64 = 60;
const WAIT_ROW: u64 = 160;
const WAIT_NOTE: u64 = 900;
const WAIT_FX: u64 = 280;

impl Sm {
    fn r8(&self, o: usize) -> u8 {
        rb(&self.img, o)
    }

    fn r16(&self, o: usize) -> u16 {
        be16(&self.img, o)
    }

    fn r32(&self, o: usize) -> u32 {
        be32(&self.img, o)
    }

    /// Build A's data layout (build D has it too).
    fn alay(&self) -> bool {
        matches!(self.build, Build::A | Build::D)
    }

    /// A field of build A's header, `o` from `g` in build A itself.
    fn hf(&self, o: usize) -> usize {
        (self.g + o).wrapping_add_signed(self.ho)
    }

    /// Build A: the table at the offset long `o` of the header, from `T`.
    fn t(&self) -> usize {
        self.g + self.tb
    }

    fn atab(&self, o: usize) -> usize {
        self.t().wrapping_add(self.r32(self.hf(o)) as usize)
    }

    fn silent(&self) -> usize {
        match self.build {
            Build::A | Build::D => self.hf(0xF2),
            _ => self.g + 0xE6 + self.c,
        }
    }

    /// Start, length (words), loop start (bytes) and loop length (words) of sample `s`.
    fn sample(&self, s: u8) -> (u32, u16, u16, u16) {
        let s = s as usize;
        if self.alay() {
            let st = self.atab(0xEE);
            let x = (s << 3) & 0xFF;
            let rec = st.wrapping_add(x + (x >> 2));
            let start = (st as u32).wrapping_add(self.r32(rec)).wrapping_add(self.r32(self.hf(0xEA)));
            return (start, self.r16(rec + 4), self.r16(rec + 6), self.r16(rec + 8));
        }
        let soff = self.r32(self.g + 0xE2 + self.c) as usize;
        let b = self.g + self.c;
        let data = (b + 0x11A8).wrapping_add(soff) as u32;
        let start = data.wrapping_add(self.r32((b + 0x1068).wrapping_add(soff) + ((4 * s) & 0xFF)));
        let w = (2 * s) & 0xFF;
        (
            start,
            self.r16((b + 0x10E8).wrapping_add(soff) + w),
            self.r16((b + 0x1128).wrapping_add(soff) + w),
            self.r16((b + 0x1168).wrapping_add(soff) + w),
        )
    }

    fn instrument(&self, i: u8) -> usize {
        let i = (i & 0x3F) as usize;
        match self.build {
            Build::A | Build::D => self.atab(0xDA).wrapping_add(16 * i),
            Build::B => self.g + 0xAE8 + 14 * i,
            Build::C => self.g + 0xAF8 + 16 * i,
        }
    }

    fn arpnote(&self, k: u8) -> u8 {
        match self.build {
            Build::A | Build::D => self.r8(self.atab(0xDE).wrapping_add(k as usize)),
            _ => self.r8(self.g + 0xE68 + self.c + k as usize),
        }
    }

    fn arpsample(&self, k: u8) -> u8 {
        match self.build {
            Build::A | Build::D => self.r8(self.atab(0xE2).wrapping_add(k as usize)),
            _ => self.r8(self.g + 0xF68 + self.c + k as usize),
        }
    }

    fn table(&self, k: usize) -> u16 {
        self.periods.get(k).copied().unwrap_or(0)
    }

    // ----------------------------------------------------------------- song structure

    fn load_step(&mut self) {
        let f;
        if self.build == Build::A {
            let r = self.t() + 8 * self.step as usize;
            self.pos0 = self.r8(r);
            self.pos_n = self.r8(r + 1);
            self.strans = self.r8(r + 2);
            f = self.r8(r + 3);
            let mut x = f;
            if self.flock == 0 {
                if f & 0x80 == 0 {
                    self.fctl = 0;
                    self.fcnt = 0;
                } else if f & 0x40 != 0 {
                    if f & 0x20 != 0 {
                        x = 0x40;
                    }
                    self.fcnt = f & 0x1F;
                    self.fctl = f & 0x1F;
                } else {
                    if f & 0x20 != 0 {
                        x = 0;
                    }
                    self.fcnt = f & 0x1F;
                    self.fctl = (f & 0x1F) | 0x80;
                }
            }
            if self.flock != 0 {
                x = 0xFF;
            }
            for i in 0..4 {
                let io = self.r8(r + 4 + i);
                self.v[i].iofs = io;
                if io & 0x80 == 0 && x & 0x80 == 0 {
                    self.v[i].master = x;
                }
            }
        } else {
            let st = self.step as usize;
            let (t, g, cc, d) = (self.t(), self.g, self.c, self.build == Build::D);
            let col = move |c: usize| if d { t + 8 * st + c } else { g + 0xE8 + cc + 0x40 * c + st };
            self.pos0 = self.r8(col(0));
            self.pos_n = self.r8(col(1));
            self.strans = self.r8(col(2));
            f = self.r8(col(3));
            for i in 0..4 {
                self.v[i].iofs = self.r8(col(4 + i));
            }
            if self.flock == 0 {
                if f & 0x80 == 0 {
                    self.fctl = 0;
                    self.master = f;
                } else {
                    let mut x = self.master;
                    if f & 0x40 != 0 {
                        if f & 0x20 != 0 {
                            x = 0x40;
                        }
                        self.fcnt = f & 0x1F;
                        self.fctl = f & 0x1F;
                    } else {
                        if f & 0x20 != 0 {
                            x = 0;
                        }
                        self.fcnt = f & 0x1F;
                        self.fctl = (f & 0x1F) | 0x80;
                    }
                    self.master = x;
                }
            }
            self.lastarp = 0xFF;
        }
        self.pos = self.pos0;
    }

    fn fetch_pattern(&mut self, i: usize) {
        let patlen = self.patlen as usize;
        let (pt, rowp) = match self.build {
            Build::A | Build::D => {
                let pl = self.atab(0xD6).wrapping_add(8 * self.pos as usize + self.v[i].trk);
                let n = self.r8(pl) as usize;
                (self.r8(pl.wrapping_add(1)), self.atab(0xE6).wrapping_add(n * patlen))
            }
            _ => {
                let pl = self.g + 0x2E8 + self.c + self.v[i].trk + self.pos as usize;
                let n = self.r8(pl) as usize;
                (self.r8(pl + 0x400), self.g + 0x1068 + self.c + n * patlen)
            }
        };
        self.v[i].ptrans = pt;
        self.v[i].rowp = rowp;
    }

    fn read_row(&mut self, i: usize) {
        let p = self.v[i].rowp.wrapping_add(self.row as usize);
        let (mut note, mut par) = (self.r8(p), self.r8(p.wrapping_add(1)));
        if self.build == Build::A {
            if self.mode & 2 == 0 {
                note = 0;
                par = 0;
            }
        } else if note == 0xFD {
            self.speed = par;
            note = 0;
            par = 0;
        } else if note == 0xFE {
            self.brk = true;
            note = 0;
        }
        if (1..=0x7F).contains(&note) {
            self.dmaon |= self.v[i].bit;
        }
        self.v[i].note = note;
        self.v[i].par = par;
    }

    fn advance(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.brk {
            self.row = self.patlen.wrapping_sub(2);
        }
        if self.tick != self.speed {
            return;
        }
        self.tick = 0;
        self.row = self.row.wrapping_add(2);
        if self.row != self.patlen {
            return;
        }
        self.row = 0;
        if self.pos != self.pos_n {
            self.pos = self.pos.wrapping_add(1);
            return;
        }
        self.step = self.step.wrapping_add(1);
        if self.build == Build::A && self.mode & 2 == 0 {
            self.step = 0;
            self.wrap();
        } else if self.step == self.endstep {
            self.step = self.loopstep;
            self.wrap();
            self.songend = true;
        }
        self.load_step();
    }

    /// The song's restart: fades, speed and master volume reset.
    fn wrap(&mut self) {
        self.fctl = 0;
        self.speed = self.speed0;
        if self.build == Build::A {
            for v in &mut self.v {
                v.master = 0x40;
            }
        } else {
            self.master = 0;
        }
    }

    // ----------------------------------------------------------------- the voices

    /// The note's period (the full form: with the transpositions), setting `cnote`.
    fn period_full(&mut self, i: usize, n: u8) -> u16 {
        let v = self.v[i];
        let m = if v.par & 0x80 != 0 { n } else { n.wrapping_add(v.ptrans).wrapping_add(self.strans) };
        self.v[i].cnote = m;
        if self.build == Build::B {
            return self.table((m.wrapping_add(v.tune) & 0x3F) as usize);
        }
        let mut k = m.wrapping_add(v.tune).wrapping_sub(0x12) & 0x3F;
        if k >= 0x2E {
            k = 0;
            self.v[i].cnote = 0x12u8.wrapping_sub(v.tune);
        }
        self.table(k as usize)
    }

    /// The arpeggio form: `m` as it is.
    fn period_arp(&mut self, i: usize, m: u8) -> u16 {
        let tune = self.v[i].tune;
        if self.build == Build::B {
            return self.table((m.wrapping_add(tune) & 0x3F) as usize);
        }
        let mut k = m.wrapping_add(tune).wrapping_sub(0x12) & 0x3F;
        let mut cn = m;
        if k >= 0x2E {
            k = 0;
            cn = 0x12u8.wrapping_sub(tune);
        }
        if matches!(self.build, Build::C | Build::D) {
            self.v[i].cnote = cn;
        }
        self.table(k as usize)
    }

    fn centre(v: &mut Voice) {
        if v.vlen & 1 != 0 {
            v.cur = v.cur.wrapping_sub((v.vdepth >> 1) as u16);
        }
    }

    fn volume_command(v: &mut Voice, n: u8) {
        if n & 0x40 == 0 && v.par & 0x40 != 0 {
            v.lev = ((v.par & 0x3F) as u16) << 4;
            v.att = 0xFFFF;
        }
    }

    fn note_start(&mut self, i: usize, hw: &mut Hw) {
        let n = self.v[i].note;
        if n == 0 || n == 0xFF {
            Self::volume_command(&mut self.v[i], n);
            return;
        }
        self.v[i].pst = 0;
        if n & 0x80 != 0 {
            let v = &mut self.v[i];
            v.vcnt = v.vlen >> 1;
            v.vdir = 1;
        } else {
            if self.build != Build::A {
                self.v[i].vdir = 0;
            }
            let par = self.v[i].par;
            let ins = if n & 0x40 != 0 || par & 0x40 != 0 { self.v[i].ins } else { par };
            self.v[i].ins = ins;
            let iofs = if self.build == Build::A && self.v[i].sfxpar != 0 { 0 } else { self.v[i].iofs };
            let r = self.instrument(ins.wrapping_add(iofs));
            let rec: [u8; 16] = std::array::from_fn(|k| self.r8(r + k));
            let s = if rec[10] != 0 { self.arpsample(rec[9]) } else { rec[0] };
            let (start, len, ls, ll) = self.sample(s);
            hw.len(i, len);
            hw.loc(i, self.base.wrapping_add(start));
            let rptr = start.wrapping_add(ls as u32);
            self.v[i].rlen = ll;
            self.v[i].rptr = rptr;
            let in_loop = match self.build {
                Build::A => rec[0] & 0x80 != 0,
                Build::B => false,
                Build::C | Build::D => s & 0x80 != 0,
            };
            if in_loop {
                hw.len(i, ll);
                hw.loc(i, self.base.wrapping_add(rptr));
            }
            let v = &mut self.v[i];
            let peak = if n & 0x40 == 0 && par & 0x40 != 0 { par & 0x3F } else { rec[2] };
            v.peak = (peak as u16) << 4;
            v.floor = (rec[4] as u16) << 4;
            v.lev = 0;
            v.att = rec[1] as u16;
            v.rel = rec[3];
            if self.build == Build::A {
                v.vdir = 0;
            }
            if self.build == Build::B {
                v.vvol = rec[8];
            }
            v.vdelay = rec[5];
            v.vdepth = rec[6];
            v.vlen = rec[7] & 15;
            v.vcnt = v.vlen >> 1;
            v.vinc = rec[7] >> 4;
            if v.vcnt != 0 {
                v.vdir = 1;
            }
            v.apos = rec[9];
            v.aend = rec[10];
            v.aloop = rec[11];
            v.afix = rec[12];
            v.tune = rec[13];
            if matches!(self.build, Build::C | Build::D) {
                v.swlen = rec[14];
                v.swcnt = rec[14];
                v.swmask = rec[15] & 15;
                v.swstep = ((rec[15] >> 4) as u16).wrapping_neg();
            }
        }
        // the pitch
        let per = self.period_full(i, n);
        let v = &mut self.v[i];
        if n & 0x40 != 0 {
            v.tgt = per << 2;
            let sp = (v.par & 0x7F) as u16;
            v.pst = if v.tgt.wrapping_sub(v.cur) & 0x8000 == 0 { sp } else { sp.wrapping_neg() };
        } else {
            hw.per(i, per);
            v.cur = per << 2;
        }
        if v.vdelay == 0 {
            Self::centre(v);
        }
        if n & 0x80 != 0 && self.build == Build::A {
            Self::volume_command(v, n);
        }
        if self.build != Build::A {
            self.dmaon |= v.bit;
        }
    }

    fn effects(&mut self, i: usize, hw: &mut Hw) {
        let mut skip = false;
        if self.v[i].aend != 0 {
            let t = self.arpnote(self.v[i].apos);
            let mut apos = self.v[i].apos.wrapping_add(1);
            if apos == self.v[i].aend {
                apos = self.v[i].aloop;
            }
            self.v[i].apos = apos;
            let s = self.arpsample(apos);
            let (start, len, ls, ll) = self.sample(s);
            if s & 0x80 != 0 {
                self.pendn |= self.v[i].bit;
                self.v[i].rlen = len;
                self.v[i].rptr = start;
            } else {
                let same = s == self.arpsample(apos.wrapping_sub(1));
                let keep = match self.build {
                    Build::B => false,
                    Build::C | Build::D => same,
                    Build::A => same || s & 0x20 != 0,
                };
                if !keep {
                    self.v[i].rlen = ll;
                    self.v[i].rptr = start.wrapping_add(ls as u32);
                }
            }
            self.lastarp = t;
            if t != 0x80 && !(matches!(self.build, Build::C | Build::D) && t == 0x82) {
                let m = if t == 0x81 {
                    self.v[i].afix
                } else {
                    self.v[i].cnote = self.v[i].cnote.wrapping_add(t);
                    self.v[i].cnote
                };
                let per = self.period_arp(i, m);
                hw.per(i, per);
                skip = true;
            }
        }
        let build = self.build;
        let v = &mut self.v[i];
        if !skip {
            if v.vdir != 0 {
                if v.vdelay != 0 {
                    v.vdelay -= 1;
                    if v.vdelay == 0 {
                        Self::centre(v);
                    }
                } else {
                    let d = if build == Build::B && v.lev != 0 { v.vvol as u16 } else { 0 };
                    if v.vdir & 0x80 == 0 {
                        if v.vlen >> 1 == v.vcnt && (build == Build::B || v.vdepth <= 0x40) {
                            v.vdepth = v.vdepth.wrapping_add(v.vinc);
                        }
                        v.cur = v.cur.wrapping_sub(v.vdepth as u16);
                        v.lev = v.lev.wrapping_sub(d);
                    } else {
                        v.cur = v.cur.wrapping_add(v.vdepth as u16);
                        v.lev = v.lev.wrapping_add(d);
                    }
                    v.vcnt = v.vcnt.wrapping_sub(1);
                    if v.vcnt == 0 {
                        v.vdir = v.vdir.wrapping_neg();
                        v.vcnt = v.vlen;
                    }
                }
            }
            hw.per(i, v.cur >> 2);
            if v.pst != 0 {
                let x = v.cur.wrapping_add(v.pst);
                let below = x.wrapping_sub(v.tgt) & 0x8000 != 0;
                if (v.pst & 0x8000 == 0) == below {
                    v.cur = x;
                    hw.per(i, x >> 2);
                } else {
                    v.pst = 0;
                    v.vcnt = v.vlen >> 1;
                    v.vdir = 1;
                    v.cur = v.tgt;
                    hw.per(i, v.tgt >> 2);
                    Self::centre(v);
                }
            }
        }
        if matches!(build, Build::C | Build::D) && v.rlen != 0 && v.swlen != 0 && self.lastarp != 0x82 {
            if v.swcnt & v.swmask == 0 {
                v.rptr = v.rptr.wrapping_add(v.swstep as i16 as i32 as u32);
            }
            v.swcnt = v.swcnt.wrapping_sub(1);
            if v.swcnt == 0 {
                v.swstep = v.swstep.wrapping_neg();
                v.swcnt = v.swlen;
            }
        }

        // the envelope and the volume
        if v.note == 0 {
            v.att = 0xFFFF;
            if v.lev > v.floor {
                v.lev = v.lev.wrapping_sub(v.rel as u16);
                if v.lev & 0x8000 != 0 || v.lev <= v.floor {
                    v.lev = v.floor;
                }
            }
        } else if v.att == 0 {
            v.lev = v.peak;
            v.att = 0xFFFF;
        } else if v.att & 0x8000 == 0 {
            v.lev = v.lev.wrapping_add(v.att);
            if v.lev >= v.peak {
                v.lev = v.peak;
                v.att = 0xFFFF;
            }
        }
        let mut vol = (v.lev >> 4) as u32;
        if build == Build::A {
            if self.fcnt == 1 && (self.flock != 0 || v.iofs & 0x80 == 0) {
                if self.flock == 0 && self.fctl & 0x80 != 0 {
                    if v.master != 0x40 {
                        v.master += 1;
                    }
                } else if v.master != 0 {
                    v.master -= 1;
                }
            }
            if v.master != 0x40 && v.sfxpar == 0 {
                vol = (vol * v.master as u32) >> 6;
            }
        } else if self.master != 0x40 && v.iofs & 0x80 == 0 {
            vol = (vol * self.master as u32) >> 6;
        }
        hw.vol(i, vol as u16);
    }

    fn loop_regs(&self, i: usize, hw: &mut Hw) {
        let v = &self.v[i];
        if v.rlen == 0 {
            hw.loc(i, self.base.wrapping_add(self.silent() as u32));
            hw.len(i, 1);
        } else if self.build == Build::A {
            hw.loc(i, self.base.wrapping_add(v.rptr));
            hw.len(i, v.rlen);
        } else {
            hw.len(i, v.rlen);
            hw.loc(i, self.base.wrapping_add(v.rptr));
        }
    }

    // ----------------------------------------------------------------- init and tick

    fn init(&mut self, hw: &mut Hw) {
        let g = self.g;
        for i in 0..4 {
            let size = match self.build {
                Build::C => 0x38,
                Build::D => 0x3C,
                _ => 0x34,
            };
            self.v[i] = Voice { trk: self.r32(g + size * i) as usize, bit: self.r16(g + size * i + 4), ..Voice::default() };
        }
        self.tick = 0;
        self.row = 0;
        self.pos = 0;
        self.step = 0;
        self.fctl = 0;
        self.fcnt = 0;
        self.flock = 0;
        self.dmaon = 0;
        self.pend = 0;
        self.pendn = 0;
        self.brk = false;
        if self.build == Build::A {
            self.speed0 = self.r8(self.hf(0xD1));
            self.patlen = self.r8(self.hf(0xD2));
            self.loopstep = self.r8(self.hf(0xD3));
            self.endstep = self.r8(self.hf(0xD4));
            self.mode = 31;
            // the game's sound-effect sample records go to records 16..23, their
            // offsets made relative to the song's sample table
            if self.ho == 0 {
                let st = self.atab(0xEE);
                let add = 0x103F2u32.wrapping_sub(self.r32(g + 0xEE)).wrapping_sub(self.r32(g + 0xEA));
                for k in 0..80 {
                    let b = self.r8(g - 0x64 + k);
                    if let Some(x) = self.img.get_mut(st + 160 + k) {
                        *x = b;
                    }
                }
                for k in 0..8 {
                    let o = st + 160 + 10 * k;
                    let v = self.r32(o).wrapping_add(add);
                    if let Some(x) = self.img.get_mut(o..o + 4) {
                        x.copy_from_slice(&v.to_be_bytes());
                    }
                }
            }
            for v in &mut self.v {
                v.master = 0x40;
            }
            // the host's DMA off came with its start (`new`); the voices' silent loop, 0xA0 and
            // the DMA on follow in the same call
            let silent = self.base.wrapping_add(self.silent() as u32);
            hw.wait(INIT_WAIT);
            for i in 0..4 {
                hw.loc(i, silent);
                hw.wait(8);
                hw.len(i, 1);
                hw.wait(8);
                hw.per(i, 0x00A0);
                hw.wait(17);
            }
            hw.wait(2);
            hw.dmacon(0x820F);
            self.step = if self.mode & 2 != 0 { self.loopstep } else { 0 };
            self.load_step();
            for i in 0..4 {
                self.fetch_pattern(i);
            }
        } else {
            let c = self.c;
            if self.build == Build::D {
                self.master = 0;
                self.speed0 = self.r8(self.hf(0xD1));
                self.patlen = self.r8(self.hf(0xD2));
                self.loopstep = self.r8(self.hf(0xD3));
                self.endstep = self.r8(self.hf(0xD4));
            } else {
                self.master = self.r8(g + 0xD9 + c);
                self.speed0 = self.r8(g + 0xDD + c);
                self.patlen = self.r8(g + 0xDE + c);
                self.loopstep = self.r8(g + 0xDF + c);
                self.endstep = self.r8(g + 0xE0 + c);
            }
            self.step = self.loopstep;
            self.load_step();
        }
        self.speed = self.speed0;
    }

    fn tick_bc(&mut self, hw: &mut Hw) {
        hw.dmacon(self.pend);
        self.dmaon = 0;
        self.brk = false;
        for i in 0..4 {
            self.loop_regs(i, hw);
            if self.tick == 0 {
                if self.row == 0 {
                    self.fetch_pattern(i);
                }
                self.read_row(i);
                hw.wait(WAIT_ROW);
            }
            hw.wait(WAIT_LOOP);
        }
        hw.dmacon(self.dmaon);
        self.dmaon = 0x8200;
        self.pendn = 0;
        let row = self.tick == 0;
        for i in 0..4 {
            if row {
                self.note_start(i, hw);
                hw.wait(WAIT_NOTE);
            }
            self.effects(i, hw);
            hw.wait(WAIT_FX);
        }
        self.advance();
        hw.dmacon(self.dmaon | self.pend);
        self.pend = self.pendn;
        if self.fcnt != 0 {
            self.fcnt -= 1;
        } else {
            let r = self.fctl & 0x7F;
            if r != 0 {
                self.fcnt = r;
                if self.fctl & 0x80 == 0 {
                    if self.master != 0 {
                        self.master -= 1;
                    }
                } else if self.master == 0x40 {
                    self.fctl = 0;
                } else {
                    self.master += 1;
                }
            }
        }
    }

    fn tick_a(&mut self, hw: &mut Hw) {
        self.dmaon = 0;
        for i in 0..4 {
            self.loop_regs(i, hw);
            if self.tick == 0 {
                self.read_row(i);
                hw.wait(WAIT_ROW);
            }
            if self.v[i].sfxnote != 0 {
                self.dmaon |= self.v[i].bit;
                self.v[i].note = self.v[i].sfxnote;
                self.v[i].par = self.v[i].sfxpar;
            }
            hw.wait(WAIT_LOOP);
        }
        hw.dmacon(self.dmaon | self.pend);
        self.dmaon |= 0x8200;
        self.pendn = 0;
        let row = self.tick == 0;
        for i in 0..4 {
            if row || self.v[i].sfxnote != 0 {
                self.note_start(i, hw);
                self.v[i].sfxnote = 0;
                hw.wait(WAIT_NOTE);
            }
            self.effects(i, hw);
            hw.wait(WAIT_FX);
        }
        self.advance();
        if self.tick == 0 && self.row == 0 {
            for i in 0..4 {
                self.fetch_pattern(i);
            }
        }
        hw.dmacon(self.dmaon | self.pend);
        self.pend = self.pendn;
        if self.fcnt != 0 {
            self.fcnt -= 1;
        } else {
            self.fcnt = self.fctl & 0x7F;
        }
    }
}

impl Replayer for Sm {
    fn format(&self) -> &'static str {
        "Sound Master"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    /// The first tick's first write follows its timer call by this much (the interrupt's entry).
    fn timer_delay(&self) -> u64 {
        ENTRY
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.started {
            self.started = true;
            self.init(hw);
        }
        match self.build {
            Build::A => self.tick_a(hw),
            _ => self.tick_bc(hw),
        }
    }
}

/// From the host's first write to the song start, and from a timer call to the first write of its
/// tick, in the reference's runs.
const START: u64 = 2006;
/// Build A: from the host's second write to its init's first write.
const INIT_WAIT: u64 = 785;
const ENTRY: u64 = 156;

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let lay = scan(d)?;
    let n = if lay.build == Build::B { 64 } else { 46 };
    let periods = (0..n).map(|k| be16(d, lay.periods + 2 * k)).collect();
    let title = m.bus.read_cstr(m.deli.file_buf);
    // the init routine sets the LED bit of CIA-A's port: the filter is off
    m.bus.ciaa.pra |= 2;
    // the host's start (DMA off, the audio modulation off) comes before the song start
    let t0 = m.bus.clock;
    m.bus.write_word(0xDFF096, 0x000F);
    m.bus.clock = t0 + 8;
    m.bus.write_word(0xDFF09E, 0x00FF);
    let mut sm = Sm {
        img: d.to_vec(),
        base,
        title,
        build: lay.build,
        g: lay.g,
        c: if lay.build == Build::C { 0x10 } else { 0 },
        ho: lay.ho,
        tb: lay.tb,
        periods,
        v: [Voice::default(); 4],
        started: false,
        songend: false,
        tick: 0,
        row: 0,
        pos: 0,
        step: 0,
        fctl: 0,
        fcnt: 0,
        flock: 0,
        dmaon: 0,
        pend: 0,
        pendn: 0,
        pos0: 0,
        pos_n: 0,
        strans: 0,
        master: 0,
        brk: false,
        speed: 0,
        speed0: 0,
        patlen: 0,
        loopstep: 0,
        endstep: 0,
        mode: 0,
        lastarp: 0,
    };
    if lay.build == Build::A {
        // build A's init runs in the host's start call, its writes before the first tick
        let mut hw = Hw { bus: &mut m.bus };
        sm.started = true;
        sm.init(&mut hw);
    }
    m.bus.clock = m.bus.clock.max(t0 + START);
    Some(Box::new(sm))
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

    const G: usize = 0x200;
    const SOFF: usize = 0x40;

    fn period(k: usize) -> u16 {
        2000 - 16 * k as u16
    }

    /// A build-B image: the three branches, an init routine with its base LEA and the
    /// filter instruction, the period-table signature, and a song of one step whose
    /// voice 0 plays note 0x10 with instrument 1 (sample 0: 8 words, a loop of 2 at
    /// byte 4), then releases it; the other voices play an empty pattern.
    fn module_b(speed: u8) -> Vec<u8> {
        let mut d = vec![0u8; G + 0x11A8 + SOFF + 0x20];
        for (o, disp) in [(0, 0x0A), (4, 0x90), (8, 0x02)] {
            put16(&mut d, o, 0x6000);
            put16(&mut d, o + 2, disp);
        }
        put16(&mut d, 0x0C, 0x47FA);
        put16(&mut d, 0x0E, (G - 0x0E) as u16);
        for (k, w) in [0x08F9u16, 0x0001, 0x00BF, 0xE001, 0x4E75].iter().enumerate() {
            put16(&mut d, 0x10 + 2 * k, *w);
        }
        for (k, w) in [0x0200u16, 0x003F, 0xE300, 0x41FA, (0x100 - 0x28) as u16].iter().enumerate() {
            put16(&mut d, 0x20 + 2 * k, *w);
        }
        for k in 0..64 {
            put16(&mut d, 0x100 + 2 * k, period(k));
        }
        for v in 0..4 {
            put32(&mut d, G + 0x34 * v, 0x100 * v as u32);
            put16(&mut d, G + 0x34 * v + 4, 1 << v);
            // voices 1..3: pattern 1 (empty)
            d[G + 0x2E8 + 0x100 * v] = (v != 0) as u8;
        }
        d[G + 0xD9] = 64;
        d[G + 0xDD] = speed;
        d[G + 0xDE] = 32;
        d[G + 0xDF] = 0;
        d[G + 0xE0] = 1;
        put32(&mut d, G + 0xE2, SOFF as u32);
        d[G + 0xE8 + 0xC0] = 0x40; // step 0's fade byte: master volume 64
        let ins = G + 0xAE8 + 14;
        d[ins..ins + 14].copy_from_slice(&[0, 0, 0x30, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let pat = G + 0x1068;
        d[pat] = 0x10;
        d[pat + 1] = 1;
        put16(&mut d, G + 0x10E8 + SOFF, 8);
        put16(&mut d, G + 0x1128 + SOFF, 4);
        put16(&mut d, G + 0x1168 + SOFF, 2);
        d
    }

    #[test]
    fn recognition_and_builds() {
        let d = module_b(3);
        let l = scan(&d).unwrap();
        assert_eq!((l.build, l.g, l.periods), (Build::B, G, 0x100));
        // the voice loop step of build C
        let mut c = d.clone();
        for (k, w) in [0xDBFCu16, 0, 0x38].iter().enumerate() {
            put16(&mut c, 0x40 + 2 * k, *w);
        }
        put16(&mut c, 0x50, 0x0C00);
        put16(&mut c, 0x52, 0x002E);
        put16(&mut c, 0x54, 0xE300);
        put16(&mut c, 0x56, 0x41FA);
        put16(&mut c, 0x58, (0x100 - 0x58) as u16);
        assert_eq!(scan(&c).unwrap().build, Build::C);
        // no filter operand before the RTS, an odd branch, a missing period signature
        let mut e = d.clone();
        put16(&mut e, 0x16, 0xE000);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 6, 0x91);
        assert!(!detect(&e));
        let mut e = d.clone();
        put16(&mut e, 0x24, 0xE200);
        assert!(!detect(&e));
        assert!(!detect(&d[..0x100]));
    }

    #[test]
    fn build_a_is_recognised_by_its_init() {
        let mut d = vec![0u8; 0x2000];
        for (o, disp) in [(0, 0x90), (4, 0x100), (8, 0x02)] {
            put16(&mut d, o, 0x6000);
            put16(&mut d, o + 2, disp);
        }
        let code = [0x47FA, (0x400 - 0x94) as u16, 0x1740, 0xFFED, 0x41EB, 0x00F4, 0xD1EB, 0x00EE, 0x2748, 0xFF98, 0x08F9, 0x0001, 0x00BF, 0xE001, 0x177C, 0x0000, 0xFFFC, 0x4E75];
        for (k, w) in code.iter().enumerate() {
            put16(&mut d, 0x92 + 2 * k, *w);
        }
        for (k, w) in [0x0C00u16, 0x002E, 0x0200, 0xE300, 0x41FA, (0x300 - 0xCA) as u16].iter().enumerate() {
            put16(&mut d, 0xC0 + 2 * k, *w);
        }
        let l = scan(&d).unwrap();
        assert_eq!((l.build, l.g, l.periods), (Build::A, 0x400, 0x300));
        // without the trailing store the filter operand is not where it must be
        put16(&mut d, 0xAE, 0x1F7C);
        assert!(scan(&d).is_none());
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_note_releases_it_and_wraps() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/sm-test/sm.test"), module_b(3)).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Sound Master");
        let mut out = vec![0f32; 2 * 960 * 4];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        // init, the pending mask, the note's DMA off, then on with bit 9
        assert_eq!(reg(&log, 0xDFF096)[..4], [0x000F, 0x0000, 0x0001, 0x8201]);
        // the silent word first, then the sample (8 words) and from the next tick its loop
        assert_eq!(reg(&log, 0xDFF0A4)[..4], [1, 8, 2, 2]);
        let lc = reg(&log, 0xDFF0A2);
        assert_eq!(lc[1].wrapping_sub(lc[0]) as usize, 0x11A8 + SOFF - 0xE6);
        assert_eq!(lc[2].wrapping_sub(lc[1]), 4);
        // the note's period at the start and in the effect pass; the volume jumps to the peak
        assert_eq!(reg(&log, 0xDFF0A6)[..3], [period(0x10), period(0x10), period(0x10)]);
        assert_eq!(reg(&log, 0xDFF0A8)[0], 0x30);
        // 16 rows of 3 ticks, then the song wraps to its loop step: the song end
        assert!(!e.songend());
        let mut out = vec![0f32; 2 * 960 * 46];
        e.render(&mut out);
        assert!(e.songend());
    }

    #[test]
    fn release_lowers_the_volume_to_the_floor() {
        let mut d = module_b(1);
        // row 1: release (note 0); the release step is 4 in units of 1/16
        d[G + 0x1068 + 2] = 0;
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("/sm-test/sm.rel"), d).unwrap();
        let mut out = vec![0f32; 2 * 960 * 4];
        e.render(&mut out);
        let log: Vec<(u32, u16)> = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        let vol = reg(&log, 0xDFF0A8);
        // 0x300 - 4 a tick: 0x2FC >> 4 = 0x2F
        assert_eq!(vol[..3], [0x30, 0x2F, 0x2F]);
    }
}

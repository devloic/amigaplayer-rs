//! Dirk Bialluch's player (`tpu.*` with a sample companion `smp.*`).
//!
//! Clean-room, written from a behavioural specification and black-box runs; no
//! original code. The specification was drawn up from the player's observable
//! behaviour, and this player was written from it alone and checked against the
//! Paula register writes of UADE's player for these files.
//!
//! A module (`MSNG`) is a subsong table followed by chunks: the sequence rows
//! (`MSEQ`, eight bytes a row: a pattern number and a transpose for each of the
//! four voices), the patterns (`MPTN`), the effect programs (`MPRG`), the speed
//! tables (`MGRV`) and the vibrato table (`MARP`). The sample companion (`MSMP`)
//! holds the 8-bit samples. Each voice reads a main event stream from its pattern
//! (notes, waits, jumps, volume and speed commands) and an effect stream from its
//! program (sample start, length, volume and pitch offsets, slides). Every tick
//! writes the volume and period of the four voices, then their sample addresses
//! and lengths, then DMACON; the pace is set by the speed tables, an event tick
//! coming every N+1 ticks. The tick is 50 Hz.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Dirk Bialluch", detect, new };

/// Paula periods of the notes (index 0 is "no note").
const PERIODS: [u16; 37] = [
    65535, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269,
    254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
];

/// Colour clocks from the stage's first write to each write of the output stage:
/// the volumes (voice 3 first), the periods after them, the addresses, the lengths;
/// the DMA enable comes at 3146 (the original waits for the raster).
const VOL_AT: [u64; 4] = [0, 324, 636, 928];
const PER_GAP: [u64; 4] = [170, 166, 154, 142];
const LOC_AT: [u64; 4] = [1086, 1100, 1114, 1128];
const LEN_AT: [u64; 4] = [1142, 1152, 1162, 1172];

/// A stream that never ends a channel's turn is cut off after this many events.
const GUARD: u32 = 100_000;

/// The period of note index `i`; an index past the table (which the modules never
/// reach) gives 0.
fn period(i: usize) -> u16 {
    PERIODS.get(i).copied().unwrap_or(0)
}

/// The class of a byte in the main (sequence) stream; 0 ends the channel's turn.
fn seq_class(e: u8) -> u8 {
    match e {
        1..=48 => 1,
        65..=112 => 2,
        128 => 3,
        129 => 4,
        130 => 5,
        131 => 6,
        144 => 7,
        160 => 8,
        161 => 9,
        162 => 10,
        163 => 11,
        164 => 21,
        192 => 15,
        176 => 13,
        177 => 14,
        178 => 20,
        208 => 22,
        241 => 19,
        240 => 12,
        252 => 16,
        253 => 17,
        254 => 18,
        _ => 0,
    }
}

/// The class of a byte in the effect (program) stream.
fn eff_class(e: u8) -> u8 {
    match e {
        1..=5 => e,
        16..=18 => e - 10,
        32..=34 => e - 23,
        48 => 12,
        49 => 13,
        64 => 14,
        65 => 15,
        80 => 16,
        128 => 17,
        176 => 18,
        192 => 19,
        240 => 20,
        255 => 21,
        254 => 22,
        _ => 0,
    }
}

struct Chunks {
    mseq: usize,
    mptn: usize,
    mptn_end: usize,
    mprg: usize,
    mprg_end: usize,
    mgrv: usize,
    marp: usize,
}

/// The first even offset from `from` at which `tag` stands.
fn find(d: &[u8], tag: &[u8; 4], from: usize) -> Option<usize> {
    (from..d.len().saturating_sub(3)).step_by(2).find(|&o| d[o..o + 4] == *tag)
}

fn chunks(d: &[u8]) -> Option<Chunks> {
    if d.len() < 12 || d[..4] != *b"MSNG" {
        return None;
    }
    let mut mseq = None;
    for o in (4..518.min(d.len().saturating_sub(3))).step_by(2) {
        if d[o..o + 4] == *b"MSEQ" {
            mseq = Some(o);
            break;
        }
        if be16(d, o) & 0xF000 != 0 {
            return None;
        }
    }
    let mseq = mseq.filter(|&o| o >= 6)?;
    let mptn = find(d, b"MPTN", mseq)?;
    let mprg = find(d, b"MPRG", mptn)?;
    let mgrv = find(d, b"MGRV", mprg)?;
    let marp = find(d, b"MARP", mgrv)?;
    let mptn_end = mptn + 6 + 2 * (be16(d, mptn + 4) as usize + 2);
    let mprg_end = mprg + 6 + 2 * (be16(d, mprg + 4) as usize + 2);
    if mptn_end > d.len() || mprg_end > d.len() {
        return None;
    }
    Some(Chunks { mseq, mptn, mptn_end, mprg, mprg_end, mgrv, marp })
}

/// Recognition by content: the `MSNG` tag, a subsong table of words whose top four
/// bits are clear up to an `MSEQ` chunk before offset 518, then the other chunks.
fn detect(d: &[u8]) -> bool {
    chunks(d).is_some()
}

#[derive(Clone, Copy, Default)]
struct Voice {
    pos: u8,
    wait: u8,
    rep: u8,
    base: usize,
    tr: u8,
    vol: u8,
    note2: u8,
    trig: u8,
    tgt2: u8,
    spd: u8,
    x: u8,
    vs: u8,
    vp: u8,
    fixed: u8,
    epos: u8,
    ewait: u8,
    erep: u8,
    off: u8,
    acc: u16,
    pitch: u16,
    prog: usize,
    samp: u32,
    len: u16,
}

pub struct Tpu {
    d: Vec<u8>,
    smp: Vec<u8>,
    c: Chunks,
    title: String,
    subsongs: u16,
    sub: u16,
    /// The companion in guest memory.
    sbase: u32,
    /// Offset of the sample data in the companion.
    so: u32,
    v: [Voice; 4],
    playing: bool,
    position: u16,
    row_pending: bool,
    speed_gate: u8,
    speed_sel: u8,
    speed_idx: u8,
    glob: u8,
    fade_amt: u8,
    fade_rate: u8,
    fade_cnt: u8,
    fade_tgt: u8,
    songend: bool,
}

/// The companion's name for a module file name: `smp.NAME` for `tpu.NAME`.
fn companion(file: &str) -> Option<String> {
    let leaf = file.rsplit(['/', ':']).next().unwrap_or(file);
    let p = leaf.get(..4).filter(|p| p.eq_ignore_ascii_case("tpu."))?;
    let _ = p;
    Some(format!("smp.{}", &leaf[4..]))
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let c = chunks(d)?;
    let file = m.bus.read_cstr(m.deli.file_buf);
    let smp = crate::deli::read_companion(m, &companion(&file)?).1.ok()?;
    if smp.get(..4)? != b"MSMP" {
        return None;
    }
    let so = 6 + 4 * (be16(&smp, 4) as u32 + 2);
    let sbase = m.alloc((smp.len() as u32).max(4));
    if sbase == 0 {
        return None;
    }
    m.bus.write_bytes(sbase, &smp);
    let subsongs = (((c.mseq - 6) / 2) as u16).max(1);
    let mut t = Tpu {
        d: d.to_vec(),
        smp,
        c,
        title: file,
        subsongs,
        sub: 0,
        sbase,
        so,
        v: Default::default(),
        playing: false,
        position: 0,
        row_pending: false,
        speed_gate: 0,
        speed_sel: 0,
        speed_idx: 0,
        glob: 0,
        fade_amt: 0,
        fade_rate: 0,
        fade_cnt: 0,
        fade_tgt: 0,
        songend: false,
    };
    let mut hw = Hw { bus: &mut m.bus };
    t.start(0, &mut hw, true);
    Some(Box::new(t))
}

impl Tpu {
    fn rb(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn rw(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// Origin of pattern `p`.
    fn pattern(&self, p: u8) -> usize {
        self.c.mptn_end + self.rw(self.c.mptn + 6 + 2 * p as usize) as usize
    }

    /// Origin of program `p`.
    fn program(&self, p: u8) -> usize {
        self.c.mprg_end + self.rw(self.c.mprg + 6 + 2 * p as usize) as usize
    }

    /// Companion offset of sample `i`.
    fn sample(&self, i: u8) -> u32 {
        self.so.wrapping_add(be32(&self.smp, 6 + 4 * i as usize))
    }

    fn start(&mut self, n: u16, hw: &mut Hw, glue: bool) {
        self.sub = n.min(self.subsongs - 1);
        self.v = Default::default();
        let p0 = self.program(0);
        for v in self.v.iter_mut() {
            v.prog = p0;
        }
        self.position = self.rw(4 + 2 * self.sub as usize);
        self.row_pending = true;
        self.playing = true;
        self.speed_gate = 0;
        self.speed_sel = 0;
        self.speed_idx = 0;
        self.glob = 0;
        self.fade_amt = 0;
        self.fade_rate = 0;
        self.fade_cnt = 0;
        self.fade_tgt = 0;
        self.songend = false;
        // the host glue's start-up: the four volume registers, once, with a value
        // derived from the host (the number of subsongs here), then ADKCON
        if glue {
            for ch in 0..4 {
                hw.vol(ch, self.subsongs);
                hw.wait(104);
            }
            hw.bus.write_word(0xDFF09E, 0x00FF);
        }
    }

    fn fetch_row(&mut self) {
        self.position &= 4095;
        let row = self.c.mseq + 4 + 8 * self.position as usize;
        for c in 0..4 {
            let p = self.rb(row + 2 * c);
            self.v[c].base = self.pattern(p);
            self.v[c].tr = self.rb(row + 2 * c + 1);
        }
        self.position = self.position.wrapping_add(1);
        for v in self.v.iter_mut() {
            v.pos = 0;
            v.wait = 0;
            v.rep = 0;
        }
        self.row_pending = false;
    }

    fn events(&mut self) {
        for c in (0..4).rev() {
            if self.v[c].wait > 0 {
                self.v[c].wait -= 1;
                continue;
            }
            let mut d = self.v[c].pos as usize;
            for _ in 0..GUARD {
                let at = self.v[c].base + d;
                let (e, a1, a2) = (self.rb(at), self.rb(at + 1), self.rb(at + 2));
                let tr = self.v[c].tr;
                let v = &mut self.v[c];
                // Ok(next): store the position and end the turn; Err(next): go on
                // reading at `next`
                let r: Result<u8, usize> = match seq_class(e) {
                    1 => {
                        v.vol = a1;
                        v.note2 = e.wrapping_add(tr).wrapping_mul(2);
                        v.trig = a2;
                        Err(d + 3)
                    }
                    2 => {
                        v.vol = 64;
                        v.note2 = e.wrapping_sub(64).wrapping_add(tr).wrapping_mul(2);
                        v.trig = a2;
                        v.wait = a1;
                        Ok((d + 3) as u8)
                    }
                    3 => {
                        v.wait = a2;
                        Ok((d + 3) as u8)
                    }
                    4 => {
                        self.speed_sel = a2;
                        self.speed_idx = 0;
                        Err(d + 3)
                    }
                    5 | 6 => Err(d + 3),
                    7 => {
                        v.tgt2 = a1.wrapping_add(tr).wrapping_mul(2);
                        v.spd = a2;
                        Err(d + 3)
                    }
                    8 => {
                        self.glob = a2;
                        Err(d + 3)
                    }
                    9 => {
                        v.vol = a2;
                        v.wait = a1;
                        Ok((d + 3) as u8)
                    }
                    10 => {
                        v.vol = v.vol.wrapping_add(a2);
                        v.wait = a1;
                        Ok((d + 3) as u8)
                    }
                    11 => {
                        v.vol = v.vol.wrapping_sub(a2);
                        v.wait = a1;
                        Ok((d + 3) as u8)
                    }
                    12 => {
                        let jump = a2.wrapping_mul(3);
                        if v.rep == 0 {
                            v.rep = a1;
                            Ok(jump)
                        } else {
                            v.rep = v.rep.wrapping_sub(1);
                            if v.rep != 0 { Ok(jump) } else { Ok((d + 3) as u8) }
                        }
                    }
                    13 => {
                        v.x = a2;
                        Err(d + 3)
                    }
                    14 => {
                        v.x = a2;
                        v.wait = a1;
                        Ok((d + 3) as u8)
                    }
                    15 => {
                        v.vs = a2;
                        v.vp = 0;
                        Err(d + 3)
                    }
                    16 => {
                        self.position = (a1 as u16) << 8 | a2 as u16;
                        self.row_pending = true;
                        self.songend = true;
                        break;
                    }
                    17 => {
                        self.songend = true;
                        self.playing = false;
                        break;
                    }
                    18 => {
                        self.row_pending = true;
                        break;
                    }
                    19 => {
                        v.base = self.c.mptn_end + be16(&self.d, self.c.mptn + 6 + 2 * a2 as usize) as usize;
                        Ok(a1.wrapping_mul(3))
                    }
                    20 => Err(d + 3),
                    21 => {
                        self.fade_amt = a1;
                        self.fade_rate = a2;
                        Err(d + 3)
                    }
                    22 => {
                        self.fade_tgt = a2;
                        Err(d + 3)
                    }
                    _ => break,
                };
                match r {
                    Ok(n) => {
                        self.v[c].pos = n;
                        break;
                    }
                    Err(n) => d = n,
                }
            }
        }
    }

    fn speed(&mut self) {
        let t = self.c.mgrv + 4;
        let mut g = self.rb(t + self.speed_sel as usize + self.speed_idx as usize);
        if g == 255 {
            self.speed_idx = 0;
            g = self.rb(t + self.speed_sel as usize);
        }
        self.speed_gate = g;
        self.speed_idx = self.speed_idx.wrapping_add(1);
    }

    fn triggers(&mut self) {
        for c in (0..4).rev() {
            let t = self.v[c].trig;
            if t == 0 {
                continue;
            }
            let q = self.program(t);
            if self.rb(q) < self.rb(self.v[c].prog) {
                continue;
            }
            let v = &mut self.v[c];
            v.prog = q;
            v.fixed = 0;
            v.spd = 0;
            v.x = 0;
            v.epos = 0;
            v.ewait = 0;
            v.erep = 0;
            v.acc = 0;
            v.pitch = 0;
            v.off = 0;
        }
        for v in self.v.iter_mut() {
            v.trig = 0;
        }
    }

    fn effects(&mut self, hw: &mut Hw) {
        for c in (0..4).rev() {
            if self.v[c].ewait > 0 {
                self.v[c].ewait -= 1;
                continue;
            }
            let idle = self.program(0);
            let ptr = self.v[c].prog + 4;
            let mut d = self.v[c].epos as usize;
            for _ in 0..GUARD {
                let at = ptr + d;
                let e = self.rb(at);
                let (a2, a3) = (self.rb(at + 2), self.rb(at + 3));
                let w = self.rw(at + 2) as u32;
                let k = eff_class(e);
                let sample = if matches!(k, 1 | 4) { self.sample(a3) } else { 0 };
                let long = be32(&self.d, at);
                let v = &mut self.v[c];
                // as in `events`
                let r: Result<u8, usize> = match k {
                    1 => {
                        hw.per(c, 80);
                        v.samp = sample;
                        hw.wait(50);
                        hw.dmacon(1 << c);
                        hw.wait(242);
                        Err(d + 4)
                    }
                    2 => {
                        v.samp = v.samp.wrapping_add(w);
                        Err(d + 4)
                    }
                    3 => {
                        v.len = (long >> 1) as u16;
                        Ok((d + 4) as u8)
                    }
                    4 => {
                        v.samp = sample;
                        Err(d + 4)
                    }
                    5 => {
                        v.samp = v.samp.wrapping_sub(w);
                        Err(d + 4)
                    }
                    6 => {
                        v.ewait = a2;
                        v.off = a3;
                        Ok((d + 4) as u8)
                    }
                    7 => {
                        v.ewait = a2;
                        v.off = v.off.wrapping_add(a3);
                        Ok((d + 4) as u8)
                    }
                    8 => {
                        v.ewait = a2;
                        v.off = v.off.wrapping_sub(a3);
                        Ok((d + 4) as u8)
                    }
                    9 => {
                        v.off = a3;
                        Err(d + 4)
                    }
                    10 => {
                        v.off = v.off.wrapping_add(a3);
                        Err(d + 4)
                    }
                    11 => {
                        v.off = v.off.wrapping_sub(a3);
                        Err(d + 4)
                    }
                    12 => {
                        v.ewait = a2;
                        v.pitch = v.pitch.wrapping_sub(a3 as u16);
                        Ok((d + 4) as u8)
                    }
                    13 => {
                        v.ewait = a2;
                        v.pitch = v.pitch.wrapping_add(a3 as u16);
                        Ok((d + 4) as u8)
                    }
                    14 => {
                        v.pitch = v.pitch.wrapping_sub(a3 as u16);
                        Err(d + 4)
                    }
                    15 => {
                        v.pitch = v.pitch.wrapping_add(a3 as u16);
                        Err(d + 4)
                    }
                    16 => {
                        v.fixed = a3;
                        Err(d + 4)
                    }
                    17 => {
                        v.ewait = a3;
                        Ok((d + 4) as u8)
                    }
                    18 => {
                        if v.x == a2 {
                            v.x = 0;
                            Err((d & 0xFF00) | a3.wrapping_mul(4) as usize)
                        } else {
                            Ok((d + 4) as u8)
                        }
                    }
                    19 => {
                        v.vs = a3;
                        v.vp = 0;
                        Err(d + 4)
                    }
                    20 => {
                        let jump = (d & 0xFF00) | a3.wrapping_mul(4) as usize;
                        if v.erep == 0 {
                            v.erep = a2;
                            Ok(jump as u8)
                        } else {
                            v.erep = v.erep.wrapping_sub(1);
                            Ok(if v.erep != 0 { jump as u8 } else { (d + 4) as u8 })
                        }
                    }
                    // restart: back to the idle program 0, from its start
                    21 => {
                        v.prog = idle;
                        Ok(0)
                    }
                    22 => {
                        v.trig = a3;
                        break;
                    }
                    _ => break,
                };
                match r {
                    Ok(n) => {
                        self.v[c].epos = n;
                        break;
                    }
                    Err(n) => d = n,
                }
            }
        }
    }

    fn fade(&mut self) {
        if self.fade_tgt == 0 {
            return;
        }
        if self.fade_cnt > 0 {
            self.fade_cnt -= 1;
            return;
        }
        self.fade_cnt = self.fade_rate;
        self.glob = self.glob.wrapping_sub(self.fade_amt);
        if self.glob & 0x80 != 0 {
            self.position = self.rw(4 + 2 * (self.fade_tgt as usize - 1));
            self.fade_tgt = 0;
            self.fade_cnt = 0;
            self.glob = 0;
            self.row_pending = true;
            self.speed_gate = 0;
        }
    }

    fn output(&mut self, hw: &mut Hw) {
        let g = self.glob as u32;
        let vib = self.c.marp + 4;
        // colour clocks from the first write of the stage
        let mut t = 0u64;
        let mut at = |hw: &mut Hw, off: u64| {
            hw.wait(off - t);
            t = off;
        };
        for c in (0..4).rev() {
            at(hw, VOL_AT[3 - c]);
            let v = self.v[c];
            let s = v.vol.wrapping_add(v.off);
            let vol = if s & 0x80 != 0 { 0 } else { (s as u32 * g) >> 6 };
            hw.vol(c, vol as u16);
            let per;
            if v.fixed != 0 {
                per = period((v.fixed & 127) as usize);
            } else {
                let vib_at = |i: u8| self.d.get(vib + i as usize).copied().unwrap_or(0);
                let mut vp = v.vp;
                let mut i = v.vs.wrapping_add(vp);
                if vib_at(i) == 128 {
                    vp = 0;
                    i = v.vs;
                }
                let d2 = vib_at(i).wrapping_mul(2);
                vp = vp.wrapping_add(1);
                let base = period((v.note2.wrapping_add(d2) / 2) as usize);
                let target = period((v.tgt2 / 2) as usize);
                let spd = v.spd as u16;
                let diff = target.wrapping_sub(base);
                let mut cur = v.acc;
                let mut nspd = v.spd;
                if diff.wrapping_sub(cur) & 0x8000 == 0 {
                    cur = cur.wrapping_add(spd);
                    if (diff as i16) <= (cur as i16) {
                        nspd = 0;
                        cur = diff;
                    }
                } else {
                    cur = cur.wrapping_sub(spd);
                    if (diff as i16) >= (cur as i16) {
                        nspd = 0;
                        cur = diff;
                    }
                }
                let vv = &mut self.v[c];
                vv.vp = vp;
                vv.spd = nspd;
                vv.acc = cur;
                per = base.wrapping_add(cur).wrapping_add(vv.pitch);
            }
            at(hw, VOL_AT[3 - c] + PER_GAP[3 - c]);
            hw.per(c, per);
        }
        for c in 0..4 {
            at(hw, LOC_AT[c]);
            hw.loc(c, self.sbase.wrapping_add(self.v[c].samp));
        }
        for c in 0..4 {
            at(hw, LEN_AT[c]);
            hw.len(c, self.v[c].len);
        }
        at(hw, 3146);
        hw.dmacon(33295);
    }
}

impl Replayer for Tpu {
    fn format(&self) -> &'static str {
        "Dirk Bialluch"
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
        self.start(n, hw, false);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if !self.playing {
            return;
        }
        if self.speed_gate > 0 {
            self.speed_gate -= 1;
        } else {
            if self.row_pending {
                self.fetch_row();
            }
            self.events();
            self.speed();
        }
        self.triggers();
        self.effects(hw);
        self.fade();
        self.output(hw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// A one-row module whose four voices all play `pattern`; program 1 starts
    /// sample 0 with a length of 2 words, program 0 is the idle loop.
    fn module(pattern: &[u8]) -> Vec<u8> {
        let mut d = b"MSNG".to_vec();
        d.extend_from_slice(&[0, 0, 0, 0]);
        d.extend_from_slice(b"MSEQ");
        for _ in 0..4 {
            d.extend_from_slice(&[0, 0]);
        }
        d.extend_from_slice(b"MPTN");
        d.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
        d.extend_from_slice(pattern);
        if d.len() % 2 == 1 {
            d.push(0);
        }
        d.extend_from_slice(b"MPRG");
        d.extend_from_slice(&[0, 1, 0, 0, 0, 8, 0, 0]);
        d.extend_from_slice(&[0, 0, 0, 0, 255, 0, 0, 0]);
        d.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 4, 255, 0, 0, 0]);
        d.extend_from_slice(b"MGRV");
        d.extend_from_slice(&[1, 255, 0, 0]);
        d.extend_from_slice(b"MARP");
        d.extend_from_slice(&[0, 128, 0, 0]);
        d
    }

    fn companion_file() -> Vec<u8> {
        let mut s = b"MSMP".to_vec();
        s.extend_from_slice(&[0, 1]);
        for o in [0u32, 16, 16] {
            s.extend_from_slice(&o.to_be_bytes());
        }
        s.extend_from_slice(&[0x40; 16]);
        s
    }

    fn reg(log: &[(u32, u16)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    fn play(pattern: &[u8], ticks: usize) -> (Engine, Vec<(u32, u16)>) {
        let dir = Path::new("/tpu-test");
        let mut e = Engine::new(48000.0);
        e.m.os.mem_files.insert(dir.join("SMP.test"), companion_file());
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&dir.join("tpu.test"), module(pattern)).unwrap();
        let mut out = vec![0f32; 2 * 960 * ticks];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap().iter().map(|w| (w.0, w.1)).collect();
        (e, log)
    }

    #[test]
    fn recognises_by_content() {
        let d = module(&[0x80, 0, 3]);
        assert!(detect(&d));
        assert!(!detect(&d[..20]));
        let mut e = d.clone();
        e[0] = b'X';
        assert!(!detect(&e));
        // a word with a top bit set before MSEQ
        let mut e = d.clone();
        e[4] = 0x10;
        assert!(!detect(&e));
        // no MSEQ before offset 518
        let mut e = b"MSNG".to_vec();
        e.extend_from_slice(&[0; 600]);
        assert!(!detect(&e));
        assert_eq!(companion("/a/tpu.song").as_deref(), Some("smp.song"));
        assert_eq!(companion("TPU.Song").as_deref(), Some("smp.Song"));
        assert_eq!(companion("song"), None);
    }

    #[test]
    fn classes() {
        assert_eq!(seq_class(13), 1);
        assert_eq!(seq_class(65), 2);
        assert_eq!(seq_class(253), 17);
        assert_eq!(seq_class(200), 0);
        assert_eq!(eff_class(1), 1);
        assert_eq!(eff_class(16), 6);
        assert_eq!(eff_class(255), 21);
        assert_eq!(eff_class(100), 0);
    }

    #[test]
    fn plays_a_note() {
        // global volume 64, note 13 at volume 64 starting program 1, a wait of 3
        let (e, log) = play(&[0xA0, 0, 64, 13, 64, 1, 0x80, 0, 3], 4);
        assert_eq!(e.playing.as_ref().unwrap().format, "Dirk Bialluch");
        // start-up volumes (the number of subsongs), then the trigger's period 80
        assert_eq!(reg(&log, 0xDFF0A8)[0], 1);
        assert_eq!(reg(&log, 0xDFF0A6)[..2], [80, 428]);
        assert_eq!(reg(&log, 0xDFF0A8)[1], 64);
        assert_eq!(reg(&log, 0xDFF0A4)[0], 2);
        assert!(reg(&log, 0xDFF096).contains(&33295));
        assert!(!e.songend());
    }

    #[test]
    fn song_end_stops_the_writes() {
        let (e, log) = play(&[0xFD, 0, 0], 6);
        assert!(e.songend());
        // the tick that reads the end still writes; nothing after it
        assert_eq!(reg(&log, 0xDFF096).iter().filter(|&&v| v == 33295).count(), 1);
    }

    #[test]
    fn refuses_a_module_without_its_companion() {
        let dir = Path::new("/tpu-test");
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        let _ = e.play_data(&dir.join("tpu.test"), module(&[0x80, 0, 3]));
        assert!(e.playing.as_ref().is_none_or(|p| p.format != "Dirk Bialluch"));
    }

    /// `PK_LIST=file` (one path a line): which open replayers claim each file.
    #[test]
    #[ignore]
    fn det() {
        let Ok(list) = std::env::var("PK_LIST") else { return };
        for p in std::fs::read_to_string(list).unwrap().lines() {
            let Ok(d) = std::fs::read(p) else { continue };
            let n: Vec<&str> = super::super::REPLAYERS.iter().filter(|o| (o.detect)(&d)).map(|o| o.name).collect();
            if !n.is_empty() {
                println!("DET\t{p}\t{}", n.join(","));
            }
        }
    }
}

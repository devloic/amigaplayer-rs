//! ProTracker 4 IFF modules (`mod3.*`, the Eagleplayer "Protracker4"): an IFF `FORM` of type
//! `MODL` (the file format of ProTracker 3.6x/4.0) that wraps an ordinary 31-instrument
//! ProTracker module (`M.K.`, or `8CHN` for eight voices) in a `PTDT` chunk, with an `INFO`
//! chunk for the tempo and a timing-mode flag.
//!
//! Clean-room, written from a behavioural specification drawn up from the Paula register writes
//! of the 68k reference player and the module data; no original code. The period, vibrato and
//! funk tables are the standard ProTracker ones, shared with the built-in ProTracker's
//! (`crate::protracker`), whose replay this one follows value for value, except that:
//! - the container is read by the player's own chunk walk (a file it fails to walk is not
//!   recognised here, where the original starts and plays silence);
//! - the initial tempo comes from INFO, and with the INFO flag bit 0 clear an Fxx of 0x20 and up
//!   sets the speed, not the tempo; F00 does nothing (no song end);
//! - a length of 0 or 1 is written as 0 (Paula then plays 65536 words from the repeat start), the
//!   repeat block after a row's DMA on goes in ascending voice order, a retrigger writes a final
//!   period, the fine slide mask starts at 0, E0x leaves the filter alone and E8x runs a
//!   Karplus-Strong filter on the sample in memory every tick;
//! - the song end is a wrap past the song length or a backward Bxx (a jump to itself on the last
//!   position tests a length byte of the wrong place and practically never ends);
//! - an arpeggio that reads past the period table reads the player's own variables;
//! - eight voices (`8CHN`) play through the Eagleplayer amplifier's virtual voices.
//!
//! Timing: the interrupt period is 5 * (1773447 / tempo + 1) colour clocks. The writes within a
//! tick are spaced as the original's code, in 68000 cycles measured between writes (the host
//! calls themselves take no time), and the host waits ten raster lines after every DMA on.
//! The costs are written as the path a voice takes and are exact for the paths the specification
//! measured; paths it did not (the unusual waveforms, E3x, E4x, E7x, EAx, ECx) use the cost of
//! the nearest measured one.
//!
//! Choices where the specification leaves the behaviour open: an instrument number above 31
//! changes nothing; a row of 64 or more (a Dxx of 64) moves on to the next position; the funk
//! inversion and the E8x filter touch only memory that is in the module's file; an arpeggio
//! reading past the table's end reads zero after the three variables the specification names.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::protracker::{FUNK, PERIODS, VIBRATO};

pub const OPEN: Open = Open { name: "Protracker4", detect, new };

/// Colour clocks per raster line.
const LINE: u64 = 227;

// Costs of the original's code in 68000 cycles between two writes, measured (see the module doc).
/// Interrupt entry to the first write of a row tick, on top of the voice's own path.
const ROW_ENTRY: u64 = 328;
/// The same for an effect tick (a period write first takes 2 cycles less).
const FX_ENTRY_VOL: u64 = 132;
/// A voice's period write for the previous empty cell, after the previous voice's volume.
const ROW_PREV_PER: u64 = 368;
/// What a previous empty cell adds to the rest of the voice's path (on a trigger, on others).
const ROW_PREV_EXTRA_TRIG: u64 = 194;
const ROW_PREV_EXTRA: u64 = 196;
/// A funk speed set adds this to every effect-tick voice.
const FX_FUNK: u64 = 206;

#[derive(Clone, Copy, Default)]
struct Smp {
    start: u32,
    len: u16,
    fine: u8,
    vol: u8,
    rep: u16,
    replen: u16,
}

#[derive(Clone, Copy, Default)]
struct Ch {
    cell: u32,
    start: u32,
    len: u16,
    rstart: u32,
    rlen: u16,
    wave: u32,
    period: u16,
    fine: u8,
    vol: u8,
    tp_target: u16,
    tp_speed: u8,
    tp_down: bool,
    vib_cmd: u8,
    vib_pos: u8,
    trem_cmd: u8,
    trem_pos: u8,
    wavectl: u8,
    glissfunk: u8,
    funk_acc: u8,
    loop_row: u8,
    loop_cnt: u8,
    off_mem: u8,
}

impl Ch {
    fn note(&self) -> u16 {
        (self.cell >> 16) as u16 & 0xFFF
    }
    fn cmd(&self) -> u8 {
        (self.cell >> 8) as u8 & 0xF
    }
    fn par(&self) -> u8 {
        self.cell as u8
    }
}

/// What the container holds.
struct Parsed {
    /// Offset of the PTDT data (the module).
    data: usize,
    tempo: u16,
    flags: u8,
    name: String,
}

const PT_HEADER: usize = 1084;

/// The player's chunk walk (specification 2.1, 2.2): the module's offset, tempo and flags.
fn parse(d: &[u8]) -> Option<Parsed> {
    if d.len() < 32 || &d[0..4] != b"FORM" || &d[8..12] != b"MODL" || &d[12..16] != b"VERS" {
        return None;
    }
    let form = be32(d, 4);
    if form as i32 > d.len() as i32 {
        return None;
    }
    let start = if be32(d, 16) == 0x16 { 30 } else { 12 };
    let find = |id: &[u8; 4]| -> Option<usize> {
        let mut pos = start;
        loop {
            if pos as u64 >= form as u64 || pos + 8 > d.len() {
                return None;
            }
            if &d[pos..pos + 4] == id {
                return Some(pos);
            }
            let size = be32(d, pos + 4) as usize;
            if size == 0 {
                return None;
            }
            pos = pos.checked_add(size)?;
        }
    };
    let info = find(b"INFO")? + 8;
    let ptdt = find(b"PTDT")? + 8;
    if info + 0x2B > d.len() || ptdt + PT_HEADER > d.len() {
        return None;
    }
    let name: String = d[info..info + 32].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
    Some(Parsed { data: ptdt, tempo: be16(d, info + 0x28), flags: d[info + 0x2A], name: name.trim().to_string() })
}

fn detect(d: &[u8]) -> bool {
    parse(d).is_some()
}

struct Pt4 {
    title: String,
    nv: usize,
    cia: bool,
    smp: [Smp; 32],
    order: [u8; 128],
    song_len: usize,
    /// The byte the Bxx song-end test reads (offset 0x3B6 of the whole file).
    b_len: u8,
    pats: Vec<u8>,
    ch: [Ch; 8],
    speed: u8,
    counter: u8,
    tempo: u32,
    pos: u16,
    row: u8,
    break_row: u8,
    pos_jump: bool,
    loop_jump: bool,
    delay_req: u8,
    delay_cnt: u8,
    mask: u8,
    dma: u8,
    songend: bool,
    /// The module's file in guest memory: sample memory outside it is left alone.
    mem_lo: u32,
    mem_hi: u32,
    /// Absolute time in 68000 cycles (twice the colour clock).
    cyc: u64,
    /// The first write of a tick is still to come.
    first: bool,
    fx_entry: bool,
    /// Cycles of the voice's path already spent before its first write (a previous empty cell's
    /// period write) and what that adds to the rest.
    skip: i64,
    extra: i64,
    vol_seg: Option<u64>,
    vol_go: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let p = parse(d)?;
    let md = &d[p.data..];
    let nv = if &md[1080..1084] == b"8CHN" { 8 } else { 4 };
    if nv == 8 && !super::start_amplifier(m, 8) {
        return None;
    }
    let mut order = [0u8; 128];
    order.copy_from_slice(&md[952..1080]);
    // the patterns stored: the highest order entry (a byte with bit 7 set does not count) + 1
    let npat = order.iter().copied().filter(|&o| o < 128).max().unwrap_or(0) as usize + 1;
    let psize = nv * 256;
    let mut pats = vec![0u8; npat * psize];
    let avail = md.len().saturating_sub(PT_HEADER).min(pats.len());
    pats[..avail].copy_from_slice(&md[PT_HEADER..PT_HEADER + avail]);
    let mut smp = [Smp::default(); 32];
    let mut at = base + (p.data + PT_HEADER + npat * psize) as u32;
    for (i, s) in smp[1..].iter_mut().enumerate() {
        let o = 20 + i * 30;
        *s = Smp { start: at, len: be16(md, o + 22), fine: md[o + 24] & 0xF, vol: md[o + 25], rep: be16(md, o + 26), replen: be16(md, o + 28) };
        at = at.wrapping_add(s.len as u32 * 2);
    }
    let t = Pt4 {
        title: p.name,
        nv,
        cia: p.flags & 1 != 0,
        smp,
        order,
        song_len: md[950] as usize,
        b_len: d[0x3B6],
        pats,
        ch: [Ch::default(); 8],
        speed: 6,
        counter: 0,
        tempo: if p.tempo == 0 { 125 } else { p.tempo as u32 },
        pos: 0,
        row: 0,
        break_row: 0,
        pos_jump: false,
        loop_jump: false,
        delay_req: 0,
        delay_cnt: 0,
        mask: 0,
        dma: 0,
        songend: false,
        mem_lo: base,
        mem_hi: base + d.len() as u32,
        cyc: 0,
        first: false,
        fx_entry: false,
        skip: 0,
        extra: 0,
        vol_seg: None,
        vol_go: 0,
    };
    let mut hw = Hw { bus: &mut m.bus };
    // all DMA off: one write with four voices, one poke each with eight
    if nv == 4 {
        hw.dmacon(0x000F);
    } else {
        hw.voice_dma(0xFF, false);
    }
    Some(Box::new(t))
}

impl Pt4 {
    // ---- time and the host's pokes ------------------------------------------------------

    /// Lets `c` cycles of the original's code pass before the next write.
    fn go(&mut self, hw: &mut Hw, c: u64) {
        self.cyc += c;
        hw.bus.clock = self.cyc / 2;
    }

    /// The same for a path total counted from the previous write, less what an earlier write of
    /// the voice's path already took, plus what that earlier write adds.
    fn seg(&mut self, hw: &mut Hw, total: u64) {
        let c = (total as i64 - self.skip + self.extra).max(0) as u64;
        self.skip = 0;
        self.extra = 0;
        self.go(hw, c);
    }

    fn loc(&mut self, hw: &mut Hw, v: usize, a: u32) {
        if self.nv == 4 {
            hw.loc(v, a);
        } else {
            hw.voice_loc(v, a);
        }
    }

    /// A length of 0 or 1 is written as 0.
    fn len(&mut self, hw: &mut Hw, v: usize, w: u16) {
        let w = if w <= 1 { 0 } else { w };
        if self.nv == 4 {
            hw.len(v, w);
        } else {
            hw.voice_len(v, w);
        }
    }

    fn per(&mut self, hw: &mut Hw, v: usize, p: u16) {
        if self.first && self.fx_entry {
            // the entry takes two cycles less before a period than before a volume
            self.cyc -= 2;
            hw.bus.clock = self.cyc / 2;
        }
        self.first = false;
        if self.nv == 4 {
            hw.per(v, p);
        } else {
            hw.voice_per(v, p);
        }
    }

    fn vol(&mut self, hw: &mut Hw, v: usize, x: u16) {
        self.first = false;
        if self.nv == 4 {
            hw.vol(v, x);
        } else {
            hw.voice_vol(v, x);
        }
    }

    fn dma_off(&mut self, hw: &mut Hw, mask: u8) {
        if self.nv == 4 {
            hw.dmacon(mask as u16 & 0xF);
        } else {
            hw.voice_dma(mask as u32, false);
        }
    }

    /// DMA on, then the host waits until ten raster lines on.
    fn dma_on(&mut self, hw: &mut Hw, mask: u8) {
        if self.nv == 4 {
            hw.dmacon(0x8000 | (mask as u16 & 0xF));
        } else {
            hw.voice_dma(mask as u32, true);
        }
        let t = hw.bus.clock;
        hw.bus.clock = (t / LINE + 10) * LINE;
        self.cyc = hw.bus.clock * 2;
    }

    fn byte(&self, hw: &Hw, a: u32) -> Option<u8> {
        (a >= self.mem_lo && a < self.mem_hi).then(|| hw.bus.ram.get(a as usize).copied()).flatten()
    }

    fn put(&self, hw: &mut Hw, a: u32, b: u8) {
        if a >= self.mem_lo && a < self.mem_hi {
            if let Some(x) = hw.bus.ram.get_mut(a as usize) {
                *x = b;
            }
        }
    }

    // ---- the tick -----------------------------------------------------------------------

    fn tick(&mut self, hw: &mut Hw) {
        // (measured: the interrupt enters one cycle into the colour clock, which decides how the
        // odd cycle counts of the costs round to colour clocks)
        self.cyc = hw.bus.clock * 2 + 1;
        self.first = true;
        self.counter = self.counter.wrapping_add(1);
        if self.counter >= self.speed {
            self.counter = 0;
            if self.delay_cnt != 0 {
                self.fx_all(hw);
            } else {
                self.row_tick(hw);
            }
            self.row = self.row.wrapping_add(1);
            if self.delay_req != 0 {
                self.delay_cnt = self.delay_req;
                self.delay_req = 0;
            }
            if self.delay_cnt != 0 {
                self.delay_cnt -= 1;
                if self.delay_cnt != 0 {
                    self.row = self.row.wrapping_sub(1);
                }
            }
            if self.loop_jump {
                self.loop_jump = false;
                self.row = self.break_row;
                self.break_row = 0;
            }
            if self.row >= 64 {
                self.next_position();
            }
        } else {
            self.fx_all(hw);
        }
        if self.pos_jump {
            self.next_position();
        }
    }

    fn next_position(&mut self) {
        self.row = self.break_row;
        self.break_row = 0;
        self.pos_jump = false;
        self.pos = self.pos.wrapping_add(1) & 0x7F;
        if self.pos as usize >= self.song_len {
            self.pos = 0;
            self.songend = true;
        }
    }

    fn cell_at(&self, v: usize) -> u32 {
        let pat = self.order[self.pos as usize & 127] as usize;
        let o = ((pat * 64 + self.row as usize) * self.nv + v) * 4;
        match self.pats.get(o..o + 4) {
            Some(b) => u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            None => 0,
        }
    }

    // ---- a row ---------------------------------------------------------------------------

    fn row_tick(&mut self, hw: &mut Hw) {
        self.dma = 0;
        self.fx_entry = false;
        self.go(hw, ROW_ENTRY);
        for v in 0..self.nv {
            self.row_voice(hw, v);
            let x = self.ch[v].vol as u16;
            if let Some(t) = self.vol_seg.take() {
                self.seg(hw, t);
            }
            let g = std::mem::take(&mut self.vol_go);
            self.go(hw, g);
            self.vol(hw, v, x);
            self.skip = 0;
            self.extra = 0;
        }
        self.go(hw, 306);
        let mask = self.dma;
        self.dma_on(hw, mask);
        for v in 0..self.nv {
            let (rs, rl) = (self.ch[v].rstart, self.ch[v].rlen);
            self.go(hw, if v == 0 { 260 } else { 208 });
            self.loc(hw, v, rs);
            self.go(hw, if rl <= 1 { 208 } else { 204 });
            self.len(hw, v, rl);
        }
    }

    /// The period lookup of a note in the finetune-0 row: the first entry it is at or above.
    fn lookup0(note: u16) -> usize {
        (0..37).find(|&i| note >= PERIODS[i]).unwrap_or(37)
    }

    fn row_voice(&mut self, hw: &mut Hw, v: usize) {
        let new = self.cell_at(v);
        let prev = self.ch[v].cell;
        let (note, cmd, par) = ((new >> 16) as u16 & 0xFFF, (new >> 8) as u8 & 0xF, new as u8);
        let ins = (((new >> 24) as usize) & 0xF0) | ((new >> 12) as usize & 0xF);
        let e = cmd == 0xE;
        let tp = note != 0 && (cmd == 3 || cmd == 5) && !(e && par >> 4 == 5);
        let ed = note != 0 && e && par >> 4 == 0xD;
        let trig = note != 0 && !tp && !ed;
        self.skip = 0;
        self.extra = 0;
        self.vol_seg = None;
        self.vol_go = 0;
        if prev == 0 {
            self.go(hw, ROW_PREV_PER);
            let p = self.ch[v].period;
            self.per(hw, v, p);
            self.skip = ROW_PREV_PER as i64;
            self.extra = if trig { ROW_PREV_EXTRA_TRIG } else { ROW_PREV_EXTRA } as i64;
        }
        self.ch[v].cell = new;
        let mut rs_set = false;
        if ins != 0 && ins < 32 {
            let s = self.smp[ins];
            let c = &mut self.ch[v];
            c.start = s.start;
            c.len = s.len;
            c.fine = s.fine;
            c.vol = s.vol;
            if s.rep != 0 {
                c.rstart = s.start.wrapping_add(s.rep as u32 * 2);
                c.len = s.rep.wrapping_add(s.replen);
                rs_set = true;
            } else {
                c.rstart = s.start;
            }
            c.rlen = s.replen;
            c.wave = c.rstart;
        }
        let ins_present = ins != 0;
        if note == 0 {
            // (an instrument without a note still sets the voice up)
            if ins_present {
                self.extra += 318 + if rs_set { 62 } else { 0 };
            }
            self.more_row(hw, v, false);
            return;
        }
        if e && par >> 4 == 5 {
            self.ch[v].fine = par & 0xF;
        }
        if tp {
            self.tone_setup(hw, v, ins_present, rs_set);
            self.more_row(hw, v, false);
            return;
        }
        let mut pre = 0u64;
        if cmd == 9 {
            // the sample offset runs before the trigger, too
            let fit = self.sample_offset(v);
            pre = if fit { 237 } else { 225 };
        }
        let fine = self.ch[v].fine as usize & 0xF;
        let i = Self::lookup0(note);
        self.ch[v].period = PERIODS[fine * 37 + i];
        if ed {
            self.more_row(hw, v, false);
            return;
        }
        // trigger
        let total = (if ins_present { 1195 } else { 877 }) + 41 * i as u64 + if ins_present && rs_set { 62 } else { 0 } + pre;
        self.seg(hw, total);
        self.dma_off(hw, 1 << v);
        let c = &mut self.ch[v];
        if c.wavectl & 4 == 0 {
            c.vib_pos = 0;
        }
        if c.wavectl & 0x40 == 0 {
            c.trem_pos = 0;
        }
        let (len, start, period) = (c.len, c.start, c.period);
        self.go(hw, if len <= 1 { 298 } else { 294 });
        self.len(hw, v, len);
        self.go(hw, 182);
        self.loc(hw, v, start);
        if start == 0 {
            let c = &mut self.ch[v];
            c.rstart = 0;
            c.rlen = 1;
            self.go(hw, 210);
            self.len(hw, v, 1);
        }
        self.go(hw, 210);
        self.per(hw, v, period);
        self.dma |= 1 << v;
        self.more_row(hw, v, true);
    }

    /// The sample offset of the cell (9xx): true when it fits in the sample.
    fn sample_offset(&mut self, v: usize) -> bool {
        let c = &mut self.ch[v];
        if c.par() != 0 {
            c.off_mem = c.par();
        }
        let o = (c.off_mem as u32) * 128;
        if o < c.len as u32 {
            c.len -= o as u16;
            c.start = c.start.wrapping_add(2 * o);
            true
        } else {
            c.len = 1;
            false
        }
    }

    /// The tone portamento's setup on a row with a note.
    fn tone_setup(&mut self, hw: &mut Hw, v: usize, ins: bool, rs: bool) {
        let _ = hw;
        let c = &mut self.ch[v];
        let t = (c.fine as usize & 0xF) * 37;
        let mut i = (0..37).find(|&i| c.note() >= PERIODS[t + i]).unwrap_or(35);
        let cost = 44 * i as u64;
        if c.fine & 8 != 0 && i > 0 {
            i -= 1;
        }
        c.tp_target = PERIODS[t + i];
        c.tp_down = false;
        if c.period == c.tp_target {
            c.tp_target = 0;
        } else if (c.period as i16) > c.tp_target as i16 {
            c.tp_down = true;
        }
        self.vol_seg = None;
        let dir = if c.tp_target == 0 { -16 } else if c.tp_down { 16 } else { 0 };
        // (the cost joins the path of the period write that follows)
        self.extra += (cost + if ins { 700 } else { 0 } + if ins && rs { 62 } else { 0 }) as i64 + dir;
    }

    /// "More effects" on a row tick. `trig`: a trigger's writes came before.
    fn more_row(&mut self, hw: &mut Hw, v: usize, trig: bool) {
        let (cmd, par) = (self.ch[v].cmd(), self.ch[v].par());
        match cmd {
            9 => {
                let fit = self.sample_offset(v);
                if trig {
                    self.vol_go = if fit { 660 } else { 626 };
                } else {
                    self.vol_seg = Some(884);
                }
            }
            0xB => {
                // the song end test reads a length byte of the wrong place
                let pos = self.pos;
                if (par as u16) < pos || (par as u16 == pos && pos == (self.b_len.wrapping_sub(1)) as u16) {
                    self.songend = true;
                }
                self.pos = (par as u16).wrapping_sub(1);
                self.break_row = 0;
                self.pos_jump = true;
                if trig {
                    self.vol_go = 686;
                } else {
                    self.vol_seg = Some(1012);
                }
            }
            0xC => {
                self.ch[v].vol = par.min(64);
                if trig {
                    self.vol_go = 644;
                } else {
                    self.vol_seg = Some(884);
                }
            }
            0xD => {
                let x = (par >> 4) as u32 * 10 + (par & 0xF) as u32;
                self.break_row = if x > 64 { 0 } else { x as u8 };
                self.pos_jump = true;
                if trig {
                    self.vol_go = 704;
                } else {
                    self.vol_seg = Some(944);
                }
            }
            0xE => self.e_cmd_row(hw, v, trig),
            0xF => {
                let mut tempo = false;
                if par == 0 {
                } else if self.cia && par >= 0x20 {
                    self.tempo = par as u32;
                    tempo = true;
                } else {
                    self.speed = par;
                    self.counter = 0;
                }
                // (F00 is the cheapest; a tempo needs the timer division)
                if trig {
                    self.vol_go = if tempo { 1188 } else if par == 0 { 608 } else { 692 };
                } else {
                    self.vol_seg = Some(if tempo { 1430 } else if par == 0 { 850 } else { 934 });
                }
            }
            _ => {
                // 0-8 and A: the period is written again
                let p = self.ch[v].period;
                if trig {
                    self.go(hw, 378);
                } else {
                    self.seg(hw, 618);
                }
                self.per(hw, v, p);
                self.vol_go = 392;
            }
        }
    }

    fn e_cmd_row(&mut self, hw: &mut Hw, v: usize, trig: bool) {
        let par = self.ch[v].par();
        let (x, y) = (par >> 4, par & 0xF);
        let tick0 = true;
        let mut seg = 1000u64;
        let mut after = 676u64;
        match x {
            1 | 2 => {
                self.mask = 0x0F;
                if x == 1 {
                    self.porta_up_fx(hw, v, true, trig);
                } else {
                    self.porta_down_fx(hw, v, true, trig);
                }
                return;
            }
            3 => self.ch[v].glissfunk = (self.ch[v].glissfunk & 0xF0) | y,
            4 => self.ch[v].wavectl = (self.ch[v].wavectl & 0xF0) | y,
            5 => self.ch[v].fine = y,
            6 if tick0 => {
                let c = &mut self.ch[v];
                if y == 0 {
                    c.loop_row = self.row & 63;
                } else {
                    let mut jump = true;
                    if c.loop_cnt == 0 {
                        c.loop_cnt = y;
                    } else {
                        c.loop_cnt -= 1;
                        if c.loop_cnt == 0 {
                            jump = false;
                        }
                    }
                    if jump {
                        self.break_row = c.loop_row;
                        self.loop_jump = true;
                    }
                }
            }
            7 => self.ch[v].wavectl = (y << 4) | (self.ch[v].wavectl & 0xF),
            8 => {
                let n = self.karplus(hw, v);
                seg += 22_866 + n;
                after += 22_866 + n;
            }
            9 => {
                // tick 0 with a note: nothing (already triggered)
                if y != 0 && self.ch[v].note() == 0 {
                    // counter 0 mod y: retrigger
                    self.retrigger(hw, v, trig);
                    return;
                }
            }
            0xA => self.ch[v].vol = self.ch[v].vol.saturating_add(y).min(64),
            0xB => self.ch[v].vol = self.ch[v].vol.saturating_sub(y),
            0xC if self.counter == y => self.ch[v].vol = 0,
            0xD if self.counter == y && self.ch[v].note() != 0 => {
                self.retrigger(hw, v, trig);
                return;
            }
            0xE if self.delay_cnt == 0 => self.delay_req = y + 1,
            0xF => {
                self.ch[v].glissfunk = (y << 4) | (self.ch[v].glissfunk & 0xF);
                if y != 0 {
                    self.funk(hw, v);
                    seg += 206;
                    after += 206;
                }
            }
            _ => {}
        }
        let (s, a) = match x {
            0 => (916, 676),
            5 => (982, 676),
            6 => (1114, 942),
            0xB => (1184, 1012),
            0xC => (1174, 676),
            0xE => (1126, 954),
            0xF => (1140 + 206 * (y != 0) as u64, 1312),
            9 => (1244, 922),
            0xD => (2746, 2746),
            8 => (seg, after),
            _ => (seg.min(1000), after),
        };
        if trig {
            self.vol_go = a;
        } else {
            self.vol_seg = Some(s);
        }
    }

    // ---- effect ticks -------------------------------------------------------------------

    fn fx_all(&mut self, hw: &mut Hw) {
        self.fx_entry = true;
        self.go(hw, FX_ENTRY_VOL);
        for v in 0..self.nv {
            self.fx_voice(hw, v);
        }
    }

    fn funk(&mut self, hw: &mut Hw, v: usize) {
        let c = &mut self.ch[v];
        let sp = (c.glissfunk >> 4) as usize;
        if sp == 0 {
            return;
        }
        c.funk_acc = c.funk_acc.wrapping_add(FUNK[sp]);
        if c.funk_acc & 0x80 == 0 {
            return;
        }
        c.funk_acc = 0;
        c.wave = c.wave.wrapping_add(1);
        if c.wave >= c.rstart.wrapping_add(c.rlen as u32 * 2) {
            c.wave = c.rstart;
        }
        let a = c.wave;
        if a != 0 {
            if let Some(b) = self.byte(hw, a) {
                self.put(hw, a, !b);
            }
        }
    }

    /// E8x: a Karplus-Strong pass over the repeat part in sample memory. Returns the byte count.
    fn karplus(&mut self, hw: &mut Hw, v: usize) -> u64 {
        let c = self.ch[v];
        let n = c.rlen as u32 * 2;
        if n == 0 || c.rstart == 0 {
            return 0;
        }
        let at = |i: u32| c.rstart.wrapping_add(i);
        for i in 0..n - 1 {
            let (a, b) = (self.byte(hw, at(i)), self.byte(hw, at(i + 1)));
            if let (Some(a), Some(b)) = (a, b) {
                self.put(hw, at(i), ((a as i8 as i16 + b as i8 as i16) >> 1) as u8);
            }
        }
        if let (Some(a), Some(b)) = (self.byte(hw, at(n - 1)), self.byte(hw, at(0))) {
            self.put(hw, at(n - 1), ((a as i8 as i16 + b as i8 as i16) >> 1) as u8);
        }
        n as u64
    }

    fn fx_voice(&mut self, hw: &mut Hw, v: usize) {
        if self.ch[v].glissfunk >> 4 != 0 {
            self.go(hw, FX_FUNK);
            self.funk(hw, v);
        }
        let c = self.ch[v];
        if c.cell & 0xFFF == 0 {
            self.go(hw, 600);
            self.vol(hw, v, c.vol as u16);
            return;
        }
        let (cmd, par) = (c.cmd(), c.par());
        let mut vol_gap = 342u64;
        let mut wrote_vol = false;
        match cmd {
            0 => {
                let k = self.counter % 3;
                if k == 0 {
                    self.go(hw, 660);
                    self.per(hw, v, c.period);
                } else {
                    let n = if k == 1 { par >> 4 } else { par & 0xF } as usize;
                    let t = (c.fine as usize & 0xF) * 37;
                    if let Some(i) = (0..37).find(|&i| c.period >= PERIODS[t + i]) {
                        let p = self.arp_word(t + i + n);
                        self.go(hw, 883 + 49 * i as u64);
                        self.per(hw, v, p);
                        vol_gap = 284;
                    } else {
                        self.go(hw, 660);
                    }
                }
            }
            1 => self.porta_up_fx(hw, v, false, false),
            2 => self.porta_down_fx(hw, v, false, false),
            3 => {
                if par != 0 {
                    self.ch[v].tp_speed = par;
                    self.ch[v].cell &= !0xFF;
                }
                self.tone_fx(hw, v, 3, par != 0);
            }
            4 => {
                self.vib_cmd(v);
                self.vibrato_fx(hw, v, 4);
            }
            5 => {
                let wrote = self.tone_fx(hw, v, 5, false);
                let up = par >> 4 != 0;
                let lim = self.slide(v);
                self.vol_go += 16 * lim as u64;
                self.vol_go += if wrote { if up { 122 } else { 130 } } else { 166 };
            }
            6 => {
                self.vibrato_fx(hw, v, 6);
                let up = par >> 4 != 0;
                let lim = self.slide(v);
                self.vol_go += if up { 122 } else { 130 } + 16 * lim as u64;
            }
            0xE => {
                self.e_fx(hw, v);
                return;
            }
            _ => {
                self.go(hw, 660);
                self.per(hw, v, c.period);
                if cmd == 7 {
                    let neg = self.ch[v].trem_pos >= 128;
                    let x = self.tremolo(v);
                    self.go(hw, if neg { 684 } else { 702 });
                    self.vol(hw, v, x);
                    wrote_vol = true;
                } else if cmd == 0xA {
                    let up = par >> 4 != 0;
                    let lim = self.slide(v);
                    vol_gap = (if up { 438 } else { 446 }) + 16 * lim as u64;
                }
            }
        }
        if !wrote_vol {
            // (the costs of the effects that set vol_gap themselves are in their helpers)
            let g = if self.vol_go != 0 { std::mem::take(&mut self.vol_go) } else { vol_gap };
            self.go(hw, g);
            let x = self.ch[v].vol as u16;
            self.vol(hw, v, x);
        }
    }

    fn arp_word(&self, idx: usize) -> u16 {
        if idx < 16 * 37 {
            return PERIODS[idx];
        }
        // past the table: the player's variables
        match idx - 16 * 37 {
            0 => self.nv as u16,
            1 => (self.ch[0].cell >> 16) as u16,
            2 => self.ch[0].cell as u16,
            _ => 0,
        }
    }

    fn vib_cmd(&mut self, v: usize) {
        let c = &mut self.ch[v];
        let p = c.par();
        if p & 0xF != 0 {
            c.vib_cmd = (c.vib_cmd & 0xF0) | (p & 0xF);
        }
        if p & 0xF0 != 0 {
            c.vib_cmd = (p & 0xF0) | (c.vib_cmd & 0xF);
        }
    }

    fn porta_up_fx(&mut self, hw: &mut Hw, v: usize, row: bool, trig: bool) {
        let m = self.mask;
        self.mask = 0xFF;
        let c = &mut self.ch[v];
        c.period = c.period.wrapping_sub((c.par() & m) as u16);
        if c.period & 0xFFF < 113 {
            c.period = (c.period & 0xF000) | 113;
        }
        let p = c.period & 0xFFF;
        self.porta_write(hw, v, p, row, trig, 646, 804);
    }

    fn porta_down_fx(&mut self, hw: &mut Hw, v: usize, row: bool, trig: bool) {
        let m = self.mask;
        self.mask = 0xFF;
        let c = &mut self.ch[v];
        c.period = c.period.wrapping_add((c.par() & m) as u16);
        if c.period & 0xFFF > 856 {
            c.period = (c.period & 0xF000) | 856;
        }
        let p = c.period & 0xFFF;
        self.porta_write(hw, v, p, row, trig, 666, 826);
    }

    fn porta_write(&mut self, hw: &mut Hw, v: usize, p: u16, row: bool, trig: bool, fx_cost: u64, row_cost: u64) {
        if row {
            if trig {
                self.go(hw, 564);
            } else {
                self.seg(hw, row_cost);
            }
            self.per(hw, v, p);
            self.vol_go = 392;
        } else {
            self.go(hw, fx_cost);
            self.per(hw, v, p);
            self.vol_go = 284;
        }
    }

    /// Tone portamento on an effect tick; false when it wrote nothing (no target).
    fn tone_fx(&mut self, hw: &mut Hw, v: usize, cmd: u8, set: bool) -> bool {
        let c = &mut self.ch[v];
        if c.tp_target == 0 {
            self.vol_go = 738 + 28 * set as u64;
            return false;
        }
        let sp = c.tp_speed as u16;
        let mut reached = false;
        if c.tp_down {
            c.period = c.period.wrapping_sub(sp);
            if c.period as i16 <= c.tp_target as i16 {
                c.period = c.tp_target;
                c.tp_target = 0;
                reached = true;
            }
        } else {
            c.period = c.period.wrapping_add(sp);
            if c.period as i16 >= c.tp_target as i16 {
                c.period = c.tp_target;
                c.tp_target = 0;
                reached = true;
            }
        }
        let mut p = c.period;
        let gl = c.glissfunk & 0xF != 0;
        let t = (c.fine as usize & 0xF) * 37;
        // (setting a new speed, reaching the target and stepping down each take their time)
        let mut cost = (if cmd == 5 { 786 } else { 750 + 28 * set as u64 }) + 46 * reached as u64 - 10 * (c.tp_down && reached) as u64;
        if gl {
            let i = (0..37).find(|&i| p >= PERIODS[t + i]).unwrap_or(35);
            p = PERIODS[t + i];
            cost += 49 * i as u64;
        }
        self.go(hw, cost);
        self.per(hw, v, p);
        self.vol_go = 308;
        true
    }

    fn vibrato_fx(&mut self, hw: &mut Hw, v: usize, cmd: u8) {
        let c = &mut self.ch[v];
        let pos = (c.vib_pos >> 2) & 0x1F;
        let neg = c.vib_pos >= 128;
        let data = match c.wavectl & 3 {
            0 => VIBRATO[pos as usize] as u16,
            1 if !neg => (pos as u16) << 3,
            1 => 255 - ((pos as u16) << 3),
            _ => 255,
        };
        let data = (data * (c.vib_cmd & 0xF) as u16) >> 7;
        let p = if !neg { c.period.wrapping_add(data) } else { c.period.wrapping_sub(data) };
        c.vib_pos = c.vib_pos.wrapping_add((c.vib_cmd >> 2) & 0x3C);
        // (a non-zero nibble of the parameter replaces the command's, which takes time)
        let nibbles = if cmd == 4 { (self.ch[v].par() >> 4 != 0) as u64 + (self.ch[v].par() & 0xF != 0) as u64 } else { 0 };
        let base = (if cmd == 4 { 846 } else { 880 }) + 47 * nibbles;
        self.go(hw, if neg { base - 10 } else { base });
        self.per(hw, v, p);
        self.vol_go = 358;
    }

    fn tremolo(&mut self, v: usize) -> u16 {
        let c = &mut self.ch[v];
        let p = c.par();
        if p & 0xF != 0 {
            c.trem_cmd = (c.trem_cmd & 0xF0) | (p & 0xF);
        }
        if p & 0xF0 != 0 {
            c.trem_cmd = (p & 0xF0) | (c.trem_cmd & 0xF);
        }
        let pos = (c.trem_pos >> 2) & 0x1F;
        let data = match (c.wavectl >> 4) & 3 {
            0 => VIBRATO[pos as usize] as i32,
            // the ramp tests the vibrato's position
            1 if c.vib_pos < 128 => (pos as i32) << 3,
            1 => 255 - ((pos as i32) << 3),
            _ => 255,
        };
        let data = (data * (c.trem_cmd & 0xF) as i32) >> 6;
        let x = if c.trem_pos < 128 { c.vol as i32 + data } else { c.vol as i32 - data };
        c.trem_pos = c.trem_pos.wrapping_add((c.trem_cmd >> 2) & 0x3C);
        x.clamp(0, 64) as u16
    }

    /// Volume slide; true when the volume stopped at its limit (which takes longer).
    fn slide(&mut self, v: usize) -> bool {
        let c = &mut self.ch[v];
        let p = c.par();
        if p >> 4 != 0 {
            let n = c.vol as u32 + (p >> 4) as u32;
            c.vol = n.min(64) as u8;
            n > 64
        } else {
            let lim = (c.vol as u32) < (p & 0xF) as u32;
            c.vol = c.vol.saturating_sub(p & 0xF);
            lim
        }
    }

    /// An E command on an effect tick (or a delayed row's).
    fn e_fx(&mut self, hw: &mut Hw, v: usize) {
        let par = self.ch[v].par();
        let (x, y) = (par >> 4, par & 0xF);
        let tick0 = self.counter == 0;
        let has_note = self.ch[v].note() != 0;
        let mut base = match x {
            0 => 886,
            1 => 872,
            2 => 892,
            5 => 950,
            6 => 968,
            9 if tick0 && has_note => 1132,
            9 => 1244,
            0xA => 1064 + 86 * tick0 as u64,
            0xB => 1082 + 80 * tick0 as u64,
            0xC => 1128,
            0xD => 1146,
            0xE => 1024 + 52 * (tick0 && self.delay_cnt == 0) as u64,
            0xF => 1140,
            _ => 950,
        };
        match x {
            1 if tick0 => {
                self.mask = 0x0F;
                return self.porta_up_fx(hw, v, false, false);
            }
            2 if tick0 => {
                self.mask = 0x0F;
                return self.porta_down_fx(hw, v, false, false);
            }
            3 => self.ch[v].glissfunk = (self.ch[v].glissfunk & 0xF0) | y,
            4 => self.ch[v].wavectl = (self.ch[v].wavectl & 0xF0) | y,
            5 => self.ch[v].fine = y,
            6 if tick0 => {
                let c = &mut self.ch[v];
                if y == 0 {
                    c.loop_row = self.row & 63;
                } else {
                    let mut jump = true;
                    if c.loop_cnt == 0 {
                        c.loop_cnt = y;
                    } else {
                        c.loop_cnt -= 1;
                        if c.loop_cnt == 0 {
                            jump = false;
                        }
                    }
                    if jump {
                        self.break_row = c.loop_row;
                        self.loop_jump = true;
                    }
                }
            }
            7 => self.ch[v].wavectl = (y << 4) | (self.ch[v].wavectl & 0xF),
            8 => {
                let n = self.karplus(hw, v);
                base += 22_866 + n;
            }
            9 if y != 0 && !(tick0 && has_note) && self.counter % y == 0 => return self.retrigger(hw, v, false),
            0xA if tick0 => self.ch[v].vol = self.ch[v].vol.saturating_add(y).min(64),
            0xB if tick0 => self.ch[v].vol = self.ch[v].vol.saturating_sub(y),
            0xC if self.counter == y => self.ch[v].vol = 0,
            0xD if self.counter == y && has_note => return self.retrigger(hw, v, false),
            0xE if tick0 && self.delay_cnt == 0 => self.delay_req = y + 1,
            0xF if tick0 => {
                self.ch[v].glissfunk = (y << 4) | (self.ch[v].glissfunk & 0xF);
                if y != 0 {
                    self.funk(hw, v);
                }
            }
            _ => {}
        }
        self.go(hw, base);
        let x = self.ch[v].vol as u16;
        self.vol(hw, v, x);
    }

    /// The retrigger of E9x / EDx; `row`: on a row tick (the voice's volume follows).
    fn retrigger(&mut self, hw: &mut Hw, v: usize, row: bool) {
        let c = self.ch[v];
        let from_vol = !row;
        let ed = c.par() >> 4 == 0xD;
        if from_vol {
            self.go(hw, if ed { 1082 } else { 1120 });
        } else {
            self.go(hw, 600);
        }
        self.dma_off(hw, 1 << v);
        self.go(hw, 190);
        self.loc(hw, v, c.start);
        self.go(hw, if c.len <= 1 { 208 } else { 204 });
        self.len(hw, v, c.len);
        self.go(hw, 186);
        self.per(hw, v, c.period);
        self.go(hw, 250);
        self.dma_on(hw, 1 << v);
        self.go(hw, 224);
        self.loc(hw, v, c.rstart);
        self.go(hw, if c.rlen <= 1 { 208 } else { 204 });
        self.len(hw, v, c.rlen);
        self.go(hw, 186);
        self.per(hw, v, c.period);
        self.go(hw, 312);
        let x = c.vol as u16;
        self.vol(hw, v, x);
    }
}

impl Replayer for Pt4 {
    fn format(&self) -> &'static str {
        "Protracker4"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick(&mut self, hw: &mut Hw) {
        Pt4::tick(self, hw);
    }

    fn tick_clocks(&self) -> u64 {
        5 * (1_773_447 / self.tempo.max(1) as u64 + 1)
    }

    /// The first interrupt follows the start by about 72 000 colour clocks in the host.
    fn timer_delay(&self) -> u64 {
        655
    }

    fn songend(&self) -> bool {
        self.songend
    }
}

#[cfg(test)]
#[path = "protracker4_tests.rs"]
mod tests;

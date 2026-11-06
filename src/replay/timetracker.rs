//! TimeTracker (`TMK.` song files with an `SMP.` sample companion, or the single-file `.tmk`
//! form of the public collections), the replayer of the Eremation demo "We're Still Alive"
//! (1993), played by the Eagleplayer "TimeTracker".
//!
//! Clean-room, written from a behavioural specification (the pattern semantics of a
//! ProTracker 2.x style replayer with its own packed module format, a fixed 50 Hz tick,
//! its own deviations in the effects, its own register-write order and raster-line waits
//! for the DMA restart) and from black-box runs of the reference player's write logs. No
//! original player code; nothing embedded: the period, sine and funk tables are the
//! standard ProTracker ones the built-in ProTracker already holds.
//!
//! The module is interpreted from its data. Sample memory is prepared once at the start
//! (delta decoding of the whole sample area, the first four bytes of each sample zeroed)
//! and then played in place by Paula. The tick writes the registers in the original's
//! order; the colour clocks between the writes are the measured ones (`GAPS`, by the
//! previous write, the write and the code path that makes it).
//!
//! Choices where the specification leaves a case undefined (never seen in a module): a
//! sample number above the sample count is ignored; an effect-only entry byte above 0xCF
//! is an empty effect (its parameter byte is consumed); a period or arpeggio read past the
//! end of the period table writes nothing; a tone portamento target past the end of the
//! table is no target; row data past the end of the file reads as zero bytes.

use super::{Hw, Open, Replayer, DT_TICK};
use crate::machine::Machine;
use crate::paula::LINE;
use crate::protracker::{FUNK, PERIODS, VIBRATO};

pub const OPEN: Open = Open { name: "TimeTracker", detect, new };

/// The song structure of a file, if it is one (the strict single-file test of the
/// specification, relaxed for the two-file form: the sample data may be missing).
struct Layout {
    /// Start offset of each subsong's block.
    blocks: Vec<usize>,
    /// Sample entries of the first block (offset of the first entry).
    smp_at: usize,
    nsamples: usize,
    delta: bool,
    total: usize,
    /// End of the row data of the furthest row, over all subsongs.
    data_end: usize,
}

fn rb(d: &[u8], o: usize) -> u8 {
    d.get(o).copied().unwrap_or(0)
}

fn rw(d: &[u8], o: usize) -> u16 {
    (rb(d, o) as u16) << 8 | rb(d, o + 1) as u16
}

/// Size of a row entry starting with byte `b`.
fn entry_size(b: u8) -> usize {
    if b == 0x80 {
        1
    } else if b >= 0x80 {
        2
    } else {
        3
    }
}

fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < 8 || &d[0..3] != b"TMK" || d[3] == 0 {
        return None;
    }
    let n = d[3] as usize;
    let mut blocks = Vec::new();
    let mut at = 4 + 2 * (n - 1);
    for i in 0..n {
        if at + 2 > d.len() {
            return None;
        }
        blocks.push(at);
        if i + 1 < n {
            at += rw(d, 4 + 2 * i) as usize;
        }
    }
    let mut nsamples = 0;
    let mut delta = false;
    let mut total = 0usize;
    let mut smp_at = 0;
    let mut data_end = 0usize;
    for (i, &b) in blocks.iter().enumerate() {
        let len = rb(d, b) as usize;
        let c = (rb(d, b + 1) & 0x7F) as usize;
        if len == 0 || len > 128 || c == 0 || c > 31 {
            return None;
        }
        if i == 0 {
            nsamples = c;
            delta = rb(d, b + 1) & 0x80 != 0;
            smp_at = b + 2;
            for k in 0..c {
                let o = b + 2 + 8 * k;
                let (fine, vol) = (rb(d, o + 6), rb(d, o + 7));
                if fine > 15 || vol > 64 {
                    return None;
                }
                total += rw(d, o) as usize * 2;
            }
        }
        let pos = b + 2 + 8 * c;
        let pad = len + (len & 1);
        let mut high = 0usize;
        for k in 0..len {
            high = high.max(rb(d, pos + k) as usize / 2);
        }
        let rowtab = pos + pad;
        let rows = rowtab + (high + 1) * 128;
        if rows > d.len() {
            return None;
        }
        for r in 0..(high + 1) * 64 {
            let mut p = rowtab + rw(d, rowtab + 2 * r) as usize;
            for _ in 0..4 {
                if p >= d.len() {
                    return None;
                }
                p += entry_size(d[p]);
            }
            if p > d.len() {
                return None;
            }
            data_end = data_end.max(p);
        }
    }
    Some(Layout { blocks, smp_at, nsamples, delta, total, data_end })
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

/// Whether `d` is a TimeTracker song (the engine leaves it unconverted for the open replayer).
pub fn is_timetracker(d: &[u8]) -> bool {
    detect(d)
}

#[derive(Clone, Copy, Default)]
struct Samp {
    len: u16,
    rep: u16,
    replen: u16,
    fine: u8,
    vol: u8,
    addr: u32,
}

#[derive(Clone, Copy, Default)]
struct Ch {
    note_per: u16,
    effect: u8,
    param: u8,
    start: u32,
    len: u16,
    orig_len: u16,
    loop_start: u32,
    loop_len: u16,
    funk_ptr: u32,
    per: u16,
    fine: u8,
    vol: u8,
    tp_target: u16,
    tp_down: bool,
    tp_speed: u8,
    vib_cmd: u8,
    vib_pos: u8,
    trem_cmd: u8,
    trem_pos: u8,
    wave: u8,
    gf: u8,
    funk_cnt: u8,
    smp_off: u8,
    loop_row: u8,
    loop_cnt: u8,
}

// write kinds (key = kind * 4 + voice)
const K_LOC: u8 = 0;
const K_LEN: u8 = 1;
const K_PER: u8 = 2;
const K_VOL: u8 = 3;
const K_OFF: u8 = 4;
const K_ON: u8 = 5;
const K_LED: u8 = 6;
const K_WAIT: u8 = 7;
const START: u8 = 255;
/// Colour clocks from the host's LED write at the start to the first tick.
const FIRST_TICK: u64 = 71584;

/// The flat period table: 16 rows of 36, as the built-in ProTracker's rows of 37 without the zero.
fn flat() -> [u16; 576] {
    let mut t = [0u16; 576];
    for r in 0..16 {
        for i in 0..36 {
            t[r * 36 + i] = PERIODS[r * 37 + i];
        }
    }
    t
}

pub struct TimeTracker {
    d: Vec<u8>,
    layout: Layout,
    sub: usize,
    samples: Vec<Samp>,
    tab: [u16; 576],
    // current subsong
    positions: Vec<u8>,
    rowtab: usize,
    ch: [Ch; 4],
    speed: u8,
    counter: u8,
    pos: usize,
    row: usize,
    break_row: usize,
    jump: bool,
    loop_flag: bool,
    delay_req: u8,
    delay_cnt: u8,
    dma_mask: u16,
    mask: u8,
    songend: bool,
    // write timing
    prev: u8,
    next_at: Option<u64>,
    cv: usize,
    form: [u8; 4],
}

/// Where the sample data comes from: the song file itself (single-file form) or the companion.
fn sample_area(d: &[u8], l: &Layout, m: &mut Machine, base: u32) -> Option<u32> {
    let total = l.total;
    let single = d.len() >= l.data_end + total;
    let src: Vec<u8> = if single {
        d[d.len() - total..].to_vec()
    } else {
        let file = m.bus.read_cstr(m.deli.file_buf);
        let comp = companion(&file)?;
        let s = crate::deli::read_companion(m, &comp).1.ok()?;
        if s.len() < total {
            return None;
        }
        s[..total].to_vec()
    };
    let mut s = src;
    if l.delta {
        // decoded from the last byte towards the first: each earlier byte plus the decoded one after it
        for i in (0..s.len().saturating_sub(1)).rev() {
            s[i] = s[i].wrapping_add(s[i + 1]);
        }
    }
    // the first four bytes of every sample are zero
    let mut at = 0usize;
    for k in 0..l.nsamples {
        let o = l.smp_at + 8 * k;
        for j in 0..4 {
            if at + j < s.len() {
                s[at + j] = 0;
            }
        }
        at += rw(d, o) as usize * 2;
    }
    let addr = if single {
        base + (d.len() - total) as u32
    } else {
        let a = m.alloc((total as u32).max(4) + 8);
        if a == 0 {
            return None;
        }
        a
    };
    m.bus.write_bytes(addr, &s);
    Some(addr)
}

/// `SMP.name` for `TMK.name`, `name.SAM` for `name.pat` (the prefix and suffix in any case).
fn companion(file: &str) -> Option<String> {
    let file = file.rsplit(['/', ':']).next().unwrap_or(file);
    let lower = file.to_ascii_lowercase();
    if lower.starts_with("tmk.") && file.is_char_boundary(4) {
        return Some(format!("SMP.{}", &file[4..]));
    }
    if lower.ends_with(".pat") {
        return Some(format!("{}.SAM", &file[..file.len() - 4]));
    }
    None
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    let area = sample_area(d, &l, m, base)?;
    let mut samples = Vec::new();
    let mut at = area;
    for k in 0..l.nsamples {
        let o = l.smp_at + 8 * k;
        let s = Samp { len: rw(d, o), rep: rw(d, o + 2), replen: rw(d, o + 4), fine: rb(d, o + 6), vol: rb(d, o + 7), addr: at };
        at = at.wrapping_add(s.len as u32 * 2);
        samples.push(s);
    }
    let mut t = TimeTracker {
        d: d.to_vec(),
        layout: l,
        sub: 0,
        samples,
        tab: flat(),
        positions: Vec::new(),
        rowtab: 0,
        ch: [Ch::default(); 4],
        speed: 6,
        counter: 0,
        pos: 0,
        row: 0,
        break_row: 0,
        jump: false,
        loop_flag: false,
        delay_req: 0,
        delay_cnt: 0,
        dma_mask: 0,
        mask: 0,
        songend: false,
        prev: START,
        next_at: None,
        cv: 4,
        form: [0; 4],
    };
    t.select(0);
    // the host's start, as the reference logs show it: two passes of the four volumes
    // (voice 3 shows a leftover byte there; DMA is off) and then the LED switched off
    let mut hw = Hw { bus: &mut m.bus };
    let t0 = hw.bus.clock;
    for (k, off) in [0u64, 128, 268, 403, 648, 776, 916, 1051].iter().enumerate() {
        hw.bus.clock = t0 + off;
        hw.vol(k & 3, if k & 3 == 3 { 0x65 } else { 0 });
    }
    hw.bus.clock = t0 + 1837;
    hw.bus.write_byte_led(0x02);
    Some(Box::new(t))
}

trait LedWrite {
    fn write_byte_led(&mut self, v: u8);
}

impl LedWrite for crate::bus::Bus {
    fn write_byte_led(&mut self, v: u8) {
        use m68k::AddressBus;
        self.write_byte(0xBFE001, v);
    }
}

impl TimeTracker {
    /// Subsong block `n` selected: the whole player state starts over.
    fn select(&mut self, n: usize) {
        let n = n.min(self.layout.blocks.len() - 1);
        self.sub = n;
        let b = self.layout.blocks[n];
        let len = rb(&self.d, b) as usize;
        let c = (rb(&self.d, b + 1) & 0x7F) as usize;
        let pos = b + 2 + 8 * c;
        self.positions = (0..len).map(|k| rb(&self.d, pos + k) / 2).collect();
        self.rowtab = pos + len + (len & 1);
        self.ch = [Ch::default(); 4];
        self.speed = 6;
        self.counter = 0;
        self.pos = 0;
        self.row = 0;
        self.break_row = 0;
        self.jump = false;
        self.loop_flag = false;
        self.delay_req = 0;
        self.delay_cnt = 0;
        self.dma_mask = 0;
        self.mask = 0;
        self.songend = false;
        self.prev = START;
    }

    // ---- write timing

    fn gap(&self, key: u8, ctx: u8) -> u64 {
        if self.prev == START {
            // the first volume write of a tick comes 265 colour clocks after the host's call
            return 265;
        }
        let f = |v: usize| if v < 4 { (self.ch[v].effect, self.form[v]) } else { (0, 0) };
        let (a, b) = (f(self.cv), f(self.cv.wrapping_sub(1)));
        let fields = [self.prev, key, ctx, a.0, a.1, b.0, b.1];
        for lvl in (0..4).rev() {
            let n = [2, 3, 5, 7][lvl];
            if let Ok(i) = GAPS.binary_search_by_key(&pack(lvl as u64, &fields[..n]), |e| e.0) {
                return GAPS[i].1 as u64;
            }
        }
        match key >> 2 {
            K_LOC => 0,
            K_LEN => 6,
            K_PER => 150,
            K_VOL => 200,
            _ => 60,
        }
    }

    /// Lets the colour clocks pass that the original's code takes from the previous write to
    /// this one (`ctx` is the code path), and makes it the previous write.
    fn at(&mut self, hw: &mut Hw, kind: u8, v: usize, ctx: u8) {
        let key = kind * 4 + if kind <= K_VOL { v as u8 } else { 0 };
        let g = self.gap(key, ctx);
        hw.wait(g);
        self.prev = key;
    }

    fn w_loc(&mut self, hw: &mut Hw, v: usize, a: u32, ctx: u8) {
        self.at(hw, K_LOC, v, ctx);
        hw.loc(v, a);
    }

    fn w_len(&mut self, hw: &mut Hw, v: usize, n: u16, ctx: u8) {
        self.at(hw, K_LEN, v, ctx);
        hw.len(v, n);
    }

    fn w_per(&mut self, hw: &mut Hw, v: usize, p: u16, ctx: u8) {
        self.at(hw, K_PER, v, ctx);
        hw.per(v, p);
    }

    fn w_vol(&mut self, hw: &mut Hw, v: usize, vol: u8, ctx: u8) {
        self.at(hw, K_VOL, v, ctx);
        hw.vol(v, vol as u16);
    }

    fn w_off(&mut self, hw: &mut Hw, mask: u16, ctx: u8) {
        self.at(hw, K_OFF, 0, ctx);
        hw.dmacon(mask);
    }

    fn w_on(&mut self, hw: &mut Hw, mask: u16, tail: u64, ctx: u8) {
        self.at(hw, K_WAIT, 0, ctx);
        raster_wait(hw, tail);
        self.prev = K_ON * 4;
        hw.dmacon(0x8000 | mask);
    }

    fn w_loc_after_wait(&mut self, hw: &mut Hw, v: usize, a: u32, tail: u64) {
        // (the wait began 31 colour clocks after the DMA-on write)
        hw.wait(31);
        // (measured: the end lies a line boundary nine lines on plus the tail and a phase of
        // the 13-clock poll round that follows from where the wait began in its line)
        let t = hw.bus.clock;
        hw.bus.clock = (t / LINE + 9) * LINE + tail + (t % LINE + 11) % 13;
        self.prev = K_LOC * 4 + v as u8;
        hw.loc(v, a);
    }

    // ---- the tick

    fn tick_inner(&mut self, hw: &mut Hw) {
        self.prev = START;
        self.cv = 4;
        self.form = [0; 4];
        for v in 0..4 {
            let vol = self.ch[v].vol;
            self.w_vol(hw, v, vol, C_REFRESH);
        }
        self.counter = self.counter.wrapping_add(1);
        if self.counter < self.speed {
            for v in 0..4 {
                self.effect_tick(v, hw);
            }
            return;
        }
        self.counter = 0;
        if self.delay_cnt != 0 {
            for v in 0..4 {
                self.effect_tick(v, hw);
            }
            self.advance();
            return;
        }
        self.dma_mask = 0;
        let pat = self.positions.get(self.pos).copied().unwrap_or(0) as usize;
        let ent = self.rowtab + pat * 128 + self.row.min(63) * 2;
        let mut p = self.rowtab + rw(&self.d, ent) as usize;
        for v in 0..4 {
            self.row_voice(v, &mut p, hw);
        }
        self.advance();
        self.cv = 4;
        self.dma_restart(hw);
    }

    fn advance(&mut self) {
        self.row += 1;
        if self.delay_req != 0 {
            self.delay_cnt = self.delay_req;
            self.delay_req = 0;
        }
        if self.delay_cnt != 0 {
            self.delay_cnt -= 1;
            if self.delay_cnt != 0 {
                self.row -= 1;
            }
        }
        if self.loop_flag {
            self.loop_flag = false;
            self.row = self.break_row;
            self.break_row = 0;
        }
        if self.row >= 64 {
            self.next_position();
        }
        let mut guard = 0;
        while self.jump && guard < 4 {
            self.next_position();
            guard += 1;
        }
    }

    fn next_position(&mut self) {
        self.row = self.break_row;
        self.break_row = 0;
        self.jump = false;
        self.pos = (self.pos + 1) & 0xFF;
        if self.pos >= self.positions.len() {
            self.pos = 0;
            self.songend = true;
        }
    }

    fn dma_restart(&mut self, hw: &mut Hw) {
        let mask = self.dma_mask;
        self.w_on(hw, mask, 54, C_RESTART);
        for v in 0..4 {
            let (ls, ll) = (self.ch[v].loop_start, self.ch[v].loop_len);
            if v == 0 {
                self.w_loc_after_wait(hw, 0, ls, 61);
            } else {
                self.w_loc(hw, v, ls, C_LOOP);
            }
            self.w_len(hw, v, ll, C_LOOP);
        }
    }

    // ---- the row tick

    fn row_voice(&mut self, v: usize, p: &mut usize, hw: &mut Hw) {
        self.cv = v;
        self.form[v] = 0;
        let c = self.ch[v];
        if c.note_per == 0 && c.effect == 0 && c.param == 0 {
            self.w_per(hw, v, c.per, C_ROW_PER);
        }
        let b = rb(&self.d, *p);
        *p += 1;
        let (note, smp, eff, par);
        if b == 0x80 {
            let c = &mut self.ch[v];
            c.effect = 0;
            c.param = 0;
            c.note_per = 0;
            self.form[v] = 0;
            self.row_fx(v, hw);
            return;
        } else if b >= 0xC0 {
            let y = rb(&self.d, *p);
            *p += 1;
            let c = &mut self.ch[v];
            c.effect = if b <= 0xCF { b & 15 } else { 0 };
            c.param = if b <= 0xCF { y } else { 0 };
            c.note_per = 0;
            self.form[v] = 1;
            self.row_fx(v, hw);
            return;
        } else if b >= 0x80 {
            note = b & 0x3F;
            smp = rb(&self.d, *p) as usize;
            *p += 1;
            eff = 0;
            par = 0;
            self.form[v] = 2;
        } else {
            let x = rb(&self.d, *p);
            let y = rb(&self.d, *p + 1);
            *p += 2;
            note = b >> 1;
            smp = (b & 1) as usize * 16 + (x >> 4) as usize;
            eff = x & 15;
            par = y;
            self.form[v] = 3;
        }
        if smp != 0 {
            self.form[v] |= 4;
        }
        if note != 0 {
            self.form[v] |= 8;
        }
        {
            let c = &mut self.ch[v];
            c.effect = eff;
            c.param = par;
        }
        if smp != 0 && smp <= self.samples.len() {
            let s = self.samples[smp - 1];
            let c = &mut self.ch[v];
            c.start = s.addr;
            c.len = s.len;
            c.orig_len = s.len;
            c.fine = s.fine;
            c.vol = s.vol;
            if s.rep != 0 {
                c.loop_start = s.addr.wrapping_add(2 * s.rep as u32);
                c.loop_len = s.replen;
                c.len = s.rep.wrapping_add(s.replen);
            } else {
                c.loop_start = s.addr;
                c.loop_len = s.replen;
            }
            c.funk_ptr = c.loop_start;
            let vol = c.vol;
            self.w_vol(hw, v, vol, C_SMP_VOL);
        }
        let nper = if note != 0 { self.tab.get(note as usize - 1).copied().unwrap_or(0) } else { 0 };
        self.ch[v].note_per = nper;
        if note == 0 {
            self.row_fx(v, hw);
            return;
        }
        if eff == 0xE && par >> 4 == 5 {
            self.ch[v].fine = par & 15;
        }
        if eff == 3 || eff == 5 {
            self.tone_setup(v, nper);
            self.row_fx(v, hw);
            return;
        }
        if eff == 9 {
            self.row_fx(v, hw);
        }
        let f = self.ch[v].fine as usize & 15;
        match self.tab.get(f * 36 + note as usize - 1).copied() {
            Some(x) => self.ch[v].per = x,
            None => {}
        }
        if eff == 0xE && par >> 4 == 0xD {
            self.row_fx(v, hw);
            return;
        }
        let bit = 1u16 << v;
        self.w_off(hw, bit, C_NOTE);
        {
            let c = &mut self.ch[v];
            if c.wave & 4 == 0 {
                c.vib_pos = 0;
            }
            if c.wave & 0x40 == 0 {
                c.trem_pos = 0;
            }
        }
        let c = self.ch[v];
        self.w_loc(hw, v, c.start, C_NOTE);
        self.w_len(hw, v, c.len, C_NOTE);
        self.w_per(hw, v, c.per, C_NOTE);
        self.dma_mask |= bit;
        self.row_fx(v, hw);
    }

    /// Step 6 of a row tick: the funk step and the row effect.
    fn row_fx(&mut self, v: usize, hw: &mut Hw) {
        if self.ch[v].gf >> 4 != 0 {
            self.funk_step(v, hw);
        }
        let (e, par) = (self.ch[v].effect, self.ch[v].param);
        match e {
            9 => self.sample_offset(v),
            0xB => {
                self.pos = (par as usize).wrapping_sub(1) & 0xFF;
                self.break_row = 0;
                self.jump = true;
                self.songend = true;
            }
            0xC => {
                let vol = par.min(64);
                self.ch[v].vol = vol;
                self.w_vol(hw, v, vol, C_FX);
            }
            0xD => {
                let r = 10 * (par >> 4) as usize + (par & 15) as usize;
                self.break_row = if r > 63 { 0 } else { r };
                self.jump = true;
            }
            0xE => self.ext(v, 0, hw),
            0xF => {
                if par != 0 {
                    self.speed = par;
                    self.counter = 0;
                }
            }
            _ => {}
        }
    }

    fn sample_offset(&mut self, v: usize) {
        let c = &mut self.ch[v];
        if c.param != 0 {
            c.smp_off = c.param;
        }
        let off = c.smp_off as u16 * 128;
        if off as i16 >= c.len as i16 {
            c.len = 1;
        } else {
            c.len -= off;
            c.start = c.start.wrapping_add(2 * off as u32);
        }
    }

    // ---- the effect tick

    fn effect_tick(&mut self, v: usize, hw: &mut Hw) {
        self.cv = v;
        self.form[v] = 0;
        if self.ch[v].gf >> 4 != 0 {
            self.funk_step(v, hw);
        }
        let c = self.ch[v];
        if c.effect == 0 && c.param == 0 {
            self.w_per(hw, v, c.per, C_IDLE);
            return;
        }
        match c.effect {
            0 => self.arpeggio(v, hw),
            1 => self.porta(v, true, hw),
            2 => self.porta(v, false, hw),
            3 => self.tone_porta(v, true, hw),
            4 => self.vibrato(v, true, hw),
            5 => {
                self.tone_porta(v, false, hw);
                self.vol_slide(v, hw);
            }
            6 => {
                self.vibrato(v, false, hw);
                self.vol_slide(v, hw);
            }
            7 => self.tremolo(v, hw),
            0xA => self.vol_slide(v, hw),
            0xE => self.ext(v, self.counter, hw),
            _ => {
                let p = self.ch[v].per;
                self.w_per(hw, v, p, C_FX);
            }
        }
    }

    fn arpeggio(&mut self, v: usize, hw: &mut Hw) {
        let c = self.ch[v];
        let ctx = C_FX;
        match self.counter % 3 {
            0 => self.w_per(hw, v, c.per, ctx),
            n => {
                let off = if n == 1 { c.param >> 4 } else { c.param & 15 } as usize;
                let base = c.fine as usize * 36;
                for i in 0..37 {
                    let Some(&x) = self.tab.get(base + i) else { break };
                    if x <= c.per {
                        if let Some(&y) = self.tab.get(base + i + off) {
                            self.w_per(hw, v, y, ctx);
                        }
                        break;
                    }
                }
            }
        }
    }

    fn porta(&mut self, v: usize, up: bool, hw: &mut Hw) {
        let amount = (self.ch[v].param & self.mask) as u16;
        self.mask = 0xFF;
        let c = &mut self.ch[v];
        if up {
            c.per = c.per.wrapping_sub(amount);
            if c.per & 0xFFF < 113 {
                c.per = c.per & 0xF000 | 113;
            }
        } else {
            c.per = c.per.wrapping_add(amount);
            if c.per & 0xFFF >= 856 {
                c.per = c.per & 0xF000 | 856;
            }
        }
        let p = c.per & 0xFFF;
        self.w_per(hw, v, p, C_FX);
    }

    fn tone_setup(&mut self, v: usize, nper: u16) {
        let f = self.ch[v].fine as usize & 15;
        let base = 37 * f;
        let mut off = 35usize;
        for i in 0..37 {
            match self.tab.get(base + i) {
                Some(&x) if x <= nper => {
                    off = i;
                    break;
                }
                _ => {}
            }
        }
        if f >= 8 && off != 0 {
            off -= 1;
        }
        let c = &mut self.ch[v];
        match self.tab.get(base + off) {
            Some(&t) => {
                c.tp_target = if t == c.per { 0 } else { t };
                c.tp_down = t < c.per;
            }
            None => c.tp_target = 0,
        }
    }

    fn tone_porta(&mut self, v: usize, set_speed: bool, hw: &mut Hw) {
        let c = &mut self.ch[v];
        if set_speed && c.param != 0 {
            c.tp_speed = c.param;
            c.param = 0;
        }
        if c.tp_target == 0 {
            return;
        }
        let sp = c.tp_speed as i32;
        let mut per = c.per as i32;
        let t = c.tp_target as i32;
        if c.tp_down {
            per -= sp;
            if per <= t {
                per = t;
                c.tp_target = 0;
            }
        } else {
            per += sp;
            if per >= t {
                per = t;
                c.tp_target = 0;
            }
        }
        c.per = per as u16;
        let mut w = c.per;
        if c.gf & 15 != 0 {
            let base = c.fine as usize * 36;
            w = (0..36).find_map(|i| self.tab.get(base + i).copied().filter(|&x| x <= c.per)).unwrap_or(self.tab.get(base + 35).copied().unwrap_or(c.per));
        }
        self.w_per(hw, v, w, C_FX);
    }

    fn wave_val(&self, w: u8, pos: u8, vib_pos: u8) -> u32 {
        let idx = (pos >> 2) as usize & 31;
        match w & 3 {
            0 => VIBRATO[idx] as u32,
            1 => {
                if vib_pos & 0x80 != 0 {
                    255 - (idx as u32 * 8)
                } else {
                    idx as u32 * 8
                }
            }
            _ => 255,
        }
    }

    fn vibrato(&mut self, v: usize, set: bool, hw: &mut Hw) {
        let c = &mut self.ch[v];
        if set {
            let (x, y) = (c.param >> 4, c.param & 15);
            if x != 0 {
                c.vib_cmd = c.vib_cmd & 15 | x << 4;
            }
            if y != 0 {
                c.vib_cmd = c.vib_cmd & 0xF0 | y;
            }
        }
        let c = self.ch[v];
        let val = self.wave_val(c.wave, c.vib_pos, c.vib_pos);
        let amount = (val * (c.vib_cmd & 15) as u32 >> 7) as u16;
        let w = if c.vib_pos & 0x80 != 0 { c.per.wrapping_sub(amount) } else { c.per.wrapping_add(amount) };
        self.w_per(hw, v, w, C_FX);
        let c = &mut self.ch[v];
        c.vib_pos = c.vib_pos.wrapping_add((c.vib_cmd >> 4).wrapping_mul(4));
    }

    fn vol_slide(&mut self, v: usize, hw: &mut Hw) {
        let c = self.ch[v];
        self.w_per(hw, v, c.per, C_FX);
        let (x, y) = (c.param >> 4, c.param & 15);
        let vol = if x != 0 { (c.vol + x).min(64) } else { c.vol.saturating_sub(y) };
        self.ch[v].vol = vol;
        self.w_vol(hw, v, vol, C_FX);
    }

    fn tremolo(&mut self, v: usize, hw: &mut Hw) {
        let c = self.ch[v];
        self.w_per(hw, v, c.per, C_FX);
        let c = &mut self.ch[v];
        let (x, y) = (c.param >> 4, c.param & 15);
        if x != 0 {
            c.trem_cmd = c.trem_cmd & 15 | x << 4;
        }
        if y != 0 {
            c.trem_cmd = c.trem_cmd & 0xF0 | y;
        }
        let c = self.ch[v];
        let val = self.wave_val(c.wave >> 4, c.trem_pos, c.vib_pos);
        let amount = (val * (c.trem_cmd & 15) as u32 >> 6) as i32;
        let vol = if c.trem_pos & 0x80 != 0 { c.vol as i32 - amount } else { c.vol as i32 + amount };
        let vol = vol.clamp(0, 64) as u8;
        self.w_vol(hw, v, vol, C_FX);
        let c = &mut self.ch[v];
        c.trem_pos = c.trem_pos.wrapping_add((c.trem_cmd >> 4).wrapping_mul(4));
    }

    // ---- extended effects

    fn ext(&mut self, v: usize, counter: u8, hw: &mut Hw) {
        let par = self.ch[v].param;
        let x = par & 15;
        match par >> 4 {
            0 => {
                self.at(hw, K_LED, 0, C_FX);
                hw.bus.write_byte_led(0);
                hw.wait(10);
                hw.bus.write_byte_led((x & 1) << 1);
                self.prev = K_LED * 4;
            }
            1 | 2 => {
                if counter == 0 {
                    self.mask = 15;
                    self.porta_ext(v, par >> 4 == 1, hw);
                    self.mask = 0xFF;
                }
            }
            3 | 4 => self.ch[v].gf = self.ch[v].gf & 0xF0 | x,
            5 => self.ch[v].fine = x,
            6 => {
                if counter == 0 {
                    let c = &mut self.ch[v];
                    if x == 0 {
                        c.loop_row = self.row as u8;
                    } else {
                        let go = if c.loop_cnt == 0 {
                            c.loop_cnt = x;
                            true
                        } else {
                            c.loop_cnt -= 1;
                            c.loop_cnt != 0
                        };
                        if go {
                            self.break_row = c.loop_row as usize;
                            self.loop_flag = true;
                        }
                    }
                }
            }
            7 => self.ch[v].wave = self.ch[v].wave & 0x0F | x << 4,
            9 => {
                if x > 0 && counter % x == 0 && !(counter == 0 && self.ch[v].note_per != 0) {
                    self.retrigger(v, hw);
                }
            }
            0xA | 0xB => {
                if counter == 0 {
                    let c = &mut self.ch[v];
                    c.vol = if par >> 4 == 0xA { (c.vol + x).min(64) } else { c.vol.saturating_sub(x) };
                    let vol = c.vol;
                    self.w_vol(hw, v, vol, C_FX);
                }
            }
            0xC => {
                if counter == x {
                    self.ch[v].vol = 0;
                    self.w_vol(hw, v, 0, C_FX);
                }
            }
            0xD => {
                if counter == x && self.ch[v].note_per != 0 {
                    self.retrigger(v, hw);
                }
            }
            0xE => {
                if counter == 0 && self.delay_cnt == 0 {
                    self.delay_req = x + 1;
                }
            }
            0xF => {
                if counter == 0 {
                    self.ch[v].gf = self.ch[v].gf & 15 | x << 4;
                    if x != 0 {
                        self.funk_step(v, hw);
                    }
                }
            }
            _ => {}
        }
    }

    /// Portamento up or down for E1x / E2x: the parameter's low nibble through the mask.
    fn porta_ext(&mut self, v: usize, up: bool, hw: &mut Hw) {
        self.porta(v, up, hw);
    }

    fn retrigger(&mut self, v: usize, hw: &mut Hw) {
        let bit = 1u16 << v;
        let c = self.ch[v];
        self.w_off(hw, bit, C_RETRIG);
        self.w_loc(hw, v, c.start, C_RETRIG);
        self.w_len(hw, v, c.len, C_RETRIG);
        self.w_on(hw, bit, 54, C_RETRIG);
        self.w_loc_after_wait(hw, v, c.loop_start, 61);
        self.w_len(hw, v, c.loop_len, C_RETRIG);
        self.w_per(hw, v, c.per, C_RETRIG);
    }

    fn funk_step(&mut self, v: usize, hw: &mut Hw) {
        let c = &mut self.ch[v];
        c.funk_cnt = c.funk_cnt.wrapping_add(FUNK[(c.gf >> 4) as usize]);
        if c.funk_cnt & 0x80 != 0 {
            c.funk_cnt = 0;
            c.funk_ptr = c.funk_ptr.wrapping_add(c.loop_len as u32 * 2);
            let limit = c.start.wrapping_add(2 * c.orig_len as u32).wrapping_sub(2 * c.loop_len as u32);
            if c.funk_ptr > limit {
                c.funk_ptr = c.loop_start;
            }
        }
        let a = c.funk_ptr;
        self.w_loc(hw, v, a, C_FX);
    }
}

/// Waits for the beam's line to change nine times, as the original's loop does: it reads
/// the line, polls it about every 12.5 colour clocks and starts a new round 17 after each
/// change it sees; the write that follows comes `tail` colour clocks after the ninth.
fn raster_wait(hw: &mut Hw, tail: u64) {
    let mut t = hw.bus.clock;
    let mut last = t / LINE;
    t += 8;
    let (mut changes, mut alt) = (0, false);
    for _ in 0..4000 {
        let l = t / LINE;
        if l != last {
            last = l;
            changes += 1;
            if changes == 9 {
                break;
            }
            t += 17;
        } else {
            t += if alt { 12 } else { 13 };
            alt = !alt;
        }
    }
    hw.bus.clock = t + tail;
}

fn pack(level: u64, fields: &[u8]) -> u64 {
    let mut v = level << 56;
    for (i, &x) in fields.iter().enumerate() {
        v |= (x as u64) << (48 - 8 * i);
    }
    v
}

// code path contexts of the gap table
const C_REFRESH: u8 = 1;
const C_IDLE: u8 = 2;
const C_FX: u8 = 3;
const C_ROW_PER: u8 = 4;
const C_SMP_VOL: u8 = 5;
const C_NOTE: u8 = 6;
const C_RESTART: u8 = 7;
const C_LOOP: u8 = 8;
const C_RETRIG: u8 = 9;

/// Measured gaps (colour clocks) from the previous write to a write, by the code path: each row is a
/// key (see `pack`: a level in the top byte, then the previous write, this write, the path and the
/// effect and entry shape of this voice and of the one before) and the gap. A row is kept only where it
/// differs from what the coarser levels give. Fitted to the reference logs of the three real modules
/// and the two synthetic ones. For the wait on the beam (write kind 7) the value is the time from the
/// previous write to the start of the wait.
static GAPS: &[(u64, u16)] = &[
    (0x0000000000000000, 376), (0x0000040000000000, 6), (0x0000080000000000, 111), (0x0000090000000000, 232),
    (0x00000A0000000000, 354), (0x00001C0000000000, 684), (0x0001050000000000, 6), (0x0002060000000000, 6),
    (0x0003070000000000, 6), (0x0004010000000000, 18), (0x0004080000000000, 135), (0x00041C0000000000, 169),
    (0x0005020000000000, 18), (0x0005090000000000, 166), (0x0006030000000000, 18), (0x00060A0000000000, 197),
    (0x00070B0000000000, 205), (0x00071C0000000000, 189), (0x0008000000000000, 376), (0x0008080000000000, 94),
    (0x0008090000000000, 133), (0x00080A0000000000, 283), (0x00080B0000000000, 387), (0x00080C0000000000, 222),
    (0x00080D0000000000, 417), (0x00080E0000000000, 605), (0x00080F0000000000, 701), (0x0008100000000000, 693),
    (0x00081C0000000000, 608), (0x00090A0000000000, 149), (0x00090B0000000000, 299), (0x00090D0000000000, 373),
    (0x00090E0000000000, 517), (0x00090F0000000000, 616), (0x0009100000000000, 357), (0x00091C0000000000, 533),
    (0x000A0A0000000000, 126), (0x000A0B0000000000, 161), (0x000A0E0000000000, 424), (0x000A0F0000000000, 464),
    (0x000A100000000000, 293), (0x000A180000000000, 229), (0x000A1C0000000000, 445), (0x000B0B0000000000, 129),
    (0x000B0F0000000000, 298), (0x000B180000000000, 224), (0x000B1C0000000000, 333), (0x000C090000000000, 106),
    (0x000C0A0000000000, 210), (0x000C0D0000000000, 181), (0x000C0F0000000000, 628), (0x000C100000000000, 198),
    (0x000C1C0000000000, 556), (0x000D0A0000000000, 121), (0x000D0B0000000000, 296), (0x000D0E0000000000, 208),
    (0x000D0F0000000000, 518), (0x000D100000000000, 225), (0x000D1C0000000000, 471), (0x000E0B0000000000, 151),
    (0x000E0F0000000000, 219), (0x000E100000000000, 240), (0x000E1C0000000000, 358), (0x000F000000000000, 336),
    (0x000F080000000000, 188), (0x000F090000000000, 346), (0x000F0A0000000000, 450), (0x000F0B0000000000, 553),
    (0x000F0C0000000000, 542), (0x000F0D0000000000, 650), (0x000F0E0000000000, 772), (0x000F0F0000000000, 870),
    (0x000F100000000000, 233), (0x000F180000000000, 1132), (0x000F1C0000000000, 259), (0x0010000000000000, 57),
    (0x0010010000000000, 57), (0x0010020000000000, 57), (0x0010030000000000, 57), (0x0014030000000000, 2037),
    (0x00181C0000000000, 105), (0x0100040600000000, 10), (0x0100040900000000, 10), (0x0100090300000000, 256),
    (0x0100090400000000, 254), (0x01000A0400000000, 358), (0x0101050600000000, 10), (0x0102060600000000, 10),
    (0x0103070600000000, 10), (0x0103070900000000, 10), (0x0104080900000000, 0), (0x01070B0900000000, 0),
    (0x0108090300000000, 157), (0x0108090400000000, 180), (0x01080A0300000000, 465), (0x01080C0500000000, 351),
    (0x01080D0500000000, 490), (0x01090A0300000000, 173), (0x01090A0400000000, 195), (0x01090B0200000000, 248),
    (0x01090B0300000000, 272), (0x01090D0300000000, 272), (0x01090E0300000000, 444), (0x01090F0300000000, 618),
    (0x010A0B0300000000, 185), (0x010A0B0400000000, 211), (0x010A0E0300000000, 275), (0x010A0F0300000000, 455),
    (0x010A100900000000, 333), (0x010B0F0500000000, 405), (0x010C090200000000, 123), (0x010C090300000000, 147),
    (0x010C0D0300000000, 343), (0x010C0D0500000000, 417), (0x010D0A0200000000, 139), (0x010D0A0300000000, 281),
    (0x010D0B0400000000, 225), (0x010D0E0300000000, 438), (0x010D0E0500000000, 438), (0x010E0B0300000000, 175),
    (0x010E0B0400000000, 137), (0x010E0F0300000000, 380), (0x010E0F0500000000, 454), (0x010E100900000000, 323),
    (0x010F080300000000, 212), (0x010F080400000000, 243), (0x010F090200000000, 309), (0x010F090300000000, 333),
    (0x010F0A0300000000, 559), (0x010F0C0300000000, 468), (0x010F0D0500000000, 723), (0x010F100900000000, 299),
    (0x0110000900000000, 12), (0x0110030900000000, 12), (0x0208090300000000, 254), (0x0208090302000000, 210),
    (0x0208090304000000, 324), (0x0208090307000000, 140), (0x02080A0303000000, 806), (0x02080C030A000000, 221),
    (0x02080C030C010000, 302), (0x02080C030C0F0000, 246), (0x02080C05000E0000, 370), (0x02080C05030F0000, 375),
    (0x02080C05040F0000, 375), (0x02080C050F0F0000, 375), (0x02090A0300000000, 850), (0x02090A0301000000, 224),
    (0x02090A0302000000, 224), (0x02090A0303000000, 255), (0x02090A0304000000, 293), (0x02090A0306000000, 291),
    (0x02090B0306000000, 385), (0x02090D0307000000, 336), (0x02090D030A000000, 257), (0x02090D05000F0000, 402),
    (0x02090D050C0F0000, 402), (0x02090E05000E0000, 512), (0x02090E05030F0000, 518), (0x020A0A0305000000, 116),
    (0x020A0B0300000000, 281), (0x020A0B0301000000, 235), (0x020A0B0303000000, 303), (0x020A0B0304000000, 304),
    (0x020A0B0306000000, 303), (0x020A0B0309000000, 180), (0x020A0E0305000000, 276), (0x020A0E030A000000, 276),
    (0x020A0E030C010000, 356), (0x020A0E030C0B0000, 299), (0x020A0E030C0F0000, 299), (0x020A0E05010F0000, 429),
    (0x020A0E05030F0000, 430), (0x020A0E050C0F0000, 430), (0x020A0F05030F0000, 528), (0x020A0F050C0F0000, 469),
    (0x020A10060C0B0000, 407), (0x020B0F0306000000, 275), (0x020B0F030A000000, 274), (0x020B0F030C010000, 355),
    (0x020B0F030C0B0000, 299), (0x020B0F05000E0000, 399), (0x020B0F05010F0000, 429), (0x020B0F05030F0000, 429),
    (0x020C090300000000, 244), (0x020C1006010F0000, 209), (0x020C1006040F0000, 209), (0x020C10060F0F0000, 218),
    (0x020D0A0302000000, 213), (0x020D0A0303000000, 267), (0x020D0A0305000000, 684), (0x020D0A030C000000, 163),
    (0x020D1006000E0000, 222), (0x020E0B0301000000, 225), (0x020E0B0303000000, 288), (0x020E0B0304000000, 341),
    (0x020E0B0306000000, 293), (0x020E0B030C000000, 174), (0x020E0F05000E0000, 448), (0x020E0F05010F0000, 1127),
    (0x020E1006010F0000, 249), (0x020E10060C0F0000, 249), (0x020E10060F0F0000, 249), (0x020F00030E010000, 476),
    (0x020F080300000000, 1094), (0x020F080301000000, 262), (0x020F080302000000, 262), (0x020F080303000000, 307),
    (0x020F080304000000, 331), (0x020F080305000000, 245), (0x020F080306000000, 330), (0x020F090300000000, 411),
    (0x020F09030C000000, 315), (0x020F0C05000E0000, 536), (0x020F1006000F0000, 242), (0x020F1006010F0000, 253),
    (0x0308090200000100, 116), (0x0308090200000300, 130), (0x0308090200000400, 139), (0x0308090200000E00, 73),
    (0x0308090300000100, 236), (0x0308090300000300, 708), (0x0308090300000400, 259), (0x0308090307000300, 153),
    (0x030809030C000100, 140), (0x030809030C000300, 154), (0x030809030C000400, 163), (0x030809040000000E, 145),
    (0x030809040000010F, 190), (0x030809040000040F, 190), (0x0308090400000601, 246), (0x0308090400000A01, 246),
    (0x0308090400000F0F, 216), (0x03080D030C01000E, 382), (0x03080D030C01040F, 427), (0x03080D030C010601, 483),
    (0x03080D05000F000E, 455), (0x03080D05000F010F, 500), (0x03080D05000F040F, 500), (0x03090A0200000400, 154),
    (0x03090A0303000200, 580), (0x03090A0303000C00, 269), (0x03090A0304000C00, 340), (0x03090A0306000C00, 286),
    (0x03090A040000000E, 161), (0x03090A040000000F, 160), (0x03090A0400000401, 262), (0x03090A0400000A01, 262),
    (0x03090B0400000301, 365), (0x03090B0400000401, 365), (0x03090B0400000601, 366), (0x03090D0307000100, 392),
    (0x03090D0307000200, 345), (0x03090D0307000E00, 408), (0x03090D030A000000, 245), (0x03090D030C0F0E01, 273),
    (0x03090D05000E0F0F, 397), (0x03090D05000F0C01, 403), (0x03090D050C0F0000, 378), (0x03090D050C0F0C0F, 378),
    (0x03090D050C0F0E01, 403), (0x03091C0700000401, 562), (0x03091C0700000601, 512), (0x03091C0700000A01, 487),
    (0x030A0A0306000A00, 125), (0x030A0B0200000100, 142), (0x030A0B0200000200, 142), (0x030A0B0200000300, 156),
    (0x030A0B0200000400, 165), (0x030A0B0300000000, 902), (0x030A0B0301000300, 231), (0x030A0B0301000400, 240),
    (0x030A0B0303000000, 298), (0x030A0B0303000C00, 266), (0x030A0B0304000400, 309), (0x030A0B0304000C00, 351),
    (0x030A0B0306000100, 284), (0x030A0B0306000300, 293), (0x030A0B0306000400, 307), (0x030A0B0306000C00, 298),
    (0x030A0B030A000400, 189), (0x030A0B030C000100, 166), (0x030A0B030C000200, 166), (0x030A0B030C000300, 180),
    (0x030A0B030C000400, 189), (0x030A0B040000000A, 176), (0x030A0B040000000E, 176), (0x030A0B040000010F, 221),
    (0x030A0B0400000201, 277), (0x030A0B0400000401, 278), (0x030A0B0400000601, 278), (0x030A0B0400000A01, 278),
    (0x030A0B0400000F0F, 247), (0x030A0E0306000A00, 276), (0x030A0E030C0F000E, 300), (0x030A0E030C0F0C0F, 300),
    (0x030A0E05010F0000, 430), (0x030A0E05030F000E, 429), (0x030A0E05030F0C0F, 429), (0x030A0E050C0F000E, 429),
    (0x030A0E050C0F0C0F, 429), (0x030A0F05000E0000, 523), (0x030A0F050C0F0000, 504), (0x030A18030E000300, 226),
    (0x030A1C0700000401, 511), (0x030A1C0700000601, 499), (0x030A1C0700000A01, 436), (0x030B0B0306000100, 130),
    (0x030B0B0306000300, 130), (0x030B0B0306000400, 130), (0x030B0F0306000100, 274), (0x030B0F0306000300, 282),
    (0x030B0F0306000400, 274), (0x030B0F0306000C00, 283), (0x030B0F030A000400, 275), (0x030B0F030A000600, 282),
    (0x030B0F030C0F030F, 299), (0x030B0F030C0F0601, 299), (0x030B0F030C0F0A01, 299), (0x030B0F05000E0000, 423),
    (0x030B0F05010F0000, 428), (0x030B0F050C0F030F, 404), (0x030B0F050C0F0601, 428), (0x030B0F050C0F0A01, 404),
    (0x030B0F050C0F0C0F, 409), (0x030B1C070000000A, 310), (0x030B1C070000000E, 298), (0x030B1C070000010F, 355),
    (0x030B1C0700000601, 399), (0x030B1C0700000A01, 412), (0x030C090200000A00, 124), (0x030C090300000A00, 620),
    (0x030C09030C000A00, 148), (0x030C09040000030F, 723), (0x030C090400000E0F, 339), (0x030C0D030C01030F, 1100),
    (0x030C0D05000F030F, 1232), (0x030D0A0200000700, 162), (0x030D0A0306000700, 304), (0x030D0A0400000C0F, 122),
    (0x030D1006000E0F0F, 233), (0x030D1006000F0C01, 224), (0x030D10060C0F0000, 223), (0x030D10060C0F0C0F, 223),
    (0x030D10060C0F0E01, 224), (0x030E0B0200000A00, 150), (0x030E0B030C000600, 175), (0x030E0B040000030F, 806),
    (0x030E1006000E000E, 249), (0x030E1006000E0C0F, 249), (0x030E10060C0F000E, 240), (0x030F090200000300, 291),
    (0x030F090400000401, 413), (0x030F090400000601, 413), (0x030F090400000A01, 413), (0x030F090400000E01, 471),
    (0x030F0A0400000001, 538), (0x030F0B0400000401, 620), (0x030F0D030C010000, 583), (0x030F0F05030F0401, 937),
    (0x030F1006000E0000, 253), (0x030F1C0700000000, 850), (0x030F1C070000030F, 960), (0x030F1C0700000C01, 284),
    (0x031001060C0F0E01, 49),
];

impl Replayer for TimeTracker {
    fn format(&self) -> &'static str {
        "TimeTracker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tick_inner(hw);
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.layout.blocks.len() as u16 - 1, self.sub as u16)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.select(n as usize);
        if n == 0 {
            return;
        }
        // the host starts a subsong other than the first twice (the reference's log): all DMA
        // off, a quick pass of the volumes, the two ordinary passes and the LED off
        hw.bus.clock += 658;
        hw.dmacon(0x000F);
        hw.bus.clock += 8;
        for v in 0..4 {
            hw.vol(v, 0);
            hw.bus.clock += 6;
        }
        let t0 = hw.bus.clock + 222;
        for (k, off) in [0u64, 128, 268, 403, 648, 776, 916, 1051].iter().enumerate() {
            hw.bus.clock = t0 + off;
            hw.vol(k & 3, 0);
        }
        hw.bus.clock = t0 + 1857;
        hw.bus.write_byte_led(0x02);
        self.next_at = Some(hw.bus.clock + FIRST_TICK);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn timer_delay(&self) -> u64 {
        FIRST_TICK - DT_TICK
    }
}

#[cfg(test)]
#[path = "timetracker_tests.rs"]
mod tests;

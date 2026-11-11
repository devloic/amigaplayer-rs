//! The newer Digital Sound Studio the Eagleplayer predates: a module whose
//! tempo byte (offset 8) is not 0 ("player module V1.2"). It is written from a
//! black-box specification of that player's register writes (its Paula and CIA
//! writes, as the write logs show them), not from its code: the specification
//! and an executable model of it were checked against the reference's logs of
//! 176 probe modules, the 11 real tempo modules and 1,750 random ones. Where
//! the player agrees with NostalgicPlayer's Digital Sound Studio (Copyright (c)
//! 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer) the effects
//! are that player's; the differences are the ones the logs show:
//!
//! * the timer is 1773375 / BPM plus one E-clock ticks (a header tempo under 28
//!   is 3904), the first tick a period after the song start; the start writes the
//!   four volumes twice, and the first row is at tick `speed`;
//! * every row ends with the DMA set of the voices that started (always, even
//!   with none), a wait of nine raster lines, and the loop registers of all
//!   four voices (a sample without a loop loops its first word);
//! * a note needs only a period, a voice keeps its sample between rows;
//! * volumes are bytes with signed compares and the player's own limits, the
//!   master volume argument is signed, arpeggio adds its offsets (the counter
//!   is shared by the voices), `$14` sets a loop on argument 0, `$7FF` stops a
//!   voice and restarts it silent at the row's end, the delay, cut and retrigger
//!   effects compare signed bytes.
//!
//! The writes fall where the reference's do: a tick is a ledger of 68000 cycles
//! (`COSTS`: the paths between two writes, solved from the reference's cycle
//! counts), a write comes at the tick's clock plus half the cycles so far, and a
//! DMA off or a row-end DMA on is followed by the original's wait of nine raster
//! lines, a loop that reads the beam (`wait_lines`), run here read for read. The
//! tempo effect and a jump to a position that is not ahead (the song's end) call
//! the host, which goes on at a whole colour clock. The start-up takes the
//! reference's time (its calls' cycles, by the positions and the samples), so
//! that the beam is where the reference's is. On the 11 tempo modules every
//! write over 300 s (338,982) has the reference's value and colour clock.

use super::SAMPLES_OFF;
use crate::bus::Bus;
use crate::replay::Hw;
use m68k::AddressBus;

include!("np_tables.rs");

#[derive(Clone, Copy, Default)]
struct Voice {
    sample: u8,
    period: u16,
    effect: u8,
    arg: u8,
    /// The sample's address plus its start offset; 0 is no sample.
    start: u32,
    len: u16,
    loop_start: u32,
    loop_len: u16,
    /// The sample's loop length in the header: 0, 1 or more (as 1, 2, 3).
    looped: u8,
    finetune: u8,
    volume: u8,
    pitch: i32,
    offset: u16,
    port_end: i32,
    port_up: bool,
    port_speed: i32,
    tone_slide: bool,
    tone_port: bool,
    loop_row: i32,
    loop_cnt: u8,
}

pub struct Np {
    base: u32,
    mem: u32,
    voices: [Voice; 4],
    tempo: u8,
    speed: u8,
    counter: u16,
    position: u16,
    row: i32,
    jump: bool,
    new_position: u16,
    set_loop: bool,
    loop_row: i32,
    inv: i32,
    next_retrig: u32,
    arp: i32,
    /// The clock the tick's cycles count from (the timer's call, or the last host call), and
    /// the 68000 cycles since.
    t0: u64,
    cyc: i64,
    /// Where the tick is, for the paths around a write: 0 a row's cells, 4 the row's end, 8 an
    /// effect tick, 12 a retrigger or a delayed note.
    ctx: usize,
    /// DSS_FIT: the paths taken since the last event, for the solver of `COSTS`.
    fit: Option<std::io::BufWriter<std::fs::File>>,
    counts: Vec<(&'static str, u8, u32)>,
    pub songend: bool,
}

fn s8(x: u32) -> i32 {
    x as u8 as i8 as i32
}

/// The original's code paths (a name and a case of it: a voice, an effect number, a branch),
/// each with the 68000 cycles it takes: solved from the reference's cycle counts between its
/// writes, beam waits and host calls, each the whole-numbered sum of its paths. Only sums are
/// measured: of the costs that fit, these are ones of small magnitudes (a negative one is a
/// branch cheaper than its other side, or a part counted with a neighbour). A path that is
/// not here costs nothing: no event of the corpus follows it.
// COSTS-BEGIN
const COSTS: &[(&str, u8, i32)] = &[
    ("ToLoc", 0, 87),
    ("ToLoc", 1, 79),
    ("ToLoc", 2, 87),
    ("ToLoc", 3, 87),
    ("ToLoc", 12, 87),
    ("ToLoc", 13, 87),
    ("ToLoc", 14, 87),
    ("ToLoc", 15, 87),
    ("FromPer", 0, -12),
    ("FromPer", 1, 19),
    ("FromPer", 2, 50),
    ("FromPer", 3, 58),
    ("FromVol", 0, 19),
    ("FromVol", 1, 19),
    ("FromVol", 2, 19),
    ("FromVol", 3, 9),
    ("Wait", 0, 62),
    ("Tick", 0, 296),
    ("Row", 0, 5),
    ("RowEnd", 0, 266),
    ("Loop", 0, 136),
    ("Loop", 1, 79),
    ("Loop", 2, -8),
    ("Loop", 3, -8),
    ("LoopSkip", 0, 112),
    ("LoopSkip", 2, 55),
    ("LoopLen", 0, 20),
    ("Play", 0, 50),
    ("Play", 1, 50),
    ("Play", 2, 50),
    ("Play", 3, 50),
    ("PlayLoc", 0, -63),
    ("PlayLen", 0, 20),
    ("PlayOn", 0, 87),
    ("PlayOn", 1, 87),
    ("PlayOn", 2, 87),
    ("PlayOn", 3, 87),
    ("SetVol", 1, 23),
    ("SetVol", 2, 46),
    ("SetVol", 3, 69),
    ("AddVol", 0, -19),
    ("SubVol", 0, -16),
    ("Ft", 0, 203),
    ("Ft", 1, 304),
    ("FtStep", 0, 27),
    ("Pitch", 0, 46),
    ("Port", 0, -85),
    ("Port", 2, 36),
    ("Port", 4, 46),
    ("Slide", 1, -19),
    ("Smp", 0, 496),
    ("Smp", 1, 661),
    ("E0", 27, 41),
    ("SmpAddr", 2, 107),
    ("Per", 0, 25),
    ("Per", 2, 44),
    ("TP", 0, 218),
    ("TPc", 0, -7),
    ("TPc", 2, 19),
    ("Ret", 0, -165),
    ("Ret", 1, -183),
    ("Ret", 2, -165),
    ("Ret", 3, 41),
    ("NDelayVol", 0, 364),
    ("Offs", 4, 173),
    ("Offs", 5, 211),
    ("NoteLoc", 0, 94),
    ("NoteLoc", 1, 102),
    ("NoteLoc", 2, 94),
    ("NoteLoc", 3, 94),
    ("NoteLen", 0, -127),
    ("NoteLoop", 2, 401),
    ("NoteLoop", 3, 397),
    ("D1", 0, -130),
    ("D1", 3, 247),
    ("D1", 4, -56),
    ("D1", 7, 137),
    ("D1", 21, 191),
    ("D1", 24, 41),
    ("Arg", 0, 18),
    ("Arg", 1, 94),
    ("NoteVol", 0, 238),
    ("D2", 0, -165),
    ("D2", 1, 41),
    ("D2", 2, 41),
    ("D2", 4, -165),
    ("D2", 5, -127),
    ("D2", 11, 453),
    ("D2", 12, -136),
    ("D2", 13, -136),
    ("D2", 14, 41),
    ("D2", 15, 41),
    ("D2", 20, 26),
    ("D2", 22, 41),
    ("D2", 25, 41),
    ("D2", 26, 41),
    ("FilterEnd", 0, -184),
    ("TempoEnd", 0, -68),
    ("PLoop", 3, 3),
    ("PLoop", 4, -65),
    ("XArg", 0, 149),
    ("XArg", 1, 321),
    ("X", 3, 50),
    ("X", 11, 100),
    ("X", 21, 50),
    ("X", 25, 50),
    ("X", 27, 205),
    ("Arp", 0, -97),
    ("Arp", 1, 79),
    ("Arp", 2, 95),
    ("ArpFt", 0, 57),
    ("ArpStep", 0, 52),
    ("Delay", 3, 2),
    ("X1b", 0, 127),
    ("FromLoc", 0, 156),
    ("FromLoc", 1, 156),
    ("FromLoc", 2, 156),
    ("FromLoc", 3, 156),
    ("FromLen", 5, 87),
    ("FromLen", 6, 87),
    ("Fx", 0, -19),
    ("SetVolNeg", 0, 4),
    ("AddVolMax", 0, 26),
    ("SlideLim", 1, 30),
    ("N7ffVol", 0, 96),
    ("N7ffVol", 1, 96),
    ("N7ffVol", 2, 96),
    ("N7ffVol", 3, 96),
    ("NoteFx", 26, 48),
    ("NoteNone", 0, 94),
    ("NoteNone", 1, 102),
    ("NoteNone", 2, 94),
    ("NoteNone", 3, 94),
    ("Jump", 1, 251),
    ("JumpEnd", 0, -74),
    ("ArpTop", 0, -27),
    ("Retrig", 3, 48),
];
// COSTS-END

fn cost(name: &str, k: u8) -> i32 {
    COSTS.iter().find(|c| c.1 == k && c.0 == name).map_or(0, |c| c.2)
}

/// The start-up's volume writes, in cycles from their call's start, and the call's cycles.
const INIT_VOL: [u64; 4] = [438, 715, 1015, 1338];
const INIT_CALL: u64 = 1408;

impl Np {
    /// Does the player's start-up: the four volumes cleared twice, after the loops over the
    /// positions and the samples that precede them. The times are the reference's (the cycles
    /// of its start-up calls on the 11 modules, whole-numbered sums; each call starts at a
    /// whole colour clock, and a host call drops an odd cycle).
    pub fn new(d: &[u8], base: u32, mem: u32, bus: &mut Bus) -> Np {
        let n = u16::from_be_bytes([d[0x59c], d[0x59d]]).wrapping_sub(1) as u64 + 1;
        let (mut max, mut raised) = (0u8, 0u64);
        for k in 0..n as usize {
            let p = d.get(0x59e + k).copied().unwrap_or(0);
            if p >= max {
                max = p;
                raised += 1;
            }
        }
        let (mut samples, mut loops) = (0u64, 0u64);
        for k in 0..31 {
            let o = 0xa + 0x2e * k;
            if u16::from_be_bytes([d[o + 0x22], d[o + 0x23]]) != 0 {
                samples += 1;
                if u16::from_be_bytes([d[o + 0x28], d[o + 0x29]]) != 1 {
                    loops += 1;
                }
            }
        }
        // the check; the load between three host calls (the positions and the samples, then
        // the samples again); a short call
        let load = 3182 + 31 * n + 4 * raised + 73 * samples + 20 * loops;
        let again = 1091 + 96 * samples;
        bus.clock += 86 / 2 + 36 / 2 + load / 2 + again / 2 + 16 / 2 + 24 / 2;
        let mut hw = Hw { bus };
        for _ in 0..2 {
            let call = hw.bus.clock;
            for (i, c) in INIT_VOL.into_iter().enumerate() {
                hw.bus.clock = call + c / 2;
                hw.vol(i, 0);
            }
            hw.bus.clock = call + INIT_CALL / 2;
        }
        // the last call; the song starts at its end
        hw.bus.clock += 1701 / 2;
        Np {
            base,
            mem,
            voices: [Voice { loop_row: -1, ..Voice::default() }; 4],
            tempo: d[8],
            speed: d[9],
            counter: 0,
            position: 0,
            row: 0,
            jump: false,
            new_position: 0,
            set_loop: false,
            loop_row: 0,
            inv: 0,
            next_retrig: 0,
            arp: 3,
            t0: 0,
            cyc: 0,
            ctx: 0,
            fit: std::env::var_os("DSS_FIT").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new),
            counts: Vec::new(),
            songend: false,
        }
    }

    /// The timer's period in colour clocks: 1773375 / BPM plus one, in E-clocks
    /// of 5; a header tempo under 28 does not fit the timer and is 3904.
    pub fn tick_clocks(&self) -> u64 {
        let t = self.tempo.max(1) as u64;
        (if t < 28 { 3904 } else { 1_773_375 / t + 1 }) * 5
    }

    fn positions(&self, hw: &Hw) -> u16 {
        hw.bus.r16(self.base + 0x59c)
    }

    /// The path `name`, case `k`, is taken.
    fn t(&mut self, name: &'static str, k: usize) {
        self.tn(name, k, 1);
    }

    fn tn(&mut self, name: &'static str, k: usize, n: u32) {
        let k = k as u8;
        self.cyc += cost(name, k) as i64 * n as i64;
        if self.fit.is_some() && n != 0 {
            match self.counts.iter_mut().find(|e| e.1 == k && e.0 == name) {
                Some(e) => e.2 += n,
                None => self.counts.push((name, k, n)),
            }
        }
    }

    /// An event (a write `W`, a beam wait `L`, a host call `H`) at the cycles so far: the
    /// clock is the tick's plus half of them.
    fn at(&mut self, hw: &mut Hw, kind: char) {
        hw.bus.clock = self.t0 + self.cyc.max(0) as u64 / 2;
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let s: Vec<String> = self.counts.iter().map(|(n, k, c)| format!("{n}.{k}:{c}")).collect();
            let _ = writeln!(f, "{kind} {} {}", hw.bus.clock, s.join(" "));
            self.counts.clear();
        }
    }

    fn dma(&mut self, hw: &mut Hw, v: u16) {
        self.at(hw, 'W');
        hw.dmacon(v);
    }
    fn loc(&mut self, hw: &mut Hw, i: usize, a: u32) {
        self.t("ToLoc", i + self.ctx);
        self.at(hw, 'W');
        hw.loc(i, a);
        self.t("FromLoc", i + self.ctx);
    }
    fn len(&mut self, hw: &mut Hw, i: usize, w: u16) {
        self.t("ToLen", i + self.ctx);
        self.at(hw, 'W');
        hw.len(i, w);
        self.t("FromLen", i + self.ctx);
    }
    fn per(&mut self, hw: &mut Hw, i: usize, p: i32) {
        self.t("ToPer", i + self.ctx);
        self.at(hw, 'W');
        hw.per(i, p as u16);
        self.t("FromPer", i + self.ctx);
    }
    fn vol(&mut self, hw: &mut Hw, i: usize, v: i32) {
        self.t("ToVol", i + self.ctx);
        self.at(hw, 'W');
        hw.vol(i, v as u16);
        self.t("FromVol", i + self.ctx);
    }

    /// A call of the host: it goes on at a whole colour clock, an odd cycle is dropped.
    fn host(&mut self, hw: &mut Hw) {
        self.at(hw, 'H');
        self.t0 = hw.bus.clock;
        self.cyc = 0;
    }

    /// The raster line the beam is in (VHPOSR's high byte) at the cycles so far.
    fn line(&self, hw: &mut Hw) -> u32 {
        hw.bus.clock = self.t0 + self.cyc.max(0) as u64 / 2;
        hw.bus.beam().0 & 0xff
    }

    /// The original's wait of nine raster lines: nine times, it reads the beam's line and
    /// reads it again until it has changed (16 cycles to the second read, 25 from one to
    /// the next, 35 on to the next line's first).
    fn wait_lines(&mut self, hw: &mut Hw) {
        self.t("Wait", 0);
        self.at(hw, 'L');
        for n in 0..9 {
            if n > 0 {
                self.cyc += 35;
            }
            let l = self.line(hw);
            self.cyc += 16;
            while self.line(hw) == l {
                self.cyc += 25;
            }
        }
    }

    pub fn tick(&mut self, hw: &mut Hw) {
        self.t0 = hw.bus.clock;
        self.cyc = 0;
        self.counts.clear();
        if let Some(f) = &mut self.fit {
            use std::io::Write;
            let _ = writeln!(f, "tick {}", hw.bus.clock);
        }
        self.t("Tick", 0);
        self.counter = self.counter.wrapping_add(1);
        if self.counter == self.speed as u16 {
            self.counter = 0;
            self.next_retrig = 0;
            self.arp = 3;
            self.ctx = 0;
            self.row(hw);
        } else {
            self.ctx = 8;
            self.t("Fx", 0);
            for i in 0..4 {
                self.t("FxV", i);
                self.make_effects(i, hw);
            }
        }
    }

    fn row(&mut self, hw: &mut Hw) {
        self.new_position = self.position;
        let pat = if self.position < self.positions(hw) {
            hw.bus.r8(self.base + 0x59e + self.position as u32) as u32
        } else {
            0
        };
        self.t("Row", 0);
        let mut mask = 0u16;
        for i in 0..4 {
            let a = self.base + 0x61e + (pat << 10) + 16 * self.row as u32 + 4 * i as u32;
            self.t("V", i);
            self.setup(i, a, hw, &mut mask);
        }
        self.t("RowEnd", 0);
        self.ctx = 4;
        self.dma(hw, 0x8000 | mask);
        self.wait_lines(hw);
        for i in 0..4 {
            if self.voices[i].effect != 0x17 {
                self.t("Loop", i);
                self.write_loop(i, hw);
            } else {
                self.t("LoopSkip", i);
            }
        }
        if self.set_loop {
            self.row = self.loop_row;
            self.set_loop = false;
        }
        self.row += 1;
        if self.row >= 64 || self.jump {
            self.jump = false;
            self.row = 0;
            for v in &mut self.voices {
                v.loop_row = -1;
            }
            let old = self.position;
            let mut p = self.new_position.wrapping_add(1);
            let mut wrapped = false;
            if p == self.positions(hw) {
                p = 0;
                wrapped = true;
            }
            self.position = p;
            if wrapped || p < old {
                self.songend = true;
            }
        }
    }

    fn write_loop(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        self.loc(hw, i, v.loop_start);
        self.t("LoopLen", 0);
        self.len(hw, i, v.loop_len);
    }

    /// A retrigger or a delayed note: the voice's DMA off, its sample, DMA on,
    /// and only its loop pair.
    fn play(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        let ctx = std::mem::replace(&mut self.ctx, 12);
        self.t("Play", i);
        self.dma(hw, 1 << i);
        self.t("PlayLoc", 0);
        self.loc(hw, i, v.start);
        self.t("PlayLen", 0);
        self.len(hw, i, v.len);
        self.wait_lines(hw);
        self.t("PlayOn", i);
        self.dma(hw, 0x8000 | 1 << i);
        self.wait_lines(hw);
        self.t("PlayLoop", i);
        self.write_loop(i, hw);
        self.ctx = ctx;
    }

    fn set_volume(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i].volume as i32 - self.inv;
        self.t("SetVol", i);
        if v < 0 {
            self.t("SetVolNeg", 0);
        }
        self.vol(hw, i, v.max(0) & 0x7f);
    }

    /// The volume writes of the add, subtract and master routines: the same
    /// but for the mask.
    fn vol_out(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i].volume as i32 - self.inv;
        self.t("VolOut", i);
        if v < 0 {
            self.t("VolOutNeg", 0);
        }
        self.vol(hw, i, v.max(0));
    }

    fn vol_out_all(&mut self, hw: &mut Hw) {
        for i in 0..4 {
            self.t("VolAll", i);
            self.vol_out(i, hw);
        }
    }

    fn add_volume(&mut self, i: usize, a: u8, hw: &mut Hw) {
        let mut n = self.voices[i].volume.wrapping_add(a);
        self.t("AddVol", 0);
        if n as i8 > 64 {
            n = 64;
            self.t("AddVolMax", 0);
        }
        self.voices[i].volume = n;
        self.vol_out(i, hw);
    }

    fn sub_volume(&mut self, i: usize, a: u8, hw: &mut Hw) {
        let v = self.voices[i].volume;
        let low = (v as i8) < (a as i8);
        self.t("SubVol", low as usize);
        let n = if low { 0 } else { v.wrapping_sub(a) };
        self.voices[i].volume = n;
        self.vol_out(i, hw);
    }

    /// The note's period in the voice's finetune: the table of finetune 0 is
    /// searched for it, the last entry stands for one not in it.
    fn adjust_finetune(&mut self, period: u16, ft: u8) -> u16 {
        if ft == 0 {
            self.t("Ft", 0);
            return period;
        }
        let k = (0..48).find(|&k| NP_PERIODS[0][k] == period).unwrap_or(47);
        self.t("Ft", 1);
        self.tn("FtStep", 0, k as u32);
        NP_PERIODS[ft as usize & 15][k]
    }

    fn adjust_tone(&mut self, period: i32, ft: u8) -> i32 {
        let t = &NP_PERIODS[ft as usize & 15];
        let k = t.iter().position(|&p| period >= p as i32);
        self.t("Tone", k.is_none() as usize);
        self.tn("ToneStep", 0, k.unwrap_or(47) as u32);
        t[k.unwrap_or(47)] as i32
    }

    fn apply_pitch(&mut self, i: usize, tone: bool, hw: &mut Hw) {
        let v = self.voices[i];
        self.t("Pitch", tone as usize);
        let p = if tone { self.adjust_tone(v.pitch, v.finetune) } else { v.pitch };
        self.per(hw, i, p);
    }

    fn do_portamento(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        if v.port_end == 0 {
            self.t("Port", 0);
            return;
        }
        let case;
        if v.port_up {
            v.pitch -= v.port_speed;
            case = if v.pitch <= v.port_end { 2 } else { 1 };
            if v.pitch <= v.port_end {
                v.pitch = v.port_end;
                v.port_end = 0;
            }
        } else {
            v.pitch += v.port_speed;
            case = if v.pitch >= v.port_end { 4 } else { 3 };
            if v.pitch >= v.port_end {
                v.pitch = v.port_end;
                v.port_end = 0;
            }
        }
        self.t("Port", case);
        self.apply_pitch(i, self.voices[i].tone_port, hw);
    }

    fn slide(&mut self, i: usize, up: bool, tone: bool, hw: &mut Hw) {
        let v = &mut self.voices[i];
        let lim = NP_LIMITS[v.finetune as usize & 15];
        let clamp;
        if up {
            v.pitch -= v.arg as i32;
            clamp = v.pitch < lim[1] as i32;
            v.pitch = v.pitch.max(lim[1] as i32);
        } else {
            v.pitch += v.arg as i32;
            clamp = v.pitch > lim[0] as i32;
            v.pitch = v.pitch.min(lim[0] as i32);
        }
        self.t("Slide", up as usize);
        if clamp {
            self.t("SlideLim", up as usize);
        }
        self.apply_pitch(i, tone, hw);
    }

    /// A voice's cell in a row step.
    fn setup(&mut self, i: usize, a: u32, hw: &mut Hw, mask: &mut u16) {
        let note = hw.bus.r32(a);
        let sample = (note >> 27) as u8;
        let period = (note >> 16) as u16 & 0x7ff;
        let (effect, arg) = ((note >> 8) as u8, note as u8);
        {
            let v = &mut self.voices[i];
            v.sample = sample;
            v.period = period;
            v.effect = effect;
            v.arg = arg;
        }
        // a sample selection and an effect take their time before the voice's writes
        self.t("Smp", (sample != 0) as usize);
        self.t("E0", effect as usize);
        if sample != 0 {
            let info = self.base + 0xa + (sample as u32 - 1) * 0x2e;
            let entry = hw.bus.r32(self.mem + SAMPLES_OFF + 4 * (sample as u32 - 1));
            let addr = entry.wrapping_add(hw.bus.r32(info + 0x1e));
            let v = &mut self.voices[i];
            v.start = addr;
            let mut case = 0;
            if addr != 0 {
                v.len = hw.bus.r16(info + 0x22);
                let ll = hw.bus.r16(info + 0x28);
                v.looped = 1 + ll.min(2) as u8;
                if ll == 0 {
                    v.loop_start = addr;
                    v.loop_len = 1;
                    case = 1;
                } else {
                    v.loop_start = addr.wrapping_add(hw.bus.r32(info + 0x24));
                    v.loop_len = ll;
                    case = 2;
                }
                v.finetune = hw.bus.r8(info + 0x2a);
                v.volume = hw.bus.r8(info + 0x2b);
            }
            self.t("SmpAddr", case);
        }
        self.t("Per", (period != 0) as usize + 2 * (sample != 0) as usize);
        if period != 0 {
            if matches!(effect, 0x1b..=0x1d) {
                self.t("TP", 0);
                let ft = self.voices[i].finetune;
                let end = self.adjust_finetune(period, ft) as i32;
                let v = &mut self.voices[i];
                v.port_end = end;
                v.port_up = false;
                let mut case = 0;
                if v.port_end == v.pitch {
                    v.port_end = 0;
                    case = 1;
                } else if v.port_end < v.pitch {
                    v.port_up = true;
                    case = 2;
                }
                self.t("TPc", case);
                self.set_volume(i, hw);
                self.t("Ret", 0);
                return;
            }
            if period == 0x7ff {
                self.t("N7ff", i);
                self.dma(hw, 1 << i);
                self.wait_lines(hw);
                self.t("N7ffVol", i);
                self.vol(hw, i, 0);
                *mask |= 1 << i;
                self.t("Ret", 1);
                return;
            }
            let (mut offs, mut offs_case) = (false, 0);
            match effect {
                0x1a => self.voices[i].finetune = arg & 15,
                0x17 => {
                    self.t("NDelay", i);
                    self.dma(hw, 1 << i);
                    self.wait_lines(hw);
                    self.t("NDelayVol", 0);
                    self.set_volume(i, hw);
                    self.t("Ret", 2);
                    return;
                }
                0x19 => {
                    let v = &mut self.voices[i];
                    offs = true;
                    let mut case = 0;
                    if arg != 0 {
                        v.offset = (arg as u16) << 7;
                        case = 3;
                    }
                    if v.offset != 0 {
                        if v.offset >= v.len {
                            v.len = 1;
                            case += 1;
                        } else {
                            v.len -= v.offset;
                            v.start = v.start.wrapping_add(v.offset as u32 * 2);
                            case += 2;
                        }
                    }
                    offs_case = case;
                }
                _ => {}
            }
            self.t("Note", i);
            let v = self.voices[i];
            self.dma(hw, 1 << i);
            self.wait_lines(hw);
            // (the original looks at the note's effect here)
            self.t("NoteFx", effect as usize);
            if offs {
                self.t("Offs", offs_case);
            }
            if v.start != 0 {
                self.t("NoteLoc", i);
                self.loc(hw, i, v.start);
                self.t("NoteLen", 0);
                self.len(hw, i, v.len);
                let p = self.adjust_finetune(period, v.finetune);
                self.voices[i].pitch = p as i32;
                self.t("NotePer", 0);
                self.per(hw, i, p as i32);
                self.t("NoteLoop", v.looped as usize);
                *mask |= 1 << i;
            } else {
                self.t("NoteNone", i);
                self.loc(hw, i, 0);
            }
        }
        self.t("D1", effect as usize);
        match effect {
            4 => {
                let n = 64 - s8(arg as u32);
                self.t("Master", (n >= 0) as usize);
                if n >= 0 {
                    self.inv = n;
                }
            }
            0xc => {
                self.t("Arg", (arg != 0) as usize);
                if arg != 0 {
                    self.add_volume(i, arg, hw);
                }
            }
            0xd => {
                self.t("Arg", (arg != 0) as usize);
                if arg != 0 {
                    self.sub_volume(i, arg, hw);
                }
            }
            0x10 => {
                self.t("Arg", (arg != 0) as usize);
                if arg != 0 {
                    self.t("MSlide", (self.inv < arg as i32) as usize);
                    self.inv = (self.inv - arg as i32).max(0);
                    self.vol_out_all(hw);
                }
            }
            0x11 => {
                self.t("Arg", (arg != 0) as usize);
                if arg != 0 {
                    self.t("MSlide", 2 + (self.inv + arg as i32 > 64) as usize);
                    self.inv = (self.inv + arg as i32).min(64);
                    self.vol_out_all(hw);
                }
            }
            3 => {
                self.voices[i].volume = arg;
                self.set_volume(i, hw);
                self.t("Ret", 3);
                return;
            }
            _ => {}
        }
        if period != 0 {
            self.t("NoteVol", 0);
            self.set_volume(i, hw);
        }
        self.t("D2", effect as usize);
        match effect {
            5 => {
                self.t("Arg", (arg != 0) as usize);
                if arg != 0 {
                    self.counter = 0;
                    self.next_retrig = 0;
                    self.arp = 3;
                    self.speed = arg;
                }
            }
            6 => {
                if arg == 0 || arg == 0xff {
                    self.t("Jump", 0);
                    self.new_position = self.position;
                    self.jump = true;
                } else if arg as u16 <= self.positions(hw) {
                    self.t("Jump", 1);
                    self.new_position = (arg as u16).wrapping_sub(2);
                    self.jump = true;
                    if arg as u16 - 1 <= self.position {
                        // the song's end: the host is told (it goes on at a whole colour clock)
                        self.host(hw);
                        self.t("JumpEnd", 0);
                    }
                } else {
                    self.t("Jump", 2);
                }
            }
            // bset.b #1,$bfe001 for 0 (filter off), bclr for the rest
            7 => {
                self.t("Filter", (arg != 0) as usize);
                self.at(hw, 'W');
                let pra = hw.bus.read_byte(0xBFE001);
                hw.bus.write_byte(0xBFE001, if arg == 0 { pra | 2 } else { pra & !2 });
                self.t("FilterEnd", 0);
            }
            8 | 9 => {
                self.t("Arg", (arg != 0) as usize);
                if arg != 0 {
                    self.slide(i, effect == 8, false, hw);
                }
            }
            0xa => {
                self.t("Arg", (arg != 0) as usize);
                self.voices[i].tone_slide = arg != 0;
            }
            0xb => {
                self.t("Tempo", (arg >= 28) as usize);
                if arg >= 28 {
                    self.tempo = arg;
                    self.counter = 0;
                    self.next_retrig = 0;
                    self.arp = 3;
                    // the original sets the host's timer
                    self.host(hw);
                    self.t("TempoEnd", 0);
                }
            }
            0x14 => {
                self.t("Arg", (arg != 0) as usize);
                if arg == 0 {
                    self.voices[i].loop_row = self.row;
                }
            }
            0x15 => {
                let v = &mut self.voices[i];
                let mut case = 0;
                let mut ret = false;
                if arg == 0 {
                    v.loop_row = -1;
                } else if v.loop_row >= 0 {
                    case = 2;
                    if v.loop_cnt == 0 {
                        v.loop_cnt = arg;
                    } else {
                        v.loop_cnt -= 1;
                        case = 3;
                        if v.loop_cnt == 0 {
                            v.loop_row = -1;
                            case = 4;
                            ret = true;
                        }
                    }
                    if !ret {
                        self.loop_row = v.loop_row - 1;
                        self.set_loop = true;
                        v.loop_row = -1;
                    }
                } else {
                    case = 1;
                }
                self.t("PLoop", case);
            }
            0x1e => {
                self.t("Arg", (arg != 0) as usize);
                self.voices[i].tone_port = arg != 0;
            }
            _ => {}
        }
    }

    /// A tick that is not a row, for voice `i`.
    fn make_effects(&mut self, i: usize, hw: &mut Hw) {
        let v = self.voices[i];
        let (a, e) = (v.arg, v.effect);
        if v.period == 0x7ff {
            self.t("X7ff", 0);
            return;
        }
        self.t("XArg", (a != 0) as usize);
        if a != 0 {
            self.t("X", e as usize);
            match e {
                0 => {
                    let x = self.arp - self.counter as i32;
                    if x == 0 {
                        self.arp += 3;
                        self.t("Arp", 0);
                        self.per(hw, i, v.pitch);
                    } else {
                        let off = if x == 1 { a & 15 } else { a >> 4 } as usize;
                        let t = &NP_PERIODS[v.finetune as usize & 15];
                        let k = (0..48).find(|&k| t[k] as i32 == v.pitch);
                        self.t("Arp", if x == 1 { 1 } else { 2 });
                        self.t("ArpFt", (v.finetune & 15) as usize);
                        self.tn("ArpStep", 0, k.unwrap_or(47).min(47 - off) as u32);
                        if k.is_none() {
                            self.t("ArpMiss", 0);
                        }
                        let k = k.unwrap_or(47);
                        if k + off >= 47 {
                            self.t("ArpTop", 0);
                        }
                        self.per(hw, i, t[(k + off).min(47)] as i32);
                    }
                }
                1 => self.slide(i, true, v.tone_slide, hw),
                2 => self.slide(i, false, v.tone_slide, hw),
                0xe => self.add_volume(i, a, hw),
                0xf => self.sub_volume(i, a, hw),
                0x12 => {
                    self.t("MSlide", (self.inv < a as i32) as usize);
                    self.inv = (self.inv - a as i32).max(0);
                    self.vol_out_all(hw);
                }
                0x13 => {
                    self.t("MSlide", 2 + (self.inv + a as i32 > 64) as usize);
                    self.inv = (self.inv + a as i32).min(64);
                    self.vol_out_all(hw);
                }
                0x16 => {
                    if s8(a as u32) < s8(self.speed as u32) {
                        let mut case = 1;
                        if self.next_retrig == 0 {
                            self.next_retrig = a as u32;
                            case = 2;
                        }
                        let hit = self.counter as u32 == self.next_retrig;
                        self.t("Retrig", case + 2 * hit as usize);
                        if hit {
                            self.next_retrig += a as u32;
                            self.play(i, hw);
                        }
                    } else {
                        self.t("Retrig", 0);
                    }
                }
                0x17 => {
                    if s8(a as u32) < s8(self.speed as u32) {
                        let hit = a as u16 == self.counter && (v.sample != 0 || v.period != 0);
                        self.t("Delay", 1 + hit as usize + (hit && v.period != 0) as usize);
                        if hit {
                            if v.period != 0 {
                                let p = self.adjust_finetune(v.period, v.finetune);
                                self.voices[i].pitch = p as i32;
                                self.per(hw, i, p as i32);
                            }
                            self.play(i, hw);
                        }
                    } else {
                        self.t("Delay", 0);
                        self.voices[i].effect = 0;
                        self.voices[i].arg = 0;
                    }
                }
                0x18 => {
                    if s8(a as u32) < s8(self.speed as u32) {
                        let hit = a as u16 == self.counter;
                        self.t("Cut", 1 + hit as usize);
                        if hit {
                            self.voices[i].volume = 0;
                            self.vol(hw, i, 0);
                        }
                    } else {
                        self.t("Cut", 0);
                        self.voices[i].effect = 0;
                        self.voices[i].arg = 0;
                    }
                }
                0x1c => {
                    self.do_portamento(i, hw);
                    self.t("PortVol", 0);
                    self.add_volume(i, a, hw);
                }
                0x1d => {
                    self.do_portamento(i, hw);
                    self.t("PortVol", 1);
                    self.sub_volume(i, a, hw);
                }
                _ => {}
            }
        }
        if e == 0x1b {
            let v = &mut self.voices[i];
            let set = v.arg != 0;
            if set {
                v.port_speed = v.arg as i32;
                v.arg = 0;
            }
            self.t("X1b", set as usize);
            self.do_portamento(i, hw);
        }
    }
}

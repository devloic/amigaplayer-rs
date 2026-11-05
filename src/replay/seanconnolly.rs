//! Sean Connolly (`s-c.*`, `scn.*`; Sean "Odie" Connolly, 1992-95), a four-voice game-music driver.
//!
//! Clean-room, written from a behavioural specification and the Paula register logs of the 68k
//! reference player (one real module, Surf Ninjas, and four synthetic edits of it); no original
//! code. A module is a flat image: the driver's code (never run here), its tables and the music
//! data. The replay interprets the module's data: subsong table, tracks, patterns, instruments,
//! wave and pitch tables, samples.
//!
//! The hardware trails the processed state by one tick: each tick first writes the previous tick's
//! register images, then steps the four voices (3, 2, 1, 0), then waits for ten raster-line edges
//! and writes the DMA enable. The placement in time of those writes (fixed offsets, a cost model of
//! the voice processing and the raster wait) is the specification's.
//!
//! Only the "type 2" driver generation is specified. The table positions of the specification are
//! those of the one measured module; how another module's are derived is not specified, so a
//! module is accepted only when the tables at those positions check out against its own data
//! (the period table's first entries and the one-bit-per-voice words). Types 1 and 3 and any other
//! layout are refused.
//!
//! Choices where the specification leaves the behaviour open: a pattern number above 67 ends the
//! voice; reads past the end of the file give 0; a pitch-sequence read that does not settle in 256
//! steps ends the sequence; the song end (voice 0's track loop, or all voices off) restarts the
//! subsong with a stop group, as the reference host does.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use std::sync::atomic::{AtomicU64, Ordering};

pub const OPEN: Open = Open { name: "Sean Connolly", detect, new };

/// The raster phase (colour clocks modulo the line of 227) of the first tick call; the reference
/// host's differs per run. `u64::MAX` selects the default. A test and measurement knob.
pub static PHASE: AtomicU64 = AtomicU64::new(u64::MAX);
/// Test knob: do not restart the subsong at a song end.
pub static NO_RESTART: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const DEFAULT_PHASE: u64 = 101;
const PHASES: [u64; 7] = [101, 112, 154, 224, 93, 217, 146];
const LINE: u64 = 227;

// Positions of the measured module (specification section 2).
const O_WORDS: usize = 0x28A;
const O_SHADOW: usize = 0x292;
const O_LAST: usize = 0x2D2;
const O_NOISE: usize = 0x312;
const O_NOISE_A: usize = 0x31C;
const O_NOISE_B: usize = 0x35C;
const O_PERIOD: usize = 0x12A2;
const O_PATPTR: usize = 0x1392;
const O_SAMPPTR: usize = 0x14A2;
const O_PSPTR: usize = 0x14E2;
const O_WAVE: usize = 0x14EE;
const O_WSEQ: usize = 0x25EE;
const O_INST: usize = 0x262E;
const O_SONGS: usize = 0x2864;
const O_TRACKS: usize = 0x28D4;
const O_PSDATA: usize = 0x284E;
const O_PATDATA: usize = 0x2A2E;
const O_SAMPDATA: usize = 0x3110;
const O_INFO: usize = 0x10;
const MIN_LEN: usize = O_SAMPDATA;
const PATTERNS: usize = 68;
const SUBSONGS: u16 = ((O_TRACKS - O_SONGS) / 16) as u16;

/// Colour clocks from the tick call to each write of the register images (section 9.1).
const T_DMA_CLEAR: u64 = 362;
const T_LC: [u64; 4] = [400, 546, 694, 837];
const T_LENPER: [u64; 4] = [426, 574, 722, 865];
const T_VOL: [u64; 4] = [526, 674, 817, 965];
const T_OFF_FIRST: u64 = 1178;
const T_OFF_STEP: u64 = 122;
const F_BASE: i64 = 7052;
const F_INACTIVE: i64 = -934;
const EDGES: u32 = 10;
/// Colour clocks from the tick call that finds the song end to the stop call's first write, and
/// from there to the next tick call.
const STOP_DELAY: u64 = 8890;
const AFTER_STOP: u64 = 753;

fn detect(d: &[u8]) -> bool {
    layout_ok(d)
}

/// The generation test of the specification (section 1) and the check of the tables at the
/// measured positions.
fn layout_ok(d: &[u8]) -> bool {
    if d.len() < MIN_LEN {
        return false;
    }
    if be16(d, 0) != 0x6000 || be16(d, 4) != 0x6000 || be16(d, 8) != 0x6000 {
        return false;
    }
    let target = 2usize.wrapping_add(be16(d, 2) as i16 as isize as usize);
    if target.checked_add(8).is_none_or(|e| e > d.len()) {
        return false;
    }
    if be32(d, target) != 0x48E7_7FFE || be32(d, target + 4) != 0xE988_41FA {
        return false;
    }
    [1u16, 2, 4, 8].iter().enumerate().all(|(i, &w)| be16(d, O_WORDS + 2 * i) == w)
        && [0x3580u16, 0x3280, 0x2FA0, 0x2D00, 0x2A60].iter().enumerate().all(|(i, &w)| be16(d, O_PERIOD + 2 * i) == w)
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    speed_reload: u8,
    speed_cnt: u8,
    len_cnt: u8,
    len_reload: u8,
    repeat_note: u8,
    tie: bool,
    /// Offset of the voice's track from the track base, and the position in it.
    track_base: usize,
    track_pos: usize,
    pat_pos: usize,
    transpose: u8,
    inst_base: u8,
    track_repeat: u8,
    inst: u8,
    note_val: u8,
    new_note: bool,
    wave_offset: u8,
    level: u8,
    sustain: u8,
    release_target: u8,
    attack_rate: u8,
    release_rate: u8,
    attack_delay: u8,
    release_delay: u8,
    attack_cnt: u8,
    release_cnt: u8,
    release_phase: bool,
    fade_on: bool,
    fade_cap: u8,
    fade_cnt: u8,
    fade_rate: u8,
    fade_reset: bool,
    period: u16,
    base_period: u16,
    slide_on: bool,
    slide_step: u16,
    slide_from: u8,
    phase_cnt: u8,
    pitch_speed: u8,
    vib_delay: u8,
    vib_shift: u8,
    pitch_mode: u8,
    ps_pos: u8,
    ps_sel: u8,
    mode: u8,
    wave_pos: u16,
    wave_min: i16,
    wave_max: i16,
    wave_step: u8,
    wave_dir: bool,
    samp_idx: u8,
    chunk_len: u16,
    chunk_cnt: u16,
    ws_pos: u16,
    ws_last: u16,
    ws_prev: u16,
    follow1: u8,
    follow2: u8,
}

/// One voice's register image: location, length, period, volume (the volume is a whole word,
/// whose upper byte can carry a leftover of the original).
#[derive(Clone, Copy, Default)]
struct Img {
    lc: u32,
    len: u16,
    per: u16,
    vol: u16,
}

struct SeanConnolly {
    d: Vec<u8>,
    base: u32,
    v: [Voice; 4],
    shadow: [Img; 4],
    last: [Img; 4],
    retrig: u8,
    retrig_prev: u8,
    sub: u16,
    /// The noise generator's x, y, z and the table toggle.
    nx: u16,
    ny: u16,
    nz: u16,
    toggle: u16,
    /// The cost of the voice processing so far, in CPU cycles.
    f: i64,
    songend: bool,
    /// 0 running; 1 the song end was signalled, the next tick call restarts the subsong; 2 the
    /// restart is done and the next tick is the immediate one after it.
    restart: u8,
    /// The grid time of the tick after the restart's immediate one.
    grid: u64,
    /// The time of the next tick when it is not the usual period after this one.
    override_at: Option<u64>,
    next_at: Option<u64>,
    first_due: u64,
    /// Noise table writes of this tick (offset in the module, bytes), applied to guest memory.
    noise_dirty: Vec<(usize, Vec<u8>)>,
}

/// The raster phase of subsong `sub`'s first tick call. The reference host's start takes a
/// different time for each subsong (it runs one more init for each); the values are those of the
/// measured module's seven runs.
fn phase(sub: u16) -> u64 {
    match PHASE.load(Ordering::Relaxed) {
        u64::MAX => PHASES.get(sub as usize).copied().unwrap_or(DEFAULT_PHASE),
        p => p % LINE,
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !layout_ok(d) {
        return None;
    }
    let img = |o: usize| -> [Img; 4] {
        std::array::from_fn(|v| {
            let p = o + 16 * v;
            Img { lc: be32(d, p), len: be16(d, p + 4), per: be16(d, p + 6), vol: be16(d, p + 8) }
        })
    };
    let mut s = SeanConnolly {
        d: d.to_vec(),
        base,
        v: [Voice::default(); 4],
        shadow: img(O_SHADOW),
        last: img(O_LAST),
        retrig: 0,
        retrig_prev: 0,
        sub: 0,
        nx: be16(d, O_NOISE),
        ny: be16(d, O_NOISE + 2),
        nz: 0,
        toggle: 0,
        f: 0,
        songend: false,
        restart: 0,
        grid: 0,
        override_at: None,
        next_at: None,
        first_due: 0,
        noise_dirty: Vec::new(),
    };
    let clock = m.bus.clock;
    s.init(0, clock);
    s.flush_noise(&mut m.bus);
    let due = clock + TICK;
    s.first_due = (phase(0) + LINE - due % LINE) % LINE;
    Some(Box::new(s))
}

/// Colour clocks between two ticks (50 Hz).
const TICK: u64 = 70940;

impl SeanConnolly {
    fn b(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }
    fn l(&self, o: usize) -> u32 {
        be32(&self.d, o)
    }
    fn per_tab(&self, i: u8) -> u16 {
        self.w(O_PERIOD + 2 * i as usize)
    }
    fn inst_b(&self, inst: u8, k: usize) -> u8 {
        self.b(O_INST + 16 * inst as usize + k)
    }
    fn inst_w(&self, inst: u8, k: usize) -> u16 {
        self.w(O_INST + 16 * inst as usize + k)
    }
    fn addr(&self, off: usize) -> u32 {
        self.base.wrapping_add(off as u32)
    }

    /// The beam position word the CPU would read: the line (low 8 bits) over the horizontal count.
    fn vhposr(clock: u64) -> u16 {
        (((clock / LINE) & 0xFF) as u16) << 8 | (((clock % LINE) / 2) as u16 & 0xFF)
    }

    /// Fills one noise table (32 words, last to first) and queues it for guest memory.
    fn gen_noise(&mut self, table_b: bool) {
        let mut words = [0u16; 32];
        for k in (0..32).rev() {
            self.nx = self.nx.wrapping_mul(3).wrapping_add(self.nz).wrapping_add(self.ny);
            self.ny = self.ny.wrapping_add(1);
            self.nz = self.nz.wrapping_add(2);
            words[k] = self.nx;
        }
        let o = if table_b { O_NOISE_B } else { O_NOISE_A };
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
        for (i, b) in bytes.iter().enumerate() {
            if let Some(x) = self.d.get_mut(o + i) {
                *x = *b;
            }
        }
        self.noise_dirty.push((o, bytes));
    }

    fn flush_noise(&mut self, bus: &mut crate::bus::Bus) {
        for (o, bytes) in self.noise_dirty.drain(..) {
            bus.write_bytes(self.base.wrapping_add(o as u32), &bytes);
        }
    }

    /// Init of subsong `s` (section 7.5): the images are not touched.
    fn init(&mut self, s: u16, clock: u64) {
        self.sub = s.min(SUBSONGS - 1);
        self.songend = false;
        self.restart = 0;
        self.override_at = None;
        self.nz = Self::vhposr(clock);
        self.gen_noise(true);
        self.gen_noise(false);
        self.toggle = 0;
        self.retrig = 0;
        // retrig_prev is kept across a re-init: the reference run shows the first tick after a
        // restart clearing the last tick's mask (the specification says init zeroes it; it does not)
        for v in 0..4 {
            let tb = self.l(O_SONGS + 16 * self.sub as usize + 4 * v) as usize;
            self.v[v] = Voice {
                active: true,
                speed_reload: 6,
                speed_cnt: 4,
                len_cnt: 1,
                len_reload: 1,
                fade_cap: 0x40,
                track_base: O_TRACKS.wrapping_add(tb),
                ..Voice::default()
            };
        }
    }

    /// The wrapper's reaction to a song end (section 3 and 8): the stop call, the init of the same
    /// subsong and a tick at once. The reference host does this in the tick call after the one
    /// that signalled the end; how it then catches up its pending timer calls varies, and the
    /// model here is the single immediate tick seen in one of the runs.
    fn restart_subsong(&mut self, hw: &mut Hw, c: u64) {
        let t0 = c + STOP_DELAY;
        hw.bus.clock = t0;
        hw.dmacon(0x000F);
        for ch in 0..4 {
            hw.bus.clock = t0 + 8 * (ch as u64 + 1);
            hw.per(ch, 0);
            hw.vol(ch, 0);
        }
        hw.bus.clock = t0 + 54;
        hw.dmacon(0x000F);
        self.init(self.sub, t0);
        self.flush_noise(hw.bus);
        // the song end stays reported for the host to see
        self.songend = true;
        self.restart = 2;
        self.grid = c + TICK;
        self.override_at = Some(t0 + AFTER_STOP);
    }

    // ---------------------------------------------------------------------------------
    // The tick

    fn run_tick(&mut self, hw: &mut Hw) {
        let c = hw.bus.clock;
        self.override_at = None;
        if self.restart == 1 {
            self.restart_subsong(hw, c);
            return;
        }
        if self.restart == 2 {
            // the tick that follows the restart at once; the next one is on the grid again
            self.restart = 0;
            self.override_at = Some(self.grid);
        }
        if self.v.iter().all(|v| !v.active) {
            self.songend = true;
            self.restart = 1;
            return;
        }
        self.f = F_BASE;
        // the previous tick's images
        hw.bus.clock = c + T_DMA_CLEAR;
        hw.dmacon(self.retrig_prev as u16);
        for ch in 0..4 {
            let im = self.last[ch];
            hw.bus.clock = c + T_LC[ch];
            hw.loc(ch, im.lc);
            hw.bus.clock = c + T_LENPER[ch];
            hw.len(ch, im.len);
            hw.per(ch, im.per);
            hw.bus.clock = c + T_VOL[ch];
            hw.vol(ch, im.vol);
        }
        self.retrig = 0;
        let mut off_n = 0u64;
        for v in (0..4).rev() {
            if self.v[v].active {
                self.voice_step(v);
            } else {
                self.shadow[v].per = 0;
                self.shadow[v].vol = 0;
                self.f += F_INACTIVE;
                hw.bus.clock = c + T_OFF_FIRST + T_OFF_STEP * off_n;
                hw.dmacon(1 << v);
                off_n += 1;
            }
        }
        // noise regeneration: the first voice (0, 1, 2, 3) whose wave number is the noise one
        if self.v.iter().any(|v| v.wave_pos == 0xFFF) {
            self.f += 1408;
            self.toggle = !self.toggle;
            self.gen_noise(self.toggle & 0x8000 != 0);
            self.flush_noise(hw.bus);
        }
        // the raster wait, then the DMA enable
        let t = wait_end(2 * c + self.f.max(0) as u64);
        hw.bus.clock = (t + 87) / 2;
        hw.dmacon(0x8200 | self.retrig_prev as u16);
        self.retrig_prev = self.retrig;
        self.last = self.shadow;
    }

    fn voice_step(&mut self, v: usize) {
        let mut vo = self.v[v];
        vo.speed_cnt = vo.speed_cnt.wrapping_sub(1);
        if vo.speed_cnt == 0 {
            vo.speed_cnt = vo.speed_reload;
            vo.len_cnt = vo.len_cnt.wrapping_sub(1);
            if vo.len_cnt == 0 {
                // a fetch tick
                self.f -= 660;
                if vo.fade_reset {
                    vo.fade_reset = false;
                    vo.fade_on = false;
                    vo.fade_cap = 0x40;
                    self.f += 189;
                }
                vo.slide_on = false;
                self.fetch(v, &mut vo);
                self.v[v] = vo;
                return;
            }
            self.f += 81;
        }
        self.reload(&mut vo);
        self.envelope(v, &mut vo);
        self.mode_step(v, &mut vo);
        self.v[v] = vo;
    }

    /// 6.3: the per-tick reload, the new-note set-up and the fade.
    fn reload(&mut self, vo: &mut Voice) {
        let i = vo.inst & 0x7F;
        vo.wave_min = self.inst_w(i, 10) as i16;
        vo.wave_max = self.inst_w(i, 12) as i16;
        vo.wave_step = self.inst_b(i, 14);
        let b4 = self.inst_b(i, 4);
        vo.attack_rate = b4 >> 4;
        vo.release_rate = b4 & 15;
        vo.release_target = self.inst_b(i, 3);
        let b5 = self.inst_b(i, 5);
        vo.attack_delay = b5 >> 4;
        vo.release_delay = b5 & 15;
        vo.pitch_speed = self.inst_b(i, 0) >> 4;
        if vo.new_note {
            self.f += 328;
            vo.new_note = false;
            vo.ps_pos = 0;
            vo.wave_dir = false;
            vo.sustain = self.inst_b(i, 2);
            vo.attack_cnt = 0;
            vo.release_cnt = 0;
            vo.pitch_mode = self.inst_b(i, 7);
            vo.phase_cnt = 0;
            let b6 = self.inst_b(i, 6);
            vo.vib_delay = (b6 >> 4).wrapping_mul(4);
            vo.vib_shift = b6 & 7;
        }
        if vo.fade_on {
            if vo.fade_cnt != vo.fade_rate {
                vo.fade_cnt = vo.fade_cnt.wrapping_add(1);
                self.f += 81;
            } else {
                vo.fade_cnt = 0;
                if vo.fade_cap != 0 {
                    vo.fade_cap -= 1;
                    self.f += 120;
                } else {
                    self.f += 102;
                }
            }
        }
    }

    /// The volume output of the envelope (6.5).
    fn out_vol(&mut self, v: usize, vo: &Voice, out: u8) {
        if (vo.fade_cap as i8) < (out as i8) {
            self.shadow[v].vol = (((vo.inst & 0x7F) >> 4) as u16) << 8 | vo.fade_cap as u16;
            self.f += 4;
        } else {
            self.shadow[v].vol = out as u16;
        }
    }

    fn envelope(&mut self, v: usize, vo: &mut Voice) {
        if !vo.release_phase {
            if vo.attack_cnt == vo.attack_delay {
                vo.attack_cnt = vo.attack_cnt.wrapping_add(1);
            } else {
                vo.attack_cnt = 0;
                if vo.sustain == vo.level {
                    vo.release_phase = true;
                    self.f += 55;
                } else {
                    vo.level = vo.level.wrapping_add(vo.attack_rate);
                    self.f += 150;
                    self.out_vol(v, vo, vo.level);
                }
            }
        } else if vo.release_cnt == vo.release_delay {
            vo.release_cnt = 0;
            if vo.release_target == vo.sustain {
                self.f += 112;
                self.out_vol(v, vo, vo.release_target);
            } else {
                let n = vo.sustain as i16 - vo.release_rate as i16;
                self.f += 175;
                vo.sustain = if n < 0 {
                    self.f += 4;
                    0
                } else {
                    n as u8
                };
                self.out_vol(v, vo, vo.sustain);
            }
        } else {
            vo.release_cnt = vo.release_cnt.wrapping_add(1);
        }
    }

    // ---------------------------------------------------------------------------------
    // Sound modes

    fn sample_bounds(&self, idx: u8) -> (usize, usize) {
        let p = O_SAMPPTR + 4 * (idx as usize & 15);
        let s = O_SAMPDATA.wrapping_add(self.l(p) as usize);
        let e = O_SAMPDATA.wrapping_add(self.l(p + 4) as usize);
        (s, e)
    }

    fn is_8svx(&self, s: usize) -> bool {
        self.l(s.wrapping_add(8)) == 0x3853_5658
    }

    fn mode_step(&mut self, v: usize, vo: &mut Voice) {
        match vo.mode & 15 {
            0 => {
                self.f += 28;
                self.sweep(v, vo);
                self.pitch(v, vo);
            }
            2 => {
                self.chunk(v, vo);
                self.pitch(v, vo);
            }
            3 => self.wave_seq(v, vo),
            _ => {
                self.sample_loop(v, vo);
                self.pitch(v, vo);
            }
        }
    }

    fn set_wave(&mut self, v: usize, vo: &Voice, wave: u16, sign_ext: bool) {
        let _ = vo;
        if wave == 0xFFF {
            self.shadow[v].lc = self.addr(O_NOISE_A);
            self.shadow[v].len = 0x40;
        } else {
            let sh = (wave << 4) as u32;
            let off = if sign_ext { sh as u16 as i16 as i32 as u32 } else { sh & 0xFFFF };
            self.shadow[v].lc = self.addr(O_WAVE).wrapping_add(off);
            self.shadow[v].len = 8;
        }
    }

    fn sweep(&mut self, v: usize, vo: &mut Voice) {
        if !vo.wave_dir {
            if vo.wave_max <= vo.wave_pos as i16 {
                vo.wave_dir = true;
                self.f -= 121;
                return;
            }
            vo.wave_pos = vo.wave_pos.wrapping_add(vo.wave_step as u16);
        } else {
            if vo.wave_min >= vo.wave_pos as i16 {
                vo.wave_dir = false;
                self.f -= 121;
                return;
            }
            vo.wave_pos = vo.wave_pos.wrapping_sub(vo.wave_step as u16);
            self.f += 10;
        }
        if vo.wave_pos == 0xFFF {
            self.f += 712;
        }
        self.set_wave(v, vo, vo.wave_pos, false);
    }

    fn sample_loop(&mut self, v: usize, vo: &mut Voice) {
        let (s, e) = self.sample_bounds(vo.samp_idx);
        if self.is_8svx(s) {
            let rep = self.w(s.wrapping_add(0x1A));
            if rep <= 4 {
                self.shadow[v].lc = self.addr(e.wrapping_sub(4));
                self.shadow[v].len = 1;
            } else {
                self.f += 63;
                let one = self.w(s.wrapping_add(0x16)) as usize;
                self.shadow[v].lc = self.addr(s.wrapping_add(0x68).wrapping_add(one));
                self.shadow[v].len = rep / 2;
            }
        } else {
            self.shadow[v].lc = self.addr(e.wrapping_sub(4));
            self.shadow[v].len = 1;
        }
    }

    fn chunk(&mut self, v: usize, vo: &mut Voice) {
        let (s, e) = self.sample_bounds(vo.samp_idx);
        let sp = if self.is_8svx(s) { s.wrapping_add(0x68) } else { s };
        let c = vo.chunk_len as u32;
        let k = vo.chunk_cnt as u32;
        let rem = (e.wrapping_sub(sp).wrapping_sub(c as usize)) as u16;
        if rem > (c.wrapping_mul(k)) as u16 {
            self.f += 173;
            self.shadow[v].lc = self.addr(sp.wrapping_add((c * k) as usize));
            self.shadow[v].len = (c / 2) as u16;
            vo.chunk_cnt = vo.chunk_cnt.wrapping_add(1);
        } else {
            vo.level = 0;
            vo.sustain = 0;
            vo.release_target = 0;
            self.shadow[v].vol = 0;
            vo.mode &= 0xF0;
        }
    }

    fn wave_seq(&mut self, v: usize, vo: &mut Voice) {
        self.f += 179;
        vo.ws_prev = vo.ws_last;
        let rec = O_WSEQ + 64 * vo.samp_idx as usize;
        let mut guard = 0;
        let (w, o) = loop {
            let w = self.w(rec + vo.ws_pos as usize);
            if w == 0xFFFF {
                self.f += 193;
                vo.ws_pos = self.w(rec + vo.ws_pos as usize + 2).wrapping_mul(2);
                guard += 1;
                if guard > 64 {
                    return;
                }
                continue;
            }
            break (w, self.w(rec + 32 + vo.ws_pos as usize));
        };
        vo.ws_last = w;
        vo.wave_pos = w;
        if w == 0xFFF {
            self.f += 696;
        }
        self.set_wave(v, vo, w, false);
        if w != 0xFFF && vo.ws_prev == 0xFFF {
            self.retrig |= 1 << v;
            self.f += 24;
        }
        self.shadow[v].per = self.per_tab(((o as u8).wrapping_add(vo.wave_offset)) as u8);
        vo.ws_pos = (vo.ws_pos + 2) & 0x1F;
    }

    // ---------------------------------------------------------------------------------
    // Pitch

    fn pitch(&mut self, v: usize, vo: &mut Voice) {
        if vo.slide_on {
            let target = self.per_tab(vo.note_val);
            let done;
            if vo.slide_from >= vo.note_val {
                vo.period = vo.period.wrapping_add(vo.slide_step);
                self.f += 81;
                self.shadow[v].per = vo.period;
                done = (target as i16) <= (vo.period as i16);
            } else {
                vo.period = vo.period.wrapping_sub(vo.slide_step);
                self.f += 82;
                self.shadow[v].per = vo.period;
                done = (target as i16) >= (vo.period as i16);
            }
            if done {
                self.f += 66;
                vo.slide_on = false;
                vo.period = vo.base_period;
                self.shadow[v].per = vo.base_period;
            }
            return;
        }
        match vo.pitch_mode {
            2 => {
                self.f += 20;
                if vo.phase_cnt != 0 {
                    vo.phase_cnt -= 1;
                    return;
                }
                vo.phase_cnt = vo.pitch_speed;
                let base = O_PSDATA.wrapping_add(self.l(O_PSPTR + 4 * (vo.ps_sel as usize % 3)) as usize);
                let mut i = 0;
                while i < 256 {
                    i += 1;
                    let b = self.b(base.wrapping_add(vo.ps_pos as usize));
                    match b {
                        0xFF => {
                            self.f += 87;
                            vo.ps_pos = self.b(base.wrapping_add(vo.ps_pos as usize + 1));
                        }
                        0xFE => {
                            self.f += 195;
                            vo.pitch_mode = self.b(base.wrapping_add(vo.ps_pos as usize + 1));
                            return;
                        }
                        _ => {
                            let n = if b < 0x19 || b >= 0x80 {
                                self.f += 277;
                                vo.note_val.wrapping_add(b)
                            } else {
                                self.f += 252;
                                b
                            };
                            self.shadow[v].per = self.per_tab(n);
                            vo.ps_pos = vo.ps_pos.wrapping_add(1);
                            return;
                        }
                    }
                }
                // a sequence that does not settle: stop the effect
                vo.pitch_mode = 0;
            }
            1 => {
                self.f += 49;
                if vo.vib_delay != 0 {
                    vo.vib_delay -= 1;
                    return;
                }
                let n = vo.note_val;
                let delta = self.per_tab(n.wrapping_sub(1)).wrapping_sub(self.per_tab(n));
                let lim = vo.pitch_speed.wrapping_mul(2);
                let ph = vo.phase_cnt;
                let d = delta >> vo.vib_shift;
                if ph < lim {
                    self.f += 247;
                    vo.period = vo.period.wrapping_add(d).wrapping_sub(d >> 1);
                } else if ph >= lim.wrapping_mul(2) {
                    self.f += 222;
                    vo.period = self.per_tab(n);
                    vo.phase_cnt = 0;
                    self.shadow[v].per = vo.period;
                    return;
                } else {
                    self.f += 254;
                    vo.period = vo.period.wrapping_sub(d).wrapping_add(d >> 1);
                }
                self.shadow[v].per = vo.period;
                vo.phase_cnt = ph.wrapping_add(1);
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------------------------
    // Track and pattern streams

    fn tb(&self, vo: &Voice, p: usize) -> u8 {
        self.b(vo.track_base.wrapping_add(p))
    }

    fn fetch(&mut self, v: usize, vo: &mut Voice) {
        let mut t = self.tb(vo, vo.track_pos);
        if t == 0xFE {
            vo.active = false;
            return;
        }
        if t == 0xFD {
            self.f += 137;
            let rate = self.tb(vo, vo.track_pos + 1);
            for x in self.v.iter_mut() {
                x.fade_on = true;
                x.fade_cap = 0x40;
                x.fade_cnt = 0;
                x.fade_rate = rate;
            }
            vo.fade_on = true;
            vo.fade_cap = 0x40;
            vo.fade_cnt = 0;
            vo.fade_rate = rate;
            vo.track_pos += 2;
            t = self.tb(vo, vo.track_pos);
        }
        if t & 0xF0 == 0xF0 {
            self.f += 101;
            vo.speed_reload = t & 15;
            vo.speed_cnt = t & 15;
            vo.track_pos += 1;
            t = self.tb(vo, vo.track_pos);
        }
        loop {
            if t & 0xC0 == 0xC0 {
                self.f += 83;
                vo.transpose = t & 0x3F;
                vo.track_pos += 1;
                t = self.tb(vo, vo.track_pos);
            }
            if t & 0x80 != 0 {
                self.f += 74;
                vo.track_repeat = t & 0x3F;
                vo.track_pos += 1;
                t = self.tb(vo, vo.track_pos);
            }
            if t & 0x60 == 0x60 {
                self.f += 222;
                vo.inst_base = t & 0x1F;
                vo.track_pos += 1;
                t = self.tb(vo, vo.track_pos);
                continue;
            }
            break;
        }
        let pat = (t & 0x7F) as usize;
        if pat >= PATTERNS {
            vo.active = false;
            return;
        }
        vo.follow1 = self.tb(vo, vo.track_pos + 1);
        vo.follow2 = self.tb(vo, vo.track_pos + 2);
        self.f += 511;
        let pbase = O_PATDATA.wrapping_add(self.l(O_PATPTR + 4 * pat) as usize);
        self.pattern(v, vo, pbase);
    }

    fn pattern(&mut self, v: usize, vo: &mut Voice, pbase: usize) {
        loop {
            let at = pbase.wrapping_add(vo.pat_pos);
            let t = self.b(at);
            match t {
                0xFE => {
                    self.f -= 81;
                    vo.slide_on = true;
                    vo.slide_from = self.b(at + 1).wrapping_add(vo.transpose);
                    vo.period = self.per_tab(vo.slide_from);
                    vo.slide_step = self.b(at + 2) as u16;
                    vo.pat_pos += 3;
                }
                0xFD => {
                    vo.ps_sel = self.b(at + 1);
                    vo.pat_pos += 2;
                }
                0xFC => {
                    vo.len_cnt = vo.len_reload;
                    break;
                }
                0xFB => {
                    self.f += 50;
                    vo.chunk_len = (self.b(at + 1) as u16) << 8 | self.b(at + 2) as u16;
                    vo.pat_pos += 3;
                }
                0xFA | 0xF9 => {
                    self.f += 174;
                    vo.tie = t == 0xFA;
                    vo.pat_pos += 1;
                }
                0xF8 => {
                    self.f += 241;
                    vo.wave_offset = self.b(at + 1);
                    vo.pat_pos += 2;
                }
                0xC0..=0xF7 => {
                    self.f += 260;
                    vo.inst = (t & 0x3F).wrapping_add(vo.inst_base);
                    vo.pat_pos += 1;
                }
                0x80..=0xBF => {
                    self.f += 273;
                    vo.len_reload = t.wrapping_sub(0x7F);
                    vo.pat_pos += 1;
                }
                0x60..=0x7F => {
                    self.f += 372;
                    vo.repeat_note = t.wrapping_sub(0x5D) & 0x3F;
                    vo.pat_pos += 1;
                }
                0xFF => {
                    // never reached directly: a pattern whose first token is its end ends the voice
                    vo.active = false;
                    return;
                }
                _ => {
                    self.note_start(v, vo, t);
                    break;
                }
            }
        }
        self.after_note(v, vo, pbase);
    }

    /// 5.3
    fn after_note(&mut self, v: usize, vo: &mut Voice, pbase: usize) {
        if vo.repeat_note != 0 {
            self.f -= 55;
            vo.repeat_note -= 1;
            return;
        }
        vo.pat_pos += 1;
        if self.b(pbase.wrapping_add(vo.pat_pos)) != 0xFF {
            return;
        }
        vo.pat_pos = 0;
        if vo.track_repeat != 0 {
            self.f += 71;
            vo.track_repeat -= 1;
            return;
        }
        self.f += 116;
        vo.track_pos += 1;
        if vo.follow1 != 0xFF {
            return;
        }
        self.f += 332;
        vo.track_pos = vo.follow2 as usize;
        vo.transpose = 0;
        vo.inst_base = 0;
        vo.fade_on = false;
        vo.fade_cnt = 0;
        vo.fade_reset = true;
        if v == 0 {
            self.songend = true;
            self.restart = if NO_RESTART.load(Ordering::Relaxed) { 0 } else { 1 };
        }
    }

    fn note_start(&mut self, v: usize, vo: &mut Voice, note: u8) {
        self.f += 1130;
        vo.new_note = true;
        vo.release_phase = false;
        vo.chunk_cnt = 0;
        vo.note_val = note.wrapping_add(vo.transpose);
        vo.base_period = self.per_tab(vo.note_val);
        if !vo.slide_on {
            vo.period = vo.base_period;
        }
        vo.len_cnt = vo.len_reload;
        let i = vo.inst;
        vo.mode = self.inst_b(i, 0);
        vo.level = self.inst_b(i, 1);
        vo.wave_pos = self.inst_w(i, 8);
        vo.wave_dir = false;
        vo.samp_idx = self.inst_b(i, 15);
        self.shadow[v].per = vo.period;
        let mut vol = vo.level as u16;
        if vo.fade_on {
            self.f += 47;
            if (vo.fade_cap as i8) < (vo.level as i8) {
                vol = vo.fade_cap as u16;
            }
        }
        self.shadow[v].vol = vol;
        if vo.tie {
            self.f -= 347;
            return;
        }
        match vo.mode & 15 {
            0 => {
                if vo.wave_pos != 0xFFF {
                    self.f -= 104;
                }
                self.set_wave(v, vo, vo.wave_pos, true);
            }
            2 => {
                self.f += 50;
                let (s, _) = self.sample_bounds(vo.samp_idx);
                self.shadow[v].lc = self.addr(s.wrapping_add(0x68));
                self.shadow[v].len = vo.chunk_len / 2;
            }
            3 => {
                self.f += 61;
                let rec = O_WSEQ + 64 * vo.samp_idx as usize;
                let w = self.w(rec);
                let o = self.w(rec + 32);
                self.set_wave(v, vo, w, false);
                self.shadow[v].per = self.per_tab(o as u8);
                vo.period = self.shadow[v].per;
                vo.ws_pos = 2;
                vo.ws_last = w;
                vo.ws_prev = w;
            }
            _ => {
                let (s, e) = self.sample_bounds(vo.samp_idx);
                let lc = s.wrapping_add(0x68);
                self.shadow[v].lc = self.addr(lc);
                if self.is_8svx(s) {
                    self.f += 60;
                    let one = self.w(s.wrapping_add(0x16));
                    let rep = self.w(s.wrapping_add(0x1A));
                    self.shadow[v].len = one.wrapping_add(rep) / 2;
                } else {
                    self.shadow[v].len = (e.wrapping_sub(2).wrapping_sub(lc) as u16) / 2;
                }
            }
        }
        self.retrig |= 1 << v;
    }
}

/// The raster wait of section 9.3 from cycle `x`: ten edges of the scanline counter, read as the
/// wrapper does. Returns the cycle of the last detecting read.
fn wait_end(x: u64) -> u64 {
    let tod = |t: u64| (t / 2 / LINE) & 0xFF;
    let mut t = x;
    let mut last = x;
    for _ in 0..EDGES {
        let v = tod(t);
        let mut r = t + 16;
        while tod(r) == v {
            r += 25;
        }
        last = r;
        t = r + 35;
    }
    last
}

impl Replayer for SeanConnolly {
    fn format(&self) -> &'static str {
        "Sean Connolly"
    }

    fn title(&self) -> String {
        let mut end = O_INFO;
        while end < 0x14A && self.l(end & !3) != 0 {
            end += 4;
        }
        let text: Vec<u8> = self.d.get(O_INFO..end.min(self.d.len())).unwrap_or(&[]).iter().copied().take_while(|&b| b != 0).collect();
        String::from_utf8_lossy(&text).replace('\r', "\n")
    }

    fn tick_clocks(&self) -> u64 {
        TICK
    }

    fn timer_delay(&self) -> u64 {
        self.first_due
    }

    fn next_tick_at(&self) -> Option<u64> {
        self.override_at
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, SUBSONGS - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let clock = hw.bus.clock;
        self.init(n, clock);
        self.flush_noise(hw.bus);
        let due = clock + TICK;
        self.next_at = Some(due + (phase(self.sub) + LINE - due % LINE) % LINE);
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.next_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.run_tick(hw);
    }
}

#[cfg(test)]
#[path = "seanconnolly_tests.rs"]
mod tests;

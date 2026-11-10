//! PreTracker (Pink/Abyss), ported from Raspberry Casket, Chris 'platon42' Hodges'
//! resourced replayer (Copyright (c) 2022), and rePlayer's PreTracker C player by
//! Daniel Collin (Copyright (c) 2026), both MIT licence: see LICENSE-PreTracker.
//!
//! The original replayer synthesizes its waveforms into chip memory at init; this
//! does it as Raspberry Casket's sample generation does, in the original's 8-bit
//! integer arithmetic (with the switches that make it the original's: no faster
//! code, no bug fixes), not as the C player does in floating point. The playing
//! follows Raspberry Casket with its "paranoia" (the original's checks), its
//! dubious track delay pitch shift and without its fixes, where they change what
//! is played; where the reference (UADE's player, Pink's V1.5 replay code) does
//! otherwise, this follows the reference (see the comments). Its Paula output is
//! the V1.5 replay's: a triggered voice's DMA off with the new sample at volume 0
//! and period $71, three raster lines, all DMA on, three more lines, then each
//! voice's loop, period and volume. The player runs on the vertical blank.

use super::pretracker_tables as t;
use super::{Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "PreTracker", detect, new };

const MAX_VOLUME: u8 = 64;
const MAX_WAVES: usize = 24;
const MAX_INSTRUMENTS: usize = 32;
const NOTE_OFF_PITCH: u8 = 0x3d;
const WI_SIZE: u32 = 42;

// wave info offsets (in the module)
const WI_LOOP_START: u32 = 0x00;
const WI_LOOP_END: u32 = 0x02;
const WI_SUBLOOP_LEN: u32 = 0x04;
const WI_ALLOW_9XX: u32 = 0x06;
const WI_SUBLOOP_WAIT: u32 = 0x07;
const WI_SUBLOOP_STEP: u32 = 0x08;
const WI_CHIPRAM: u32 = 0x0a;
const WI_LOOP_OFFSET: u32 = 0x0c;
const WI_CHORD_NOTE1: u32 = 0x0e;
const WI_CHORD_SHIFT: u32 = 0x11;
const WI_OSC_PHASE_SPD: u32 = 0x13;
const WI_FLAGS: u32 = 0x14;
const WI_OSC_PHASE_MIN: u32 = 0x15;
const WI_OSC_PHASE_MAX: u32 = 0x16;
const WI_OSC_BASENOTE: u32 = 0x17;
const WI_OSC_GAIN: u32 = 0x18;
const WI_SAM_LEN: u32 = 0x19;
const WI_MIX_WAVE: u32 = 0x1a;
const WI_VOL_ATTACK: u32 = 0x1b;
const WI_VOL_DELAY: u32 = 0x1c;
const WI_VOL_DECAY: u32 = 0x1d;
const WI_VOL_SUSTAIN: u32 = 0x1e;
const WI_FLT_TYPE: u32 = 0x1f;
const WI_FLT_RESONANCE: u32 = 0x20;
const WI_PITCH_RAMP: u32 = 0x21;
const WI_FLT_START: u32 = 0x22;
const WI_FLT_MIN: u32 = 0x23;
const WI_FLT_MAX: u32 = 0x24;
const WI_FLT_SPEED: u32 = 0x25;
const WI_MOD_WETNESS: u32 = 0x26;
const WI_MOD_LENGTH: u32 = 0x27;
const WI_MOD_PREDELAY: u32 = 0x28;
const WI_MOD_DENSITY: u32 = 0x29;

/// The unpacked instrument info, as Raspberry Casket's uii_*.
#[derive(Clone, Copy, Default)]
struct Inst {
    vibrato_delay: u16,
    vibrato_depth: u16,
    vibrato_speed: u16,
    adsr_release: u8,
    adsr_attack: u16,
    adsr_decay: u16,
    adsr_sustain: u16,
    pattern_steps: u8,
}

/// A voice's output, as ocd_*.
#[derive(Clone, Copy, Default)]
struct Out {
    ptr: u32,
    len: u16,
    lof: u16,
    per: u16,
    vol: u8,
    trg: u8,
}

/// One channel, as pcd_* (field sizes as there).
#[derive(Clone, Copy, Default)]
struct Chan {
    pat_portamento_dest: u16,
    pat_pitch_slide: u16,
    pat_vol_ramp_speed: u8,
    pat_2nd_inst_num4: u8,
    pat_2nd_inst_delay: u8,
    wave_offset: u8,
    inst_pitch_slide: u16,
    inst_sel_arp_note: u16,
    inst_note_pitch: u16,
    inst_curr_port_pitch: u16,
    inst_line_ticks: u8,
    inst_pitch_pinned: u8,
    inst_vol_slide: u8,
    inst_step_pos: u8,
    inst_wave_num: u16,
    track_delay_offset: u8,
    inst_speed_stop: u8,
    inst_pitch: u16,
    inst_vol: u16,
    loaded_inst_vol: u8,
    pat_vol: u8,
    arp_notes: [u8; 4],
    last_trigger_pos: u16,
    pat_portamento_speed: u8,
    pat_adsr_rel_delay: u8,
    note_off_delay: u8,
    inst_pattern_steps: u8,
    note_delay: u8,
    track_delay_steps: u8,
    track_delay_vol16: u8,
    inst_num4: u16,
    inst_subloop_wait: u16,
    inst_loop_offset: u16,
    /// The instrument (1-based; 0: none).
    inst: u16,
    /// Guest address of the wave info.
    waveinfo: u32,
    channel_mask: u8,
    channel_num: u8,
    adsr_phase: u16,
    adsr_volume: u16,
    adsr_phase_speed: u8,
    inst_ping_pong_dir: u8,
    adsr_pos: u16,
    adsr_vol64: u16,
    new_inst_num: u8,
    vibrato_pos: u16,
    vibrato_delay: u16,
    vibrato_depth: u16,
    vibrato_speed: u16,
    adsr_release: u8,
    out: Out,
    delay: [Out; 32],
}

pub struct PreTracker {
    /// Module bounds in guest memory.
    base: u32,
    size: u32,
    version: u8,
    title: String,
    subsong_count: u16,
    sub: u16,
    // MySong
    waveinfo_table: [u32; MAX_WAVES],
    inst_patterns: [u32; MAX_INSTRUMENTS],
    wavelength: [u32; MAX_WAVES],
    num_waves: u8,
    num_steps: u8,
    patterns: u32,
    curr_pat_pos: u16,
    pat_pos_len: u16,
    pat_restart_pos: u16,
    pos_data: u32,
    waveinfo_ptr: u32,
    insts: [Inst; MAX_INSTRUMENTS],
    // MyPlayer
    pat_curr_row: u8,
    next_pat_row: u8,
    next_pat_pos: u8,
    /// The speed byte of the last Fxx: one speed, or above $2f a shuffle of
    /// two (even rows the high nibble, odd ones the low).
    pat_speed: u8,
    pat_line_ticks: u8,
    pat_stopped: u8,
    /// The empty sample, at the start of the sample buffer.
    sample_buffer: u32,
    wave_samples: [u32; MAX_WAVES],
    period_table: [u16; 16 * 12 * 3],
    ch: [Chan; 4],
    /// Colour clocks per frame (the player runs on the vertical blank).
    frame: u64,
    /// Alternates the colour clock the original's loop spends half of (see `output`).
    half: u64,
    songend: bool,
    /// When the last silence (song start) began, whether the first tick after
    /// it is still to come, and the vertical blank of the last tick.
    silence_at: u64,
    first: bool,
    vblank: u64,
    /// Subsong changes so far (the original's take longer each time).
    changes: u16,
    /// The tick's work so far: rows read, instrument steps, their pattern
    /// entries, a new row.
    work: [u32; 4],
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 0x5c && &d[0..3] == b"PRT" && (0x19..=0x1e).contains(&d[3])
}

/// The start of the positions, the patterns and the four counts of `sub`.
struct Layout {
    restart: u8,
    steps: u8,
    positions: u8,
    pos_data: u32,
    pattern_data: u32,
}

fn layout(d: &[u8], sub: u16) -> Layout {
    let be32 = |o| super::be32(d, o);
    if d[3] == 0x1e {
        // V1.5: a header of 8 bytes for each subsong in front of the positions
        // (restart, patterns, steps, positions, pattern data offset), the
        // subsongs' positions one after the other
        let n = d[0x5a].max(1) as u32;
        let posd = be32(4);
        let mut before = 0u32;
        for i in 0..sub as u32 {
            before += d.get((posd + 8 * i + 3) as usize).copied().unwrap_or(0) as u32;
        }
        let h = (posd + 8 * sub as u32) as usize;
        let b = |k| d.get(h + k).copied().unwrap_or(0);
        Layout {
            restart: b(0),
            steps: b(2),
            positions: b(3),
            pos_data: posd + 8 * n + 8 * before,
            pattern_data: be32(8).wrapping_add(super::be32(d, h + 4)),
        }
    } else {
        Layout {
            restart: d[0x3c],
            positions: d[0x3e],
            steps: d[0x3f],
            pos_data: be32(4),
            pattern_data: be32(8),
        }
    }
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let mut p = PreTracker {
        base,
        size: d.len() as u32,
        version: d[3],
        title: d[0x14..0x28].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect::<String>().trim().to_string(),
        subsong_count: if d[3] == 0x1e { d[0x5a].max(1) as u16 } else { 1 },
        sub: 0,
        waveinfo_table: [0; MAX_WAVES],
        inst_patterns: [0; MAX_INSTRUMENTS],
        wavelength: [0; MAX_WAVES],
        num_waves: d[0x41],
        num_steps: 0,
        patterns: 0,
        curr_pat_pos: 0,
        pat_pos_len: 0,
        pat_restart_pos: 0,
        pos_data: 0,
        waveinfo_ptr: 0,
        insts: [Inst::default(); MAX_INSTRUMENTS],
        pat_curr_row: 0,
        next_pat_row: 0xff,
        next_pat_pos: 0xff,
        pat_speed: 6,
        pat_line_ticks: 6,
        pat_stopped: 1,
        sample_buffer: 0,
        wave_samples: [0; MAX_WAVES],
        period_table: [0; 16 * 12 * 3],
        ch: [Chan::default(); 4],
        frame: m.bus.frame,
        half: 0,
        songend: false,
        silence_at: 0,
        first: true,
        vblank: 0,
        changes: 0,
        work: [0; 4],
    };
    p.song_init(d);

    // the sample buffer: the empty sample (a word), then each wave
    let mut total = 2u32;
    let mut wavetotal = [0u32; MAX_WAVES];
    for i in 0..p.num_waves as usize {
        let l = p.wavelength[i];
        wavetotal[i] = if m.bus.r8(p.waveinfo_table[i] + WI_FLAGS) & 4 != 0 { l * 15 / 8 } else { l };
        total += wavetotal[i];
    }
    let buf = m.alloc(total);
    if buf == 0 {
        return None;
    }
    p.sample_buffer = buf;
    let mut a = buf + 2;
    for i in 0..p.num_waves as usize {
        p.wave_samples[i] = a;
        a += wavetotal[i];
    }
    for n in 0..36 {
        let (lo, hi) = (t::PERIODS[n] as i32, t::PERIODS[n + 1] as i32);
        let mut acc = lo << 16;
        let inc = ((hi - lo) << 16) >> 4;
        for k in 0..16 {
            p.period_table[16 * n + k] = (acc >> 16) as u16;
            acc = acc.wrapping_add(inc);
        }
    }
    Wavegen::run(&p, &mut m.bus, total);
    p.init_channels();

    let mut hw = Hw { bus: &mut m.bus };
    p.start(&mut hw);
    Some(Box::new(p))
}

impl PreTracker {
    /// pre_SongInit, for subsong `sub`.
    fn song_init(&mut self, d: &[u8]) {
        let l = layout(d, self.sub);
        self.pat_restart_pos = l.restart as u16;
        self.pat_pos_len = l.positions as u16;
        self.num_steps = l.steps;
        self.pos_data = self.base.wrapping_add(l.pos_data);
        self.patterns = self.base.wrapping_add(l.pattern_data);
        // the instrument names (32, or 64 for V1.5), of up to 23 characters
        let mut o = super::be32(d, 0xc) as usize;
        let names = if self.version == 0x1e { 64 } else { 32 };
        for _ in 0..names {
            let mut k = 0;
            loop {
                let c = d.get(o).copied().unwrap_or(0);
                o += 1;
                k += 1;
                if c == 0 || k == 23 {
                    break;
                }
            }
        }
        let count = d[0x40] as usize;
        let mut pat = o + 8 * count;
        for i in 0..count.min(MAX_INSTRUMENTS) {
            let b = |k: usize| d.get(o + 8 * i + k).copied().unwrap_or(0) as usize;
            self.inst_patterns[i] = self.base.wrapping_add(pat as u32);
            // a value past its table reads what follows it in the original
            let tb = |tab: &[u8], k: usize| tab.get(k).copied().unwrap_or(0);
            let depth = tb(&t::VIB_DEPTH, b(1)) as u16;
            let speed = tb(&t::VIB_SPEED, b(2)) as i16;
            let sustain = if b(5) == 15 { 16 } else { b(5) as u16 };
            self.insts[i] = Inst {
                vibrato_delay: tb(&t::VIB_DELAY, b(0)) as u16 + 1,
                vibrato_depth: depth,
                vibrato_speed: ((speed as i32 * depth as i16 as i32) as i16 >> 4) as u16,
                adsr_attack: t::FAST_ROLL_OFF_16.get(b(3)).copied().unwrap_or(0),
                adsr_decay: tb(&t::RAMP_UP_16, b(4)) as u16,
                adsr_sustain: sustain << 6,
                adsr_release: tb(&t::RAMP_UP_16, b(6)),
                pattern_steps: b(7) as u8,
            };
            pat += 3 * b(7);
        }
        // the wave names (24), then the wave infos at an even address
        let mut o = super::be32(d, 0x10) as usize;
        for _ in 0..MAX_WAVES {
            let mut k = 0;
            loop {
                let c = d.get(o).copied().unwrap_or(0);
                o += 1;
                k += 1;
                if c == 0 || k == 23 {
                    break;
                }
            }
        }
        let wi = (self.base.wrapping_add(o as u32) + 1) & !1;
        self.waveinfo_ptr = wi;
        for i in 0..self.num_waves as usize {
            let w = wi + WI_SIZE * i as u32;
            self.waveinfo_table[i.min(MAX_WAVES - 1)] = w;
            let sl = d.get((w - self.base + WI_SAM_LEN) as usize).copied().unwrap_or(0) as u32;
            self.wavelength[i.min(MAX_WAVES - 1)] = (sl + 1) << 7;
        }
    }

    fn init_channels(&mut self) {
        for (i, c) in self.ch.iter_mut().enumerate() {
            *c = Chan::default();
            c.pat_vol = MAX_VOLUME;
            c.track_delay_offset = 0xff;
            c.waveinfo = self.waveinfo_ptr;
            c.adsr_phase = 3;
            // forwards (Raspberry Casket starts backwards; the C player and
            // the reference forwards)
            c.inst_ping_pong_dir = 0xff;
            c.out.ptr = self.sample_buffer;
            c.out.len = 2;
            c.out.per = 0x7b;
            c.channel_num = i as u8;
            c.channel_mask = 1 << i;
        }
        self.pat_curr_row = 0;
        self.next_pat_row = 0xff;
        self.next_pat_pos = 0xff;
        self.pat_speed = 6;
        // the first row lasts 15 ticks in the reference when it sets no
        // speed (Raspberry Casket and the C player: 6)
        self.pat_line_ticks = 15;
        self.pat_stopped = 1;
        self.curr_pat_pos = 0;
    }

    /// What the original does before its first tick: the filter off, the
    /// voices cleared and on, the sample generation, then all voices on the
    /// empty sample; the first tick comes on the second vertical blank after.
    fn start(&mut self, hw: &mut Hw) {
        use m68k::AddressBus;
        // the audio interrupts off
        hw.bus.write_word(0xdff09a, 0x0780);
        hw.wait(10);
        hw.bus.write_word(0xdff09c, 0x0780);
        hw.wait(10);
        hw.bus.write_word(0xdff09c, 0x0780);
        hw.wait(18);
        let x = hw.bus.read_byte(0xbfe001);
        hw.bus.write_byte(0xbfe001, x | 2);
        hw.wait(10);
        hw.dmacon(0x000f);
        hw.wait(71013);
        for ch in 0..4 {
            hw.loc(ch, 0);
            hw.wait(10);
            hw.len(ch, 2);
            hw.wait(10);
            hw.vol(ch, 0);
            hw.wait(10);
            hw.per(ch, 0);
            hw.wait(10);
        }
        hw.dmacon(0x820f);
        // the sample generation, which takes seconds on a 68000, is not waited
        // for: the silence comes at the middle of the next frame
        let f = hw.bus.clock / self.frame + 2;
        let now = hw.bus.clock;
        hw.wait(f * self.frame + self.frame / 2 - now);
        self.silence(hw);
    }

    /// All voices on the empty sample, as the original's song start writes
    /// them (at init, and again for each change of subsong).
    fn silence(&mut self, hw: &mut Hw) {
        self.silence_at = hw.bus.clock;
        self.first = true;
        for ch in 0..4 {
            hw.dmacon(1 << ch);
            hw.wait(16);
            hw.loc(ch, self.sample_buffer);
            hw.wait(18);
            hw.len(ch, 1);
            hw.wait(10);
            hw.vol(ch, 0);
            hw.wait(18);
            hw.per(ch, 0x71);
            hw.wait(21);
        }
        // the rest of the song start's work
        let now = hw.bus.clock;
        hw.wait((self.silence_at + SONG_START_WORK).saturating_sub(now));
    }

    /// The vertical blank of this tick: the second after the song start's
    /// work for the first, then one each frame (the engine's ticks come a
    /// frame apart, at most a frame before it).
    fn next_vblank(&mut self) -> u64 {
        self.vblank = if self.first {
            self.first = false;
            ((self.silence_at + SONG_START_WORK) / self.frame + 2) * self.frame
        } else {
            self.vblank + self.frame
        };
        self.vblank
    }

    /// The original's time from the vertical blank to its tick's first write,
    /// by the work the tick did: a least-squares fit of the reference's times
    /// over 37 modules (rms error 309 colour clocks, 1247 for a constant).
    fn work_time(&self, bus: &Bus) -> u64 {
        let w = self.work.map(|x| x as i64);
        let high = self.period_table[0x219];
        let mut t = 193 + 514 * w[0] + 177 * w[1] + 171 * w[2] + 184 * w[3];
        for c in self.ch.iter() {
            t += [366, 166, 105, 134][c.adsr_phase.min(3) as usize];
            t += if c.out.trg != 0 { 516 } else { 0 };
            t += if c.inst != 0 { 197 } else { 0 };
            t -= if c.track_delay_steps != 0 { 169 } else { 0 };
            t += if bus.r16(c.waveinfo.wrapping_add(WI_SUBLOOP_LEN)) != 0 { 116 } else { 0 };
            t += if c.out.per < high { 107 } else { 0 };
            t += if c.vibrato_speed != 0 && c.vibrato_delay == 0 { 4 } else { 0 };
            t += if c.pat_portamento_dest != 0 { 49 } else { 0 };
        }
        t.max(0) as u64
    }

    fn inst(&self, n: u16) -> Inst {
        if n == 0 { Inst::default() } else { self.insts.get(n as usize - 1).copied().unwrap_or_default() }
    }


    /// Loads an instrument (`n4`: its number times 4) as a new note does.
    fn trigger_inst(&mut self, c: usize, n4: u16, keep_port: bool) {
        let inst = self.inst(n4 >> 2);
        let ch = &mut self.ch[c];
        ch.new_inst_num = n4 as u8;
        ch.inst_num4 = n4;
        ch.inst = n4 >> 2;
        ch.inst_pattern_steps = inst.pattern_steps;
        ch.pat_portamento_dest = 0;
        ch.pat_pitch_slide = 0;
        ch.pat_vol_ramp_speed = 0;
        ch.pat_2nd_inst_num4 = 0;
        ch.pat_2nd_inst_delay = 0;
        ch.wave_offset = 0;
        ch.inst_pitch_slide = 0;
        ch.inst_sel_arp_note = 0;
        ch.inst_note_pitch = 0;
        if !keep_port {
            ch.inst_curr_port_pitch = 0;
        }
        ch.inst_line_ticks = 0;
        ch.inst_pitch_pinned = 0;
        ch.inst_vol_slide = 0;
        ch.inst_step_pos = 0;
        ch.inst_wave_num = 0x00ff;
        ch.track_delay_offset = 0xff;
        ch.inst_speed_stop = 1;
        ch.inst_pitch = 0x10;
        ch.inst_vol = MAX_VOLUME as u16;
        ch.loaded_inst_vol = MAX_VOLUME;
        ch.pat_vol = MAX_VOLUME;
    }

    fn release(&mut self, c: usize) {
        let ch = &mut self.ch[c];
        let v = (ch.adsr_volume as i16 >> 6) as u16;
        ch.adsr_vol64 = v;
        ch.adsr_pos = 16;
        let s = (16u16.wrapping_sub(v)) >> 1;
        ch.adsr_phase_speed = (s as u8).wrapping_add(ch.adsr_release);
        ch.adsr_phase = 3;
    }

    /// The pattern part of a tick: each channel's row, when it has not been
    /// read yet, and its pattern effects.
    fn process_pattern(&mut self, bus: &Bus) {
        let mut c = 0usize;
        loop {
            match self.pattern_channel(c, bus) {
                Next::Test => {}
                Next::Cont => {
                    if self.ch[c].track_delay_steps != 0 {
                        // the next channel only plays this one's delayed output
                        if self.ch[c].channel_num >= 2 {
                            break;
                        }
                        c += 1;
                    }
                }
                Next::TrackDelay => {
                    if self.ch[c].channel_num >= 2 {
                        break;
                    }
                    c += 1;
                }
            }
            if self.ch[c].channel_num == 3 {
                break;
            }
            c += 1;
        }
    }

    fn pattern_channel(&mut self, c: usize, bus: &Bus) -> Next {
        // the tracker's own release delay (nothing in a module sets it)
        let d = self.ch[c].pat_adsr_rel_delay;
        if (d as i8) > 0 {
            self.ch[c].pat_adsr_rel_delay = d - 1;
            if d == 1 {
                self.release(c);
            }
        }
        let mut second = false;
        let n4 = self.ch[c].pat_2nd_inst_num4 as u16;
        if n4 != 0 {
            let dl = self.ch[c].pat_2nd_inst_delay;
            if dl != 0 {
                self.ch[c].pat_2nd_inst_delay = dl - 1;
            } else {
                self.trigger_inst(c, n4, true);
                second = true;
            }
        }
        if !second {
            let ch = &mut self.ch[c];
            let dest = ch.pat_portamento_dest;
            if dest != 0 {
                let mut p = ch.inst_curr_port_pitch as i16;
                let s = ch.pat_portamento_speed as i16;
                let dest = dest as i16;
                if p >= dest {
                    p = p.wrapping_sub(s);
                    if p < dest {
                        ch.pat_portamento_dest = 0;
                        p = dest;
                    }
                } else {
                    p = p.wrapping_add(s);
                    if p > dest {
                        ch.pat_portamento_dest = 0;
                        p = dest;
                    }
                }
                ch.inst_curr_port_pitch = p as u16;
            }
            let r = ch.pat_vol_ramp_speed;
            if r != 0 {
                let mut v = r.wrapping_add(ch.pat_vol) as i8;
                if v < 0 {
                    v = 0;
                }
                if v > MAX_VOLUME as i8 {
                    v = MAX_VOLUME as i8;
                }
                ch.pat_vol = v as u8;
            }
        }

        // continue_with_inst: the note delay, and the row once
        let nd = self.ch[c].note_delay;
        if (nd as i8) < 0 {
            return Next::Cont;
        }
        if nd != 0 {
            let nd = nd - 1;
            if nd != 0 {
                self.ch[c].note_delay = nd;
                return Next::Test;
            }
            self.ch[c].note_delay = 0xff;
        }
        let pos = self.pos_data.wrapping_add(
            ((self.curr_pat_pos.wrapping_mul(4).wrapping_add(c as u16)).wrapping_mul(2)) as i16 as i32 as u32,
        );
        let row = self.pat_curr_row;
        if row >= self.num_steps {
            return Next::Cont;
        }
        let pat = bus.r8(pos);
        if pat == 0 {
            return Next::Cont;
        }
        self.work[0] += 1;
        let mut d0 = bus.r8(pos + 1) as i8 as i16 as u16;
        let a0 = self
            .patterns
            .wrapping_add((pat as u32 - 1) * self.num_steps as u32 * 3)
            .wrapping_add(row as u32 * 3);
        let ie = bus.r8(a0 + 1);
        let mut d2 = (ie & 15) as u16;
        let mut d4 = (ie >> 4) as u16;
        let d5 = bus.r8(a0 + 2) as u16;
        if d2 == 0xe && self.ch[c].note_delay == 0 {
            let lo = (d5 & 15) as u8;
            match d5 >> 4 {
                0xd => {
                    if self.pat_speed != 0 {
                        self.ch[c].note_delay = lo;
                        return Next::Test;
                    }
                }
                0xa => self.ch[c].note_off_delay = lo,
                _ => {}
            }
        }
        self.ch[c].note_delay = 0xff;

        let mut d6 = 0u8;
        let pc = bus.r8(a0);
        if pc & 0x80 != 0 {
            d4 += 16;
        }
        let d7 = (pc & 0x3f) as u16;
        d4 <<= 2;
        if d4 != 0 && d7 == 0 {
            // only an instrument, no pitch
            self.ch[c].pat_vol = self.ch[c].loaded_inst_vol;
            if d4 == self.ch[c].inst_num4 {
                self.ch[c].adsr_phase = 0;
                self.ch[c].adsr_volume = 0;
            }
        }
        let mut d3: u8 = pc & 0x40;
        // the routes through the original's branches
        enum R {
            Done,
            Trigger,
            ClearPort,
        }
        let mut route = R::Done;
        if d3 == 0 {
            if d2 == 0 {
                // 0xx: a second instrument
                if d5 != 0 {
                    let d1 = (d5 & 15) << 2;
                    if d7 != 0 {
                        d3 = d4 as u8;
                        d4 = d1;
                    } else {
                        d0 = (d0.wrapping_add(1)) << 4;
                        if d1 as u8 == 0 {
                            self.ch[c].arp_notes = [0; 4];
                            d2 = d7;
                            d3 = d4 as u8;
                            route = R::ClearPort;
                        } else {
                            d2 = 0;
                            d3 = d4 as u8;
                            d4 = d1;
                            route = R::Trigger;
                        }
                    }
                } else {
                    d2 = 0;
                    d3 = 0;
                }
            }
        } else {
            if (d2 as u8 | d5 as u8) != 0 {
                let ch = &mut self.ch[c];
                ch.arp_notes[0] = d2 as u8;
                ch.arp_notes[1] = (d5 >> 4) as u8;
                ch.arp_notes[2] = (d5 & 15) as u8;
            }
            d6 = 1;
            d2 = 0;
            d3 = 0;
        }

        match route {
            R::Done => {
                if d7 as u8 == NOTE_OFF_PITCH {
                    self.release(c);
                    return self.pattern_effects(c, d2, d3, d5, d6);
                }
                if d7 == 0 {
                    return self.pattern_effects(c, d2, d3, d5, d6);
                }
                d0 = (d0.wrapping_add(d7)) << 4;
                if d4 as u8 != 0 {
                    if d2 as u8 == 3 {
                        // a portamento to the note
                        if d6 == 0 {
                            self.ch[c].arp_notes = [0; 4];
                            d6 = 0;
                        }
                        self.new_portamento(c, d0, d5);
                        return self.pattern_effects(c, d2, d3, d5, d6);
                    }
                    self.trigger_inst(c, d4, false);
                }
                self.after_trigger(c, d0, d2, d5, &mut d6);
            }
            R::Trigger => {
                self.trigger_inst(c, d4, false);
                self.after_trigger(c, d0, d2, d5, &mut d6);
            }
            R::ClearPort => {
                let ch = &mut self.ch[c];
                ch.inst_pitch = 0x10;
                ch.inst_curr_port_pitch = d0;
                ch.pat_portamento_dest = 0;
            }
        }
        self.pattern_effects(c, d2, d3, d5, d6)
    }

    fn after_trigger(&mut self, c: usize, d0: u16, d2: u16, d5: u16, d6: &mut u8) {
        if *d6 == 0 {
            self.ch[c].arp_notes = [0; 4];
        }
        if d2 as u8 == 3 {
            self.new_portamento(c, d0, d5);
        } else {
            let ch = &mut self.ch[c];
            ch.inst_pitch = 0x10;
            ch.inst_curr_port_pitch = d0;
            ch.pat_portamento_dest = 0;
        }
    }

    fn new_portamento(&mut self, c: usize, d0: u16, d5: u16) {
        let ch = &mut self.ch[c];
        ch.pat_portamento_dest = d0.wrapping_add(0x10);
        ch.inst_curr_port_pitch = ch.inst_curr_port_pitch.wrapping_add(ch.inst_pitch);
        ch.inst_pitch = 0;
        if d5 as u8 != 0 {
            ch.pat_portamento_speed = d5 as u8;
        }
    }

    fn pattern_effects(&mut self, c: usize, d2: u16, d3: u8, d5: u16, d6: u8) -> Next {
        let ch = &mut self.ch[c];
        if d3 != 0 {
            ch.pat_2nd_inst_num4 = d3;
            ch.pat_2nd_inst_delay = (d5 as u8) >> 4;
        }
        ch.pat_vol_ramp_speed = 0;
        ch.pat_pitch_slide = 0;
        if d6 != 0 {
            return Next::Cont;
        }
        let d5b = d5 as u8;
        match d2 & 15 {
            0xf => {
                // the original shuffles above $2f (Raspberry Casket from $2f)
                self.pat_speed = d5b;
                if d5b > 0x2f {
                    self.pat_line_ticks = if self.pat_curr_row & 1 != 0 { d5b & 15 } else { d5b >> 4 };
                } else {
                    self.pat_line_ticks = d5b;
                }
                self.pat_stopped = (d5b != 0) as u8;
                if d5b == 0 {
                    self.songend = true;
                }
            }
            4 => {
                ch.vibrato_pos = 0;
                ch.vibrato_delay = 1;
                ch.vibrato_depth = (ch.vibrato_depth & 0xff00) | t::VIB_DEPTH[(d5 & 15) as usize] as u16;
                let s = t::VIB_SPEED[(d5b >> 4) as usize] as i16 as i32;
                ch.vibrato_speed = ((s * ch.vibrato_depth as i16 as i32) as i16 >> 4) as u16;
            }
            5 => {
                if ch.channel_num == 3 {
                    return Next::Cont;
                }
                for o in self.ch[c + 1].delay.iter_mut() {
                    o.vol = 0;
                }
                let ch = &mut self.ch[c];
                if d5b == 0 {
                    ch.track_delay_steps = 0xff;
                    return Next::TrackDelay;
                }
                ch.track_delay_steps = (d5b & 15) << 1;
                ch.track_delay_vol16 = d5b >> 4;
            }
            0xa => {
                if d5b != 0 {
                    // the original negates the whole byte (Raspberry Casket
                    // only the low nibble)
                    ch.pat_vol_ramp_speed = if d5b & 15 != 0 { d5b.wrapping_neg() } else { d5b >> 4 };
                }
            }
            1 => ch.pat_pitch_slide = d5,
            2 => ch.pat_pitch_slide = d5.wrapping_neg(),
            9 => ch.wave_offset = d5b,
            0xb => self.next_pat_pos = d5b,
            0xd => self.next_pat_row = d5b,
            0xc => ch.pat_vol = d5b.min(MAX_VOLUME),
            _ => {}
        }
        Next::Cont
    }

    fn advance_pattern(&mut self) {
        self.pat_line_ticks = self.pat_line_ticks.wrapping_sub(1);
        if self.pat_line_ticks != 0 {
            return;
        }
        self.work[3] += 1;
        for ch in self.ch.iter_mut() {
            ch.note_delay = 0;
        }
        let steps = self.num_steps;
        let mut row = self.pat_curr_row.wrapping_add(1);
        let mut pos = self.curr_pat_pos;
        let brk = self.next_pat_row;
        let mut next = false;
        if (brk as i8) >= 0 {
            self.next_pat_row = 0xff;
            row = brk;
            if row >= steps {
                row = steps.wrapping_sub(1);
            }
            next = true;
        } else if row >= steps {
            row = 0;
            next = true;
        }
        if next {
            pos = pos.wrapping_add(1);
        }
        let jump = self.next_pat_pos;
        if (jump as i8) >= 0 {
            self.next_pat_pos = 0xff;
            if (jump as u16) <= self.curr_pat_pos {
                self.songend = true;
            }
            pos = (pos & 0xff00) | jump as u16;
            row = 0;
        }
        if (pos as i16) >= (self.pat_pos_len as i16) {
            pos = self.pat_restart_pos;
            self.songend = true;
        }
        self.pat_curr_row = row;
        self.curr_pat_pos = pos;
        self.pat_line_ticks = if self.pat_speed > 0x2f {
            if row & 1 != 0 { self.pat_speed & 15 } else { self.pat_speed >> 4 }
        } else {
            self.pat_speed
        };
    }

    /// The instrument part of a tick, for every channel: its instrument
    /// pattern, ADSR, volume, loop, pitch, and the track delay.
    fn process_instruments(&mut self, bus: &Bus) {
        let mut c = 0usize;
        loop {
            self.inst_channel(c, bus);
            // the track delay
            let d3 = self.ch[c].track_delay_steps;
            if d3 != 0 {
                if self.ch[c].channel_num == 3 {
                    return;
                }
                let mut d0: u16 = 31;
                let d1: u16;
                if d3 == 0xff {
                    let ch = &mut self.ch[c];
                    ch.track_delay_steps = 0;
                    let n = &mut self.ch[c + 1];
                    n.pat_vol = 0;
                    n.track_delay_steps = 0;
                    n.track_delay_offset = 0xff;
                    d1 = 0;
                } else {
                    let off = (self.ch[c + 1].track_delay_offset.wrapping_add(1)) & 31;
                    self.ch[c + 1].track_delay_offset = off;
                    let mut e = self.ch[c].out;
                    // the first delayed output triggers its voice (Raspberry
                    // Casket writes the zero it has tested there: the
                    // reference triggers)
                    if e.trg == 0 && self.ch[c + 1].track_delay_steps == 0 {
                        e.trg = self.ch[c + 1].channel_mask;
                    }
                    let v16 = self.ch[c].track_delay_vol16 as i8 as i16 as u16;
                    e.vol = ((v16.wrapping_mul(e.vol as u16)) >> 4) as u8;
                    self.ch[c + 1].delay[off as usize] = e;
                    self.ch[c + 1].track_delay_steps = d3;
                    let k = off.wrapping_sub(d3);
                    d0 &= k as u16;
                    d1 = t::MINUS4_PLUS4[(k & 7) as usize] as i16 as u16;
                }
                let n = &mut self.ch[c + 1];
                n.out = n.delay[d0 as usize];
                // the original shifts the delayed voice's period by the distance
                let per = n.out.per;
                let sh = ((((d1 as i16 as i32) * (per as i16 as i32)) << 4) >> 16) as u16;
                n.out.per = per.wrapping_add(sh);
                c += 1;
            }
            if self.ch[c].channel_num == 3 {
                return;
            }
            c += 1;
        }
    }

    fn inst_channel(&mut self, c: usize, bus: &Bus) {
        let wsamp = self.wave_samples;
        if self.ch[c].inst != 0 {
            let ch = &mut self.ch[c];
            let s = ch.inst_pitch_slide.wrapping_add(ch.pat_pitch_slide);
            if s != 0 {
                let mut p = s.wrapping_add(ch.inst_pitch);
                if (p as i16) > 36 << 4 {
                    p = 36 << 4;
                }
                ch.inst_pitch = p;
            }
            let vs = ch.inst_vol_slide;
            if vs != 0 {
                let mut v = vs.wrapping_add(ch.inst_vol as u8) as i8;
                if v < 0 {
                    v = 0;
                }
                if v > MAX_VOLUME as i8 {
                    v = MAX_VOLUME as i8;
                }
                ch.inst_vol = (ch.inst_vol & 0xff00) | v as u8 as u16;
            }
            let mut d2 = ch.inst_line_ticks;
            if d2 == 0 {
                self.work[1] += 1;
                d2 = self.inst_steps(c, bus);
            }
            let ch = &mut self.ch[c];
            if (ch.inst_wave_num as u8 as i8) < 0 {
                // no wave selected yet: wave 1
                ch.inst_wave_num = 0;
                ch.waveinfo = self.waveinfo_ptr;
                ch.out.trg = ch.channel_mask;
                let wi = ch.waveinfo;
                let lof_w = bus.r16(wi + WI_LOOP_OFFSET);
                let mut d3: u16 = 0;
                if bus.r16(wi + WI_SUBLOOP_LEN) == 0 {
                    ch.out.ptr = wsamp[0].wrapping_add(lof_w as i16 as i32 as u32);
                    let cr = bus.r16(wi + WI_CHIPRAM);
                    let len = if (cr.wrapping_sub(1) as i16) <= lof_w as i16 { 2 } else { cr.wrapping_sub(lof_w) };
                    ch.out.len = len;
                    d3 = 0xffff;
                } else {
                    ch.out.ptr = wsamp[0];
                }
                ch.out.lof = d3;
                ch.inst_subloop_wait = bus.r8(wi + WI_SUBLOOP_WAIT) as u16 + 1;
                ch.inst_loop_offset = lof_w;
                ch.inst_ping_pong_dir = 0xff;
            }
            if d2 != 0xff {
                ch.inst_line_ticks = d2.wrapping_sub(1);
            }
        }
        self.adsr_volume_out(c, bus);
    }

    /// The instrument pattern's steps: returns the new line ticks.
    fn inst_steps(&mut self, c: usize, bus: &Bus) -> u8 {
        let wsamp = self.wave_samples;
        let wit = self.waveinfo_table;
        let ch = &mut self.ch[c];
        ch.inst_pitch_slide = 0;
        ch.inst_vol_slide = 0;
        let mut d0 = ch.inst_step_pos as u16;
        let mut d2: u8;
        if d0 as u8 >= ch.inst_pattern_steps {
            d2 = 0xff;
            d2 = d2.wrapping_add(ch.inst_speed_stop);
            ch.inst_line_ticks = d2;
            ch.inst_step_pos = d0 as u8;
            return d2;
        }
        let mut d4: i16 = -1;
        let mut d7 = false;
        let mut d3 = false; // stitching: no new pitch
        let pat = self.inst_patterns[(ch.inst as usize).wrapping_sub(1).min(MAX_INSTRUMENTS - 1)];
        let mut a0 = pat.wrapping_add(d0 as u32 * 3);
        loop {
            // inst_pat_loop
            self.work[2] += 1;
            let b = bus.r8(a0);
            a0 = a0.wrapping_add(1);
            d2 = 0;
            if b & 0x80 != 0 {
                if d4 < 0 {
                    d4 = d0 as i16;
                }
                d2 = 1;
            }
            if !d3 {
                let p = b & 0x3f;
                if p != 0 {
                    ch.inst_note_pitch = ((p as u16) - 1) << 4;
                    ch.inst_pitch_pinned = (b & 0x40 != 0) as u8;
                }
            }
            let cmd = bus.r8(a0) & 15;
            let d5 = bus.r8(a0 + 1) as u16;
            a0 = a0.wrapping_add(2);
            let mut fetch_next = false;
            match cmd {
                0 | 4 => {
                    let w = d5.wrapping_sub(1);
                    if w < MAX_WAVES as u16 && w << 2 != ch.inst_wave_num {
                        let w4 = w << 2;
                        ch.inst_wave_num = w4;
                        let wi = wit[w as usize];
                        ch.waveinfo = wi;
                        ch.out.trg = ch.channel_mask;
                        let mut a1 = wsamp[w as usize];
                        let chip = bus.r16(wi + WI_CHIPRAM);
                        if cmd == 0 {
                            let lof = bus.r16(wi + WI_LOOP_OFFSET);
                            let mut d3w: u16 = 0;
                            if bus.r16(wi + WI_SUBLOOP_LEN) == 0 {
                                a1 = a1.wrapping_add(lof as i16 as i32 as u32);
                                let l = chip.wrapping_sub(lof);
                                ch.out.len = if chip > lof { l } else { 2 };
                                d3w = 0xffff;
                            }
                            ch.out.lof = d3w;
                            ch.out.ptr = a1;
                            ch.inst_ping_pong_dir = 0xff;
                            ch.inst_subloop_wait = bus.r8(wi + WI_SUBLOOP_WAIT) as u16 + 1;
                            ch.inst_loop_offset = lof;
                        } else {
                            let mut d5w = chip;
                            let mut d3w: u16 = 0;
                            if bus.r16(wi + WI_SUBLOOP_LEN) == 0 {
                                let lof = bus.r16(wi + WI_LOOP_OFFSET);
                                a1 = a1.wrapping_add(lof as i16 as i32 as u32);
                                d5w = if chip > lof { chip.wrapping_sub(lof) } else { 2 };
                                ch.out.len = d5w;
                                d3w = 0xffff;
                            }
                            ch.out.lof = d3w;
                            ch.out.ptr = a1;
                            let d6 = ch.inst_loop_offset;
                            if d5w < d6 {
                                ch.inst_ping_pong_dir = 0xff;
                            }
                            ch.inst_subloop_wait = 0;
                            ch.inst_loop_offset = d6.wrapping_sub(bus.r16(wi + WI_SUBLOOP_STEP));
                        }
                    }
                }
                1 => ch.inst_pitch_slide = d5,
                2 => ch.inst_pitch_slide = d5.wrapping_neg(),
                3 => match d5 {
                    1 => {
                        let v = (ch.adsr_volume as i16 >> 6) as u16;
                        ch.adsr_vol64 = v;
                        ch.adsr_pos = 16;
                        ch.adsr_phase_speed = ((16u16.wrapping_sub(v) >> 1) as u8).wrapping_add(ch.adsr_release);
                        ch.adsr_phase = 3;
                    }
                    2 => {
                        ch.adsr_phase = 0;
                        ch.adsr_volume = 0;
                    }
                    _ => {}
                },
                0xa => {
                    // the original negates the whole byte
                    let b = d5 as u8;
                    ch.inst_vol_slide = if b & 15 != 0 { b.wrapping_neg() } else { b >> 4 };
                }
                0xb => {
                    if (d5 as i16) < (d0 as i16) && (d4 < 0 || (d4 as u8 as i8) > (d5 as u8 as i8)) {
                        d0 = d5;
                        if d7 {
                            // a second jump in one tick: wait a step
                            let d2 = ch.inst_speed_stop;
                            ch.inst_line_ticks = d2;
                            ch.inst_step_pos = d0 as u8;
                            return d2;
                        }
                        d4 = -1;
                        a0 = pat.wrapping_add(d5 as u32 * 3);
                        fetch_next = true;
                    }
                }
                0xc => ch.inst_vol = if (d5 as i16) > MAX_VOLUME as i16 { MAX_VOLUME as u16 } else { d5 },
                0xe => {
                    let k = d5 & 3;
                    if k == 0 {
                        ch.inst_sel_arp_note = 0;
                    } else {
                        let hi = d5 >> 4;
                        if hi == 0 {
                            let a = ch.arp_notes[k as usize - 1] as u16;
                            if a != 0 {
                                ch.inst_sel_arp_note = a << 4;
                            } else {
                                d0 = d0.wrapping_add(1);
                                fetch_next = true;
                            }
                        } else if hi == 1 {
                            ch.inst_sel_arp_note = (ch.arp_notes[k as usize - 1] as u16) << 4;
                        }
                    }
                }
                0xf => ch.inst_speed_stop = if d5 == 0 { 0xff } else { d5 as u8 },
                _ => {}
            }
            if !fetch_next {
                // inst_cmd_cont_next
                d0 = d0.wrapping_add(1);
                if d2 == 0 {
                    let d2 = d2.wrapping_add(ch.inst_speed_stop);
                    ch.inst_line_ticks = d2;
                    ch.inst_step_pos = d0 as u8;
                    return d2;
                }
            }
            // inst_fetch_next
            d7 = true;
            d3 = d2 != 0;
            if d0 as u8 >= ch.inst_pattern_steps {
                break;
            }
        }
        let d2 = 0xffu8.wrapping_add(ch.inst_speed_stop);
        ch.inst_line_ticks = d2;
        ch.inst_step_pos = d0 as u8;
        d2
    }

    /// ADSR, the output volume, the wave loop, the pitch and the period.
    fn adsr_volume_out(&mut self, c: usize, bus: &Bus) {
        let wsamp = self.wave_samples;
        let inst = self.inst(self.ch[c].inst);
        let ch = &mut self.ch[c];
        let d1 = ch.inst_vol;
        let mut d2: u16;
        if ch.new_inst_num != 0 {
            ch.loaded_inst_vol = d1 as u8;
            ch.vibrato_delay = inst.vibrato_delay;
            ch.vibrato_depth = inst.vibrato_depth;
            ch.vibrato_speed = inst.vibrato_speed;
            ch.adsr_release = inst.adsr_release;
            ch.adsr_phase = 0;
            ch.adsr_volume = 0;
            ch.new_inst_num = 0;
            ch.vibrato_pos = 0;
            d2 = 0;
            d2 = Self::attack(ch, &inst, d2);
        } else {
            d2 = ch.adsr_volume;
            match ch.adsr_phase {
                0 => d2 = Self::attack(ch, &inst, d2),
                1 => d2 = Self::decay_release(ch, &inst, d2, true),
                2 => {}
                _ => {
                    let p = ch.adsr_pos.wrapping_add(ch.adsr_vol64);
                    ch.adsr_pos = p;
                    let p = p.wrapping_sub(16);
                    if (p as i16) >= 0 {
                        ch.adsr_pos = p;
                        d2 = Self::decay_release(ch, &inst, d2, false);
                    }
                }
            }
        }
        ch.adsr_volume = d2;
        let nd = ch.note_off_delay;
        if nd != 0 {
            ch.note_off_delay = nd - 1;
            if nd == 1 {
                ch.adsr_volume = 0;
                ch.adsr_phase = 3;
            }
        }
        // the volume: instrument * ADSR * pattern
        let v = ((d1 as u32 * ((d2 >> 4) as u32)) & 0xffff) >> 6;
        ch.out.vol = ((ch.pat_vol as u32 * v) as u16 >> 6) as u8;

        // the wave's loop, and the pattern's wave offset
        let wi = ch.waveinfo;
        let d3 = bus.r16(wi + WI_SUBLOOP_LEN);
        if d3 != 0 {
            ch.out.len = d3;
            let step = bus.r16(wi + WI_SUBLOOP_STEP);
            let wo = ch.wave_offset;
            let mut moved = true;
            let (mut d1, d4dir);
            if wo != 0 && bus.r8(wi + WI_ALLOW_9XX) != 0 {
                d1 = (wo as u16) << 7;
                ch.wave_offset = 0;
                d4dir = ch.inst_ping_pong_dir;
                if (d4dir as i8) >= 0 {
                    d1 = d1.wrapping_add(step);
                } else {
                    d1 = d1.wrapping_sub(step);
                }
            } else {
                // the original moves the loop at a wait of 1 or 0 and else
                // counts down, unsigned: so that the wait below 0 a loop that
                // hit its end exactly leaves holds it (Raspberry Casket counts
                // down first, signed). A voice that never had an instrument
                // keeps its loop where it is in the reference.
                if ch.inst_subloop_wait > 1 || ch.inst == 0 {
                    ch.inst_subloop_wait = ch.inst_subloop_wait.wrapping_sub(1);
                    moved = false;
                    d1 = 0;
                    d4dir = 0;
                } else {
                    d4dir = ch.inst_ping_pong_dir;
                    d1 = ch.inst_loop_offset;
                }
            }
            let d4 = if moved {
                ch.inst_subloop_wait = bus.r8(wi + WI_SUBLOOP_WAIT) as u16;
                let d2: u16;
                let mut fits = false;
                if (d4dir as i8) < 0 {
                    d1 = d1.wrapping_add(step);
                    let start = d1.wrapping_add(d3);
                    let mut end = bus.r16(wi + WI_LOOP_END);
                    if end < d1 {
                        end = bus.r16(wi + WI_CHIPRAM);
                    }
                    let (r, borrow) = end.overflowing_sub(start);
                    d2 = r;
                    if !borrow && r != 0 {
                        fits = true;
                    } else {
                        d1 = d1.wrapping_add(d2);
                        ch.inst_ping_pong_dir = 0;
                    }
                } else {
                    d1 = d1.wrapping_sub(step);
                    let ls = bus.r16(wi + WI_LOOP_START);
                    d2 = ls.wrapping_sub(d1);
                    if (d2 as i16) < 0 {
                        fits = true;
                    } else {
                        d1 = ls;
                        ch.inst_ping_pong_dir = 0xff;
                    }
                }
                if !fits && d2 == 0 {
                    ch.inst_subloop_wait = ch.inst_subloop_wait.wrapping_sub(1);
                }
                ch.inst_loop_offset = d1;
                d1
            } else {
                ch.inst_loop_offset
            };
            ch.out.lof = d4;
            ch.out.ptr = wsamp.get((ch.inst_wave_num >> 2) as usize).copied().unwrap_or(0);
        } else {
            let wo = ch.wave_offset;
            if wo != 0 && bus.r8(wi + WI_ALLOW_9XX) != 0 {
                let d1 = ((wo as u16) << 8) >> 1;
                ch.wave_offset = 0;
                ch.out.trg = ch.channel_mask;
                let chip = bus.r16(wi + WI_CHIPRAM);
                let d2 = chip.wrapping_sub(d1);
                ch.out.len = if (d2 as i16) > 0 { d2 } else { 2 };
                let s = wsamp.get((ch.inst_wave_num >> 2) as usize).copied().unwrap_or(0);
                ch.out.ptr = s.wrapping_add(d1 as u32);
            }
        }

        // the pitch
        let mut d0 = ch.inst_pitch.wrapping_sub(0x10);
        if ch.inst_pitch_pinned == 0 {
            d0 = d0.wrapping_add(ch.inst_sel_arp_note).wrapping_add(ch.inst_curr_port_pitch).wrapping_sub(0x10);
        }
        d0 = d0.wrapping_add(ch.inst_note_pitch);
        // vibrato
        let mut vib = true;
        // a word, as in the original (Raspberry Casket counts its low byte,
        // so that the longest delay, 256, is none)
        let vd = ch.vibrato_delay;
        if vd != 0 {
            let vd = vd - 1;
            ch.vibrato_delay = vd;
            if vd != 0 {
                vib = false;
            }
        }
        if vib && ch.vibrato_speed != 0 {
            let mut sp = ch.vibrato_speed as i16;
            let mut dp = ch.vibrato_depth as i16;
            let mut pos = sp.wrapping_add(ch.vibrato_pos as i16);
            let flip = if pos > dp {
                true
            } else {
                dp = dp.wrapping_neg();
                pos < dp
            };
            if flip {
                sp = sp.wrapping_neg();
                ch.vibrato_speed = sp as u16;
                pos = dp;
            }
            ch.vibrato_pos = pos as u16;
            d0 = d0.wrapping_add((pos >> 3) as u16);
        }

        // the sample for the pitch: high notes play a higher octave's
        let mut d3 = ch.out.len;
        let d6: u16;
        if (d0 as i16) > 0x219 {
            if bus.r8(wi + WI_FLAGS) & 4 == 0 {
                d6 = 0x231;
            } else {
                let mut d2 = bus.r16(wi + WI_CHIPRAM) as u32;
                let d5 = ((d0.wrapping_sub(0x219)) >> 6) as usize;
                // the original reads past its tables above pitch $818
                let d5 = d5.min(t::OCTAVE_SELECT.len() - 1);
                let mut d1 = t::OCTAVE_SELECT[d5];
                let mut shift = true;
                if ch.out.lof != 0xffff {
                    ch.out.lof >>= d1;
                    d3 >>= d1;
                    ch.out.len = d3;
                } else if ch.out.trg != 0 {
                    let a3 = wsamp.get((ch.inst_wave_num >> 2) as usize).copied().unwrap_or(0);
                    let d4 = ch.out.ptr.wrapping_sub(a3);
                    let d6 = d3.wrapping_add(d4 as u16);
                    let d7 = (d2 as u16).wrapping_sub(d6);
                    if d3 >= d7 {
                        d3 = d3.wrapping_add(d6).wrapping_sub(d2 as u16) >> d1;
                    } else {
                        d3 = 2;
                    }
                    ch.out.len = d3;
                    let d4 = (d4 & 0xffff_0000) | ((d4 as u16) >> d1) as u32;
                    ch.out.ptr = d4.wrapping_add(a3);
                } else {
                    shift = false;
                }
                if shift {
                    // past the lower octaves' samples to this one's
                    let mut d4 = 0u32;
                    while d1 != 0 {
                        d4 = d4.wrapping_add(d2);
                        d2 = (d2 & 0xffff_0000) | ((d2 as u16) >> 1) as u32;
                        d1 -= 1;
                    }
                    ch.out.ptr = ch.out.ptr.wrapping_add(d4);
                }
                d0 = d0.wrapping_sub((t::OCTAVE_NOTE_OFFSET[d5] as u16) << 2);
                if (d0 as i16) > 0x231 {
                    d0 = 0x231;
                }
                d6 = if (d0 as i16) < 0 { 0 } else { d0 };
            }
        } else {
            d6 = if (d0 as i16) < 0 { 0 } else { d0 };
        }
        if ch.out.trg != 0 && ch.out.lof != 0xffff {
            ch.out.ptr = ch.out.ptr.wrapping_add(ch.out.lof as u32);
            ch.out.lof = 0;
        }
        if d3 != ch.last_trigger_pos {
            ch.last_trigger_pos = d3;
            ch.out.trg = ch.channel_mask;
        }
        ch.out.per = self.period_table.get(d6 as usize).copied().unwrap_or(0);
    }

    fn attack(ch: &mut Chan, inst: &Inst, d2: u16) -> u16 {
        let d2 = d2.wrapping_add(inst.adsr_attack);
        if (d2 as i16) < (MAX_VOLUME as i16) << 4 {
            return d2;
        }
        ch.adsr_phase = 1;
        ch.adsr_phase_speed = inst.adsr_decay as u8;
        (MAX_VOLUME as u16) << 4
    }

    fn decay_release(ch: &mut Chan, inst: &Inst, d2: u16, decay: bool) -> u16 {
        let sp = ch.adsr_phase_speed;
        let (d4, d5) = if sp >= 0x8f { (2u16, 0x8fu8) } else { (t::ROLL_OFF[sp as usize], sp + 1) };
        ch.adsr_phase_speed = d5;
        let d2 = d2.wrapping_sub(d4);
        if !decay {
            return if (d2 as i16) < 0 { 0 } else { d2 };
        }
        if (d2 as i16) > inst.adsr_sustain as i16 {
            return d2;
        }
        ch.adsr_phase = 2;
        inst.adsr_sustain
    }

    /// The Paula part of a tick, as the V1.5 replay writes it.
    fn output(&mut self, hw: &mut Hw) {
        // the loop over the voices, as the reference's writes time it: a voice that restarts
        // (DMA off, LOC, LEN, volume 0, period $71) leaves 26.5 colour clocks to the next voice's
        // first write, one that does not (volume, period) 21.5 and then 6 more before its first
        // write, so the next voice's volume comes 27.5 after this one's period, its DMA off 26.5
        // after a restarting one's
        for c in 0..4 {
            let o = self.ch[c].out;
            self.half += 1;
            let j = self.half & 1;
            if o.trg != 0 {
                self.ch[c].out.trg = 0;
                hw.dmacon(1 << c);
                hw.wait(16);
                hw.loc(c, o.ptr);
                hw.wait(18);
                hw.len(c, o.len >> 1);
                hw.wait(10);
                hw.vol(c, 0);
                hw.wait(18);
                hw.per(c, 0x71);
                if c < 3 {
                    hw.wait(26 + j);
                }
            } else {
                if c > 0 {
                    hw.wait(6);
                }
                hw.vol(c, o.vol as u16);
                hw.wait(12);
                hw.per(c, o.per);
                if c < 3 {
                    hw.wait(21 + j);
                }
            }
        }
        raster_wait(hw, 2 * 637);
        hw.dmacon(0x800f);
        raster_wait(hw, 2 * 632 + 139);
        for c in 0..4 {
            let o = self.ch[c].out;
            hw.dmacon(0x8000 | 1 << c);
            hw.wait(42);
            if o.lof != 0xffff {
                hw.loc(c, o.ptr.wrapping_add(o.lof as u32));
            } else {
                hw.loc(c, self.sample_buffer);
                hw.wait(14);
                hw.len(c, 1);
            }
            hw.wait(12);
            hw.per(c, o.per);
            hw.wait(16);
            hw.vol(c, o.vol as u16);
            hw.wait(10);
        }
    }
}

/// Colour clocks the original's song start goes on after its silence before
/// it waits for the vertical blank (from the reference's first tick: the
/// second vertical blank after, the third when the silence comes later than
/// this before one).
const SONG_START_WORK: u64 = 7000;

/// The original's wait for the beam to go three lines on, as its writes show (measured on UADE's
/// replay, 499 ticks of one module): it reads the line number 23 colour clocks after the call
/// (the previous write) and polls on a grid of 69.5 colour clocks from there, so the write comes
/// at the first of `base / 2 + k * 69.5` after the call (base in half colour clocks) that is at
/// least 84 colour clocks into the third line after the one it read.
fn raster_wait(hw: &mut Hw, base: u64) {
    let now = hw.bus.clock;
    let target = ((now + 23) / LINE + 3) * LINE + 84;
    let need = 2 * (target - now);
    let mut gap = base;
    while gap < need {
        gap += 139;
    }
    while gap >= need + 139 {
        gap -= 139;
    }
    hw.wait(gap / 2);
}

enum Next {
    /// To the next channel (pat_channels_loop_test).
    Test,
    /// Through the track delay test (pat_play_cont).
    Cont,
    /// A track delay just set: skips the next channel.
    TrackDelay,
}

/// The sample generation (pre_PlayerInit), as Raspberry Casket's with
/// PRETRACKER_FASTER_CODE and PRETRACKER_BUGFIX_CODE off: the original's
/// arithmetic, byte for byte.
struct Wavegen;

/// The oscillators' waveforms of a note: saw, square and triangle (128 bytes
/// each, a period of the note's length), as in the original's memory, one
/// note's after the other.
const OWB_SIZE: usize = 385;

impl Wavegen {
    fn osc_buffers() -> Vec<u8> {
        let mut b = vec![0u8; 12 * OWB_SIZE + 2 * OWB_SIZE];
        for (n, &p) in t::LOG12.iter().enumerate() {
            let base = n * OWB_SIZE;
            let (saw, sqr, tri) = (base, base + 128, base + 256);
            b[base + 384] = p;
            let p = p as u16;
            let inc = 0xff00u32 / p as u32;
            let half = p >> 1;
            let quarter = half >> 1;
            let mut acc: u16 = 0;
            let mut tp = (p - quarter) as isize;
            for pos in 0..p {
                let mut d2 = (acc >> 8) as u8;
                b[saw + pos as usize] = 0x7fu8.wrapping_sub(d2);
                d2 = d2.wrapping_add(d2);
                if pos == quarter {
                    tp -= p as isize;
                }
                let v;
                if (pos as i32) > (acc as i16 as i32) {
                    b[(tri as isize + tp) as usize] = 0x7fu8.wrapping_sub(d2);
                    v = 0x80;
                } else {
                    b[(tri as isize + tp) as usize] = d2.wrapping_add(0x80);
                    v = if half == pos { 0x80 } else { 0x7f };
                }
                tp += 1;
                b[sqr + pos as usize] = v;
                acc = acc.wrapping_add(inc as u16);
            }
        }
        b
    }

    fn run(p: &PreTracker, bus: &mut Bus, total: u32) {
        let osc = Self::osc_buffers();
        let base = p.sample_buffer;
        // the sample buffer, as bytes (index 0: the empty sample)
        let mut buf = vec![0u8; total as usize];
        let off = |a: u32| a.wrapping_sub(base) as usize;
        let wi8 = |bus: &Bus, w: u32, o: u32| bus.r8(w.wrapping_add(o));
        for ord in 0..p.num_waves as usize {
            let wave = bus.r8(p.base.wrapping_add(0x42 + ord as u32)) as usize;
            let wave = if p.version > 0x19 { wave } else { ord };
            let (start, len, wi) = if wave < MAX_WAVES && wave < p.num_waves as usize {
                (off(p.wave_samples[wave]), p.wavelength[wave] as usize, p.waveinfo_table[wave])
            } else {
                // past the waves: the original's tables hold 0 there (no
                // sample, at address 0: nothing is generated)
                (usize::MAX, 0, 0)
            };
            if len == 0 {
                continue;
            }
            let s = &mut buf[start..start + len];
            s.fill(0);
            let g = |o| wi8(bus, wi, o);
            Self::tones(s, &osc, &g);
            Self::filter(s, &g);
            if g(WI_MOD_DENSITY) & 0x20 == 0 {
                Self::modulator(s, &g);
            }
            Self::envelope(s, &g);
            if g(WI_MOD_DENSITY) & 0x20 != 0 {
                Self::modulator(s, &g);
            }
            let mix = g(WI_MIX_WAVE);
            if mix != 0 {
                let m = (mix - 1) as usize;
                // not with itself, as in the reference (a module can ask for
                // it); a wave past the waves the original takes from past its
                // tables: none is mixed here
                if m != wave && m < p.num_waves as usize {
                    let ml = p.wavelength[m] as usize;
                    let n = len.min(ml);
                    let ms = off(p.wave_samples[m]);
                    for j in 0..n {
                        let a = buf[start + j] as i8;
                        let b = buf[ms + j] as i8;
                        buf[start + j] = a.saturating_add(b) as u8;
                    }
                }
            }
            let flags = g(WI_FLAGS);
            let sl = g(WI_SAM_LEN) as usize;
            if flags & 4 != 0 {
                let l = (sl + 1) << 7;
                let n = (l * 7 / 8).min(buf.len().saturating_sub(start + l));
                for j in 0..n {
                    buf[start + l + j] = buf[start + 2 * j];
                }
            }
        }
        bus.write_bytes(base, &buf);
    }

    /// The chord's notes on the oscillator (or noise), twice for unisono.
    fn tones(s: &mut [u8], osc: &[u8], g: &dyn Fn(u32) -> u8) {
        let cn = [g(WI_CHORD_NOTE1), g(WI_CHORD_NOTE1 + 1), g(WI_CHORD_NOTE1 + 2)];
        let chord_flag: u16 = if cn[0] | cn[1] | cn[2] == 0 { 1 } else { 0 };
        let basenote = g(WI_OSC_BASENOTE);
        let pitches = [basenote, basenote.wrapping_add(cn[0]), basenote.wrapping_add(cn[1]), basenote.wrapping_add(cn[2])];
        let flags = g(WI_FLAGS);
        let mut unisono = false;
        let mut was_osc = false;
        loop {
            for k in 0..4 {
                let note = pitches[k];
                if k != 0 && note == basenote {
                    continue;
                }
                let note = note as i8 as i16;
                let adj = (note as u16).wrapping_add(144);
                let octave = (adj / 12) as i16 - 12;
                let in_oct = (adj % 12) as usize;
                let ob = in_oct * OWB_SIZE;
                let kind = flags & 3;
                was_osc = kind != 3;
                let wave = match kind {
                    0 => Some(ob),
                    1 => Some(ob + 256),
                    2 => Some(ob + 128),
                    _ => None,
                };
                let mut d6: i32 = 0x8000;
                if octave > 0 {
                    d6 = ((d6 as u32) << octave) as i32;
                } else if octave < 0 {
                    d6 >>= -octave;
                }
                // the pitch ramp
                let mut d3 = g(WI_PITCH_RAMP) as i8 as i32;
                let linear = flags & 0x10 != 0;
                if linear {
                    if d3 > 0 {
                        d3 *= d3;
                    } else {
                        let sh = (octave as u32) & 63;
                        d3 = if sh >= 32 { 0 } else { ((d3 as u32) << sh) as i32 };
                        d3 = d3.wrapping_add(d3);
                    }
                } else if d3 > 0 {
                    d3 *= d3;
                }
                let d2 = d3.wrapping_shl(10);
                let d7 = (osc[ob + 384] as i32) << 15;
                let d5 = ((15 - octave) as u16) << 3;
                let pmin = ((g(WI_OSC_PHASE_MIN) as u32 * d5 as u32) << 6) as i32;
                let pmax = ((g(WI_OSC_PHASE_MAX) as u32 * d5 as u32) << 6) as i32;
                let mut speed = (g(WI_OSC_PHASE_SPD) as i32) << 11;
                let (limit, lo) = if pmax >= pmin {
                    (pmax, pmin)
                } else {
                    speed = -speed;
                    (pmin, pmax)
                };
                let osc_min = lo;
                let d4 = (g(WI_CHORD_SHIFT) as u32 * (chord_flag.wrapping_add(k as u16) as u32)) as u16;
                let d4 = d4.wrapping_add(g(WI_OSC_PHASE_MIN) as u16) as u32;
                // the start phase: a full 32-bit product in the reference (Raspberry
                // Casket multiplies the low words, which drops it from octave 5)
                let mut d0 = ((d6 as u32) >> 4).wrapping_mul((d4 << 4) & 0xffff) as i32;
                while d0 > d7 {
                    d0 -= d7;
                }
                match wave {
                    None => Self::noise(s, g, octave, note, d2, linear),
                    Some(w) => {
                        let mut d1 = d6;
                        if unisono {
                            let u = (g(WI_MOD_DENSITY) & 0x18) >> 3;
                            d1 = (d6 >> (9 - u)).wrapping_add(d6);
                        }
                        Self::osc(s, osc, w, g(WI_OSC_GAIN), d0, d1, d2, d6, d7, pmin, limit, speed, osc_min, linear);
                    }
                }
            }
            if g(WI_MOD_DENSITY) & 0x18 != 0 && was_osc && !unisono {
                unisono = true;
                continue;
            }
            break;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn osc(
        s: &mut [u8],
        osc: &[u8],
        w: usize,
        gain: u8,
        mut d0: i32,
        mut d1: i32,
        mut d2: i32,
        d6: i32,
        d7: i32,
        pmin: i32,
        limit: i32,
        mut d5: i32,
        osc_min: i32,
        linear: bool,
    ) {
        let mut a1 = pmin;
        let mut a2: i32 = 0;
        for o in s.iter_mut() {
            let mut d4 = d0.wrapping_sub(a1);
            if d4 < 0 {
                d4 = 0;
            }
            let idx = (d4 >> 15) as i16 as isize;
            let smp = osc.get((w as isize + idx) as usize).copied().unwrap_or(0) as i8 as i16;
            let d3 = ((gain as i16 as i32 * smp as i32) as i16) >> 7;
            let v = ((*o as i8 as i16) + d3).clamp(-128, 127);
            *o = v as u8;
            d0 = d0.wrapping_add(d1);
            if d0 >= d7 {
                d0 -= d7;
                a1 = a1.wrapping_add(d5);
                if a1 >= limit {
                    d5 = -d5;
                    a1 = limit;
                }
                if a1 <= osc_min {
                    d5 = -d5;
                    a1 = osc_min;
                }
            }
            if d2 != 0 {
                a2 = a2.wrapping_add(d2);
                d1 = (a2 >> 10).wrapping_add(d6);
                if linear {
                    d2 -= d2 >> 7;
                }
                if (d1 as u32) >= d7 as u32 {
                    d2 = 0;
                    d1 = 0;
                }
            }
        }
    }

    fn noise(s: &mut [u8], g: &dyn Fn(u32) -> u8, octave: i16, note: i16, mut d2: i32, linear: bool) {
        let mut d5: u32 = 0x8000;
        if octave < 0 {
            let d3 = (-(note as i32)) as u32;
            let q = ((d3 / 12) as u16).wrapping_add(1);
            let sh = (q & 63) as u32;
            d5 = if sh >= 16 { 0 } else { ((d5 as u16) >> sh) as u32 };
            let d4 = (d5 & 0xffff) / 12;
            let mut d0 = d3 as u16 as i16;
            loop {
                d0 = d0.wrapping_sub(12);
                if d0 < 0 {
                    break;
                }
            }
            let d0 = d0.wrapping_neg() as u16;
            d5 = d5.wrapping_add(d0 as u32 * (d4 & 0xffff));
        }
        let mut seed: u16 = (g(WI_OSC_PHASE_MIN) as u16) + (g(WI_CHORD_SHIFT) as u16) + 1;
        let mut a1 = d5 as i32;
        let mut a5: i32 = 0x8000;
        let mut d6: i32 = 0;
        let gain = g(WI_OSC_GAIN) as i16 as i32;
        let mut i = 0usize;
        let n = s.len();
        loop {
            seed ^= seed << 13;
            seed ^= seed >> 9;
            seed ^= seed << 7;
            if a5 != 0x8000 {
                let d4 = a5.wrapping_add(-0x8000);
                let d1 = a5.wrapping_add(-0x8001);
                let d1 = (d1 as u32 & 0xffff_0000) | (d1 as u32 & 0x8000);
                a5 = d4.wrapping_sub(d1 as i32);
            }
            let smp = seed as u8 as i8 as i32;
            let d1 = ((gain * smp) >> 7).clamp(-128, 127) as u8;
            let d1 = d1.wrapping_add(s[i]);
            loop {
                s[i] = d1;
                i += 1;
                a5 = a5.wrapping_add(a1);
                if d2 != 0 {
                    d6 = d6.wrapping_add(d2);
                    a1 = (d6 >> 10).wrapping_add(d5 as i32);
                    if linear {
                        d2 -= d2 >> 7;
                    }
                    if a1 <= 0x1ff {
                        d2 = 0;
                        a5 = 0;
                        a1 = 0x200;
                    }
                }
                if i == n {
                    return;
                }
                if a5 > 0x7fff {
                    break;
                }
            }
        }
    }

    fn filter(s: &mut [u8], g: &dyn Fn(u32) -> u8) {
        let ty = g(WI_FLT_TYPE);
        if ty == 0 {
            return;
        }
        let mut taps = [0i16; 4]; // d3, d4, d5, d6
        let mut d0: i32 = (g(WI_FLT_START) as i32) << 8;
        let fmin: i32 = (g(WI_FLT_MIN) as i32) << 8;
        let fmax: i32 = (g(WI_FLT_MAX) as i32) << 8;
        let mut d3: i32 = (g(WI_FLT_SPEED) as i8 as i32) << 7;
        let mut i = 0usize;
        loop {
            let mut a6 = d0.wrapping_add(d3);
            enum K {
                Normal,
                Val(u16),
                Max,
                Min,
            }
            let k = if d3 > 0 {
                if d0 > fmax {
                    if d0 > 0xff00 || a6 <= 0xfeff {
                        K::Normal
                    } else {
                        d3 = -d3;
                        a6 = 0xff00;
                        K::Val(0)
                    }
                } else if a6 < fmax {
                    K::Normal
                } else {
                    if fmin == fmax {
                        a6 = fmin;
                    } else {
                        d3 = -d3;
                        a6 = fmax;
                    }
                    K::Max
                }
            } else if d0 < fmin {
                if d0 < 0 || a6 > 0 {
                    K::Normal
                } else {
                    d3 = -d3;
                    a6 = 0;
                    K::Val(0xff)
                }
            } else if a6 > fmin {
                K::Normal
            } else {
                a6 = fmin;
                if fmin != fmax {
                    d3 = -d3;
                }
                K::Min
            };
            let mut d2: u16 = match k {
                K::Normal => (((a6 as u16) >> 8) as u8 ^ 0xff) as u16,
                K::Val(v) => v,
                K::Max => (g(WI_FLT_MAX) ^ 0xff) as u16,
                K::Min => (g(WI_FLT_MIN) ^ 0xff) as u16,
            };
            if ty & 1 == 0 {
                d2 = (d2 & 0xff00) | ((d2 as u8) ^ 0xff) as u16;
            }
            let mut c0 = d2.wrapping_add(d2);
            let res = g(WI_FLT_RESONANCE) as u16;
            if res != 0 {
                let d1 = ((d2 as i16 as i32) << 8) as u32;
                let mut div = (0x5bu16.wrapping_sub(res)).wrapping_add(0x5bu16.wrapping_sub(res));
                if (div as i16) < 0x36 {
                    div = 0x36;
                }
                let q = (d1 / div as u32) as u16;
                c0 = d2.wrapping_add(q);
            }
            let (c0, d2) = (c0 as i16 as i32, d2 as i16 as i32);
            let end = i + 64;
            let [mut t3, mut t4, mut t5, mut t6] = taps;
            while i < end {
                let d1 = s.get(i).copied().unwrap_or(0) as i8 as i16;
                let mut d7 = t4.wrapping_sub(t5);
                d7 = ((d7 as i32 * c0) >> 8) as i16;
                d7 = d7.wrapping_sub(t4);
                d7 = d7.wrapping_add(d1);
                d7 = ((d7 as i32 * d2) >> 8) as i16;
                t4 = t4.wrapping_add(d7);
                let mut d7 = t4.wrapping_sub(t5);
                d7 = ((d7 as i32 * d2) >> 8) as i16;
                t5 = t5.wrapping_add(d7);
                let mut d7 = t5.wrapping_sub(t6);
                d7 = ((d7 as i32 * d2) >> 8) as i16;
                t6 = t6.wrapping_add(d7);
                let mut d7 = t6.wrapping_sub(t3);
                d7 = ((d7 as i32 * d2) >> 8) as i16;
                t3 = t3.wrapping_add(d7);
                let mut d7 = t3;
                match ty {
                    2 => d7 = d7.wrapping_sub(d1),
                    3 => d7 = d7.wrapping_sub(t4).wrapping_sub(t5).wrapping_sub(t6) >> 1,
                    4 => d7 = d7.wrapping_sub(t4).wrapping_neg(),
                    _ => {}
                }
                if let Some(o) = s.get_mut(i) {
                    *o = d7.clamp(-128, 127) as u8;
                }
                i += 1;
            }
            taps = [t3, t4, t5, t6];
            if i >= s.len() {
                return;
            }
            d0 = a6;
        }
    }

    fn modulator(s: &mut [u8], g: &dyn Fn(u32) -> u8) {
        let wet = g(WI_MOD_WETNESS) as i32;
        let density = (g(WI_MOD_DENSITY) & 7) as u16;
        if wet == 0 || density == 0 {
            return;
        }
        let post = g(WI_MOD_DENSITY) & 0x20 != 0;
        let n = s.len() as u16;
        for run in 0..density {
            let mut d5 = g(WI_MOD_LENGTH) as u32 * t::MODULATOR_RAMP_8[run as usize] as u32;
            let mut d1 = g(WI_MOD_PREDELAY) as u32;
            if post {
                d1 = (d1 << 8) & 0xffff;
            } else {
                d5 >>= 2;
                d1 <<= 6;
            }
            d5 += d1;
            let mut acc: u16 = 0;
            for pos in 0..n {
                acc = acc.wrapping_add(run).wrapping_add(8);
                let r = t::RAMP_UP_DOWN_32[(acc >> 11) as usize] as u32;
                let d1 = (r + d5) >> 6;
                let d0 = pos.wrapping_sub(d1 as u16) as i16;
                if d0 < 0 {
                    continue;
                }
                let mut v = s[d0 as usize] as i8 as i16;
                if run & 1 != 0 {
                    v = v.wrapping_neg();
                }
                let v = ((wet * v as i32) as i16 >> 8) as i8;
                s[pos as usize] = v.saturating_add(s[pos as usize] as i8) as u8;
            }
        }
    }

    /// The volume envelope: attack, delay, decay to the sustain.
    fn envelope(s: &mut [u8], g: &dyn Fn(u32) -> u8) {
        let flags = g(WI_FLAGS);
        let boost = flags & 8 != 0;
        let fast = flags & 0x20 != 0;
        let n = s.len();
        let mut i = 0usize;
        // d4: the samples left - 1, a word counter
        let mut d4: u16 = (n as u16).wrapping_sub(1);
        let attack = g(WI_VOL_ATTACK) as u32;
        let rd = |s: &[u8], i: usize| s.get(i).copied().unwrap_or(0) as i8 as i16;
        let wr = |s: &mut [u8], i: usize, v: u8| {
            if let Some(o) = s.get_mut(i) {
                *o = v;
            }
        };
        enum Ph {
            DelayNormal,
            DelayBoosted,
        }
        let ph;
        if attack == 0 {
            if g(WI_VOL_SUSTAIN) == 0xff {
                return;
            }
            ph = if boost { Ph::DelayBoosted } else { Ph::DelayNormal };
        } else {
            let mut d1: u32 = match attack {
                1 => 0x20000,
                2 => 0x10000,
                a => 0x20000 / a,
            };
            if fast {
                d1 <<= 4;
            }
            let mut d3 = d1;
            if d3 > 0xffffff {
                ph = if boost { Ph::DelayBoosted } else { Ph::DelayNormal };
            } else {
                loop {
                    let v = rd(s, i) as i32 * (d3 >> 16) as i16 as i32;
                    let v = if boost { ((v as i16) >> 6).clamp(-128, 127) } else { (v as i16) >> 8 };
                    wr(s, i, v as u8);
                    i += 1;
                    d4 = d4.wrapping_sub(1);
                    if (d4 as i16) < 0 {
                        return;
                    }
                    d3 = d3.wrapping_add(d1);
                    if d3 > 0xffffff {
                        break;
                    }
                }
                ph = if boost { Ph::DelayBoosted } else { Ph::DelayNormal };
            }
        }
        // the delay, at volume $ff
        let end = i + ((g(WI_VOL_DELAY) as usize) << 4) + 2;
        loop {
            let v = rd(s, i) as i32 * 0xff;
            let v = match ph {
                Ph::DelayNormal => (v as i16) >> 8,
                Ph::DelayBoosted => ((v as i16) >> 6).clamp(-128, 127),
            };
            wr(s, i, v as u8);
            i += 1;
            if i == end {
                break;
            }
            d4 = d4.wrapping_sub(1);
            if d4 == 0xffff {
                return;
            }
        }
        d4 = d4.wrapping_sub(1);
        // the decay
        let decay = g(WI_VOL_DECAY) as u32;
        let sustain = g(WI_VOL_SUSTAIN);
        if decay != 0 {
            let d7 = (((decay * decay) as u16 >> 2) as u32 + decay) & 0xffff;
            let d3i = if fast { 0 } else { decay };
            let mut d3: u32 = (((d3i << 8) & 0xffff) as u32) << 4;
            let mut d2 = (d3 >> 16) as u16;
            let mut a1 = d2 as usize; // index of the upper bound
            let mut a2 = t::ROLL_OFF[a1] as i16; // lower bound
            a1 += 1;
            let mut d1: i16 = 0;
            let mut d5: u16 = 0xffff;
            let tab = |k: usize| t::ROLL_OFF.get(k).copied().unwrap_or(0) as i16;
            loop {
                d3 = d3.wrapping_add(d7);
                let idx = (d3 >> 16) as u16;
                if idx <= 0x8e {
                    if idx > d2 {
                        a2 = tab(a1);
                        // the original's slip: the new upper bound is the
                        // new lower one's entry, not the next
                        a1 = idx as usize;
                        d2 = idx;
                    }
                    d1 = a2;
                    let d0 = tab(a1).wrapping_sub(d1);
                    if d0 != 0 {
                        let f = ((d3 & 0xffff) as u16 >> 8) as i16 as i32;
                        d1 = (((f * d0 as i32) >> 8) as i16).wrapping_add(a2);
                    }
                }
                let (r, borrow) = d5.overflowing_sub(d1 as u16);
                d5 = r;
                if borrow || r == 0 {
                    break;
                }
                let d6 = (d5 >> 8) as u8;
                if d6 <= sustain {
                    break;
                }
                let v = rd(s, i) as i32 * d6 as i32;
                let v = if boost { (v >> 6).clamp(-128, 127) } else { v >> 8 };
                wr(s, i, v as u8);
                i += 1;
                d4 = d4.wrapping_sub(1);
                if d4 == 0xffff {
                    return;
                }
            }
        }
        // the sustain, to the end
        loop {
            let v = if sustain == 0 {
                0
            } else {
                let v = rd(s, i) as i32 * sustain as i32;
                if boost { ((v as i16) >> 6).clamp(-128, 127) } else { (v as i16) >> 8 }
            };
            wr(s, i, v as u8);
            i += 1;
            d4 = d4.wrapping_sub(1);
            if d4 == 0xffff {
                return;
            }
        }
    }
}

impl Replayer for PreTracker {
    fn format(&self) -> &'static str {
        "PreTracker"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        self.frame
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsong_count - 1, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        let d: Vec<u8> = (0..self.size).map(|i| hw.bus.r8(self.base + i)).collect();
        self.sub = n.min(self.subsong_count - 1);
        self.song_init(&d);
        self.init_channels();
        self.songend = false;
        // the reference's subsong change comes some 6700 colour clocks after
        // the last, 28.5 more for each change
        self.changes += 1;
        let k = self.changes as u64;
        let after = if k == 1 { 6669 } else { 6724 + 57 * (k - 2) / 2 };
        let now = hw.bus.clock;
        hw.wait((self.silence_at + after).saturating_sub(now));
        self.silence(hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        let vblank = self.next_vblank();
        self.work = [0; 4];
        if self.pat_stopped != 0 {
            self.process_pattern(hw.bus);
            self.advance_pattern();
        }
        self.process_instruments(hw.bus);
        let at = vblank + self.work_time(hw.bus);
        let now = hw.bus.clock;
        hw.wait(at.saturating_sub(now));
        self.output(hw);
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One position, one pattern with one note on voice 0 setting speed 6, one
    /// instrument whose one step selects the one wave: a saw, generated at init.
    fn module() -> Vec<u8> {
        let mut d = b"PRT\x1b".to_vec();
        for o in [0x5cu32, 0x64, 0x70, 0x9b] {
            d.extend(o.to_be_bytes()); // positions, patterns, instrument names, wave names
        }
        d.extend(b"test");
        d.resize(0x3c, 0);
        d.extend([0, 1, 1, 4, 1, 1]); // restart, patterns, positions, steps, instruments, waves
        d.extend(0..24u8); // the order the waves are generated in
        d.resize(0x5c, 0);
        d.extend([1, 0, 0, 0, 0, 0, 0, 0]); // position 0: pattern 1 on voice 0
        d.extend([0x19, 0x1f, 0x06]); // pitch $19, instrument 1, speed 6
        d.extend([0; 9]);
        d.extend([0; 32]); // instrument names
        d.extend([0, 0, 0, 0, 0, 15, 0, 1]); // fastest attack, full sustain, 1 step
        d.extend([0x01, 0x00, 0x01]); // its step: wave 1
        d.extend([0; 24]); // wave names
        d.push(0); // to an even address
        let mut w = [0u8; 42];
        w[0x0b] = 0x80; // chip RAM length
        w[0x17] = 24; // base note
        w[0x18] = 0x40; // gain
        w[0x1e] = 0xff; // sustain: no volume envelope
        d.extend(w);
        d
    }

    /// The sample-length byte of a looping wave (flags bit 2) far past the sample buffer
    /// must not index past it.
    #[test]
    fn oversized_sample_length_stays_in_the_buffer() {
        let mut d = module();
        let n = d.len();
        d[n - 42 + 0x14] = 4; // flags
        d[n - 42 + 0x19] = 0xff; // sample length
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        let _ = e.play_data(Path::new("test.prt"), d);
        let mut out = vec![0f32; 2 * 960];
        e.render(&mut out);
    }

    #[test]
    fn generates_a_wave_and_plays_it() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.prt"), module()).unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "PreTracker");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // init: all off, the voices cleared and on, then the silence: each
        // voice off, all on, each voice's loop
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..15], &[0x000F, 0x820F, 1, 2, 4, 8, 1, 2, 4, 8, 0x800F, 0x8001, 0x8002, 0x8004, 0x8008]);
        // voice 0: the empty sample, then the wave (64 words) at volume 0 and
        // period $71, then its period (pitch $19: PERIODS[24]) at volume 64
        assert_eq!(reg(0xDFF0A4)[..3], [2, 1, 0x40]);
        assert_eq!(reg(0xDFF0A6)[..4], [0, 0x71, 0x71, super::t::PERIODS[24]]);
        assert_eq!(reg(0xDFF0A8)[..4], [0, 0, 0, 64]);
        let wave = ((reg(0xDFF0A0)[2] as usize) << 16) | reg(0xDFF0A2)[2] as usize;
        let ram = &e.m.bus.ram[wave..wave + 128];
        // a falling saw at half gain, four cycles of 32 bytes at base note 24
        let saw = |i: usize| if i % 32 == 0 { 0x3f } else { (64 - 4 * (i % 32) as i32) as u8 };
        assert!(ram.iter().enumerate().all(|(i, &b)| b == saw(i)), "{ram:x?}");
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

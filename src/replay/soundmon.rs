//! Brian Postma's SoundMon 2.0 ("V.2") and 2.2 ("V.3"), ported from NostalgicPlayer's
//! SoundMon player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! The two versions follow their own original players, which differ more than
//! NostalgicPlayer's merged one lets on: version 2.0 is checked against SoundMon
//! V5.02 and 2.2 against SoundMon 2.2 V2.1 (Eagleplayers, adapted by DEFECT).
//! Where NostalgicPlayer differs from them, this follows the originals:
//!
//! * Paula voice n plays the step column 3 - n, and the columns are read 3 to 0.
//! * The sample pointer and length are written every tick, a one-shot sample's as
//!   the player's empty word with length 0; the arpeggio sets the period of a
//!   2.0 voice for one tick only, and 2.0 cycles it over 3 ticks, not 4.
//! * A new instrument comes with no range check; 2.0 turns DMA off only for a
//!   voice whose instrument changes (or that played a one-shot sample), and
//!   turns each restarted voice on by itself.
//! * A restart writes the volume byte, $FF for "the instrument's", before the
//!   instrument's volume; volumes are not clamped to 64.
//! * A 2.0 sample plays its loop only; a 2.2 one plays whole, then its loop.
//! * 2.0 synth sounds take their LFO and EG delays as words (with the byte before
//!   each) and an EG start value from a byte NostalgicPlayer skips; 2.2's
//!   averaging works (NostalgicPlayer never runs it), over all 32 bytes from the
//!   byte before the waveform.
//! * Jumps go where the original goes: 2.0's repeat counter (starting at 1, so a
//!   repeat end without a start does not jump) sets the step and carries on with
//!   the next row; 2.2's jump starts the step's first row.
//! * DMA off and on are as far apart as in the originals, so Paula restarts a
//!   voice that is still playing.
//!
//! The synth effects change the waveforms in the module, in guest memory, at the
//! moment the original does, and restore them when the voice restarts.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "SoundMon", detect, new };

/// Bytes of the player's empty sample: what Paula reads after a one-shot sample,
/// at length 0 (65536 words). The original's is its own memory, which is not silent.
const EMPTY_SIZE: u32 = 0x20000;

#[derive(Default)]
struct Voice {
    /// Bit 15: the voice restarts on this row.
    period: u16,
    /// $FF: the instrument's.
    volume: u8,
    instrument: u8,
    /// The sample pointer and length (words) written every tick.
    addr: u32,
    len: u16,
    note: u8,
    arp_value: u8,
    auto_slide: u8,
    auto_arp: u8,
    eg_ptr: u16,
    lfo_ptr: u16,
    adsr_ptr: u16,
    mod_ptr: u16,
    /// Words in 2.0, bytes in 2.2.
    eg_count: u16,
    lfo_count: u16,
    adsr_count: u16,
    mod_count: u8,
    fx_count: u8,
    synth: bool,
    old_eg: u8,
    eg_control: u8,
    lfo_control: u8,
    adsr_control: u8,
    mod_control: u8,
    vibrato: u8,
    fx_control: u8,
    /// The waveform the synth effects change (0: none), and its original bytes.
    wave: u32,
    saved: [u8; 32],
}

pub struct SoundMon {
    /// Version 2.2 ("V.3"), else 2.0 ("V.2").
    v22: bool,
    title: String,
    d: Vec<u8>,
    steps: u16,
    /// Guest address of the waveforms.
    waves: u32,
    /// Guest address of each instrument's sample.
    samples: [u32; 15],
    empty: u32,
    /// The module's end in guest memory.
    end: u32,
    step: u16,
    /// The row in the track, times 3.
    pat: u8,
    st: u8,
    tr: u8,
    count: u8,
    delay: u8,
    arp: u8,
    vib: usize,
    /// 2.0's repeat counter.
    rep: u8,
    /// 2.2's jump.
    jump: bool,
    new_pos: u8,
    /// The voices whose DMA goes off for the new row, then (2.2) those restarted.
    mask: u16,
    voices: [Voice; 4],
    songend: bool,
    /// 2.0: the colour clock of the last write of the tick's effects (the row's
    /// writes are timed from it), and the last voice whose volume the row set.
    t0: u64,
    last_vol: Option<usize>,
    /// 2.0: when the tick started, and whether its synth step wrote the first voice's volume.
    t_tick: u64,
    synth0: bool,
    /// 2.0: when the first voice's LFO wrote its period this tick.
    lfo0: Option<u64>,
    /// 2.0: the volume bytes the row has set so far.
    n_vol: u8,
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 512 && matches!(&d[26..29], b"V.2" | b"V.3")
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let v22 = &d[26..29] == b"V.3";
    let title: String = d[..26].iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
    let wave_num = d[29] as u32;
    let steps = be16(d, 30);
    // the waveforms follow the tracks, up to the highest one any step plays
    let mut max_track = 1i16;
    for k in 0..steps as usize * 4 {
        max_track = max_track.max(be16(d, 0x200 + 4 * k) as i16);
    }
    let waves = base + 0x200 + steps as u32 * 16 + max_track as u32 * 48;
    let mut samples = [0; 15];
    let mut a = waves + wave_num * 64;
    for (k, s) in samples.iter_mut().enumerate() {
        *s = a;
        let o = 0x20 + 32 * k;
        if d[o] != 0xff {
            a += be16(d, o + 0x18).wrapping_mul(2) as u32;
        }
    }
    let empty = m.alloc(EMPTY_SIZE);
    if empty == 0 {
        return None;
    }
    let mut voices: [Voice; 4] = Default::default();
    for v in &mut voices {
        v.addr = empty;
        v.len = 1;
    }
    Some(Box::new(SoundMon {
        v22,
        title,
        d: d.to_vec(),
        steps,
        waves,
        samples,
        empty,
        end: base + d.len() as u32,
        step: 0,
        pat: 0,
        st: 0,
        tr: 0,
        count: 1,
        delay: 6,
        arp: 1,
        vib: 0,
        // the original starts it at 1: a repeat end without its start does not jump
        rep: 1,
        jump: false,
        new_pos: 0,
        mask: 0,
        voices,
        songend: false,
        t0: 0,
        last_vol: None,
        t_tick: 0,
        synth0: false,
        lfo0: None,
        n_vol: 0,
    }))
}

/// A note's period. A note past the table makes the original read its own memory
/// there; this plays period 0 (Paula's slowest) instead.
fn period(note: i32) -> u16 {
    usize::try_from(note + 35).ok().and_then(|i| PERIODS.get(i)).copied().unwrap_or(0)
}

/// The length the original writes: 1 (and 0) as 0.
fn plen(len: u16) -> u16 {
    if len <= 1 {
        0
    } else {
        len
    }
}

/// DMA on as the original's ENPP_DMAMask does it, waiting for the DMA to start.
fn dma_on(hw: &mut Hw, mask: u16) {
    hw.dmacon(0x8000 | mask & 0xF);
    let (now, line) = (hw.bus.clock, crate::paula::LINE);
    hw.wait((now / line + 10) * line - now);
}

/// Colour clocks the originals take (measured in their write logs) from the DMA
/// off to the first restart, and for a restart up to its DMA on (2.0) or the next
/// voice's (2.2). Paula stops a playing voice only at the end of its word, so DMA
/// off and on at one moment would not restart it.
/// Colour clocks from the song's start to the first tick (the originals' set-up, which writes
/// the voices' empty sample first), from their write logs.
const TIMER_DELAY_20: u64 = 112;
/// 2.0 tick: the first voice's period follows its synth volume write by FIRST_PER_AFTER_VOL_20,
/// the pointer follows the volume byte by VOL_TO_LOC_20 or the tick's start by TICK_TO_LOC_20
/// when no period is written.
const FIRST_PER_AFTER_VOL_20: u64 = 315;
const VOL_TO_LOC_20: u64 = 345;
const TICK_TO_LOC_20: u64 = 78;
/// ... and the LFO's period (when the tick's first write): 48 after the start, the pointer
/// PER_TO_LOC_20 after it.
const TICK_TO_LFO_20: u64 = 48;
const PER_TO_LOC_20: u64 = 294;
/// 2.2: the originals' set-up takes some 19000 to 30000 colour clocks (it walks the song's steps
/// and the instruments) before the first tick, but their renders (WAV) do not contain that time,
/// only their write logs do, so the first tick comes at once.
const TIMER_DELAY_22: u64 = 0;
/// 2.2 tick, from the reference logs: each voice's synth step takes SYNTH_STEP_22 colour clocks,
/// its volume write comes VOL_PRE_22 into the step and the step goes on VOL_POST_22 after it,
/// an LFO period write LFO_PRE_22 after that.
const SYNTH_STEP_22: u64 = 28;
const VOL_PRE_22: u64 = 170;
const VOL_POST_22: u64 = 171;
const LFO_PRE_22: u64 = 135;
/// 2.2: the row's decoding takes this long (a fit; it depends on the row) before the DMA goes off.
const ROW_BASE_22: u64 = 1250;

const NEXT_COL_20: u64 = 167;
/// 2.0, from the last write of the tick's effects: a row's volume byte of voice k is
/// written VOL_TIME_20 + VOL_STEP_20 * k later, and DMA goes off AFTER_VOL_20[k] after
/// the last of those (ROW_NONE_20 after the effects with none), then the first
/// restarted voice's period follows OFF_TO_PER_20[voice].
const VOL_TIME_20: u64 = 596;
const VOL_STEP_20: u64 = 301;
/// ... and each volume byte already written delays the later ones by VOL_COST_20.
const VOL_COST_20: u64 = 289;
const AFTER_VOL_20: [u64; 4] = [1475, 859, 715, 256];
const ROW_NONE_20: u64 = 1781;
const OFF_TO_PER_20: [u64; 4] = [328, 190, 223, 255];
const OFF_WAIT_22: u64 = 388;
/// 2.2, from the reference logs: from the DMA off to the first restarted voice's period
/// (by voice), and from one voice's restart to the next voice's; within a restart the
/// period is followed by the pointer (voice 0: 169, the others 352), the length 40, the
/// volume (voice 0: 119, the others 225) and, for the last voice, the DMA on 300 after
/// the volume.
const OFF_TO_PER_22: [u64; 4] = [270, 301, 333, 364];
const NEXT_VOICE_22: u64 = 210;
const PER_TO_LOC_22: [u64; 4] = [169, 352, 352, 352];
const LOC_TO_LEN_22: u64 = 40;
const LEN_TO_VOL_22: [u64; 4] = [119, 225, 225, 225];
const VOL_TO_ON_22: u64 = 300;

/// A counter's decrement: 2.0 keeps words, 2.2 bytes.
fn dec(v22: bool, c: u16) -> u16 {
    if v22 {
        (c as u8).wrapping_sub(1) as u16
    } else {
        c.wrapping_sub(1)
    }
}

/// Steps a table pointer; a control of 1 plays the table once.
fn advance(ptr: &mut u16, control: &mut u8, len: u16) {
    *ptr = ptr.wrapping_add(1);
    if *ptr == len {
        *ptr = 0;
        if *control == 1 {
            *control = 0;
        }
    }
}

impl Replayer for SoundMon {
    fn format(&self) -> &'static str {
        if self.v22 {
            "SoundMon 2.2"
        } else {
            "SoundMon 2.0"
        }
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn timer_delay(&self) -> u64 {
        if self.v22 { TIMER_DELAY_22 } else { TIMER_DELAY_20 }
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        // Paula has played up to now from the waveforms as they were
        let b = &mut *hw.bus;
        b.paula.sync(b.clock, &b.ram);
        self.t_tick = b.clock;
        self.synth0 = false;
        self.lfo0 = None;
        if self.v22 {
            self.arp = self.arp.wrapping_sub(1) & 3;
            self.vib = (self.vib + 1) & 7;
            for i in 0..4 {
                self.effects(i, hw);
            }
            for i in 0..4 {
                self.synth(i, hw);
            }
        } else {
            for i in 0..4 {
                self.synth(i, hw);
            }
            self.arp = self.arp.wrapping_sub(1);
            for i in 0..4 {
                self.effects(i, hw);
            }
            if self.arp == 0 {
                self.arp = 3;
            }
        }

        self.count = self.count.wrapping_sub(1);
        if self.count != 0 {
            return;
        }
        self.count = self.delay;
        self.t0 = hw.bus.clock.saturating_sub(141);
        self.last_vol = None;
        self.n_vol = 0;
        self.next_row(hw);
        if self.v22 {
            hw.wait(ROW_BASE_22);
        }
        if !self.v22 {
            // the row's decoding takes as long as the original's before the DMA goes off
            let after = self.last_vol.map_or(ROW_NONE_20, |k| {
                VOL_TIME_20 + VOL_STEP_20 * k as u64 + VOL_COST_20 * (self.n_vol as u64 - 1) + AFTER_VOL_20[k]
            });
            let target = self.t0 + after;
            hw.wait(target.saturating_sub(hw.bus.clock));
        }
        hw.dmacon(self.mask & 0xF);
        if self.v22 {
            let first = (0..4).find(|&i| self.voices[i].period & 0x8000 != 0);
            hw.wait(first.map_or(OFF_WAIT_22, |i| OFF_TO_PER_22[i]));
            for v in &mut self.voices {
                if v.period & 0x8000 != 0 && v.wave != 0 {
                    hw.bus.write_bytes(v.wave, &v.saved);
                    v.wave = 0;
                }
            }
            self.mask = 0;
            for i in 0..4 {
                if self.voices[i].period & 0x8000 != 0 {
                    self.play_22(i, hw);
                }
            }
            hw.wait(VOL_TO_ON_22.saturating_sub(NEXT_VOICE_22));
            dma_on(hw, self.mask);
        } else {
            let mut first = true;
            for i in 0..4 {
                if self.voices[i].period & 0x8000 != 0 {
                    hw.wait(if first { OFF_TO_PER_20[i] } else { 0 });
                    first = false;
                    self.play_20(i, hw);
                }
            }
        }
    }
}

impl SoundMon {
    fn b(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }

    fn w(&self, o: usize) -> u16 {
        be16(&self.d, o)
    }

    /// An instrument's 32 bytes (0: the module's header, as the original takes it).
    fn inst(&self, n: u8) -> [u8; 32] {
        std::array::from_fn(|k| self.b(n as usize * 32 + k))
    }

    /// A waveform's address.
    fn wave(&self, n: u8) -> u32 {
        self.waves + 64 * n as u32
    }

    fn sample(&self, inst: u8) -> u32 {
        // past the 15 the original reads a sample address from its own memory past
        // its table; this plays the empty sample there (with the same lengths and
        // volumes, from the module's bytes where the instrument would be)
        (inst as usize).checked_sub(1).and_then(|i| self.samples.get(i)).copied().unwrap_or(self.empty)
    }

    /// A table's byte at `ptr` (the original indexes with a signed word).
    fn table(&self, hw: &Hw, n: u8, ptr: u16) -> u8 {
        hw.bus.r8(self.wave(n).wrapping_add(ptr as i16 as u32))
    }

    /// The real-time effects: autoslide, vibrato (2.2), arpeggio; the period,
    /// sample pointer and length every tick.
    fn effects(&mut self, i: usize, hw: &mut Hw) {
        let v22 = self.v22;
        let (arp, vib) = (self.arp, VIBRATO[self.vib] as i32);
        let synth0 = self.synth0;
        let t_tick = self.t_tick;
        let v = &mut self.voices[i];
        v.period = v.period.wrapping_add(v.auto_slide as i8 as u16);
        let mut pre = None;
        if v22 {
            let p = if v.vibrato != 0 { ((vib / v.vibrato as i32) as u16).wrapping_add(v.period) } else { v.period };
            hw.per(i, p);
        } else if v.lfo_control == 0 {
            // 2.0: the LFO sets the period instead
            if i == 0 && synth0 {
                hw.wait(FIRST_PER_AFTER_VOL_20);
            }
            hw.per(i, v.period);
        } else if i == 0 {
            // no period write: the pointer follows the volume byte (or the interrupt) as
            // far as the original's code takes
            let at = match self.lfo0 {
                Some(t) => t + PER_TO_LOC_20,
                None => t_tick + if synth0 { VOL_TO_LOC_20 } else { TICK_TO_LOC_20 },
            };
            pre = Some(at.saturating_sub(hw.bus.clock));
        }
        // the writes' spacing in the original's log: the first voice's follow the tick's
        // set-up, each voice's period, pointer and length then 84, 41 and 141 colour
        // clocks apart
        hw.wait(pre.unwrap_or(if i == 0 && !v22 { 294 } else { 84 }));
        hw.loc(i, v.addr);
        // 2.2: a voice whose synth step writes its volume this tick takes 2 fewer clocks here
        let vol_now = v22 && v.synth && v.adsr_control != 0 && dec(true, v.adsr_count) == 0;
        hw.wait(if v22 { if vol_now { 39 } else { 41 } } else if i == 0 { 39 } else { 41 });
        hw.len(i, plen(v.len));
        hw.wait(if v22 { 138 } else { 141 });
        if v.arp_value != 0 || v.auto_arp != 0 {
            let add = match arp {
                0 => (v.arp_value >> 4) + (v.auto_arp >> 4),
                1 => (v.arp_value & 15) + (v.auto_arp & 15),
                _ => 0,
            };
            let p = period(add.wrapping_add(v.note) as i8 as i32);
            hw.per(i, p);
            if v22 {
                v.period = p;
            }
        }
    }

    fn next_row(&mut self, hw: &mut Hw) {
        self.mask = 0;
        for i in 0..4 {
            let e = 0x200 + (self.step as usize * 16 & 0xffff) + 4 * (3 - i);
            let track = self.w(e);
            self.st = self.b(e + 2);
            self.tr = self.b(e + 3);
            let row = 0x200
                + self.pat as usize
                + (self.steps as usize * 16 & 0xffff)
                + track.wrapping_sub(1) as usize * 48;
            let (note, io, data) = (self.b(row), self.b(row + 1), self.b(row + 2));
            let opt = io & 15;
            let (v22, st, tr) = (self.v22, self.st, self.tr);
            let v = &mut self.voices[i];
            if note != 0 {
                v.auto_slide = 0;
                v.auto_arp = 0;
                if v22 {
                    v.vibrato = 0;
                }
                // a transpose optional can leave out the note transpose
                let n = if opt != 0xa || data & 0xf0 == 0 { note.wrapping_add(tr) as i8 as i32 } else { note as i32 };
                v.note = n as u8;
                v.period = period(n);
                let restart = !v22 || opt < 0xd;
                if restart {
                    v.period |= 0x8000;
                    v.volume = 0xff;
                }
                let mut inst = io >> 4;
                if inst == 0 {
                    inst = v.instrument;
                }
                if opt != 0xa || data & 0x0f == 0 {
                    inst = inst.wrapping_add(st);
                }
                let change = if v22 {
                    restart && (!v.synth || inst != v.instrument)
                } else {
                    v.len == 1 || inst != v.instrument
                };
                if change {
                    v.instrument = inst;
                    self.mask |= 1 << i;
                }
            }
            self.optional(i, opt, data, hw);
        }

        if self.jump {
            self.jump = false;
            self.pat = 0;
            self.step = self.step & 0xff00 | self.new_pos as u16;
            return;
        }
        self.pat += 3;
        if self.pat == 0x30 {
            self.pat = 0;
            self.step = self.step.wrapping_add(1);
            if self.step == self.steps {
                self.step = 0;
                self.songend = true;
            }
        }
    }

    fn optional(&mut self, i: usize, opt: u8, data: u8, hw: &mut Hw) {
        let v22 = self.v22;
        let v = &mut self.voices[i];
        match opt {
            0 => v.arp_value = data,
            1 => {
                v.volume = data;
                if !v22 || !v.synth {
                    if !v22 {
                        let t = self.t0 + VOL_TIME_20 + VOL_STEP_20 * i as u64 + VOL_COST_20 * self.n_vol as u64;
                        hw.wait(t.saturating_sub(hw.bus.clock));
                        self.last_vol = Some(i);
                        self.n_vol += 1;
                    }
                    hw.vol(i, data as u16);
                }
            }
            2 => {
                self.count = data;
                self.delay = data;
            }
            // ENPP_PokeCommand's LED filter
            3 if data == 0 => hw.bus.ciaa.pra |= 2,
            3 => hw.bus.ciaa.pra &= !2,
            4 => {
                v.period = v.period.wrapping_sub(data as u16);
                v.arp_value = 0;
            }
            5 => {
                v.period = v.period.wrapping_add(data as u16);
                v.arp_value = 0;
            }
            6 if v22 => v.vibrato = data,
            6 => self.rep = data,
            7 if v22 => {
                self.new_pos = data;
                self.jump = true;
            }
            7 => {
                self.rep = self.rep.wrapping_sub(1);
                if self.rep != 0 {
                    self.step = data as u16;
                }
            }
            8 => v.auto_slide = data,
            9 if !v22 => v.auto_arp = data,
            0xb if v22 => v.fx_control = data,
            9 | 0xd | 0xe | 0xf if v22 => {
                v.auto_arp = data;
                if opt == 0xd {
                    v.fx_control ^= 1;
                }
                if opt != 0xf {
                    v.adsr_ptr = 0;
                    if v.adsr_control == 0 {
                        v.adsr_control = 1;
                    }
                }
            }
            _ => {}
        }
    }

    /// The volume a restart writes: the voice's byte, then the instrument's if that is $FF.
    fn restart_volume(hw: &mut Hw, i: usize, volume: u8, inst_volume: u16) -> bool {
        hw.vol(i, volume as u16);
        if volume == 0xff {
            hw.wait(151);
            hw.vol(i, inst_volume);
        }
        volume == 0xff
    }

    /// The first ADSR volume of a synth sound.
    fn adsr_start(&mut self, i: usize, table: u8, inst_volume: u8, hw: &mut Hw) {
        let t = self.table(hw, table, 0);
        let v = &mut self.voices[i];
        if v.volume == 0xff {
            v.volume = inst_volume;
        }
        hw.vol(i, (t.wrapping_add(0x80) as u16 >> 2) * v.volume as u16 >> 6);
    }

    fn save_wave(&mut self, i: usize, wave: u32, hw: &Hw) {
        let v = &mut self.voices[i];
        v.wave = wave;
        for (k, s) in v.saved.iter_mut().enumerate() {
            *s = hw.bus.r8(wave + k as u32);
        }
    }

    /// 2.0's restart of a voice.
    fn play_20(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        v.period &= 0x7fff;
        let mut two = false;
        let mut sample = false;
        // the waveform the last sound changed (the pointer stays)
        if v.wave != 0 {
            hw.bus.write_bytes(v.wave, &v.saved);
        }
        hw.per(i, v.period);
        hw.wait(181);
        let inst = v.instrument;
        let n = self.inst(inst);
        let w = |o: usize| be16(&n, o);
        if n[0] == 0xff {
            let wave = self.wave(n[1]);
            let v = &mut self.voices[i];
            v.synth = true;
            v.eg_ptr = 0;
            v.lfo_ptr = 0;
            v.adsr_ptr = 0;
            v.eg_count = w(0x16).wrapping_add(1);
            v.lfo_count = w(0xe).wrapping_add(1);
            v.adsr_count = 1;
            v.eg_control = n[0x11];
            v.lfo_control = n[9];
            v.adsr_control = n[4];
            v.old_eg = n[0x13];
            v.addr = wave;
            v.len = w(2);
            hw.loc(i, wave);
            hw.wait(40);
            hw.len(i, plen(v.len));
            hw.wait(119);
            if n[4] != 0 {
                self.adsr_start(i, n[5], n[0x19], hw);
            } else {
                two = Self::restart_volume(hw, i, v.volume, n[0x19] as u16);
            }
            self.save_wave(i, wave, hw);
            // the EG's start value: the waveform's first bytes inverted
            if n[0x11] != 0 && n[0x13] != 0 {
                let e = n[0x13] >> 3;
                self.voices[i].old_eg = e;
                // a value below 8 has the original invert 65536 bytes; this stops at
                // the module's end
                let count = if e == 0 { 0x10000 } else { e as u32 };
                for a in wave..(wave + count).min(self.end) {
                    let x = hw.bus.r8(a);
                    hw.bus.w8(a, x.wrapping_neg());
                }
            }
        } else {
            sample = true;
            let a = self.sample(inst);
            let empty = self.empty;
            let v = &mut self.voices[i];
            v.wave = 0;
            v.synth = false;
            v.lfo_control = 0;
            v.adsr_control = 0;
            hw.loc(i, a);
            hw.wait(40);
            hw.len(i, plen(w(0x18)));
            hw.wait(119);
            two = Self::restart_volume(hw, i, v.volume, w(0x1e));
            hw.wait(if two { 187 } else { 201 });
            // only the loop plays: its pointer is written before DMA starts
            v.len = w(0x1c);
            v.addr = a + w(0x1a) as u32;
            if v.len == 1 {
                v.addr = empty;
            } else {
                hw.loc(i, v.addr);
                hw.len(i, plen(v.len));
            }
        }
        if !sample {
            hw.wait(if two { 187 } else { 201 });
        }
        hw.dmacon(0x8000 | 1 << i);
        // the original waits ten lines for the DMA to start, then goes on
        let (now, line) = (hw.bus.clock, crate::paula::LINE);
        hw.wait((now / line + 10) * line - now + NEXT_COL_20);
    }

    /// 2.2's restart of a voice.
    fn play_22(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        v.period &= 0x7fff;
        hw.per(i, v.period);
        hw.wait(PER_TO_LOC_22[i]);
        let inst = v.instrument;
        let n = self.inst(inst);
        let w = |o: usize| be16(&n, o);
        if n[0] == 0xff {
            let wave = self.wave(n[1]);
            let v = &mut self.voices[i];
            v.synth = true;
            v.eg_ptr = 0;
            v.lfo_ptr = 0;
            v.adsr_ptr = 0;
            v.mod_ptr = 0;
            v.eg_count = n[0x14].wrapping_add(1) as u16;
            v.lfo_count = n[0xe].wrapping_add(1) as u16;
            v.adsr_count = 1;
            v.mod_count = n[0x1c].wrapping_add(1);
            v.fx_count = n[0x18].wrapping_add(1);
            v.fx_control = n[0x16];
            v.eg_control = n[0x10];
            v.lfo_control = n[9];
            v.adsr_control = n[4];
            v.mod_control = n[0x19];
            v.old_eg = 0;
            v.addr = wave;
            v.len = w(2);
            hw.loc(i, wave);
            hw.wait(LOC_TO_LEN_22);
            hw.len(i, plen(v.len));
            hw.wait(LEN_TO_VOL_22[i]);
            if n[4] != 0 {
                self.adsr_start(i, n[5], n[0x1d], hw);
            } else {
                Self::restart_volume(hw, i, v.volume, n[0x1d] as u16);
            }
            if n[0x10] != 0 || n[0x19] != 0 || n[0x16] != 0 {
                self.save_wave(i, wave, hw);
            }
        } else {
            let a = self.sample(inst);
            let empty = self.empty;
            let v = &mut self.voices[i];
            v.synth = false;
            v.lfo_control = 0;
            hw.loc(i, a);
            hw.wait(LOC_TO_LEN_22);
            hw.len(i, plen(w(0x18)));
            hw.wait(LEN_TO_VOL_22[i]);
            Self::restart_volume(hw, i, v.volume, w(0x1e));
            v.len = w(0x1c);
            let loop_start = w(0x1a);
            v.addr = a + loop_start as u32;
            if v.len == 1 {
                v.addr = empty;
            } else if loop_start >> 1 != 0 {
                // the original means to write the length up to the loop end here,
                // but calls ENPP_PokeLen through the voice's synth buffer pointer
                // instead of its EaglePlayer base: it jumps into its own code and
                // writes nothing. The whole sample plays, then (from the next tick)
                // its loop.
            } else {
                hw.loc(i, v.addr);
                hw.len(i, plen(v.len));
            }
        }
        hw.wait(NEXT_VOICE_22);
        self.mask |= 1 << i;
    }

    /// The synth effects of a voice playing a synth sound.
    fn synth(&mut self, i: usize, hw: &mut Hw) {
        let v22 = self.v22;
        if v22 {
            hw.wait(SYNTH_STEP_22);
        }
        if !self.voices[i].synth {
            return;
        }
        let n = self.inst(self.voices[i].instrument);
        let w = |o: usize| be16(&n, o);
        // the instrument's offsets that differ between the versions
        let (lfo_speed, eg_table, eg_len, eg_speed) = if v22 { (0xf, 0x11, 0x12, 0x15) } else { (0x10, 0x12, 0x14, 0x18) };

        if self.voices[i].adsr_control != 0 {
            let v = &mut self.voices[i];
            v.adsr_count = dec(v22, v.adsr_count);
            if v.adsr_count == 0 {
                v.adsr_count = n[8] as u16;
                let t = self.table(hw, n[5], self.voices[i].adsr_ptr);
                let v = &mut self.voices[i];
                if v22 {
                    hw.wait(VOL_PRE_22);
                }
                hw.vol(i, (t.wrapping_add(0x80) as u16 >> 2) * v.volume as u16 >> 6);
                if v22 {
                    hw.wait(VOL_POST_22);
                }
                if i == 0 {
                    self.synth0 = true;
                }
                let v = &mut self.voices[i];
                advance(&mut v.adsr_ptr, &mut v.adsr_control, w(6));
            }
        }

        if self.voices[i].lfo_control != 0 {
            let v = &mut self.voices[i];
            v.lfo_count = dec(v22, v.lfo_count);
            if v.lfo_count == 0 {
                v.lfo_count = n[lfo_speed] as u16;
                let mut t = self.table(hw, n[0xa], self.voices[i].lfo_ptr) as i8 as i32;
                if n[0xb] != 0 {
                    t /= n[0xb] as i32;
                }
                if i == 0 && !v22 {
                    let at = self.t_tick + if self.synth0 { FIRST_PER_AFTER_VOL_20 } else { TICK_TO_LFO_20 };
                    hw.wait(at.saturating_sub(hw.bus.clock));
                    self.lfo0 = Some(hw.bus.clock);
                }
                if v22 {
                    hw.wait(LFO_PRE_22);
                }
                let v = &mut self.voices[i];
                hw.per(i, v.period.wrapping_add(t as u16));
                advance(&mut v.lfo_ptr, &mut v.lfo_control, w(0xc));
            }
        }

        let wave = self.voices[i].wave;
        if v22 && wave == 0 {
            return;
        }
        if self.voices[i].eg_control != 0 {
            let v = &mut self.voices[i];
            v.eg_count = dec(v22, v.eg_count);
            // without a waveform the counter stays at 0
            if v.eg_count == 0 && wave != 0 {
                v.eg_count = n[eg_speed] as u16;
                let t = self.table(hw, n[eg_table], self.voices[i].eg_ptr);
                self.eg(i, t.wrapping_add(0x80) >> 3, hw);
                let v = &mut self.voices[i];
                advance(&mut v.eg_ptr, &mut v.eg_control, w(eg_len));
            }
        }
        if !v22 {
            return;
        }
        self.fx(i, n[0x17], wave, hw);

        if self.voices[i].mod_control != 0 {
            let v = &mut self.voices[i];
            v.mod_count = v.mod_count.wrapping_sub(1);
            if v.mod_count == 0 {
                v.mod_count = n[0x1b];
                let t = self.table(hw, n[0x1a], self.voices[i].mod_ptr);
                hw.bus.w8(wave + 32, t);
                let v = &mut self.voices[i];
                advance(&mut v.mod_ptr, &mut v.mod_control, w(0x1e));
            }
        }
    }

    /// The EG: inverts the waveform's first bytes up to the new value, or restores
    /// them down to it.
    fn eg(&mut self, i: usize, new: u8, hw: &mut Hw) {
        let v = &mut self.voices[i];
        let old = v.old_eg;
        v.old_eg = new;
        if new > old {
            for k in old..new {
                hw.bus.w8(v.wave + k as u32, v.saved[k as usize & 31].wrapping_neg());
            }
        } else {
            for k in (new..old).rev() {
                hw.bus.w8(v.wave + k as u32, v.saved[k as usize & 31]);
            }
        }
    }

    /// 2.2's waveform effects.
    fn fx(&mut self, i: usize, speed: u8, wave: u32, hw: &mut Hw) {
        let bus = &mut *hw.bus;
        let v = &mut self.voices[i];
        // towards a target waveform, by `speed` a byte
        let transform = |bus: &mut crate::bus::Bus, target: &dyn Fn(&crate::bus::Bus, u32) -> u8| {
            for k in 0..32 {
                let s = target(bus, k) as i8;
                let a = wave + k;
                let x = bus.r8(a);
                if s > x as i8 {
                    bus.w8(a, x.wrapping_add(speed));
                } else if s < x as i8 {
                    bus.w8(a, x.wrapping_sub(speed));
                }
            }
        };
        let saved = v.saved;
        match v.fx_control {
            1 => {
                v.fx_count = v.fx_count.wrapping_sub(1);
                if v.fx_count == 0 {
                    v.fx_count = speed;
                    // averaging, from the byte before the waveform to the one after
                    let mut prev = bus.r8(wave.wrapping_sub(1)) as i8 as i16;
                    for k in (0..32).step_by(2) {
                        let a = wave + k;
                        let x = (bus.r8(a + 1) as i8 as i16 + prev) >> 1;
                        bus.w8(a, x as u8);
                        prev = (bus.r8(a + 2) as i8 as i16 + x) >> 1;
                        bus.w8(a + 1, prev as u8);
                    }
                }
            }
            2 => transform(bus, &|_, k| saved[31 - k as usize]),
            3 | 5 => transform(bus, &|_, k| saved[k as usize]),
            4 => transform(bus, &|bus, k| bus.r8(wave + 0x40 + k)),
            6 => {
                v.fx_count = v.fx_count.wrapping_sub(1);
                if v.fx_count == 0 {
                    v.fx_control = 0;
                    v.fx_count = 1;
                    for k in 0..32 {
                        let x = bus.r8(wave + 0x40 + k);
                        bus.w8(wave + k, x);
                    }
                }
            }
            _ => {}
        }
    }
}

include!("soundmon_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One step of one track with one note, all voices, of instrument 1: a synth
    /// sound on the module's one waveform, at volume 64 (its offset in `mark`'s layout).
    fn module(mark: &[u8; 3], volume_at: usize) -> Vec<u8> {
        let mut d = b"test".to_vec();
        d.resize(26, 0);
        d.extend(mark);
        d.extend([1, 0, 1]); // 1 waveform, 1 step
        d.resize(0x200, 0);
        d[0x20] = 0xff; // synth, waveform 0
        d[0x23] = 16; // of 16 words
        d[0x20 + volume_at] = 64;
        for _ in 0..4 {
            d.extend([0, 1, 0, 0]); // track 1, no transposes
        }
        d.extend([25, 0x10, 0]); // note 25, instrument 1
        d.resize(0x240, 0);
        d.extend((0..64).map(|k| (k * 4) as u8)); // the waveform
        d
    }

    fn play(d: Vec<u8>, format: &str) -> Vec<(u32, u16, u64)> {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.bp"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, format);
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        e.m.bus.write_log.take().unwrap()
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn plays_a_synth_note_on_paula() {
        let log = play(module(b"V.2", 0x19), "SoundMon 2.0");
        // the first tick's empty voices, then the note's period, waveform and volume
        assert_eq!(&reg(&log, 0xDFF0A6)[..2], &[0, super::PERIODS[60]]);
        assert_eq!(&reg(&log, 0xDFF0A4)[..2], &[0, 16]);
        assert_eq!(&reg(&log, 0xDFF0A8)[..2], &[0xff, 64]);
        // 2.0 turns each voice on by itself
        assert_eq!(&reg(&log, 0xDFF096)[..5], &[0x000F, 0x8001, 0x8002, 0x8004, 0x8008]);

        let log = play(module(b"V.3", 0x1d), "SoundMon 2.2");
        assert_eq!(&reg(&log, 0xDFF0A6)[..2], &[0, super::PERIODS[60]]);
        assert_eq!(&reg(&log, 0xDFF0A8)[..2], &[0xff, 64]);
        assert_eq!(&reg(&log, 0xDFF096)[..2], &[0x000F, 0x800F]);
    }
}

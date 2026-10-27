//! SidMon 1.0, ported from NostalgicPlayer's SidMon 1.0 player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A SidMon 1.0 module is the editor's replayer with the song data in it, and
//! NostalgicPlayer finds the song by reading the replayer's code (which comes in
//! several versions). The original this follows, the "SIDMon 1.0" Eagleplayer
//! (V4.01, "adapted by DEFECT"), has one replayer of its own: it finds the song by
//! its ID string (or the replayer's `lea` to it), reads the eleven offsets before
//! the ID, and plays from the module in guest memory, as this port does. So where
//! NostalgicPlayer's versions differ, this plays as the Eagleplayer: one period
//! table (NostalgicPlayer's second), the finetune not limited to 6, no special
//! samples, the waveform mixing and the filter effect (waveform 1 negated at the
//! first mix position every tick) always on, and a loop start of 0 as no loop.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * every voice ends with DMACON $800F (which waits 10 lines); a voice that
//!   plays a sample, or changes instrument after one, turns its DMA off first;
//! * location, period, volume and length are written only when the original's
//!   flags for them are nonzero: the volume not when the envelope's level is 0
//!   (or ended), the length only with an instrument (16 words for a waveform);
//! * a sample's loop is written on the second tick after it starts, and no loop
//!   plays the word before the song's offsets, with a length of 1;
//! * the first tick plays no row but the effects, with the instruments of the
//!   first rows and waveform list 0 (read before the list);
//! * a note without an instrument on a sample voice restarts the sample, also
//!   note $FF; an instrument row with a sample skips the instrument and waveform
//!   setup when the original's d1 happens to be 1 (left so by the previous
//!   voice's phase shift);
//! * the envelope, notes, arpeggio, bend and waveform steps use the original's
//!   byte and word arithmetic (the note offset wraps at 128, the bend stops on
//!   reaching its period, the phase shift rounds down, the mix rounds down);
//! * nothing is range-checked: instruments, waveforms, samples and positions
//!   read on in memory as the original does (a period past the table, which the
//!   original reads past its code hunk, plays 0 here).
//!
//! Checked against the Eagleplayer's write logs (run with an empty DELIPLAYERS
//! directory, as the DeliPlayer Sidmon1.0_note would play these first), 37 of
//! 52 modules write the same registers tick by tick; the other 15 differ only
//! where a period is read past the table, mostly for instrument 0 (32 bytes
//! before the instruments) with a stray finetune, or an instrument from 60 on.
//! With those periods masked, all 52 are the same. The time between writes is
//! the original's too (see `PLAY`), within a few dozen colour clocks on most
//! code paths.

use super::{be32, Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "SidMon 1.0", detect, new };

/// The ID the original looks for, with the word after it.
const ID: &[u8; 34] = b" SID-MON BY R.v.VLIET  (c) 1988 \0\0";

/// The song data's offset in the module, as the original's Check finds it: the
/// ID at an even offset from $2C, or else the replayer's `lea ID(pc),a0` followed
/// by `adda.l -$2C(a0),a0; lea x(pc),a1; move.l a0,(a1)` in its first 100 bytes.
fn find(d: &[u8]) -> Option<usize> {
    let id = (0x2c..d.len().saturating_sub(1)).step_by(2).find(|&o| d.get(o..o + ID.len()) == Some(&ID[..]));
    let id = id.or_else(|| {
        (0..100).step_by(2).find_map(|o| {
            let w = |k: usize| super::be16(d, o + k);
            (w(0) == 0x41fa && be32(d, o + 4) == 0xd1e8ffd4 && w(8) == 0x43fa && w(12) == 0x2288)
                .then(|| (o as isize + 2 + w(2) as i16 as isize) as usize)
        })
    })?;
    (id >= 0x2c && id <= d.len()).then_some(id)
}

fn detect(d: &[u8]) -> bool {
    find(d).is_some()
}

/// One voice: the original's 32-byte voice structure, and its loop.
#[derive(Default, Clone, Copy)]
struct Voice {
    /// Guest address of the position's sequence entry (track, -, transpose).
    seq: u32,
    /// Guest address of the next row (note, instrument, effect, param, duration).
    row: u32,
    /// Guest address of the instrument (32 bytes).
    instrument: u32,
    period: u16,
    bend_to: u8,
    bend_speed: u8,
    /// The note's byte offset in the period table (twice the note).
    note: u8,
    arp_index: u8,
    /// 0 attack, 2 decay, 4 sustain, 6 release, 8 done.
    envelope: u8,
    row_count: u8,
    volume: u8,
    sustain: u8,
    pitch: u16,
    phase_index: u8,
    phase_speed: u8,
    pitch_fall: u16,
    wave_index: u8,
    waveform: u8,
    wave_speed: u8,
    /// 1 when a sample started this tick, 2 the tick after, 3 once its loop is
    /// written; 0 for a waveform.
    sample: u8,
    /// The sample's loop ($FFFFFFFF: none) and its length in words.
    loop_addr: u32,
    loop_len: u32,
}

/// The module's bounds in guest memory: the player writes only inside them.
#[derive(Clone, Copy)]
struct Module {
    base: u32,
    size: u32,
}

impl Module {
    fn w8(self, bus: &mut Bus, a: u32, v: u8) {
        if a.wrapping_sub(self.base) < self.size {
            bus.w8(a, v);
        }
    }
}

pub struct SidMon1 {
    module: Module,
    /// The song data's ID, which the offsets before it count from.
    id: u32,
    seqs: [u32; 4],
    instruments: u32,
    waveforms: u32,
    song: u32,
    wave_lists: u32,
    track_data: u32,
    track_offsets: u32,
    samples: u32,
    speed_counter: u32,
    row: u32,
    position: u32,
    new_track: bool,
    loop_song: bool,
    mix_counter: [u32; 2],
    mix_position: [u32; 2],
    /// The original's d1 is 1 as the next voice starts.
    d1_one: bool,
    voices: [Voice; 4],
    songend: bool,
}

/// What a voice writes at its end (zero: not written).
#[derive(Default)]
struct Writes {
    loc: u32,
    per: u16,
    vol: u16,
    len: u16,
    /// A sample was set up this tick (a waveform does not replace it).
    sample: bool,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let id = base + find(d)? as u32;
    // the Eagleplayer's InitSound points the voices at a 32-byte data hunk of zeros
    let zeros = m.alloc(32);
    if zeros == 0 {
        return None;
    }
    let bus = &mut m.bus;
    let off = |bus: &Bus, k: u32| id.wrapping_add(bus.r32(id.wrapping_sub(0x2c - 4 * k)));
    let mut p = SidMon1 {
        module: Module { base, size: d.len() as u32 },
        id,
        seqs: std::array::from_fn(|i| off(bus, i as u32)),
        instruments: off(bus, 4),
        waveforms: off(bus, 5),
        song: off(bus, 6),
        wave_lists: off(bus, 7),
        track_data: off(bus, 8),
        track_offsets: off(bus, 9),
        samples: off(bus, 10),
        speed_counter: 0,
        row: 0xffff_ffff,
        position: 1,
        new_track: false,
        loop_song: false,
        mix_counter: [0; 2],
        mix_position: [0; 2],
        d1_one: false,
        voices: Default::default(),
        songend: false,
    };
    p.speed_counter = bus.r32(p.song + 0x20);
    let mut hw = Hw { bus };
    hw.dmacon(0x000f);
    hw.wait(800);
    for ch in 0..4 {
        let row = p.track(hw.bus, p.seqs[ch]);
        let instr = hw.bus.r8(row + 1).wrapping_sub(1) as u32;
        p.voices[ch] = Voice {
            seq: p.seqs[ch],
            row,
            instrument: p.instruments.wrapping_add(32 * instr),
            period: 0x9999,
            envelope: 8,
            ..Default::default()
        };
        hw.loc(ch, zeros);
        hw.wait(105);
        hw.len(ch, 16);
        hw.wait([129, 141, 152, 147][ch]);
        hw.vol(ch, 0);
        hw.wait(if ch < 3 { 302 } else { 155 });
    }
    dma_on(&mut hw);
    Some(Box::new(p))
}

/// The original's timing, measured on the Eagleplayer's write logs (colour
/// clocks). Paula stops a playing voice only at the end of its word and restarts
/// it only once stopped, so the time from a note's DMA off to the voice's DMA on
/// decides whether the note starts.
///
/// From the timer interrupt to the first voice.
const PLAY: u64 = 178;
/// ENPP_DMAMask turning a voice off.
const DMA_OFF: u64 = 110;

#[derive(Clone, Copy)]
enum Reg {
    None,
    Loc,
    Per,
    Vol,
    Len,
    Dma,
}

/// From the voice's effects to its first write, by the code they run.
fn effects_time(ch: usize, v: &Voice, phase_shift: u8) -> u64 {
    // the scope pointer's compare chain is longer for the later voices
    let mut t = 623 + [0, 15, 30, 34][ch];
    t += [-2i64, 0, -6, 26, -17].get(v.envelope as usize / 2).copied().unwrap_or(-17);
    if v.bend_speed != 0 {
        t += 111;
    }
    if phase_shift != 0 {
        t += if v.phase_speed == 0 { 111 } else { 18 };
    }
    t += if v.sample != 0 {
        -28
    } else if v.wave_speed != 0 {
        -10
    } else if v.wave_index != 0x10 {
        90
    } else {
        0
    };
    t as u64
}

/// From one of a voice's writes to the next.
fn gap(last: Reg, next: Reg, ch: usize) -> u64 {
    use Reg::*;
    match (last, next) {
        (None, Dma) => 22,
        (None, _) => 0,
        (Loc, Per) => 135,
        (Loc, Vol) => 197,
        (Loc, Len) => 105,
        (Loc, Dma) => 157,
        (Per, Vol) => [163, 174, 186, 181][ch],
        (Per, Len) => 148,
        (Per, Dma) => 141,
        (Vol, Len) => 125,
        (Vol, Dma) => 118,
        (Len, Dma) => 114,
        _ => 120,
    }
}

/// The host's ENPP_DMAMask with the voices on: DMACON $800F, then it waits as
/// dtg_WaitAudioDMA does, to 10 lines past the current one.
fn dma_on(hw: &mut Hw) {
    hw.dmacon(0x800f);
    let now = hw.bus.clock;
    hw.wait((now / LINE + 10) * LINE - now);
}

/// The period at `offset` bytes into finetune `ft`'s table (the original's word
/// arithmetic; past its table it reads past its code hunk, here 0).
fn period(ft: u8, offset: u16) -> u16 {
    let o = (ft as u16).wrapping_mul(136) as i16 as i32 + offset as i32;
    if o < 0 || o as usize / 2 >= PERIODS.len() {
        return 0;
    }
    PERIODS.get(o as usize / 2).copied().unwrap_or(0)
}

/// `base + ((n - 1) << shift)` in the original's word arithmetic.
fn entry(base: u32, n: u16, shift: u32) -> u32 {
    base.wrapping_add((n.wrapping_sub(1) << shift) as i16 as i32 as u32)
}

impl Replayer for SidMon1 {
    fn format(&self) -> &'static str {
        "SidMon 1.0"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        hw.wait(PLAY);
        // the voices play in the order of their bits 1, 2, 8, 4
        for ch in [0, 1, 3, 2] {
            self.play_voice(ch, hw);
        }
        self.speed_counter = self.speed_counter.wrapping_add(1);
        if hw.bus.r32(self.song + 0x20) < self.speed_counter {
            self.speed_counter = 0;
        }
        self.new_track = false;
        self.loop_song = false;
        if self.speed_counter == 0 {
            self.row = self.row.wrapping_add(1);
            if self.row == hw.bus.r32(self.song + 0x18) {
                self.row = 0;
                self.new_track = true;
                self.position = self.position.wrapping_add(1);
                if hw.bus.r32(self.song + 0x1c) <= self.position {
                    self.songend = true;
                    self.position = 1;
                    self.loop_song = true;
                }
            }
        }
        self.mix(hw, 0);
        self.mix(hw, 1);
        // the filter effect
        let a = self.waveforms.wrapping_add(self.mix_position[0]);
        let b = hw.bus.r8(a).wrapping_neg();
        self.module.w8(hw.bus, a, b);
        // a sample's loop, on the second tick after it started
        hw.wait(389);
        for ch in 0..4 {
            hw.wait(34);
            let v = &mut self.voices[ch];
            match v.sample {
                1 => v.sample = 2,
                2 => {
                    v.sample = 3;
                    let (a, len) = if v.loop_addr != 0xffff_ffff {
                        (v.loop_addr, v.loop_len as u16)
                    } else {
                        (self.id.wrapping_sub(0x2c), 1)
                    };
                    hw.loc(ch, a);
                    hw.wait(107);
                    hw.len(ch, len);
                    hw.wait(146);
                }
                _ => {}
            }
        }
    }
}

impl SidMon1 {
    /// The first row of the track of the sequence entry at `seq`.
    fn track(&self, bus: &Bus, seq: u32) -> u32 {
        let n = (bus.r32(seq) as u16).wrapping_mul(4) as i16 as i32 as u32;
        self.track_data.wrapping_add(bus.r32(self.track_offsets.wrapping_add(n)))
    }

    /// Starts sample `n` (16 and up) on the voice.
    fn play_sample(&mut self, ch: usize, n: u16, hw: &mut Hw, w: &mut Writes) {
        if self.voices[ch].sample != 0 {
            hw.dmacon(1 << ch);
            hw.wait(DMA_OFF);
        }
        let bus = &*hw.bus;
        let v = &mut self.voices[ch];
        let info = self.samples.wrapping_add(4).wrapping_add((n.wrapping_sub(0x10) << 5) as i16 as i32 as u32);
        let data = self.samples.wrapping_add(bus.r32(self.samples)).wrapping_add(4);
        let start = bus.r32(info);
        let end = bus.r32(info + 8);
        w.loc = data.wrapping_add(start);
        w.sample = true;
        let ls = bus.r32(info + 4);
        v.loop_addr = if ls == 0 { 0xffff_ffff } else { data.wrapping_add(ls) };
        v.loop_len = (data.wrapping_add(end).wrapping_sub(v.loop_addr) as i32 >> 1) as u32;
        w.len = (end.wrapping_sub(start) as i32 >> 1) as u16;
        v.sample = 1;
    }

    fn play_voice(&mut self, ch: usize, hw: &mut Hw) {
        let mut w = Writes::default();
        let d1_one = std::mem::take(&mut self.d1_one);
        if self.speed_counter == 0 {
            if self.new_track {
                let v = &mut self.voices[ch];
                v.row_count = 0;
                v.seq = v.seq.wrapping_add(6);
                if self.loop_song {
                    v.seq = self.seqs[ch];
                }
                let seq = v.seq;
                self.voices[ch].row = self.track(hw.bus, seq);
            }
            if self.voices[ch].row_count != 0 {
                self.voices[ch].row_count -= 1;
            } else {
                self.play_row(ch, hw, &mut w, d1_one);
            }
        }
        let v = self.voices[ch];
        let shift = hw.bus.r8(v.instrument + 0x1c);
        let time = effects_time(ch, &v, shift);
        self.effects(ch, hw, &mut w);

        // each write waits the time the original takes from the one before
        hw.wait(time);
        let mut last = Reg::None;
        let mut write = |hw: &mut Hw, r: Reg| {
            hw.wait(gap(last, r, ch));
            last = r;
        };
        if w.loc != 0 {
            write(hw, Reg::Loc);
            hw.loc(ch, w.loc);
        }
        if w.per != 0 {
            write(hw, Reg::Per);
            hw.per(ch, self.voices[ch].period);
        }
        if w.vol != 0 {
            write(hw, Reg::Vol);
            hw.vol(ch, (w.vol >> 2) & 0x7f);
        }
        if w.len != 0 {
            write(hw, Reg::Len);
            hw.len(ch, w.len);
        }
        write(hw, Reg::Dma);
        dma_on(hw);
    }

    fn play_row(&mut self, ch: usize, hw: &mut Hw, w: &mut Writes, d1_one: bool) {
        let row = self.voices[ch].row;
        let r = |k: u32| hw.bus.r8(row + k);
        let (note, instr, effect, param, duration) = (r(0), r(1), r(2), r(3), r(4));
        if instr != 0 {
            let inst = entry(self.instruments, instr as u16, 5);
            w.len = 0x10;
            // a new track's first row takes longer to get here
            hw.wait(if self.new_track { 325 + 108 } else { 325 });
            if self.voices[ch].sample != 0 {
                hw.dmacon(1 << ch);
                hw.wait(DMA_OFF);
            }
            self.voices[ch].sample = 0;
            let wf = hw.bus.r32(inst);
            // the sample's setup returns past the instrument's when d1 is 1
            if wf as u8 >= 0x10 {
                self.play_sample(ch, wf as u16, hw, w);
            }
            hw.wait(if w.sample { 645 } else { 165 });
            if !(wf as u8 >= 0x10 && d1_one) {
                let v = &mut self.voices[ch];
                v.wave_index = 0;
                v.waveform = wf as u8;
                // the list number's low byte less 1, then the word times 16
                let n = (wf as u16 & 0xff00) | v.waveform.wrapping_sub(1) as u16;
                let list = self.wave_lists.wrapping_add((n << 4) as i16 as i32 as u32);
                let first = hw.bus.r8(list);
                v.wave_speed = hw.bus.r8(list + 1);
                let wave = self.waveforms.wrapping_add(((first as u16).wrapping_sub(1) as u32) << 5);
                if !w.sample {
                    w.loc = wave;
                }
                v.instrument = inst;
                v.envelope = 0;
                v.pitch_fall = 0;
                v.pitch = 0;
                v.row_count = duration;
            }
        } else if note != 0 {
            self.voices[ch].row_count = duration;
            if self.voices[ch].sample != 0 {
                // the sample's DMA off comes later on the later voices (a longer
                // compare chain for the voice's bit)
                let chain = [0, 11, 22, 17][ch];
                hw.wait(435 + chain);
                let n = self.voices[ch].waveform as u16;
                self.play_sample(ch, n, hw, w);
                hw.wait(440 - DMA_OFF);
            } else {
                // a new track's first row and a note off take shorter ways
                hw.wait(if self.new_track { 296 - 41 } else if note == 0xff { 296 - 125 } else { 296 });
            }
        } else {
            hw.wait(60);
        }

        let module = self.module;
        let song = self.song;
        let v = &mut self.voices[ch];
        let inst = v.instrument;
        if note == 0xff {
            v.row_count = duration;
        } else if note != 0 {
            v.row_count = duration;
            let transpose = hw.bus.r8(v.seq + 5);
            let n = note.wrapping_add(transpose) as u16;
            let p = period(hw.bus.r8(inst + 0x1e), 2 * n + 2);
            v.period = p;
            v.note = (2 * n) as u8;
            w.per = p;
            v.envelope = 0;
            v.volume = 0;
            v.sustain = 0;
            v.pitch = 0;
            v.pitch_fall = 0;
            v.bend_speed = 0;
            match effect {
                // a new attack (NostalgicPlayer also clears the waveform delay
                // without one)
                0 => {
                    if param != 0 {
                        module.w8(hw.bus, inst + 0x15, param);
                        module.w8(hw.bus, inst + 0x14, param);
                        v.wave_speed = 0;
                    }
                }
                // the speed's and the rows' low bytes in the song data
                2 | 3 => {
                    let a = song + if effect == 2 { 0x23 } else { 0x1b };
                    module.w8(hw.bus, a, param);
                    v.wave_speed = 0;
                }
                _ => {
                    v.bend_to = effect.wrapping_add(transpose);
                    v.bend_speed = param;
                }
            }
            v.phase_speed = hw.bus.r8(inst + 0x1d);
        }
        self.voices[ch].row = row + 5;
    }

    fn effects(&mut self, ch: usize, hw: &mut Hw, w: &mut Writes) {
        let bus = &*hw.bus;
        let v = &mut self.voices[ch];
        let inst = v.instrument;
        let r = |k: u32| bus.r8(inst + k) as u16;

        // the envelope; its level (a word) is the volume flag
        let vol = v.volume as u16;
        match v.envelope {
            0 => {
                let (speed, max) = (r(0x14), r(0x15));
                w.vol = vol + speed;
                if w.vol > max {
                    v.envelope += 2;
                    w.vol = max;
                }
                v.volume = w.vol as u8;
            }
            2 | 6 => {
                let k = if v.envelope == 2 { 0x16 } else { 0x1a };
                let (speed, min) = (r(k), r(k + 1));
                w.vol = vol.wrapping_sub(speed);
                // decay ends at $FF00 and up, release above it (the same here)
                if w.vol <= min || w.vol >= 0xff00 {
                    v.envelope += 2;
                    w.vol = min;
                    if k == 0x16 {
                        v.sustain = r(0x18) as u8;
                    }
                }
                v.volume = w.vol as u8;
            }
            4 => {
                w.vol = vol;
                v.sustain = v.sustain.wrapping_sub(1);
                if v.sustain == 0 {
                    v.envelope += 2;
                }
            }
            _ => {}
        }

        // the arpeggio
        v.arp_index = (v.arp_index + 1) & 0xf;
        let arp = r(4 + v.arp_index as u32) as u8;
        let ft = r(0x1e) as u8;
        v.period = period(ft, v.note.wrapping_add(arp.wrapping_mul(2)) as u16);
        w.per = v.period;

        // the bend, which stops on reaching its period
        if v.bend_speed != 0 {
            let speed = (v.bend_speed as i8).wrapping_neg() as i16;
            let to = period(ft, 2 * v.bend_to as u16);
            v.pitch = v.pitch.wrapping_add(speed as u16);
            v.period = v.period.wrapping_add(v.pitch);
            if (speed < 0 && to >= v.period) || (speed >= 0 && to <= v.period) {
                v.note = v.bend_to.wrapping_mul(2);
                v.period = to;
                v.pitch = 0;
                v.bend_speed = 0;
            }
        }

        // the phase shift, by a waveform (rounding down)
        let shift = r(0x1c) as u8;
        if shift != 0 {
            if v.phase_speed == 0 {
                let wave = self.waveforms.wrapping_add(((shift - 1) as u32) << 5);
                v.phase_index = (v.phase_index + 1) & 0x1f;
                let d = bus.r8(wave + v.phase_index as u32) as i8 as i16 >> 2;
                v.period = v.period.wrapping_add(d as u16);
                self.d1_one = d == 1;
            } else {
                v.phase_speed -= 1;
            }
        }

        // the pitch fall
        let fall = (r(0x1f) as u8 as i8 as i16).wrapping_neg();
        v.pitch_fall = v.pitch_fall.wrapping_add(fall as u16);
        v.period = v.period.wrapping_add(v.pitch_fall);

        // the waveform list, for a waveform
        if v.sample == 0 {
            if v.wave_speed != 0 {
                v.wave_speed -= 1;
            } else if v.wave_index != 0x10 {
                let list = (self.wave_lists)
                    .wrapping_add((v.waveform.wrapping_sub(1) as u32) << 4)
                    .wrapping_add(v.wave_index as u32);
                let b = bus.r8(list);
                if b == 0xff {
                    v.wave_index = bus.r8(list + 1) & 0xfe;
                } else {
                    w.loc = self.waveforms.wrapping_add((b.wrapping_sub(1) as u32) << 5);
                    v.wave_speed = bus.r8(list + 1);
                    v.wave_index = v.wave_index.wrapping_add(2);
                }
            }
        }
    }

    /// Waveform mix `k`: every `speed` ticks the destination becomes the average
    /// of the first source and the second rotated by the mix position.
    fn mix(&mut self, hw: &mut Hw, k: usize) {
        let s = self.song + 4 * k as u32;
        let speed = hw.bus.r32(s + 0x24);
        if speed == 0 {
            return;
        }
        hw.wait(if self.mix_counter[k] == 0 { 1985 } else { 31 });
        let bus = &mut *hw.bus;
        if self.mix_counter[k] == 0 {
            self.mix_counter[k] = speed;
            let wave = |n: u32| entry(self.waveforms, n as u16, 5);
            let src1 = wave(bus.r32(s));
            let src2 = wave(bus.r32(s + 8));
            self.mix_position[k] = (self.mix_position[k] + 1) & 0x1f;
            let dest = wave(bus.r32(s + 0x10));
            let mut p = self.mix_position[k];
            for i in (0..32).rev() {
                let a = bus.r8(src1 + i).wrapping_add(0x80) as u16;
                let b = bus.r8(src2 + p).wrapping_add(0x80) as u16;
                self.module.w8(bus, dest + i, ((a + b) >> 1) as u8 ^ 0x80);
                p = p.wrapping_sub(1) & 0x1f;
            }
        }
        self.mix_counter[k] -= 1;
    }
}

include!("sidmon1_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// Where the module's parts are, from its start: one position of one track
    /// with one note on instrument 1, a waveform with a list that holds it.
    const SEQ: usize = 0x4e;
    const INST: usize = SEQ + 12;
    const WAVES: usize = INST + 32;
    const SONG: usize = WAVES + 32;
    const LISTS: usize = SONG + 0x2c;
    const TRACKS: usize = LISTS + 16;
    const OFFSETS: usize = TRACKS + 6;
    const SAMPLES: usize = OFFSETS + 4;

    fn module() -> Vec<u8> {
        let mut d = Vec::new();
        // the four voices' sequences, then the other parts, from the ID
        for at in [SEQ, SEQ, SEQ, SEQ, INST, WAVES, SONG, LISTS, TRACKS, OFFSETS, SAMPLES] {
            d.extend((at as u32 - 0x2c).to_be_bytes());
        }
        d.extend(super::ID);
        d.extend([0; 12]); // track 0, no transpose
        let mut inst = [0u8; 32];
        inst[3] = 1; // waveform 1,
        inst[0x14] = 64; // attack by 64
        inst[0x15] = 255; // to 255
        d.extend(inst);
        d.extend([0x40; 32]);
        // 64 rows, 2 positions, speed 3, no mixing
        for v in [0u32, 0, 0, 0, 0, 0, 64, 2, 3, 0, 0] {
            d.extend(v.to_be_bytes());
        }
        d.extend([1, 0, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // waveform 1, looped
        d.extend([12, 1, 0, 0, 63, 0]); // note 12 instrument 1, for 64 rows
        d.extend([0; 8]); // track 0 at 0; no samples
        assert_eq!(d.len(), SAMPLES + 4);
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.sd1"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "SidMon 1.0");
        let base = e.m.deli.list.first().unwrap().0;
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound: DMA off, every voice at 16 words of silence, then DMA on
        assert_eq!(&reg(0xDFF096)[..2], &[0x000F, 0x800F]);
        assert_eq!(&reg(0xDFF0D4)[..1], &[16]);
        // the note: the waveform, its period, the attack's levels / 4, 16 words
        let loc = (base + WAVES as u32) as u16;
        assert!(reg(0xDFF0A2).contains(&loc));
        assert!(reg(0xDFF0A6).contains(&super::PERIODS[12]));
        assert_eq!(&reg(0xDFF0A8)[..5], &[0, 16, 32, 48, 63]);
        assert_eq!(&reg(0xDFF0A4)[..2], &[16, 16]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

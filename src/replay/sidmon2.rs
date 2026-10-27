//! SidMon 2.0, ported from NostalgicPlayer's SidMon 2.0 player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Where the original player's register writes differ from what NostalgicPlayer
//! asks of its mixer, this follows the original (checked against the Sidmon-II
//! Eagleplayer V2.3, "adapted by DEFECT"). The player works on the module in
//! memory as the original does: it writes the sample addresses into the module's
//! sample list, keeps the sample negation state there (its index counts down),
//! and effect 5 changes the instrument; out-of-range numbers read whatever
//! follows a table, and nothing is range-checked or clamped as NostalgicPlayer
//! does. Every row tick writes both DMACON masks (even 0) and every voice's loop
//! address and length; a new waveform plays from its sample's start with its
//! whole length; a length of 0 or 1 word is written as 0, as the Eagleplayer's
//! poke does; effect C writes its volume unclamped and effect 6 takes 0 (256
//! rows).

use super::{Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "SidMon 2.0", detect, new };

const ID: &[u8; 28] = b"SIDMON II - THE MIDI VERSION";

/// Envelope states, counting down as the original does.
const ATTACK: u16 = 4;
const DECAY: u16 = 3;
const SUSTAIN: u16 = 2;
const RELEASE: u16 = 1;

/// One voice, with the original's field sizes.
#[derive(Default)]
struct Voice {
    /// This voice's offset in the position, transpose tables.
    seq: u32,
    sample_addr: u32,
    sample_len: u16,
    period: u16,
    volume: u16,
    envelope: u16,
    sustain: u16,
    /// Guest address of the instrument (32 bytes in the module).
    instrument: u32,
    loop_addr: u32,
    loop_len: u16,
    original_note: u16,
    wave_delay: u8,
    wave_offset: u8,
    arp_delay: u8,
    arp_offset: u8,
    vib_delay: u8,
    vib_offset: u8,
    note: u16,
    instr: u16,
    effect: u16,
    arg: u16,
    bend_counter: u8,
    instrument_transpose: u8,
    pitch_bend: u16,
    slide_note: u16,
    slide_speed: u16,
    /// Guest address of the next track byte.
    track: u32,
    empty_rows: u8,
    note_transpose: u8,
    sample: u16,
}

pub struct SidMon2 {
    /// Module bounds in guest memory (the player writes only inside them).
    base: u32,
    size: u32,
    positions: u32,
    note_transposes: u32,
    instrument_transposes: u32,
    instruments: u32,
    waveforms: u32,
    arpeggios: u32,
    vibratoes: u32,
    sample_info: u32,
    track_table: u32,
    tracks: u32,
    /// The last position (the stored count is one less than the positions).
    last_position: u8,
    speed: u8,
    position: u8,
    row: u8,
    rast: u8,
    pattern_length: u8,
    rast2: u8,
    /// The 4-byte table effect 0 builds its arpeggio in.
    arp_table: [u8; 4],
    /// The sample list entries the last negation went through.
    negated: [u32; 4],
    voices: [Voice; 4],
    songend: bool,
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 86 && &d[58..86] == ID
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    // the block lengths at 6 follow each other from the ID at 58
    let mut t = [0u32; 12];
    t[0] = base + 58;
    for i in 0..11 {
        t[i + 1] = t[i].wrapping_add(super::be32(d, 6 + 4 * i));
    }
    let mut p = SidMon2 {
        base,
        size: d.len() as u32,
        positions: t[2],
        note_transposes: t[3],
        instrument_transposes: t[4],
        instruments: t[5],
        waveforms: t[6],
        arpeggios: t[7],
        vibratoes: t[8],
        sample_info: t[9],
        track_table: t[10],
        tracks: t[11],
        last_position: d[2],
        speed: d[3],
        position: 0,
        row: 0,
        rast: 0,
        pattern_length: 64,
        rast2: 0,
        arp_table: [0; 4],
        negated: [0; 4],
        voices: Default::default(),
        songend: false,
    };
    let bus = &mut m.bus;
    let per_voice = d[2] as u32 + 1;
    for i in 0..4 {
        p.voices[i].seq = i as u32 * per_voice;
        p.find_note(i, bus);
    }

    // The samples follow the tracks: the original finds them by reading 64 rows
    // of the last track with voice 0, which keeps that last row's note and effect.
    let mut a = p.tracks.wrapping_add(bus.r16(p.tracks.wrapping_sub(2)) as u32);
    for _ in 0..64 {
        a = read_row(&mut p.voices[0], bus, a);
    }
    p.voices[0].empty_rows = 0;
    a = (a + 1) & !1;
    let samples = ((super::be16(d, 4) >> 6).wrapping_sub(1) as u32) + 1;
    for s in 0..samples {
        let info = p.sample_info.wrapping_add(64 * s);
        p.w32(bus, info, a);
        a = a.wrapping_add(2 * bus.r16(info + 4) as u32);
    }

    for i in 0..4 {
        let v = &mut p.voices[i];
        v.envelope = ATTACK;
        v.instrument = p.instruments.wrapping_add(32 * v.instrument_transpose as u32);
        p.instrument_sample(i, bus);
    }
    Some(Box::new(p))
}

/// Reads one row of a track at `a` into the voice; returns the next address.
fn read_row(v: &mut Voice, bus: &Bus, mut a: u32) -> u32 {
    v.note = 0;
    v.instr = 0;
    v.effect = 0;
    v.arg = 0;
    if v.empty_rows != 0 {
        v.empty_rows -= 1;
        return a;
    }
    let mut next = || {
        let b = bus.r8(a);
        a = a.wrapping_add(1);
        b
    };
    // a negative byte counts empty rows; 0 and $70 and up start an effect
    let b = next();
    if b == 0 {
        v.effect = next() as u16;
        v.arg = next() as u16;
        return a;
    }
    let mut b = b;
    for field in 0..3 {
        if b & 0x80 != 0 {
            v.empty_rows = !b;
            break;
        }
        if field == 2 || b >= 0x70 {
            v.effect = b as u16;
            v.arg = next() as u16;
            break;
        }
        if field == 0 {
            v.note = b as u16;
        } else {
            v.instr = b as u16;
        }
        b = next();
    }
    a
}

/// The original reads its period table with any index. Right after it lie the
/// four sample list entries its last negation went through, as addresses
/// (Flipper - Title plays note 80, the low word of voice 3's); past those, and
/// before the table, this plays period 0.
fn period(negated: &[u32; 4], index: u16) -> u16 {
    let i = index as i16 as usize;
    match PERIODS.get(i) {
        Some(&p) => p,
        None => match i.checked_sub(PERIODS.len()).filter(|&k| k < 8) {
            Some(k) => (negated[k / 2] >> if k % 2 == 0 { 16 } else { 0 }) as u16,
            None => 0,
        },
    }
}

/// The Eagleplayer's length poke writes a length of 0 or 1 word as 0.
fn poke_len(len: u16) -> u16 {
    if len <= 1 {
        0
    } else {
        len
    }
}

impl Replayer for SidMon2 {
    fn format(&self) -> &'static str {
        "SidMon 2.0"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.rast2 += 1;
        if self.rast2 == 3 {
            self.rast2 = 0;
        }
        self.rast = self.rast.wrapping_add(1);
        if self.rast as i8 >= self.speed as i8 {
            self.rast = 0;
            self.rast2 = 0;
            let mut mask = 0u16;
            for i in 0..4 {
                let v = &mut self.voices[i];
                v.track = read_row(v, hw.bus, v.track);
                if v.note != 0 {
                    mask = mask.wrapping_add(1 << i);
                    v.note = (v.note & 0xff00) | (v.note as u8).wrapping_add(v.note_transpose) as u16;
                }
            }
            hw.dmacon(mask & 0xf);
            // Paula stops a playing voice only at the end of its word, and restarts
            // it only once stopped, so the time the original takes from DMA off to
            // on decides which notes retrigger. The waits are measured on its
            // write logs (to within some 40 colour clocks: the negation varies).
            hw.wait(7);
            for i in 0..4 {
                self.play_voice(i, hw);
            }
            self.negate(hw.bus);
            hw.wait(640);
            hw.dmacon(0x8000 | (mask & 0xf));
            // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does: to
            // 10 lines past the current one
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
            for (i, v) in self.voices.iter().enumerate() {
                hw.loc(i, v.loop_addr);
                hw.wait(107);
                hw.len(i, poke_len(v.loop_len));
                hw.wait(106);
            }
            self.row = self.row.wrapping_add(1);
            if self.row == self.pattern_length {
                self.row = 0;
                if self.position == self.last_position {
                    self.position = 0xff;
                    self.songend = true;
                }
                self.position = self.position.wrapping_add(1);
                for i in 0..4 {
                    self.find_note(i, hw.bus);
                }
            }
        }
        for i in 0..4 {
            self.effects(i, hw);
        }
        if self.rast != 0 {
            self.negate(hw.bus);
        }
    }
}

impl SidMon2 {
    fn in_module(&self, a: u32) -> bool {
        a.wrapping_sub(self.base) < self.size
    }

    /// The player's writes to its module; anything outside it is left alone.
    fn w8(&self, bus: &mut Bus, a: u32, v: u8) {
        if self.in_module(a) {
            bus.w8(a, v);
        }
    }

    fn w16(&self, bus: &mut Bus, a: u32, v: u16) {
        self.w8(bus, a, (v >> 8) as u8);
        self.w8(bus, a.wrapping_add(1), v as u8);
    }

    fn w32(&self, bus: &mut Bus, a: u32, v: u32) {
        self.w16(bus, a, (v >> 16) as u16);
        self.w16(bus, a.wrapping_add(2), v as u16);
    }

    /// Starts the voice's track at the current position.
    fn find_note(&mut self, i: usize, bus: &Bus) {
        let v = &mut self.voices[i];
        let n = v.seq + self.position as u32;
        let track = bus.r8(self.positions.wrapping_add(n)) as u32;
        v.track = self.tracks.wrapping_add(bus.r16(self.track_table.wrapping_add(2 * track)) as u32);
        v.note_transpose = bus.r8(self.note_transposes.wrapping_add(n));
        v.instrument_transpose = bus.r8(self.instrument_transposes.wrapping_add(n));
        v.empty_rows = 0;
    }

    /// Takes the sample of the instrument's first waveform.
    fn instrument_sample(&mut self, i: usize, bus: &Bus) {
        let v = &mut self.voices[i];
        let list = bus.r8(v.instrument) as u32;
        let s = bus.r8(self.waveforms.wrapping_add(16 * list));
        v.sample = s as u16;
        let info = self.sample_info.wrapping_add(64 * s as u32);
        v.sample_addr = bus.r32(info);
        v.sample_len = bus.r16(info + 4);
        v.loop_addr = v.sample_addr.wrapping_add(2 * bus.r16(info + 6) as u32);
        v.loop_len = bus.r16(info + 8);
    }

    fn play_voice(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        v.pitch_bend = 0;
        if v.note == 0 {
            hw.wait(41);
            return;
        }
        v.volume = 0;
        (v.wave_delay, v.wave_offset, v.arp_delay, v.arp_offset, v.vib_delay, v.vib_offset) = (0, 0, 0, 0, 0, 0);
        v.bend_counter = 0;
        v.slide_speed = 0;
        v.envelope = ATTACK;
        v.sustain = 0;
        // a note takes 497 colour clocks, 158 more when it sets the instrument
        hw.wait(if v.instr != 0 { 458 } else { 300 });
        if v.instr != 0 {
            let n = (v.instr as u8).wrapping_sub(1).wrapping_add(v.instrument_transpose);
            v.instrument = self.instruments.wrapping_add(32 * n as u32);
            self.instrument_sample(i, hw.bus);
        }
        let v = &mut self.voices[i];
        let arp = hw.bus.r8(v.instrument + 4) as u32;
        let first = hw.bus.r8(self.arpeggios.wrapping_add(16 * arp)) as i8 as u16;
        v.original_note = v.note.wrapping_add(first);
        v.period = period(&self.negated, v.original_note);
        hw.loc(i, v.sample_addr);
        hw.wait(105);
        hw.len(i, poke_len(v.sample_len));
        hw.wait(89);
        hw.per(i, v.period);
        hw.wait(3);
    }

    /// The sample negation: every voice's sample at most once a call.
    fn negate(&mut self, bus: &mut Bus) {
        let mut infos = [0u32; 4];
        for i in 0..4 {
            let a = self.sample_info.wrapping_add((self.voices[i].sample as i16 as i32 * 64) as u32);
            infos[i] = a;
            if bus.r16(a + 0x1a) != 0 {
                continue;
            }
            self.w16(bus, a + 0x1a, 0xffff);
            let index = bus.r16(a + 0x18);
            if index != 0 {
                // NostalgicPlayer counts the index up; the original counts it down
                self.w16(bus, a + 0x18, index.wrapping_sub(1) & 0x1f);
                continue;
            }
            self.w16(bus, a + 0x18, bus.r16(a + 0xe));
            let status = bus.r16(a + 0x10);
            if status == 0 {
                continue;
            }
            let start = 2 * bus.r16(a + 0xa) as u32;
            let end = (2 * bus.r16(a + 0xc) as u32).wrapping_sub(1);
            let mut pos = bus.r32(a + 0x14);
            let p = bus.r32(a).wrapping_add(start).wrapping_add(pos);
            self.w8(bus, p, !bus.r8(p));
            let speed = bus.r16(a + 0x12) as i16 as i32 as u32;
            pos = pos.wrapping_add(speed);
            let reverse = if (pos as i32) < 0 {
                if status == 2 {
                    pos = end;
                    false
                } else {
                    true
                }
            } else if pos > end {
                if status == 1 {
                    pos = 0;
                    false
                } else {
                    true
                }
            } else {
                false
            };
            if reverse {
                pos = pos.wrapping_sub(speed);
                self.w16(bus, a + 0x12, (speed as u16).wrapping_neg());
            }
            self.w32(bus, a + 0x14, pos);
        }
        for a in infos {
            self.w16(bus, a + 0x1a, 0);
        }
        self.negated = infos;
    }

    fn effects(&mut self, i: usize, hw: &mut Hw) {
        self.envelope(i, hw.bus);
        hw.vol(i, self.voices[i].volume >> 2);
        self.waveform(i, hw);
        self.arpeggio(i, hw.bus);
        self.soundtracker(i, hw);
        self.vibrato(i, hw.bus);
        self.pitch_bend(i, hw.bus);
        self.note_slide(i);
        let v = &mut self.voices[i];
        v.period = v.period.wrapping_add(v.pitch_bend);
        if v.period as i16 <= 95 {
            v.period = 95;
        } else if v.period as i16 >= 5760 {
            v.period = 5760;
        }
        hw.per(i, v.period);
    }

    fn envelope(&mut self, i: usize, bus: &Bus) {
        let v = &mut self.voices[i];
        let a = v.instrument + 0x10;
        let (level, speed) = match v.envelope {
            ATTACK => (bus.r8(a), bus.r8(a + 1)),
            DECAY => (bus.r8(a + 2), bus.r8(a + 3)),
            RELEASE => (bus.r8(a + 5), bus.r8(a + 6)),
            SUSTAIN => {
                if bus.r8(a + 4) as u16 != v.sustain {
                    v.sustain = v.sustain.wrapping_add(1);
                } else {
                    v.envelope -= 1;
                }
                return;
            }
            _ => return,
        };
        let (level, speed) = (level as i16, speed as u16);
        if v.envelope == ATTACK {
            v.volume = v.volume.wrapping_add(speed);
            if level > v.volume as i16 {
                return;
            }
        } else if speed != 0 {
            v.volume = v.volume.wrapping_sub(speed);
            if level < v.volume as i16 {
                return;
            }
        } else {
            v.envelope -= 1;
            return;
        }
        v.volume = level as u16;
        v.envelope -= 1;
    }

    fn waveform(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        let inst = v.instrument;
        let len = hw.bus.r8(inst + 1);
        if len == 0 {
            return;
        }
        let delay = hw.bus.r8(inst + 3);
        if delay != v.wave_delay {
            v.wave_delay = v.wave_delay.wrapping_add(1);
            return;
        }
        v.wave_delay = delay.wrapping_sub(hw.bus.r8(inst + 2));
        if len == v.wave_offset {
            v.wave_offset = 0xff;
        }
        v.wave_offset = v.wave_offset.wrapping_add(1);
        let n = 16 * hw.bus.r8(inst) as u32 + v.wave_offset as u32;
        let s = hw.bus.r8(self.waveforms.wrapping_add(n));
        if s & 0x80 != 0 {
            v.wave_offset = v.wave_offset.wrapping_sub(1);
            return;
        }
        // NostalgicPlayer takes the new sample's loop; the original its whole sample
        v.sample = s as u16;
        let info = self.sample_info.wrapping_add(64 * s as u32);
        v.loop_addr = hw.bus.r32(info);
        v.loop_len = hw.bus.r16(info + 4);
        hw.loc(i, v.loop_addr);
        hw.len(i, poke_len(v.loop_len));
    }

    fn arpeggio(&mut self, i: usize, bus: &Bus) {
        let v = &mut self.voices[i];
        let inst = v.instrument;
        let len = bus.r8(inst + 5);
        if len == 0 {
            return;
        }
        let delay = bus.r8(inst + 7);
        if delay != v.arp_delay {
            v.arp_delay = v.arp_delay.wrapping_add(1);
            return;
        }
        v.arp_delay = delay.wrapping_sub(bus.r8(inst + 6));
        if len == v.arp_offset {
            v.arp_offset = 0xff;
        }
        v.arp_offset = v.arp_offset.wrapping_add(1);
        let n = 16 * bus.r8(inst + 4) as u32 + v.arp_offset as u32;
        let add = bus.r8(self.arpeggios.wrapping_add(n)) as i8 as u16;
        v.period = period(&self.negated, add.wrapping_add(v.original_note));
    }

    fn vibrato(&mut self, i: usize, bus: &Bus) {
        let v = &mut self.voices[i];
        let inst = v.instrument;
        let len = bus.r8(inst + 9);
        if len == 0 {
            return;
        }
        let delay = bus.r8(inst + 0xb);
        if delay != v.vib_delay {
            v.vib_delay = v.vib_delay.wrapping_add(1);
            return;
        }
        v.vib_delay = delay.wrapping_sub(bus.r8(inst + 0xa));
        if len == v.vib_offset {
            v.vib_offset = 0xff;
        }
        v.vib_offset = v.vib_offset.wrapping_add(1);
        let n = 16 * bus.r8(inst + 8) as u32 + v.vib_offset as u32;
        v.period = v.period.wrapping_add(bus.r8(self.vibratoes.wrapping_add(n)) as i8 as u16);
    }

    fn pitch_bend(&mut self, i: usize, bus: &Bus) {
        let v = &mut self.voices[i];
        let speed = bus.r8(v.instrument + 0xc);
        if speed == 0 {
            return;
        }
        if bus.r8(v.instrument + 0xd) != v.bend_counter {
            v.bend_counter = v.bend_counter.wrapping_add(1);
        } else {
            v.pitch_bend = v.pitch_bend.wrapping_add(speed as i8 as u16);
        }
    }

    fn note_slide(&mut self, i: usize) {
        let v = &mut self.voices[i];
        if v.effect != 0 && v.effect < 0x70 && v.arg != 0 {
            v.slide_note = period(&self.negated, v.effect);
            let d = v.slide_note.wrapping_sub(v.period) as i16;
            if d == 0 {
                return;
            }
            v.slide_speed = if d < 0 { v.arg.wrapping_neg() } else { v.arg };
        }
        let speed = v.slide_speed as i16;
        if speed == 0 {
            return;
        }
        v.period = v.period.wrapping_add(speed as u16);
        let (p, target) = (v.period as i16, v.slide_note as i16);
        if (speed < 0 && p <= target) || (speed > 0 && p >= target) {
            v.slide_speed = 0;
            v.period = v.slide_note;
        }
    }

    /// The track effects $70-$7F (0-4 only between rows).
    fn soundtracker(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        if v.effect < 0x70 {
            return;
        }
        let e = v.effect & 0xf;
        if self.rast == 0 && e < 5 {
            return;
        }
        let arg = v.arg;
        match e {
            0 => {
                self.arp_table[0] = (arg >> 4) as u8;
                self.arp_table[2] = (arg & 0xf) as u8;
                let add = self.arp_table[self.rast2 as usize] as u16;
                v.period = period(&self.negated, add.wrapping_add(v.original_note));
            }
            1 => v.pitch_bend = arg.wrapping_neg(),
            2 => v.pitch_bend = arg,
            3 | 4 => {
                if v.envelope != 0 {
                    return;
                }
                if self.rast == 0 && v.instr != 0 {
                    // only the low byte (NostalgicPlayer sets the whole volume)
                    v.volume = (v.volume & 0xff00) | hw.bus.r8(v.instrument + 0x11) as u16;
                }
                let vol = if e == 3 {
                    let vol = v.volume.wrapping_add(4 * arg);
                    if vol as i16 >= 0x100 {
                        0xff
                    } else {
                        vol
                    }
                } else {
                    let vol = v.volume.wrapping_sub(4 * arg);
                    if (vol as i16) < 0 {
                        0
                    } else {
                        vol
                    }
                };
                v.volume = vol;
            }
            5 => {
                let a = v.instrument;
                self.w8(hw.bus, a + 0x10, arg as u8);
                self.w8(hw.bus, a + 0x11, arg as u8);
            }
            // NostalgicPlayer ignores a length of 0; the original plays 256 rows
            6 => self.pattern_length = arg as u8,
            0xc => {
                // the original writes the argument unclamped
                hw.vol(i, arg);
                let vol = 4 * arg;
                v.volume = if vol as i16 >= 0xff { 0xff } else { vol };
            }
            0xf => {
                if arg & 0xf != 0 {
                    self.speed = (arg & 0xf) as u8;
                }
            }
            _ => {}
        }
    }
}

include!("sidmon2_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One position, one 64-row track with one note, one instrument playing a
    /// 16-word looped sample.
    fn module() -> Vec<u8> {
        let mut d = vec![0, 0, 0, 1, 0, 64]; // 1 position, speed 1, 1 sample
        // ID, song length, positions, transposes, instruments, waveforms,
        // arpeggios, vibratoes, sample info, track table, tracks
        for len in [28u32, 0, 4, 4, 4, 32, 16, 16, 16, 64, 2, 4] {
            d.extend(len.to_be_bytes());
        }
        d.resize(58, 0);
        d.extend(super::ID);
        d.extend([0; 12]); // all voices on track 0, no transposes
        let mut inst = [0u8; 32];
        inst[0x10] = 255; // attack to 255,
        inst[0x11] = 64; // by 64
        d.extend(inst);
        d.extend([0; 48]); // waveform list 0: sample 0; no arpeggio, vibrato
        let mut info = [0u8; 64];
        info[5] = 16; // 16 words, loop from 0 for 16 words
        info[9] = 16;
        d.extend(info);
        d.extend([0, 0]);
        d.extend([37, 1, !63, 0]); // note 37 instrument 1, 63 empty rows
        d.extend([0x40; 32]);
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.sd2"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "SidMon 2.0");
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the first tick plays the row: DMA off and on for every voice, the sample
        // and then its loop, at the note's period; the attack writes 64/4 first
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..2], &[0x000F, 0x800F]);
        assert_eq!(&reg(0xDFF0A4)[..2], &[16, 16]);
        assert_eq!(reg(0xDFF0A6)[0], super::PERIODS[37]);
        assert_eq!(&reg(0xDFF0A8)[..4], &[16, 32, 48, 63]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

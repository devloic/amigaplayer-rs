//! Delta Music 2.0, ported from NostalgicPlayer's Delta Music 2.0 player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A Delta Music 2.0 module is the editor's replayer with the song after it, and
//! the original player (the DeltaMusic 2.0 Eagleplayer, V2.2) plays it with the
//! module's own tables and variables, found through the code's pc-relative
//! references. So does this port: it reads the periods, arpeggios, tracks,
//! blocks, instruments and sound tables from the module in guest memory, at the
//! addresses the original computes, and starts from the state the editor saved
//! in the module (speed, noise seed, the voices' envelopes and sound tables).
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * every voice writes period and volume every tick from the start (with the
//!   module's first instrument), and every tick ends with DMACON $800F;
//! * a sample's loop is written on the tick after its note, whatever its length
//!   (none for 0 or 1 words in NostalgicPlayer), and the sample's first word is
//!   cleared in memory then;
//! * a sound table step writes the waveform's location and length (no
//!   retrigger), also a length of 0;
//! * the noise waveform is written big-endian; the sound table delay is the
//!   whole byte (not its low 3 bits), and the sound table position wraps at 64
//!   (not 48), reading on into the next instrument;
//! * a decaying envelope does not stop at its fifth step but reads on; the
//!   vibrato's last step takes its sustain from the instrument's sample flag;
//! * instrument 0 is not the first instrument: its offset is read 64K after
//!   the offset table, as the original's word arithmetic does;
//! * effects 9 to $80 are effects 1 to 8 again;
//! * the writes are spaced as the original's (`Globals::gap`), so that a note's
//!   DMA off has time to stop the voice before the tick's DMA on;
//! * any byte with bit 7 set marks a sample (not only $FF); and indexes past the
//!   period table or the track read on in memory, as the original does.
//!
//! The one read this cannot follow exactly: a period past the table (note,
//! transpose and arpeggio adding up to 85 or more) lands in the original's
//! variables, which hold its running state there and the editor's saved state
//! here. No module of the archive plays one.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Delta Music 2.0", detect, new };

/// The replayer's pc-relative references the original player follows: offset of
/// the instruction in the module and its opcode. The target of each is at
/// offset + 2 + its displacement.
const REFS: [(usize, u16); 16] = [
    (0x216, 0x43fa), // the four tracks' loop and length (copied here)
    (0x2e2, 0x217a),
    (0x2f6, 0x217a),
    (0x30a, 0x217a),
    (0x04c, 0x41fa), // voices 1-4
    (0x064, 0x41fa),
    (0x07c, 0x41fa),
    (0x08e, 0x41fa),
    (0x314, 0x41fa), // the sound effect voices (not played)
    (0x328, 0x41fa),
    (0x33c, 0x41fa),
    (0x350, 0x41fa),
    (0x0aa, 0x49fa), // periods
    (0x022, 0x41fa), // global variables
    (0x106, 0x49fa), // arpeggios
    (0x204, 0x41fa), // the track headers, then the song data
];
const VOICES: usize = 4;
const PERIODS: usize = 12;
const GLOBALS: usize = 13;
const ARPEGGIOS: usize = 14;
const SONG: usize = 15;

fn refs(d: &[u8]) -> Option<[usize; 16]> {
    if d.get(0xbc6..0xbca)? != b".FNL" {
        return None;
    }
    let mut a = [0; 16];
    for (t, &(off, op)) in a.iter_mut().zip(&REFS) {
        if be16(d, off) != op || off + 4 > d.len() {
            return None;
        }
        *t = (off as isize + 2 + be16(d, off + 2) as i16 as isize).try_into().ok()?;
    }
    Some(a)
}

fn detect(d: &[u8]) -> bool {
    refs(d).is_some()
}

/// The module's addresses in guest memory.
#[derive(Clone, Copy)]
struct Song {
    periods: u32,
    arpeggios: u32,
    blocks: u32,
    /// Instrument offsets: instrument n's is at 2 * n - 2.
    instrument_table: u32,
    instruments: u32,
    waveforms: u32,
    /// Sample offsets (8 longs), from `samples`.
    sample_table: u32,
    samples: u32,
}

#[derive(Default)]
struct Voice {
    /// Guest address of the instrument.
    instrument: u32,
    track: u32,
    block: u32,
    /// Byte offsets in the track (2 per entry) and in the block (4 per row).
    track_position: u16,
    block_position: u16,
    track_loop: u16,
    track_length: u16,
    table_delay: u8,
    table_position: u8,
    final_period: u16,
    period: u16,
    note: u8,
    transpose: u8,
    max_volume: u8,
    pitch_bend: u16,
    volume: u16,
    /// Byte offset of the envelope step (3 per step).
    volume_position: u16,
    volume_sustain: u8,
    portamento: u8,
    /// Nonzero while the vibrato goes down.
    vibrato_down: u8,
    vibrato_period: u16,
    vibrato_delay: u8,
    /// Byte offset of the vibrato step (3 per step).
    vibrato_position: u8,
    vibrato_sustain: u8,
    arpeggio: u32,
    arpeggio_position: u16,
    /// A sample started last tick: its loop is written this tick.
    loop_pending: bool,
}

struct Globals {
    noise: u32,
    global_volume: u8,
    speed: u8,
    tick: u8,
    /// DMACON's high byte at the end of a tick (the module's, $80).
    dma: u8,
    songend: bool,
    /// The last register write of this tick, for `gap`.
    last: Option<Write>,
}

#[derive(Clone, Copy, PartialEq)]
enum Write {
    DmaOff,
    Loc,
    Len,
    Per,
    Vol,
    DmaOn,
}

impl Globals {
    /// Lets the time pass that the original takes between its last write and
    /// this one (median colour clocks in its write logs). Paula stops a playing
    /// voice only at the end of its word and restarts it only once stopped, so
    /// the time from a note's DMA off to the tick's DMA on decides whether the
    /// note starts.
    fn gap(&mut self, hw: &mut Hw, next: Write) {
        use Write::*;
        let clocks = match (self.last, next) {
            (None, _) => 0,
            (Some(Vol), Len | DmaOn) => 232,
            (Some(Vol), DmaOff) => 243,
            (Some(Vol), Loc) => 271,
            (Some(Vol), Per) => 500,
            (Some(Len), Loc) => 115,
            (Some(Len), Per) => 410,
            (Some(Loc), Len) => 101,
            (Some(Loc), Loc) => 212,
            (Some(Loc), Per) => 478,
            (Some(DmaOff), Loc) => 426,
            (Some(Per), Vol) => 127,
            _ => 250,
        };
        hw.wait(clocks);
        self.last = Some(next);
    }
}

pub struct DeltaMusic2 {
    song: Song,
    g: Globals,
    voices: [Voice; 4],
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let a = refs(d)?;
    let at = |o: usize| base + o as u32;
    // the song data, as the original's InitSound walks it
    let mut p = a[SONG] + 16;
    let tracks: Vec<usize> = (0..4)
        .map(|i| {
            let t = p;
            p += be16(d, a[SONG] + 4 * i + 2) as usize;
            t
        })
        .collect();
    let blocks = p + 4;
    let instrument_table = blocks + be32(d, p) as usize;
    let instruments = instrument_table + 0x100;
    p = instruments + be16(d, instrument_table + 0xfe) as usize;
    let waveforms = p + 4;
    let sample_table = waveforms + be32(d, p) as usize + 0x40;
    let song = Song {
        periods: at(a[PERIODS]),
        arpeggios: at(a[ARPEGGIOS]),
        blocks: at(blocks),
        instrument_table: at(instrument_table),
        instruments: at(instruments),
        waveforms: at(waveforms),
        sample_table: at(sample_table),
        samples: at(sample_table + 0x20),
    };

    let g = a[GLOBALS];
    let globals = Globals {
        noise: be32(d, g + 0x2a),
        global_volume: d[g + 0x2e],
        speed: d[g + 0x2f],
        tick: 1,
        dma: d[g],
        songend: false,
        last: None,
    };
    let voices = std::array::from_fn(|i| {
        // what the original's InitSound leaves as the editor saved it
        let s = |o: usize| d.get(a[VOICES + i] + o).copied().unwrap_or(0);
        let w = |o: usize| be16(d, a[VOICES + i] + o);
        Voice {
            instrument: song.instruments,
            track: at(tracks[i]),
            block: 0,
            track_position: 0,
            block_position: 0,
            track_loop: be16(d, a[SONG] + 4 * i),
            track_length: be16(d, a[SONG] + 4 * i + 2),
            table_delay: s(0x16),
            table_position: s(0x17),
            final_period: w(0x18),
            period: w(0x1a),
            note: s(0x1c),
            transpose: s(0x2b),
            max_volume: 63,
            pitch_bend: 0,
            volume: 0,
            volume_position: 0,
            volume_sustain: 0,
            portamento: 0,
            vibrato_down: s(0x27),
            vibrato_period: w(0x28),
            vibrato_delay: s(0x2a),
            vibrato_position: s(0x34),
            vibrato_sustain: s(0x35),
            arpeggio: at(a[ARPEGGIOS]),
            arpeggio_position: 0,
            loop_pending: false,
        }
    });
    // InitPlayer runs InitSound's code, then InitSound: each turns the DMA off
    let mut hw = Hw { bus: &mut m.bus };
    hw.dmacon(0x000F);
    hw.dmacon(0x000F);
    Some(Box::new(DeltaMusic2 { song, g: globals, voices }))
}

impl Replayer for DeltaMusic2 {
    fn format(&self) -> &'static str {
        "Delta Music 2.0"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.g.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the noise waveform (waveform 0) is new every tick
        let mut n = self.g.noise;
        for i in 0..16 {
            n = n.rotate_left(7).wrapping_add(0x6eca756d) ^ 0x9e59a92b;
            hw.bus.w32(self.song.waveforms + 4 * i, n);
        }
        self.g.noise = n;

        self.g.last = None;
        self.g.tick = self.g.tick.wrapping_sub(1);
        if (self.g.tick as i8) < 0 {
            self.g.tick = self.g.speed;
        }
        for (i, v) in self.voices.iter_mut().enumerate() {
            v.play(i, hw, &self.song, &mut self.g);
        }
        // every voice played
        self.g.gap(hw, Write::DmaOn);
        hw.dmacon(if self.g.dma & 0x80 != 0 { 0x800F } else { 0x000F });
    }
}

/// The result of `a - b` is negative (the 68000's N flag after a compare).
fn mi(a: u16, b: u16) -> bool {
    (a.wrapping_sub(b) as i16) < 0
}

impl Voice {
    fn play(&mut self, i: usize, hw: &mut Hw, song: &Song, g: &mut Globals) {
        let r8 = |hw: &Hw, a: u32| hw.bus.r8(a);
        let r16 = |hw: &Hw, a: u32| hw.bus.r16(a);
        let mut ins = self.instrument;

        if self.loop_pending {
            self.loop_pending = false;
            let s = Self::sample(hw, song, ins);
            g.gap(hw, Write::Len);
            hw.len(i, r16(hw, ins + 4));
            hw.bus.w16(s, 0);
            g.gap(hw, Write::Loc);
            hw.loc(i, s + r16(hw, ins + 2) as u32);
        }

        if g.tick == 0 {
            if self.block_position == 0 {
                let t = self.track.wrapping_add(self.track_position as i16 as u32);
                self.block = song.blocks + ((r8(hw, t) as u32) << 6);
                self.transpose = r8(hw, t + 1);
                self.track_position = self.track_position.wrapping_add(2);
                if !mi(self.track_position, self.track_length) {
                    self.track_position = self.track_loop;
                    g.songend = true;
                }
            }
            let row = self.block + self.block_position as u32;
            let note = r8(hw, row);
            if note != 0 {
                g.gap(hw, Write::DmaOff);
                hw.dmacon(1 << i);
                self.note = note;
                self.period = r16(hw, song.periods + 2 * note.wrapping_add(self.transpose) as u32);
                // instrument 0 reads its offset 64K on (the word is 2 * n - 2)
                let n = (2 * r8(hw, row + 1) as u32).wrapping_sub(2) & 0xffff;
                ins = song.instruments + r16(hw, song.instrument_table + n) as u32;
                self.instrument = ins;
                self.loop_pending = r8(hw, ins + 0x26) & 0x80 != 0;
                if self.loop_pending {
                    let s = Self::sample(hw, song, ins);
                    g.gap(hw, Write::Loc);
                    hw.loc(i, s);
                    g.gap(hw, Write::Len);
                    hw.len(i, r16(hw, ins));
                }
                self.table_delay = 0;
                self.table_position = 0;
                self.volume = 0;
                self.volume_sustain = 0;
                self.volume_position = 0;
                self.arpeggio_position = 0;
                self.vibrato_down = 0;
                self.vibrato_period = 0;
                self.vibrato_delay = r8(hw, ins + 0x16);
                self.vibrato_position = 0;
                self.vibrato_sustain = r8(hw, ins + 0x17);
            }
            // effects 1 to $80; 9 and on are 1 to 8 again
            let effect = r8(hw, row + 2).wrapping_sub(1);
            if (effect as i8) >= 0 {
                let arg = r8(hw, row + 3);
                match effect & 7 {
                    0 => g.speed = arg & 0x0f,
                    1 => {
                        // the LED filter, as ENPP_PokeCommand sets it
                        if arg == 0 {
                            hw.bus.ciaa.pra |= 2;
                        } else {
                            hw.bus.ciaa.pra &= !2;
                        }
                    }
                    2 => self.pitch_bend = (arg as u16).wrapping_neg(),
                    3 => self.pitch_bend = arg as u16,
                    4 => self.portamento = arg,
                    5 => self.max_volume = arg & 0x3f,
                    6 => g.global_volume = arg & 0x3f,
                    _ => self.arpeggio = song.arpeggios + ((arg & 0x3f) as u32) * 16,
                }
            }
            self.block_position = (self.block_position + 4) & 0x3f;
        }

        // the sound table, for a synth sound
        if r8(hw, ins + 0x26) & 0x80 == 0 {
            if self.table_delay != 0 {
                self.table_delay -= 1;
            } else {
                self.table_delay = r8(hw, ins + 0x27);
                let table = ins + 0x28;
                let mut pos = self.table_position;
                let mut wave = r8(hw, table + pos as u32);
                if wave == 0xff {
                    pos = r8(hw, table + pos as u32 + 1);
                    wave = r8(hw, table + pos as u32);
                }
                if wave != 0xff {
                    let a = song.waveforms + ((wave as u32) << 8);
                    g.gap(hw, Write::Loc);
                    hw.loc(i, a);
                    g.gap(hw, Write::Len);
                    hw.len(i, r16(hw, ins));
                    self.table_position = pos.wrapping_add(1) & 0x3f;
                }
            }
        }

        // vibrato
        let vib = ins + 0x15 + self.vibrato_position as u32;
        let speed = r8(hw, vib) as u16;
        self.vibrato_period = if self.vibrato_down == 0 {
            self.vibrato_period.wrapping_add(speed)
        } else {
            self.vibrato_period.wrapping_sub(speed)
        };
        self.vibrato_delay = self.vibrato_delay.wrapping_sub(1);
        if self.vibrato_delay == 0 {
            self.vibrato_delay = r8(hw, vib + 1);
            self.vibrato_down = !self.vibrato_down;
        }
        if self.vibrato_sustain != 0 {
            self.vibrato_sustain -= 1;
        } else {
            self.vibrato_position = self.vibrato_position.wrapping_add(3);
            if self.vibrato_position == 15 {
                self.vibrato_position = 12;
            }
            // the next step's sustain; after the last step, the sample flag ($26)
            self.vibrato_sustain = r8(hw, vib + 5);
        }

        // the volume envelope
        if self.volume_sustain != 0 {
            self.volume_sustain -= 1;
        } else {
            let env = ins + 6 + self.volume_position as u32;
            let speed = r8(hw, env) as u16;
            let level = r8(hw, env + 1) as u16;
            if !mi(level, self.volume) {
                self.volume = self.volume.wrapping_add(speed);
                if mi(level, self.volume) {
                    self.volume = level;
                    self.volume_position = self.volume_position.wrapping_add(3);
                    if self.volume_position == 15 {
                        self.volume_position = 12;
                    }
                    self.volume_sustain = r8(hw, env + 2);
                }
            } else {
                self.volume = self.volume.wrapping_sub(speed);
                // going down, the envelope does not stop at its last step
                if !mi(level, self.volume) {
                    self.volume = level;
                    self.volume_position = self.volume_position.wrapping_add(3);
                    self.volume_sustain = r8(hw, env + 2);
                }
            }
        }

        // portamento
        let speed = self.portamento as u16;
        if speed != 0 {
            if !mi(self.period, self.final_period) {
                self.final_period = self.final_period.wrapping_add(speed);
                if mi(self.period, self.final_period) {
                    self.final_period = self.period;
                }
            } else {
                self.final_period = self.final_period.wrapping_sub(speed);
                if !mi(self.period, self.final_period) {
                    self.final_period = self.period;
                }
            }
        }

        // arpeggio
        let mut arp = r8(hw, self.arpeggio + self.arpeggio_position as u32);
        if self.arpeggio_position as u8 != 0 && arp == 0x80 {
            self.arpeggio_position = 0;
            arp = r8(hw, self.arpeggio);
        }
        self.arpeggio_position = (self.arpeggio_position + 1) & 0x0f;
        let mut period = self.final_period;
        if self.portamento == 0 {
            let n = arp.wrapping_add(self.note).wrapping_add(self.transpose);
            period = r16(hw, song.periods + 2 * n as u32);
            self.final_period = period;
        }

        let bend = r16(hw, ins + 0x24).wrapping_sub(self.pitch_bend);
        self.vibrato_period = self.vibrato_period.wrapping_sub(bend);
        g.gap(hw, Write::Per);
        hw.per(i, period.wrapping_add(self.vibrato_period));

        let mut volume = ((self.volume >> 2) & 0x3f) as u8;
        if (volume.wrapping_sub(self.max_volume) as i8) >= 0 {
            volume = self.max_volume;
        }
        if (volume.wrapping_sub(g.global_volume) as i8) >= 0 {
            volume = g.global_volume;
        }
        g.gap(hw, Write::Vol);
        hw.vol(i, volume as u16);
    }

    /// Guest address of the instrument's sample (one of 8).
    fn sample(hw: &Hw, song: &Song, ins: u32) -> u32 {
        let n = (hw.bus.r8(ins + 0x27) & 7) as u32;
        song.samples.wrapping_add(hw.bus.r32(song.sample_table + 4 * n))
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// The replayer's references to its tables (at the addresses every module
    /// has them), one track per voice with one block whose first row plays note
    /// 25 with instrument 1: a synth sound playing waveform 1.
    fn module() -> Vec<u8> {
        let targets = [
            0x3ac, 0x3b0, 0x3b4, 0x3b8, 0x878, 0x8b4, 0x904, 0x954, 0x9a4, 0x9f4, 0xa44, 0xa94, 0xae2, 0xb8c, 0xbca,
            0xfca,
        ];
        let mut d = vec![0u8; 0xfca];
        for (&(off, op), &t) in super::REFS.iter().zip(&targets) {
            d[off..off + 2].copy_from_slice(&op.to_be_bytes());
            d[off + 2..off + 4].copy_from_slice(&((t - off - 2) as u16).to_be_bytes());
        }
        d[0xae2 + 50..0xae2 + 52].copy_from_slice(&1712u16.to_be_bytes()); // period of note 25
        d[0xb8c] = 0x80; // DMACON's high byte
        d[0xb8c + 0x2e] = 63; // global volume
        d[0xb8c + 0x2f] = 6; // speed
        d[0xbc6..0xbca].copy_from_slice(b".FNL");
        for _ in 0..4 {
            d.extend([0, 0, 0, 2]); // loop 0, one entry
        }
        d.extend([0; 8]); // each track: block 0, no transpose
        d.extend(64u32.to_be_bytes());
        let mut block = [0u8; 64];
        block[..4].copy_from_slice(&[25, 1, 0, 0]);
        d.extend(block);
        let mut table = [0u8; 256];
        table[0xff] = 88; // instrument 1 at offset 0, 88 bytes of instruments
        d.extend(table);
        let mut ins = [0u8; 88];
        ins[0..6].copy_from_slice(&[0, 16, 0, 0, 0, 16]); // 16 words, looping
        ins[6..9].copy_from_slice(&[64, 255, 0]); // volume up by 64 a tick
        ins[0x28..0x2a].copy_from_slice(&[1, 0xff]); // sound table: waveform 1, then back to 0
        d.extend(ins);
        d.extend(512u32.to_be_bytes());
        d.extend([0; 256]); // waveform 0: noise
        d.extend((0..256).map(|i| if i < 16 { 0x40 } else { 0xc0 })); // waveform 1
        d.extend([0; 64 + 32]); // unknown, sample offsets
        d
    }

    #[test]
    fn plays_a_synth_sound_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.dm2"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Delta Music 2.0");
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // every voice: waveform 1 (16 words) at period 1712, the envelope at 64 / 4
        assert_eq!(reg(0xDFF0A4)[0], 16);
        assert_eq!(reg(0xDFF0A6)[0], 1712);
        assert_eq!(reg(0xDFF0A8)[..2], [16, 32]);
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..7], &[0x000F, 0x000F, 0x0001, 0x0002, 0x0004, 0x0008, 0x800F]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

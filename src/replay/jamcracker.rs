//! JamCracker, ported from NostalgicPlayer's JamCracker player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Where the original player's register writes differ from what NostalgicPlayer
//! asks of its mixer, this follows the original (checked against the JamCracker
//! Eagleplayer, "JamCracker player module V1.33"). Like the original it plays
//! from the module in guest memory: it patches the pattern and sample addresses
//! into the module's tables, and its AM phase effect rewrites the waveform inside
//! the sample. A note's sample pointer and length go to Paula before its DMA goes
//! on, and the loop (or, for a one-shot sample, one word of silence) right after;
//! both DMACON masks are written on every row, even when no note starts.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "JamCracker", detect, new };

/// Instrument flags.
const LOOP: u8 = 1;
const AM: u8 = 2;

/// Pattern row byte 2.
const SPEED_PORTA_TO_NOTE: u8 = 0x40;
const SPEED_SET_VOLUME: u8 = 0x80;

#[derive(Default)]
struct Voice {
    /// Where an AM instrument's waveform lies in its sample (0x80 + 0x40 * voice).
    wave_offset: u16,
    dmacon: u16,
    /// The sample (length in words, guest address) Paula plays next.
    len: u16,
    addr: u32,
    /// Index of the note in PERIODS.
    per_index: usize,
    /// The arpeggio's periods, rotated every tick.
    pers: [u16; 3],
    por: i16,
    delta_por: i16,
    por_level: i16,
    vib: i16,
    delta_vib: i16,
    vol: i16,
    delta_vol: i16,
    vol_level: u16,
    phase: u16,
    delta_phase: i16,
    vib_cnt: u8,
    vib_max: u8,
    flags: u8,
}

pub struct JamCracker {
    /// Guest addresses of the instrument table (40 bytes an instrument), the
    /// pattern table (6 bytes a pattern) and the song table (one word a position).
    inst_table: u32,
    patt_table: u32,
    song_table: u32,
    song_len: u16,
    /// The player's word of silence, played after a sample that does not loop.
    silence: u32,
    /// The next row, the rows left in its pattern, and the song position.
    row: u32,
    note_cnt: u8,
    song_pos: u32,
    positions_left: u16,
    wait: u8,
    wait_cnt: u8,
    tmp_dmacon: u16,
    voices: [Voice; 4],
    songend: bool,
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 6 && d.starts_with(b"BeEp")
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let num_inst = be16(d, 4) as usize;
    let inst_off = 6;
    let patt_count_off = inst_off + 40 * num_inst;
    let num_patt = be16(d, patt_count_off) as usize;
    let patt_off = patt_count_off + 2;
    let song_len_off = patt_off + 6 * num_patt;
    let song_len = be16(d, song_len_off);
    let song_off = song_len_off + 2;
    let data_off = song_off + 2 * song_len as usize;
    if data_off > d.len() || num_patt == 0 || song_len == 0 {
        return None;
    }

    // as the original's InitSound: the patterns follow the song table, then the
    // samples, and their addresses go into the tables (a sample of size 0 gets
    // the address of the next one)
    let mut a = base + data_off as u32;
    for i in 0..num_patt {
        let e = base + (patt_off + 6 * i) as u32;
        m.bus.w32(e + 2, a);
        // adda.w: the pattern's size in bytes is taken as a signed word
        a = a.wrapping_add((be16(d, patt_off + 6 * i) as u32 * 32) as u16 as i16 as u32);
    }
    for i in 0..num_inst {
        let e = base + (inst_off + 40 * i) as u32;
        m.bus.w32(e + 0x24, a);
        a = a.wrapping_add(be32(d, inst_off + 40 * i + 0x20));
    }

    // a one-shot AM instrument makes the phase effect rewrite 64 bytes below and
    // at the silence (the original: its own 4-byte data hunk and what follows it);
    // here they stay in this allocation
    let mem = m.alloc(0x80 + 4);
    if mem == 0 {
        return None;
    }
    let silence = mem + 0x80;

    let mut voices: [Voice; 4] = Default::default();
    for (i, v) in voices.iter_mut().enumerate() {
        v.wave_offset = 0x80 + 0x40 * i as u16;
        v.dmacon = 1 << i;
        v.pers = [PERIODS[0], 0, 0];
        v.vol_level = 0x40;
    }

    let mut p = JamCracker {
        inst_table: base + inst_off as u32,
        patt_table: base + patt_off as u32,
        song_table: base + song_off as u32,
        song_len,
        silence,
        row: 0,
        note_cnt: 0,
        song_pos: 0,
        positions_left: 0,
        wait: 6,
        wait_cnt: 1,
        tmp_dmacon: 0,
        voices,
        songend: false,
    };
    p.song_pos = p.song_table;
    p.positions_left = song_len;
    p.load_pattern(&m.bus);
    Some(Box::new(p))
}

impl Replayer for JamCracker {
    fn format(&self) -> &'static str {
        "JamCracker"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.wait_cnt = self.wait_cnt.wrapping_sub(1);
        if self.wait_cnt == 0 {
            self.new_note(hw);
            self.wait_cnt = self.wait;
        }
        for i in 0..4 {
            self.set_channel(i, hw);
        }
    }
}

/// A note's period. The original's table is followed by another one and then by
/// its variables, which a note past the table would read; no module seen does.
fn period(index: usize) -> u16 {
    PERIODS.get(index).copied().unwrap_or(135)
}

impl JamCracker {
    /// The pattern at the song position: its row count (a byte, so 256 rows are 0)
    /// and first row.
    fn load_pattern(&mut self, bus: &crate::bus::Bus) {
        let e = self.patt_table.wrapping_add(bus.r16(self.song_pos) as u32 * 6);
        self.note_cnt = bus.r8(e + 1);
        self.row = bus.r32(e + 2);
    }

    fn new_note(&mut self, hw: &mut Hw) {
        let row = self.row;
        self.row = self.row.wrapping_add(32);
        self.note_cnt = self.note_cnt.wrapping_sub(1);
        if self.note_cnt == 0 {
            self.song_pos = self.song_pos.wrapping_add(2);
            self.positions_left = self.positions_left.wrapping_sub(1);
            if self.positions_left == 0 {
                self.songend = true;
                self.song_pos = self.song_table;
                self.positions_left = self.song_len;
            }
            self.load_pattern(hw.bus);
        }

        self.tmp_dmacon = 0;
        for i in 0..4 {
            self.nw_note(row + 8 * i as u32, i, hw);
        }
        hw.dmacon(self.tmp_dmacon);
        // Paula stops a playing voice only at the end of its word, and restarts it
        // only once stopped: the time the original takes between DMA off and on
        // (measured: 258 colour clocks, and 450 more for each note) decides which
        // notes retrigger
        hw.wait(258 + 450 * self.tmp_dmacon.count_ones() as u64);
        for i in 0..4 {
            let v = &mut self.voices[i];
            if self.tmp_dmacon & v.dmacon != 0 {
                hw.loc(i, v.addr);
                hw.len(i, v.len);
                hw.per(i, v.pers[0]);
                if v.flags & LOOP == 0 {
                    v.addr = self.silence;
                    v.len = 1;
                }
            }
        }
        hw.dmacon(0x8000 | self.tmp_dmacon);
        // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does: to
        // 10 lines past the current one
        let now = hw.bus.clock;
        hw.wait((now / LINE + 10) * LINE - now);
        // every voice's loop, whether a note started on it or not
        for (i, v) in self.voices.iter().enumerate() {
            hw.loc(i, v.addr);
            hw.len(i, v.len);
        }
    }

    /// One voice's part of the row at `a`.
    fn nw_note(&mut self, a: u32, i: usize, hw: &mut Hw) {
        let bus = &*hw.bus;
        let b: [u8; 8] = std::array::from_fn(|k| bus.r8(a + k as u32));
        let [note, instr, speed, arp, vib, phase, vol, porta] = b;
        let v = &mut self.voices[i];

        if note != 0 {
            let index = note as usize - 1;
            if speed & SPEED_PORTA_TO_NOTE != 0 {
                v.por_level = period(index) as i16;
            } else {
                self.tmp_dmacon |= v.dmacon;
                v.per_index = index;
                v.pers = [period(index); 3];
                v.por = 0;
                // the instrument's record, wherever its number points (as the
                // original, which does not check it against the instrument count)
                let rec = self.inst_table.wrapping_add((instr as i8 as i16 as u16 as u32) * 40);
                let addr = bus.r32(rec + 0x24);
                if addr == 0 {
                    v.addr = self.silence;
                    v.len = 1;
                    v.flags = 0;
                } else {
                    let flags = bus.r8(rec + 0x1f);
                    if flags & AM == 0 {
                        v.addr = addr;
                        v.len = (bus.r32(rec + 0x20) >> 1) as u16;
                    } else {
                        // NostalgicPlayer copies the waveform to a buffer; the
                        // original plays (and phases) it in place
                        v.addr = addr.wrapping_add(v.wave_offset as u32);
                        v.len = 0x20;
                    }
                    v.flags = flags;
                    v.vol = v.vol_level as i16;
                }
            }
        }

        if speed & 15 != 0 {
            self.wait = speed & 15;
        }

        let pi = v.per_index;
        if arp != 0 {
            if arp == 255 {
                v.pers = [period(pi); 3];
            } else {
                v.pers[2] = period(pi + (arp & 15) as usize);
                v.pers[1] = period(pi + (arp >> 4) as usize);
                v.pers[0] = period(pi);
            }
        }

        if vib != 0 {
            if vib == 255 {
                v.vib = 0;
                v.delta_vib = 0;
                v.vib_cnt = 0;
            } else {
                v.vib = 0;
                v.delta_vib = (vib & 15) as i16;
                v.vib_max = vib >> 4;
                v.vib_cnt = vib >> 5;
            }
        }

        if phase != 0 {
            v.phase = 0;
            v.delta_phase = if phase == 255 { -1 } else { (phase & 15) as i16 };
        }

        // the original sets only the low bytes of the volume words
        if vol == 255 {
            v.delta_vol = 0;
        } else if speed & SPEED_SET_VOLUME != 0 {
            v.vol = (v.vol & !0xff) | vol as i16;
            v.vol_level = (v.vol_level & !0xff) | vol as u16;
            v.delta_vol = 0;
        } else if vol != 0 {
            let d = (vol & 0x7f) as i16;
            v.delta_vol = if vol & 0x80 != 0 { -d } else { d };
        }

        if porta != 0 {
            if porta == 255 {
                v.por = 0;
                v.delta_por = 0;
            } else {
                v.por = 0;
                // byte arithmetic, as the original: a slide of $80 or more towards
                // a note is negative (NostalgicPlayer keeps it positive)
                let d = if speed & SPEED_PORTA_TO_NOTE != 0 {
                    if v.por_level > v.pers[0] as i16 {
                        porta
                    } else {
                        porta.wrapping_neg()
                    }
                } else if porta & 0x80 == 0 {
                    v.por_level = 135;
                    porta.wrapping_neg()
                } else {
                    v.por_level = 1019;
                    porta & 0x7f
                };
                v.delta_por = d as i8 as i16;
            }
        }
    }

    fn set_channel(&mut self, i: usize, hw: &mut Hw) {
        let v = &mut self.voices[i];
        // (all three periods are never 0: the first one is a note's)
        for _ in 0..3 {
            if v.pers[0] != 0 {
                break;
            }
            v.pers.rotate_left(1);
        }

        let mut per = (v.pers[0] as i16).wrapping_add(v.por);
        if v.por < 0 {
            per = per.max(v.por_level);
        } else if v.por != 0 {
            per = per.min(v.por_level);
        }
        per = per.wrapping_add(v.vib).clamp(135, 1019);
        hw.per(i, per as u16);
        v.pers.rotate_left(1);

        v.por = v.por.wrapping_add(v.delta_por).clamp(-1019, 1019);

        if v.vib_cnt != 0 {
            v.vib = v.vib.wrapping_add(v.delta_vib);
            v.vib_cnt -= 1;
            if v.vib_cnt == 0 {
                v.delta_vib = v.delta_vib.wrapping_neg();
                v.vib_cnt = v.vib_max;
            }
        }

        // the original's volume routine masks the volume to 7 bits and caps it at 64
        hw.vol(i, ((v.vol as u16) & 0x7f).min(0x40));
        v.vol = v.vol.wrapping_add(v.delta_vol).clamp(0, 64);

        // NostalgicPlayer tests the loop flag here; the original the AM flag
        if v.flags & AM != 0 && v.delta_phase != 0 {
            if v.delta_phase < 0 {
                v.delta_phase = 0;
            }
            // the waveform at the voice's offset in the sample becomes the mean
            // of the sample's start and the start shifted by phase / 4
            let out = v.addr;
            let src = out.wrapping_sub(v.wave_offset as u32);
            let shifted = src.wrapping_add(v.phase as u32 / 4);
            for k in 0..64 {
                let x = hw.bus.r8(src + k) as i8 as i16 + hw.bus.r8(shifted + k) as i8 as i16;
                m68k::AddressBus::write_byte(&mut *hw.bus, out + k, (x >> 1) as u8);
            }
            v.phase = v.phase.wrapping_add(v.delta_phase as u16);
            if v.phase as i16 >= 0x100 {
                v.phase -= 0x100;
            }
        }
    }
}

include!("jamcracker_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// Two instruments (a looped sample and a one-shot one), one pattern of two
    /// rows, one position.
    fn module() -> Vec<u8> {
        let mut d = b"BeEp".to_vec();
        d.extend([0, 2]);
        for (flags, size) in [(1u8, 64u32), (0, 32)] {
            let mut inst = vec![0u8; 31];
            inst.push(flags);
            inst.extend(size.to_be_bytes());
            inst.extend([0; 4]);
            d.extend(inst);
        }
        d.extend([0, 1, 0, 2, 0, 0, 0, 0]); // 1 pattern of 2 rows
        d.extend([0, 1, 0, 0]); // 1 position: pattern 0
        // row 0: voice 0 note 13 with instrument 0 at volume 48, voice 1 note 1 with
        // instrument 1; row 1 empty
        d.extend([13, 0, 0x80, 0, 0, 0, 48, 0, 1, 1, 0, 0, 0, 0, 0, 0]);
        d.extend([0; 16 + 32]);
        d.extend((0..96).map(|i| i as u8));
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.jam"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "JamCracker");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // DMA off and on for the two notes, then off and on with no note
        assert_eq!(&reg(0xDFF096)[..4], &[0x0003, 0x8003, 0x0000, 0x8000]);
        // voice 0: its note's period and length, then the loop again
        assert_eq!(&reg(0xDFF0A4)[..2], &[32, 32]);
        assert_eq!(reg(0xDFF0A6)[..2], [super::PERIODS[12]; 2]);
        assert_eq!(reg(0xDFF0A8)[0], 48);
        // voice 1: the one-shot sample, then one word of silence
        assert_eq!(&reg(0xDFF0B4)[..2], &[16, 1]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

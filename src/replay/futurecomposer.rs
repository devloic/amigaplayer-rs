//! Future Composer 1.3 and 1.4, ported from NostalgicPlayer's Future Composer player
//! and its 1.3 converter (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! NostalgicPlayer plays 1.4 modules, and converts 1.3 ones to 1.4 first. This
//! follows the original players instead (checked against FutureComposer1.3 and
//! FutureComposer1.4, Eagleplayer 2.02), which differ from it and from each
//! other in many details, so the two versions are played by one engine with the
//! differences spelt out. Like the originals, it reads the module where it lies in
//! guest memory, through the pointers the 68k code keeps (sequences, patterns and
//! the instruments' sequences are not bounds checked there either), and the
//! players' own tables lie in guest memory as they lie in their data hunks.
//!
//! What the originals write and NostalgicPlayer does not ask of its mixer:
//! * the period and volume of every voice every tick, and DMACON on (`$8000` and
//!   the voices started) every tick; a voice's loop goes in the tick after it starts
//! * the silent instrument every voice starts with plays volume 1 and note +1 for
//!   one tick
//! * a sample is played as stored (loop start and length unchecked), even an empty
//!   one; an `E9` without a multi-sample still restarts the voice, and one with it
//!   writes the loop start as the period too (overwritten later in the tick)
//! * the periods past the fifth octave: the octave of 3424 doubled, not 1712 again
//! * the speed of the next position is read every fourth position change of any
//!   voice, the first time at the fifth, from that voice's own column
//! * 1.4: a volume bend runs on past 64 (written as 64) and stops at 0 only when it
//!   wraps negative; the portamento of a row is taken from the next row's info byte
//!   whenever its own info byte (or the leftover high byte of d1) is not zero, not
//!   only when bit 7 asks for it, because the Eagleplayer's scope writes between
//!   the bit test and the branch set the flags anew
//! * 1.3: no `$49` pattern end, no pitch or volume bend, portamento every tick
//!   (NostalgicPlayer's converter doubles it instead), periods within 113..1712, and
//!   samples stored without the pad word 1.4 has between them
//!
//! Where the originals read memory that is not theirs, the port cannot follow: a
//! sample number past 1.4's 90 samples reads beyond its data hunk (the port reads
//! an empty entry), and a note past 1.3's 84 periods or a sample number past its
//! table reads sample addresses that 1.3 keeps relative to its own waveforms, so
//! their value depends on where the player was loaded.

use super::{be32, Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;

pub const OPEN13: Open = Open { name: "FutureComposer1.3", detect: detect13, new: new13 };
pub const OPEN14: Open = Open { name: "FutureComposer1.4", detect: detect14, new: new14 };

/// The players' silent instrument: its volume sequence and its frequency sequence.
const SILENT: [u8; 8] = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe1];

/// Layout of the player memory: 1.4 has the silent sequence and then the sample
/// table (16 bytes an entry: address, length, loop start, loop length). 1.3 has the
/// silent sequence, its period table, the sample table (10 bytes an entry, the
/// address relative to the waveforms) and then the waveforms, as in its data hunk;
/// a note past the period table reads the sample table, as the original does.
const FC14_TABLE: u32 = 8;
const FC13_PERIODS_AT: u32 = 8;
const FC13_TABLE: u32 = 0xb0;
const FC13_WAVES_AT: u32 = 0x2ea;
/// Room for every sample number a byte can give (the originals check none).
const FC14_MEM: u32 = FC14_TABLE + 256 * 16;
const FC13_MEM: u32 = FC13_TABLE + 256 * 10;

/// A sequence that loops without a step forward would hang the 68k player; the
/// port gives up on it after this many commands in one tick.
const MAX_COMMANDS: u32 = 256;

/// One voice: the fields of the original's voice structure (offset in the comment).
#[derive(Default)]
struct Voice {
    seq: u32,            // $00 this voice's column of the sequence list
    bend_speed: u8,      // $04 pitch bend (1.4)
    bend_time: u8,       // $05
    seq_pos: u16,        // $06 the next position, in bytes
    note: u8,            // $08
    vol_seq: u32,        // $0a the volume sequence's values
    vol_bend_speed: u8,  // $0e (1.4)
    vol_bend_time: u8,   // $0f
    vol_pos: u16,        // $10
    frq_seq: u32,        // $12
    sound_transpose: u8, // $16
    vol_counter: u8,     // $17
    vol_speed: u8,       // $18
    vol_sustain: u8,     // $19
    frq_sustain: u8,     // $1a
    vib_speed: u8,       // $1b
    vib_depth: u8,       // $1c
    vib_value: u8,       // $1d
    vib_delay: u8,       // $1e
    pattern: u32,        // $22
    vol_bend_flag: bool, // $26
    port_flag: bool,     // $27 (1.4: portamento every other tick)
    pattern_pos: u16,    // $28 in bytes
    bend_flag: bool,     // $2a
    frq_transpose: u8,   // $2b
    transpose: u8,       // $2c
    volume: u8,          // $2d
    vib_flag: u8,        // $2e
    portamento: u8,      // $2f
    frq_pos: u16,        // $32
    seq_end: u32,        // $34
    pitch: u16,          // $38
    loop_start: u16,     // $40
    loop_len: u16,       // $42
    sample: u32,         // $44
    loop_wait: u16,      // $48 ticks until the loop is written
}

pub struct FutureComposer {
    v14: bool,
    /// The player memory (see FC14_TABLE and FC13_TABLE).
    mem: u32,
    patterns: u32,
    frq_seqs: u32,
    vol_seqs: u32,
    voices: [Voice; 4],
    /// DMACON bits of the voices started this tick.
    dma: u16,
    /// Position changes, counted to read the speed every fourth.
    spd_temp: u8,
    speed_counter: u16,
    speed: u16,
    songend: bool,
}

fn detect13(d: &[u8]) -> bool {
    d.len() >= 0x64 && d[..4] == *b"SMOD"
}

fn detect14(d: &[u8]) -> bool {
    d.len() >= 0xb4 && d[..4] == *b"FC14"
}

fn new13(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    new(d, base, m, false)
}

fn new14(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    new(d, base, m, true)
}

fn new(d: &[u8], base: u32, m: &mut Machine, v14: bool) -> Option<Box<dyn Replayer>> {
    let at = |o: usize| base.wrapping_add(be32(d, o));
    // away from the module, which a sequence number past the module's sequences
    // reads beyond: on the original that memory is empty, not the player's tables
    let top = std::mem::replace(&mut m.alloc_top, true);
    let mem = m.alloc(if v14 { FC14_MEM } else { FC13_MEM });
    m.alloc_top = top;
    if mem == 0 {
        return None;
    }
    let b = &mut m.bus;
    b.write_bytes(mem, &SILENT);
    let mut addr = at(0x20);
    if v14 {
        for i in 0..10 {
            let (e, h) = (mem + FC14_TABLE + 16 * i, 0x28 + 6 * i as usize);
            b.w32(e, addr);
            b.write_bytes(e + 4, &d[h..h + 6]);
            addr = addr.wrapping_add(2 * b.r16(e + 4) as u32 + 2);
        }
        let mut wave = at(0x24);
        for i in 0..80 {
            let (e, len) = (mem + FC14_TABLE + 16 * (10 + i), d[0x64 + i as usize] as u16);
            b.w32(e, wave);
            b.w16(e + 4, len);
            b.w16(e + 8, len);
            wave = wave.wrapping_add(2 * len as u32);
        }
    } else {
        for (i, &p) in FC13_PERIODS.iter().enumerate() {
            b.w16(mem + FC13_PERIODS_AT + 2 * i as u32, p);
        }
        let waves = mem + FC13_WAVES_AT;
        for i in 0..10 {
            let (e, h) = (mem + FC13_TABLE + 10 * i, 0x28 + 6 * i as usize);
            b.w32(e, addr.wrapping_sub(waves));
            b.write_bytes(e + 4, &d[h..h + 6]);
            addr = addr.wrapping_add(2 * b.r16(e + 4) as u32);
        }
        let mut off = 0;
        for (i, &len) in FC13_WAVE_LENGTHS.iter().enumerate() {
            let e = mem + FC13_TABLE + 10 * (10 + i as u32);
            b.w32(e, off);
            b.w16(e + 4, len as u16);
            b.w16(e + 8, len as u16);
            off += 2 * len as u32;
        }
        b.write_bytes(waves, &FC13_WAVES);
    }

    let seqs = base + if v14 { 0xb4 } else { 0x64 };
    let num_seqs = (be32(d, 4) / 13) & 0xffff;
    let speed = match b.r8(seqs + 12) {
        0 => 3,
        s => s as u16,
    };
    let patterns = at(0x08);
    let mut voices: [Voice; 4] = Default::default();
    for (i, v) in voices.iter_mut().enumerate() {
        let col = seqs + 3 * i as u32;
        *v = Voice {
            seq: col,
            seq_pos: 13,
            seq_end: col + num_seqs * 13,
            vol_seq: mem,
            frq_seq: mem,
            vol_counter: 1,
            vol_speed: 1,
            pattern: patterns.wrapping_add((b.r8(col) as u32) << 6),
            transpose: b.r8(col + 1),
            sound_transpose: b.r8(col + 2),
            ..Default::default()
        };
    }
    Some(Box::new(FutureComposer {
        v14,
        mem,
        patterns,
        frq_seqs: at(0x10),
        vol_seqs: at(0x18),
        voices,
        dma: 0,
        spd_temp: 0,
        speed_counter: speed,
        speed,
        songend: false,
    }))
}

/// `adda.w`: a word offset is signed.
fn add_w(a: u32, off: u16) -> u32 {
    a.wrapping_add(off as i16 as u32)
}

/// What the volume write leaves of a volume byte (`and #$7f`, at most 64).
fn volume(v: u8) -> u16 {
    (v & 0x7f).min(0x40) as u16
}

impl Replayer for FutureComposer {
    fn format(&self) -> &'static str {
        if self.v14 { "FutureComposer1.4" } else { "FutureComposer1.3" }
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.speed_counter = self.speed_counter.wrapping_sub(1);
        if self.speed_counter == 0 {
            self.speed_counter = self.speed;
            // the interrupt is called with d1 = 0, and each voice leaves it to the next
            let mut d1 = 0;
            for i in 0..4 {
                d1 = self.new_note(i, d1, hw);
            }
        }

        self.dma = 0;
        if self.v14 {
            for i in 0..4 {
                let (per, vol) = self.effect(i, hw);
                hw.per(i, per);
                hw.vol(i, volume(vol));
            }
        } else {
            let pv: Vec<(u16, u8)> = (0..4).map(|i| self.effect(i, hw)).collect();
            for (i, &(per, vol)) in pv.iter().enumerate() {
                hw.per(i, per);
                hw.vol(i, volume(vol));
            }
        }
        hw.dmacon(0x8000 | self.dma);

        // the loop of a sample started in the last tick
        for i in 0..4 {
            let v = &mut self.voices[i];
            if v.loop_wait == 0 {
                continue;
            }
            v.loop_wait -= 1;
            if v.loop_wait != 1 {
                continue;
            }
            v.loop_wait = 0;
            if self.v14 {
                hw.loc(i, add_w(v.sample, v.loop_start));
                hw.len(i, v.loop_len);
            } else {
                // 1.3 writes the first voice's length first
                let a = v.sample.wrapping_add(v.loop_start as u32);
                if i == 0 {
                    hw.len(i, v.loop_len);
                    hw.loc(i, a);
                } else {
                    hw.loc(i, a);
                    hw.len(i, v.loop_len);
                }
            }
        }
    }
}

impl FutureComposer {
    /// The next row of voice `i`. `d1` is the low word of the 68k's d1, which 1.4
    /// tests for the portamento; returns what the voice leaves in it.
    fn new_note(&mut self, i: usize, mut d1: u16, hw: &mut Hw) -> u16 {
        let b: &Bus = hw.bus;
        let v = &mut self.voices[i];
        let mut row = add_w(v.pattern, v.pattern_pos);
        if (self.v14 && b.r8(row) == 0x49) || v.pattern_pos == 0x40 {
            v.pattern_pos = 0;
            let mut pos = add_w(v.seq, v.seq_pos);
            if pos == v.seq_end {
                self.songend = true;
                v.seq_pos = 0;
                pos = v.seq;
            }
            self.spd_temp = self.spd_temp.wrapping_add(1);
            if self.spd_temp == 5 {
                self.spd_temp = 1;
                // the speed byte of the position, from this voice's column
                let speed = b.r8(pos + 12) as u16;
                if speed != 0 {
                    self.speed_counter = speed;
                    self.speed = speed;
                }
            }
            d1 = (b.r8(pos) as u16) << 6;
            v.transpose = b.r8(pos + 1);
            v.sound_transpose = b.r8(pos + 2);
            v.pattern = self.patterns.wrapping_add(d1 as u32);
            v.seq_pos = v.seq_pos.wrapping_add(13);
            row = v.pattern;
        }

        let note = b.r8(row);
        let info = b.r8(row + 1);
        d1 = (d1 & 0xff00) | info as u16;
        let portamento = if note != 0 {
            v.pitch = 0;
            true
        } else {
            d1 &= 0xc0;
            d1 != 0
        };
        if portamento {
            v.portamento = 0;
            let next = if self.v14 { d1 != 0 } else { info & 0x80 != 0 };
            if next {
                v.portamento = b.r8(row + 3);
            }
        }

        let note = note & 0x7f;
        if note != 0 {
            v.note = note;
            let bit = 1 << i;
            self.dma |= bit;
            hw.dmacon(bit);
            let b: &Bus = hw.bus;
            let inst = (info & 0x3f).wrapping_add(v.sound_transpose) as u16;
            let vs = self.vol_seqs.wrapping_add((inst << 6) as u32);
            v.vol_pos = 0;
            v.vol_counter = b.r8(vs);
            v.vol_speed = b.r8(vs);
            let frq = b.r8(vs + 1) as u16;
            v.vib_speed = b.r8(vs + 2);
            v.vib_flag = 0x40;
            v.vib_depth = b.r8(vs + 3);
            v.vib_value = b.r8(vs + 3);
            v.vib_delay = b.r8(vs + 4);
            v.vol_seq = vs + 5;
            d1 = frq << 6;
            v.frq_seq = self.frq_seqs.wrapping_add(d1 as u32);
            v.frq_pos = 0;
            v.vol_sustain = 0;
            v.frq_sustain = 0;
        }
        v.pattern_pos = v.pattern_pos.wrapping_add(2);
        d1
    }

    /// A sample table entry: (address, length, loop start, loop length).
    fn sample(&self, b: &Bus, n: u8) -> (u32, u16, u16, u16) {
        let e = if self.v14 {
            let e = self.mem + FC14_TABLE + 16 * n as u32;
            (b.r32(e), e + 4)
        } else {
            let e = self.mem + FC13_TABLE + 10 * n as u32;
            (b.r32(e).wrapping_add(self.mem + FC13_WAVES_AT), e + 4)
        };
        (e.0, b.r16(e.1), b.r16(e.1 + 2), b.r16(e.1 + 4))
    }

    /// Voice `i`'s frequency and volume sequences. Returns its period and volume.
    fn effect(&mut self, i: usize, hw: &mut Hw) -> (u16, u8) {
        self.frequency_sequence(i, hw);
        self.volume_sequence(i, hw.bus);
        let b: &Bus = hw.bus;
        let v = &mut self.voices[i];

        let mut note = v.frq_transpose;
        if (note as i8) >= 0 {
            note = note.wrapping_add(v.note).wrapping_add(v.transpose);
        }
        let note2 = (note & 0x7f) as u16 * 2;
        let mut period = if self.v14 {
            FC14_PERIODS[note2 as usize / 2]
        } else {
            b.r16(self.mem + FC13_PERIODS_AT + note2 as u32)
        };

        let mut flag = v.vib_flag;
        if v.vib_delay != 0 {
            v.vib_delay -= 1;
        } else {
            let depth = v.vib_depth.wrapping_add(v.vib_depth);
            let mut value = v.vib_value;
            if flag & 0x80 == 0 || flag & 0x01 == 0 {
                if flag & 0x20 != 0 {
                    value = value.wrapping_add(v.vib_speed);
                    if value >= depth {
                        flag &= !0x20;
                        value = depth;
                    }
                } else {
                    let (r, borrow) = value.overflowing_sub(v.vib_speed);
                    value = if borrow {
                        flag |= 0x20;
                        0
                    } else {
                        r
                    };
                }
                v.vib_value = value;
            }
            let mut vib = (value as i16 - (depth >> 1) as i16) as u16;
            // doubled once for every octave below the fifth
            let (mut octave, mut done) = (note2 as u8).overflowing_add(0xa0);
            while !done {
                vib = vib.wrapping_add(vib);
                (octave, done) = octave.overflowing_add(0x18);
            }
            period = period.wrapping_add(vib);
        }
        v.vib_flag = flag ^ 1;

        // 1.4 slides every other tick, 1.3 every tick
        v.port_flag = !v.port_flag;
        if (v.port_flag || !self.v14) && v.portamento != 0 {
            let p = v.portamento as u16;
            let p = if p > 0x1f { (p & 0x1f).wrapping_neg() } else { p };
            v.pitch = v.pitch.wrapping_sub(p);
        }
        if self.v14 {
            v.bend_flag = !v.bend_flag;
            if v.bend_flag && v.bend_time != 0 {
                v.bend_time -= 1;
                v.pitch = v.pitch.wrapping_sub(v.bend_speed as i8 as u16);
            }
        }

        period = period.wrapping_add(v.pitch);
        let max = if self.v14 { 3424 } else { 1712 };
        let period = if period <= 0x70 { 0x71 } else { period.min(max) };
        (period, v.volume)
    }

    fn frequency_sequence(&mut self, i: usize, hw: &mut Hw) {
        let bit = 1u16 << i;
        let mut guard = 0;
        'sustain: loop {
            let v = &mut self.voices[i];
            if v.frq_sustain != 0 {
                v.frq_sustain -= 1;
                return;
            }
            loop {
                guard += 1;
                if guard > MAX_COMMANDS {
                    return;
                }
                let b: &Bus = hw.bus;
                let v = &mut self.voices[i];
                let mut a = add_w(v.frq_seq, v.frq_pos);
                let mut cmd = b.r8(a);
                if cmd == 0xe1 {
                    return;
                }
                if cmd == 0xe0 {
                    v.frq_pos = (b.r8(a + 1) & 0x3f) as u16;
                    a = add_w(v.frq_seq, v.frq_pos);
                    cmd = b.r8(a);
                }
                let arg1 = b.r8(a + 1);
                let arg2 = b.r8(a + 2);
                match cmd {
                    // set waveform
                    0xe2 => {
                        self.dma |= bit;
                        hw.dmacon(bit);
                        self.play_sample(i, arg1, hw);
                        let v = &mut self.voices[i];
                        v.vol_pos = 0;
                        v.vol_counter = 1;
                        v.frq_pos = v.frq_pos.wrapping_add(2);
                    }
                    // set loop
                    0xe4 => {
                        self.play_sample(i, arg1, hw);
                        self.voices[i].frq_pos = self.voices[i].frq_pos.wrapping_add(2);
                    }
                    // multi-sample (1.4)
                    0xe9 if self.v14 => {
                        self.dma |= bit;
                        hw.dmacon(bit);
                        let (s, ..) = self.sample(hw.bus, arg1);
                        if hw.bus.r32(s) == u32::from_be_bytes(*b"SSMP") {
                            let e = s + 4 + 16 * arg2 as u32;
                            let b: &Bus = hw.bus;
                            let addr = (s + 4 + 0x140).wrapping_add(b.r32(e));
                            let (len, start, loop_len) = (b.r16(e + 4), b.r16(e + 6), b.r16(e + 8));
                            hw.loc(i, addr);
                            hw.len(i, len);
                            // the original writes the loop start as the period here
                            hw.per(i, start);
                            let v = &mut self.voices[i];
                            v.sample = addr;
                            v.loop_start = start;
                            v.loop_len = loop_len;
                            v.vol_pos = 0;
                            v.vol_counter = 1;
                            v.loop_wait = 3;
                        }
                        self.voices[i].frq_pos = self.voices[i].frq_pos.wrapping_add(3);
                    }
                    // jump to another sequence
                    0xe7 => {
                        v.frq_seq = self.frq_seqs.wrapping_add((arg1 as u32) << 6);
                        v.frq_pos = 0;
                        continue;
                    }
                    // pitch bend (1.4)
                    0xea if self.v14 => {
                        v.bend_speed = arg1;
                        v.bend_time = arg2;
                        v.frq_pos = v.frq_pos.wrapping_add(3);
                    }
                    // sustain
                    0xe8 => {
                        v.frq_sustain = arg1;
                        v.frq_pos = v.frq_pos.wrapping_add(2);
                        continue 'sustain;
                    }
                    // vibrato
                    0xe3 => {
                        v.frq_pos = v.frq_pos.wrapping_add(3);
                        v.vib_speed = arg1;
                        v.vib_depth = arg2;
                    }
                    _ => {}
                }
                let b: &Bus = hw.bus;
                let v = &mut self.voices[i];
                v.frq_transpose = b.r8(add_w(v.frq_seq, v.frq_pos));
                v.frq_pos = v.frq_pos.wrapping_add(1);
                return;
            }
        }
    }

    /// `E2`/`E4`: sample `n` from its start; its loop follows in the next tick.
    fn play_sample(&mut self, i: usize, n: u8, hw: &mut Hw) {
        let (addr, len, start, loop_len) = self.sample(hw.bus, n);
        hw.loc(i, addr);
        hw.len(i, len);
        let v = &mut self.voices[i];
        v.sample = addr;
        v.loop_start = start;
        v.loop_len = loop_len;
        // 1.3 waits a tick less for an address below $10000 (it tests the high word)
        v.loop_wait = if self.v14 || addr >> 16 != 0 { 3 } else { 2 };
    }

    fn volume_sequence(&mut self, i: usize, b: &Bus) {
        let v14 = self.v14;
        let v = &mut self.voices[i];
        let mut guard = 0;
        'sustain: loop {
            if v.vol_sustain != 0 {
                v.vol_sustain -= 1;
                return;
            }
            if v14 && v.vol_bend_time != 0 {
                v.volume_bend();
                return;
            }
            v.vol_counter = v.vol_counter.wrapping_sub(1);
            if v.vol_counter != 0 {
                return;
            }
            v.vol_counter = v.vol_speed;
            loop {
                guard += 1;
                if guard > MAX_COMMANDS {
                    return;
                }
                let a = add_w(v.vol_seq, v.vol_pos);
                match b.r8(a) {
                    0xe1 => return,
                    0xea if v14 => {
                        v.vol_bend_speed = b.r8(a + 1);
                        v.vol_bend_time = b.r8(a + 2);
                        v.vol_pos = v.vol_pos.wrapping_add(3);
                        v.volume_bend();
                        return;
                    }
                    0xe8 => {
                        v.vol_pos = v.vol_pos.wrapping_add(2);
                        v.vol_sustain = b.r8(a + 1);
                        // 1.3 goes back to the sustain test, which counts it down at once
                        if v14 {
                            return;
                        }
                        continue 'sustain;
                    }
                    0xe0 => v.vol_pos = (b.r8(a + 1) & 0x3f).wrapping_sub(5) as u16,
                    c => {
                        v.volume = c;
                        v.vol_pos = v.vol_pos.wrapping_add(1);
                        return;
                    }
                }
            }
        }
    }
}

impl Voice {
    /// 1.4's volume bend, every other tick; it stops when the volume goes negative.
    fn volume_bend(&mut self) {
        self.vol_bend_flag = !self.vol_bend_flag;
        if self.vol_bend_flag {
            self.vol_bend_time = self.vol_bend_time.wrapping_sub(1);
            self.volume = self.volume.wrapping_add(self.vol_bend_speed);
            if (self.volume as i8) < 0 {
                self.vol_bend_time = 0;
                self.volume = 0;
            }
        }
    }
}

include!("futurecomposer_tables.rs");

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// One position, every voice on a pattern with one note, whose instrument plays
    /// waveform 0 (sample 10) at volume 64.
    fn module(v14: bool) -> Vec<u8> {
        let head = if v14 { 0xb4 } else { 0x64 };
        let (pat, frq, vol, smp) = (head + 14, head + 14 + 64, head + 14 + 128, head + 14 + 192);
        let mut d = vec![0u8; smp + 32];
        d[..4].copy_from_slice(if v14 { b"FC14" } else { b"SMOD" });
        let words = [13, pat, 64, frq, 64, vol, 64, smp, smp];
        for (i, w) in words.iter().enumerate() {
            d[4 + 4 * i..8 + 4 * i].copy_from_slice(&(*w as u32).to_be_bytes());
        }
        if v14 {
            d[0x64] = 16; // waveform 0: 16 words
            for (i, b) in d[smp..].iter_mut().enumerate() {
                *b = (i as u8) << 3;
            }
        }
        d[head + 12] = 3; // speed
        d[pat] = 37; // note 37, instrument 0
        d[frq..frq + 4].copy_from_slice(&[0xe2, 10, 0, 0xe1]); // waveform 0, transpose 0
        d[vol + 5..vol + 7].copy_from_slice(&[0x40, 0xe1]);
        d
    }

    fn play(v14: bool) -> (Vec<(u32, u16, u64)>, Vec<f32>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.fc"), module(v14)).unwrap();
        let f = if v14 { "FutureComposer1.4" } else { "FutureComposer1.3" };
        assert_eq!(e.playing.as_ref().unwrap().format, f);
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        (e.m.bus.write_log.take().unwrap(), out)
    }

    #[test]
    fn plays_a_note_on_paula() {
        for v14 in [false, true] {
            let (log, out) = play(v14);
            let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
            // the silent instrument for two ticks (volume 1, then 0), then the note:
            // every voice's DMA off, then each restarted on waveform 0 (16 words)
            assert_eq!(&reg(0xDFF0A8)[..3], &[1, 0, 64]);
            assert_eq!(reg(0xDFF0A6)[2], super::FC14_PERIODS[37]);
            assert_eq!(reg(0xDFF0A4), [16, 16]); // the sample, then its loop
            let dma = reg(0xDFF096);
            assert_eq!(&dma[..11], &[0x8000, 0x8000, 1, 2, 4, 8, 1, 2, 4, 8, 0x800F]);
            assert!(out.iter().any(|&s| s != 0.0), "no sound");
        }
    }
}

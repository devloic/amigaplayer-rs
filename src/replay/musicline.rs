//! Musicline Editor, ported from NostalgicPlayer's Musicline Editor player
//! (Copyright (c) Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the MusiclineEditor Eagleplayer (the same file as UADE's),
//! which plays both kinds of module: 4-voice tunes on Paula directly, from a
//! CIA timer, and 8-voice tunes ("play mode" set in the tune) mixed two voices
//! to a Paula channel, in software, from the channel-0 audio interrupt. Like the
//! original, the port loads the module into memory blocks of its own, one per
//! chunk, in the original's order and sizes (so its samples land where the
//! original's do, and the data hunk, which the loader puts at the top of memory,
//! differs from the original's address by a constant), and plays from them; the tables of
//! tunes, parts, arpeggios, instruments and samples point at an empty area for
//! the missing ones, as the original's do.
//!
//! Where NostalgicPlayer differs from what the original writes to Paula, this
//! follows the original:
//! * every tick acknowledges the audio interrupt (INTREQ $0080); the first one
//!   after InitSound writes the four volumes (0, or 64 for 8 voices) and does
//!   not read the tune, which InitSound already did once;
//! * a voice that does not restart its note writes its period and volume each
//!   tick; the restarted ones are switched off together (DMACON with the mask
//!   of the voices, written even when it is 0), and after the CIA-B timer B
//!   wait (100, or the longest period those voices played on the last tick)
//!   get volume, location, length and period, then DMA on, and after a second
//!   wait (150) their loop; a voice playing a waveform restarts only when the
//!   waveform changes, but has its location and length written all the same;
//! * muted voices write no volume; the volume is computed with the 68000's
//!   word multiplies, and the master volume is DeliTracker's (64);
//! * a waveform effect's output is copied to the voice's buffer in the data
//!   hunk, which Paula plays; a sample loop effect writes the loop's location
//!   and length directly, and its end the empty sample (length 1);
//! * the tune reads up to 256 sequence entries a row, jumps without
//!   NostalgicPlayer's guard against endless loops, and a part ending in
//!   position 0 stops the player (DMA off) for good; tunes with no channels
//!   are subsongs too, and an empty sequence holds $0010 (part 0) words;
//! * nothing is range-checked (an arpeggio's volume up to 255, lengths as the
//!   module gives them, the sample loader as the original's);
//! * a phase effect with fill steps through the sizer table across its passes
//!   in the high and low types (as one table, row after row) but restarts the row
//!   each pass in the medium type;
//! * the filter effect sets the LED filter through the CIA;
//! * the song never ends (the original reports no end to DeliTracker).
//!
//! 8 voices: the original mixes voice k and voice k+4 into Paula channel k at
//! period $7E, into a 50 Hz buffer whose length follows the tempo (562 bytes
//! at 125 BPM), double-buffered; each tick runs from the channel-0 audio
//! interrupt, mixes into its fast-memory buffer and copies that to the chip one.
//! The port does the same, with the same volume table (computed: a sample byte
//! times the volume / 128) and the same 16.16 stepping.
//!
//! The 112 of 113 8-voice test modules compared have mix buffers byte-identical to
//! UADE's; the other one (corpus line 255) differs only where UADE itself plays a
//! buffer twice (a late interrupt), then one shifted by part of a buffer.
//!
//! The original's handler does not fit a 68000's time: its code on the engine's 68000
//! clock (the Eagleplayer, which counts instruction cycles) takes 1.0 to 5.2 buffers
//! (100 to 360k colour clocks; 129 of 129 8-voice modules; it acknowledges once more
//! when it returns and the next buffer is loaded in the following interrupt) so every
//! buffer plays two to five times and the music runs at a half to a fifth of its
//! speed. UADE runs a 68020 at full speed and plays one buffer per interrupt, as does
//! this port; with DELI_CPU_SPEED=4 (the engine's CPU four times as fast) the
//! reference's register writes are identical to the port's for all of module 243
//! (corpus line 245). refcmp runs the reference that way for such modules.
//!
//! 4 voices, the time of every register write. The reference is the Eagleplayer run by the engine's
//! 68000, whose clock counts the cycles of each instruction (half a colour clock each, whatever the
//! data), so the port counts the same cycles: every branch of the code it follows adds what the
//! original's takes (`cyc`, from the handler's entry; the numbers were measured on the Eagleplayer's runs
//! of 192 four-voice modules, as the cycles between two register writes against the branches the port
//! took between them, and on modules made to change one thing at a time), and a register is written
//! when the clock reaches the entry plus half of that (`reach`). This covers the row (sequence, part
//! lines, instrument start, arpeggio, the effects of a line), each voice's slides, envelope, vibrato,
//! tremolo and loop, the five waveform effects (their set-up by type and each byte of a pass) and the
//! copy of their output, the writes of the voices that do not restart and the DMA routine. The effects'
//! output is copied to the played wave without running Paula up to the copy, as the original's code does
//! under the engine (it touches no register meanwhile). UADE has none of this (a faster CPU: its ticks
//! stay on the timer's period).
//!
//! The interrupts (4 voices). The original's CIA-B has two timers in play: timer A, the tick, which
//! runs on whatever the code does, and timer B (one-shot), the DMA restart's two waits (100 E-clocks, or
//! the longest period the restarted voices played, and then 150). A tune whose effects (256-byte phase,
//! mix and resonance passes) take longer than the timer's period makes the next tick late, and what
//! happens then is the reference engine's way of serving the CIA, which the port follows (`CiaB`,
//! `tick`):
//! * the engine counts the timers only when it looks at the CIA: when the original's code reads or
//!   writes a CIA register (the DMA routine's write of timer B at a tick's end, the read of the interrupt
//!   flags that follows it, timer B's own write in its interrupt, a tempo effect's write of timer A) and
//!   after it has let time pass itself;
//! * a dispatch takes the underflows found so far and calls their handlers one after the other with no
//!   time between, the tick first, then timer B's; an underflow found during those calls waits for the
//!   next dispatch;
//! * the next dispatch comes at the next underflow as the timers stood at the last look (at once when
//!   that is past: an underflow nothing has looked at yet); when an underflow is already pending and
//!   the next one is still to come, it comes where the engine stops next: the end of the output sample,
//!   or at once when a vertical blank (the engine's frame grid, from clock 0) has passed since the
//!   dispatch before began;
//! * the tick's handler returns 29 cycles sooner when its read of the flags does not show timer A (the
//!   tick was called for an underflow that an earlier read had seen), and the first tick, which finds
//!   no timer B flag, signals the player's process: 90 cycles more, and the engine runs the process for
//!   226 colour clocks after that dispatch's handlers;
//! * timer B's interrupt has three steps: the voices' registers and DMA on (and timer B again, 150), then
//!   their loops and the DMA mask cleared, then nothing; the DMA routine at a tick's end (DMA off for the
//!   voices that restart and those still waiting, timer B started) sets the first step again, so an
//!   interrupt that waited for a tick to end runs the first step again, a note starts two ticks late,
//!   the second step never comes and the DMA mask stays set (the voice restarts every tick with no
//!   LOC/LEN writes); a write of timer B while it counts only changes its latch.
//! The engine asks the port for the time of the next dispatch (`Replayer::next_tick_at`).
//!
//! 8 voices (the CPU of the reference eight times as fast, `DELI_CPU_SPEED`, so a colour clock is 16
//! cycles): InitSound's writes come `EIGHT_FIRST_WRITE` after the loading (8 colour clocks later when
//! the tune has a groove), StartInt's acknowledge after the pre-roll row, whose time is counted in
//! 68000 cycles from what the row does (`eight_row_cycles`: measured on the Eagleplayer with rows made
//! for the purpose, one change at a time; it equals the reference's stretch on all 147 8-voice modules
//! compared), the song starts `EIGHT_ACK_TO_START` after StartInt's four volume writes, and each tick's
//! handler acknowledges `EIGHT_CALL_TO_ACK` after the audio interrupt is taken and writes the four mix
//! buffer locations (`EIGHT_LOC_AT`) after that (the first tick also the four volumes,
//! `EIGHT_FIRST_VOL_AT`). The row's exact time matters to what is heard: StartInt's DMA-on write falls
//! on a raster line of the grid that runs before the song start, and when that line's end comes before
//! the start (the write in its last 26 colour clocks) the first buffer starts a line earlier.
//!
//! The time before the first tick (4 voices). The original's loader runs on the engine's 68000
//! clock, which counts floor(cycles / 2) of each stretch of code between two host-library calls, so
//! the loading time is the sum of the stretches of each chunk kind (per channel of a tune, per line
//! and voice bit of a part, per instrument, and per sample: the wave's, the decrunch's records,
//! literals and deltas; a last chunk of an unknown kind makes the closing stretch longer): it equals
//! the original's call times on the 192 four-voice modules (`Cost`). The timer starts a fixed time
//! after the last call plus one stretch of the init that holds the first row's work, counted in
//! cycles as a tick's row is, the voices' set-up (longer with a groove) and EndSound when the tune
//! ends in that row (`four_clocks`); its first tick comes 65536 E-clocks after, from the E-clock
//! edge the start fell in (`start_int`), and the later ones follow its latch. A tune whose first row
//! sets the tempo (effect $42) has the timer's latch loaded before the player starts, so the first
//! tick comes after that period (`preload`) rather than the default one.
//!
//! The tables (`musicline_tables.rs`) come from NostalgicPlayer's Tables.cs; the
//! 68k player was used only to check them, none of its code or data is copied.

use super::{Hw, Open, Replayer};
use crate::bus::FRAME;
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "Musicline Editor", detect, new };

include!("musicline_tables.rs");

// The original's memory: the tables block (AllocMem $2A00).
const T_TUNES: u32 = 0x0000;
const T_PARTS: u32 = 0x0400;
const T_ARPS: u32 = 0x1400;
const T_INSTS: u32 = 0x1800;
const T_SAMPLES: u32 = 0x1c00;
/// An empty sequence: 256 words $0010.
const T_ZERO_CHANNEL: u32 = 0x2000;
/// Zeros, which the missing parts, arpeggios, instruments and samples point at.
const T_ZERO: u32 = 0x2200;
const TAB_SIZE: u32 = 0x2a00;
/// The data hunk: the empty sample (a word), then the voices' wave buffers.
const DATA_SIZE: u32 = 0x404;
/// The mix buffers: 4 channels of $1400 bytes, two halves of $A00.
const MIX_SIZE: u32 = 0x5000;

// A tune.
const TU_TEMPO: u32 = 0x20;
const TU_SPEED: u32 = 0x22;
const TU_GROOVE: u32 = 0x23;
const TU_PLAY_MODE: u32 = 0x26;
const TU_CHANNELS: u32 = 0x27;
const TU_SEQ: u32 = 0x28;

// An instrument (its chunk as read) and a sample's header share these.
const IS_NUMBER: u32 = 0x20;
const IS_TYPE: u32 = 0x21;
const IS_POINTER: u32 = 0x22;
const IS_LENGTH: u32 = 0x26;
const IS_REPEAT: u32 = 0x28;
const IS_REPEAT_LENGTH: u32 = 0x2c;
const IS_FINE_TUNE: u32 = 0x2e;
const IS_SEMI_TONE: u32 = 0x30;
// An instrument's.
const I_START: u32 = 0x32;
const I_REPEAT_START: u32 = 0x36;
const I_VOLUME: u32 = 0x3a;
const I_TRANSPOSABLE: u32 = 0x3c;
const I_SLIDE_SPEED: u32 = 0x3d;
const I_EFFECT1: u32 = 0x3e;
const I_EFFECT2: u32 = 0x3f;
const I_ENVELOPE: u32 = 0x40;
const I_VIBRATO: u32 = 0x58;
const I_TREMOLO: u32 = 0x64;
const I_ARP_TABLE: u32 = 0x70;
const I_ARP_SPEED: u32 = 0x72;
const I_ARP_GROOVE: u32 = 0x73;
const I_ETPF_BITS: u32 = 0x74;
const I_TRANSFORM_WAVES: u32 = 0x75;
const I_TRANSFORM: u32 = 0x7a;
const I_PHASE: u32 = 0x86;
const I_PHASE_TYPE: u32 = 0x92;
const I_MRL_BITS: u32 = 0x94;
const I_MIX_WAVE: u32 = 0x95;
const I_MIX: u32 = 0x96;
const I_RESONANCE: u32 = 0xa2;
const I_BOOST: u32 = 0xae;
const I_RESONANCE_AMP: u32 = 0xaf;
const I_FILTER: u32 = 0xb0;
const I_FILTER_TYPE: u32 = 0xbd;
const I_LOOP: u32 = 0xbe;

// Sample types.
const SAMPLE: u8 = 0;
const WAVE16: u8 = 1;
const WAVE32: u8 = 2;
const WAVE64: u8 = 3;
const WAVE128: u8 = 4;

// Instrument effects 1 and 2.
const E1_ENVELOPE: u8 = 1;
const E1_VIBRATO: u8 = 2;
const E1_TREMOLO: u8 = 4;
const E1_ARPEGGIO: u8 = 8;
const E1_LOOP: u8 = 0x10;
const E1_LOOP_STOP: u8 = 0x20;
const E1_HOLD_SUSTAIN: u8 = 0x40;
const E1_WAVE_SAMPLE_LOOP: u8 = 0x80;
const E2_TRANSFORM: u8 = 1;
const E2_PHASE: u8 = 2;
const E2_MIX: u8 = 4;
const E2_RESONANCE: u8 = 8;
const E2_FILTER: u8 = 0x10;
// Their arguments: InstrumentFlag and MixFlag.
const F_HOLD_SUSTAIN: u8 = 1;
const F_TRANSFORM_INIT: u8 = 2;
const F_TRANSFORM_STEP: u8 = 4;
const F_PHASE_INIT: u8 = 8;
const F_PHASE_STEP: u8 = 0x10;
const F_PHASE_FILL: u8 = 0x20;
const F_FILTER_INIT: u8 = 0x40;
const F_FILTER_STEP: u8 = 0x80;
const M_MIX_INIT: u8 = 1;
const M_MIX_STEP: u8 = 2;
const M_MIX_BUFF: u8 = 4;
const M_MIX_COUNTER: u8 = 8;
const M_RESONANCE_INIT: u8 = 0x10;
const M_RESONANCE_STEP: u8 = 0x20;
const M_LOOP_INIT: u8 = 0x40;
const M_LOOP_STEP: u8 = 0x80;
const B_FILTER: u8 = 1;
const B_RESONANCE: u8 = 2;
const B_MIX: u8 = 4;

// Arpeggio flags.
const A_ENABLED: u8 = 1;
const A_WILL_SET_NOTE: u8 = 2;
const A_USE_TABLE: u8 = 4;
const A_USE_WAVE_SAMPLE: u8 = 8;
const A_ONE_STEP: u8 = 0x10;
const A_NOTE_ABSOLUTE: u8 = 0x20;
// Arpeggio effects.
const AE_SLIDE_UP: u8 = 1;
const AE_SLIDE_DOWN: u8 = 2;
const AE_SET_VOLUME: u8 = 3;
const AE_VOLUME_UP: u8 = 4;
const AE_VOLUME_DOWN: u8 = 5;
const AE_RESTART: u8 = 6;
// Restart flags.
const R_PART_FX: u8 = 1;
const R_ARP: u8 = 2;
const R_ARP_VOLUME: u8 = 4;
// Play flags.
const P_RETRIGGER: u8 = 1;
const P_FIRST_ROW_TICK: u8 = 2;
const P_KEEP_FINE_TUNE: u8 = 4;

// Part effects used as states.
const FX_PORTAMENTO: u8 = 0x03;
const FX_SLIDE_TO_VOLUME: u8 = 0x14;
const FX_VOLUME_UP: u8 = 0x11;
const FX_VOLUME_DOWN: u8 = 0x12;
const FX_CHANNEL_UP: u8 = 0x21;
const FX_CHANNEL_DOWN: u8 = 0x22;
const FX_SLIDE_TO_CHANNEL: u8 = 0x24;
const FX_MASTER_UP: u8 = 0x31;
const FX_MASTER_DOWN: u8 = 0x32;
const FX_SLIDE_TO_MASTER: u8 = 0x34;
const FX_PT_PORTAMENTO: u8 = 0xe3;
const FX_PT_VOLUME_UP: u8 = 0xe6;
const FX_PT_VOLUME_DOWN: u8 = 0xe7;
const FX_PT_TREMOLO: u8 = 0xe8;
const FX_PT_VIBRATO: u8 = 0xea;

// The CIA's E-clock in colour clocks, and the times of the 8-voice handler and of the starts (colour
// clocks the original's code takes, from the Eagleplayer's write logs).
const ECLOCK: u64 = 5;
/// 8 voices: the time the handler lets pass after its register writes.
const PLAY_TIME: u64 = 2669;
/// ... on the first tick (the volume writes instead of the tune's reading).
const FIRST_PLAY_TIME: u64 = 2683;
/// 4 voices: the first tick's handler signals the player's process (the tick whose read of the CIA's
/// flags finds no timer B underflow yet); the engine runs that process for this long right after the
/// handlers of that dispatch.
const FIRST_TICK_PROCESS: u64 = 226;
/// EndSound's writes of a 4-voice tune that ends in its first row, before the timer starts.
const END_BEFORE_START: u64 = 579;
/// 4 voices, the init's stretch that holds the pre-roll row, in 68000 cycles: a tune with a groove takes
/// 16 more for each voice's set-up (all 5 such modules of the corpus), and EndSound, run when the tune
/// ends in that row, 518 (one module; the 8-voice row has the same two numbers).
const GROOVE_INIT: u64 = 64;
const END_SOUND_INIT: u64 = 518;
/// The loader's last stretch is this much longer (colour clocks) when the chunks end with one of an
/// unknown kind and not with the file: 202 cycles (one module of the corpus, and modules with such a
/// chunk appended), which make 13 colour clocks of the 8-voice reference's.
const UNKNOWN_CHUNK: u64 = 101;
const UNKNOWN_CHUNK8: u64 = 13;
/// 8 voices (the reference's CPU running eight times as fast as the 68000's: it counts floor(cycles / 16) of
/// each stretch between two host calls, `Cost::clk8`): the first write of InitSound comes this many
/// colour clocks after the loading (the fixed init stretches: 373,919 cycles from the last host call),
/// and `EIGHT_GROOVE_INIT` later when the tune has a groove (16 cycles more for each of the 8 voices).
const EIGHT_FIRST_WRITE: u64 = 27_617;
const EIGHT_GROOVE_INIT: u64 = 8;
/// 8 voices (the reference's CPU running eight times as fast): InitSound's writes of a channel
/// (location, length, period) come 0, 1 and 2 colour clocks after the first, the next channel's 4
/// after. The last one comes `EIGHT_LAST_WRITE_CYCLE` sixteenths of a colour clock (68000 cycles) into
/// its colour clock; the pre-roll row (`eight_row_cycles`) ends the stretch at a host call, which drops
/// the fraction, and StartInt's acknowledge comes `EIGHT_ROW_TO_ACK` colour clocks later (five more host
/// calls, 92, 160, 14, 132 and 228 cycles apart, and 156 cycles: each floored to colour clocks); the
/// interrupt enable, DMA-on and the four volumes come 1 colour clock apart after it, and the song starts
/// 22 after the last volume write (the writes before it are the engine's time before the start).
const EIGHT_LAST_WRITE_CYCLE: u64 = 3;
const EIGHT_ROW_TO_ACK: u64 = 46;
const EIGHT_ACK_TO_START: u64 = 22;
/// 8 voices: the handler's acknowledge comes this many colour clocks after the interrupt is taken, a
/// tick's four location writes these colour clocks after its acknowledge (each channel's location and
/// length words in one move) and the four volumes of the first tick after the last location.
const EIGHT_CALL_TO_ACK: u64 = 6;
const EIGHT_LOC_AT: [u64; 4] = [9, 11, 12, 14];
const EIGHT_FIRST_VOL_AT: [u64; 4] = [19, 20, 22, 23];


// The original's loading and init, with 4 voices, as stretches of code between host calls (the
// library's A-line calls; the clock counts floor(cycles / 2) of each stretch, see `Cost`).
/// Colour clocks from the player's start to the first chunk's header read (the init before
/// the file's chunks: 24 fixed stretches, the last one with the first chunk's 70 cycles).
const LOAD_START: u64 = 33_335;
/// Colour clocks from the DMA-on write that ends the start (4 voices) to the song start.
const START_LAG: u64 = 172;
/// Cycles of code before each chunk header read (it runs on from the chunk before).
const CHUNK_ENTRY: u64 = 70;
/// From the end-of-file header read to the timer start: the calls that close the loader
/// (fixed, 1083 colour clocks) and the init's fixed stretch (372668 cycles), to which the
/// first row's work is added.
const AFTER_LOAD: u64 = 1_083 + 372_668 / 2;

#[derive(Clone, Copy, Default)]
struct Counter {
    counter: u16,
    speed: i16,
    repeat: u16,
    repeat_end: u16,
    turns: i16,
    delay: u16,
    step: bool,
    save: u16,
}

impl Counter {
    fn one_way(&mut self, cyc: &mut u64) {
        *cyc += 124;
        if self.step || self.delay == 0 {
            self.counter = self.counter.wrapping_add(self.speed as u16) & 0x1ff;
        } else {
            *cyc -= 12;
            self.delay -= 1;
        }
    }

    fn count(&mut self, cyc: &mut u64) {
        *cyc += 29;
        if !(self.step || self.delay == 0) {
            *cyc += 53;
            self.delay -= 1;
            return;
        }
        *cyc += 37;
        if self.turns < 0 {
            *cyc += 21;
            return;
        }
        *cyc += 41;
        let mut c = self.counter as i16;
        c = c.wrapping_add(self.speed);
        if !self.step {
            *cyc += 21;
        }
        let (lo, hi) = if self.repeat < self.repeat_end {
            *cyc += 21;
            (self.repeat, self.repeat_end)
        } else {
            *cyc += 31;
            (self.repeat_end, self.repeat)
        };
        *cyc += 33;
        if self.speed < 0 {
            if c as i32 >= lo as i32 {
                self.counter = c as u16;
                return;
            }
        } else if c as i32 <= hi as i32 {
            self.counter = c as u16;
            return;
        }
        *cyc += 59;
        if self.turns != 0 {
            *cyc += 25;
            self.turns -= 1;
            if self.turns == 0 {
                *cyc += 16;
                self.turns = -1;
            }
        }
        c = c.wrapping_sub(self.speed);
        self.speed = self.speed.wrapping_neg();
        self.counter = c as u16;
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    voice_off: bool,
    muted: bool,
    ended: bool,
    speed: u8,
    groove: u8,
    speed_part: u8,
    groove_part: u8,
    speed_counter: u8,
    arp_speed_counter: u8,
    part_groove: bool,
    arp_groove: bool,
    wave_number: u8,
    wave_number_old: u8,
    instrument: u32,
    wave_sample: u32,
    part_note: u8,
    part_instrument: u8,
    part_effects: [(u8, u8); 5],
    arp: u8,
    arp_position: u8,
    arp_table: u8,
    arp_wait: bool,
    arp_note: u8,
    arp_volume_slide: u8,
    arp_pitch_slide: u8,
    arp_pitch_down: bool,
    arp_calc_note: u16,
    tune_position: u8,
    part_position: u8,
    tune_jump_counter: u8,
    part_jump_counter: u8,
    repeat_original: u32,
    wave_pointer: u32,
    wave_length: u16,
    repeat_pointer: u32,
    repeat_length: u16,
    volume1: u16,
    volume2: u16,
    volume3: u16,
    note: u16,
    period: u16,
    transpose: i16,
    semi_tone: i16,
    fine_tune: i16,
    sample_offset_active: bool,
    sample_offset: u8,
    old_instrument: u8,
    restart: u8,
    volume_add: u8,
    volume_slide: u8,
    channel_volume_slide: u8,
    master_volume_slide: u8,
    volume_set: u16,
    channel_volume: u16,
    volume_add_number: i16,
    channel_volume_add_number: i16,
    master_volume_add_number: i16,
    volume_slide_speed: u16,
    channel_volume_slide_speed: u16,
    master_volume_slide_speed: u16,
    volume_slide_to: u16,
    channel_volume_slide_to: u16,
    master_volume_slide_to: u16,
    volume_slide_down: bool,
    channel_volume_slide_down: bool,
    master_volume_slide_down: bool,
    volume_slide_off: bool,
    channel_volume_slide_off: bool,
    master_volume_slide_off: bool,
    volume: u8,
    instrument_pitch_slide: u8,
    boost: u8,
    transpose_number: u8,
    part_number: u16,
    pitch_slide: u8,
    pitch_slide_down: bool,
    pitch_slide_speed: u16,
    pitch_slide_note: u16,
    pitch_slide_to: i16,
    pitch_add: i16,
    arp_volume_slide_speed: u16,
    arp_pitch_slide_speed: u16,
    arp_pitch_slide_to: i16,
    arp_pitch_slide_note: u16,
    pt_pitch_slide: u8,
    pt_pitch_slide_down: bool,
    pt_pitch_slide_speed: u16,
    pt_pitch_slide_speed2: u16,
    pt_pitch_slide_note: u16,
    pt_pitch_slide_to: i16,
    pt_pitch_add: i16,
    effect1: u8,
    effect2: u8,
    arg1: u8,
    arg2: u8,
    envelope_volume: u16,
    envelope: [u16; 12],
    play: u8,
    wave_or_sample: u8,
    phase_init: u8,
    filter_init: u8,
    transform_init: u8,
    tune_wait: u8,
    vibrato: u8,
    vibrato_down: bool,
    vibrato_wave: u8,
    part_vibrato_wave: u8,
    vibrato_counter: u16,
    vibrato_speed: u16,
    vibrato_depth_now: u16,
    vibrato_delay: u16,
    vibrato_attack_speed: u16,
    vibrato_attack_length: u16,
    vibrato_depth: u16,
    vibrato_note: u16,
    pt_tremolo_position: i8,
    pt_tremolo_command: u8,
    pt_tremolo_wave: u8,
    pt_vibrato_position: i8,
    pt_vibrato_command: u8,
    pt_vibrato_wave: u8,
    pt_vibrato_note: u16,
    tremolo: u8,
    tremolo_down: bool,
    tremolo_wave: u8,
    part_tremolo_wave: u8,
    tremolo_counter: u16,
    tremolo_speed: u16,
    tremolo_depth_now: u16,
    tremolo_delay: u16,
    tremolo_attack_speed: u16,
    tremolo_attack_length: u16,
    tremolo_depth: u16,
    filter_last: i8,
    resonance_last: i8,
    filter_last_init: bool,
    resonance_last_init: bool,
    resonance_amp: u8,
    resonance_init: u8,
    phase_type: u16,
    filter_type: u8,
    transform_counter: Counter,
    transform_speed: u16,
    phase_counter: Counter,
    phase_speed: u16,
    mix_counter: Counter,
    mix_speed: u16,
    resonance_counter: Counter,
    resonance_speed: u16,
    filter_counter: Counter,
    filter_speed: u16,
    mix_wave: u8,
    mix_init: u8,
    transform_waves: [u8; 6],
    loop_init: u8,
    loop_repeat: u16,
    loop_repeat_end: u16,
    loop_length: u16,
    loop_step: i32,
    loop_wait: u16,
    loop_wait_counter: i16,
    loop_delay: i16,
    loop_turns: i16,
    loop_counter: u16,
    loop_counter_save: u16,
    loop_counter_max: u16,
    loop_speed: i16,
    loop_sample: u32,
    // The 8-voice mixer's state.
    mix_table: u32,
    mix_sample: u32,
    mix_pos: u32,
    mix_count: u32,
    mix_frac_acc: u32,
    mix_frac: u16,
    mix_done: bool,
    mix_loop: bool,
    mix_len: u16,
    mix_step: u32,
    /// The voice's effect buffers (transform, phase, mix, resonance, filter).
    bufs: u32,
    /// Its buffer in the data hunk (4 voices).
    wave_buffer: u32,
}

const B_TRANSFORM: u32 = 0;
const B_PHASE: u32 = 0x100;
const B_MIXB: u32 = 0x200;
const B_RES: u32 = 0x300;
const B_FILT: u32 = 0x400;

/// One of CIA-B's two timers, counted in E-clocks as the reference's engine counts them: the counter
/// goes below zero (latch + 1) E-clocks after it was loaded; timer A then reloads and runs on,
/// timer B (one-shot) stops.
#[derive(Clone, Copy)]
struct CiaTimer {
    latch: u16,
    counter: i64,
    running: bool,
    one_shot: bool,
}

impl CiaTimer {
    /// Runs the timer for `ticks` E-clocks; true when it underflowed.
    fn advance(&mut self, ticks: u64) -> bool {
        if !self.running {
            return false;
        }
        self.counter -= ticks as i64;
        if self.counter >= 0 {
            return false;
        }
        if self.one_shot {
            self.running = false;
            self.counter = self.latch as i64;
        } else {
            let period = self.latch as i64 + 1;
            self.counter += ((-self.counter - 1) / period + 1) * period;
        }
        true
    }

    /// A write of the latch's high byte: a stopped one-shot timer is loaded and started, a running
    /// timer only has its latch changed.
    fn write_high(&mut self, latch: u16) {
        self.latch = latch;
        if !self.running {
            self.counter = latch as i64;
            self.running = self.one_shot;
        }
    }
}

/// CIA-B as the reference's engine keeps it under the original's two interrupt handlers (timer A, the
/// tick, and timer B, the DMA restart's waits). The engine counts the timers only when it looks at the
/// CIA (`sync`): when the original's code reads or writes one of its registers, and each time the engine
/// itself has let time pass. An underflow found by a look sets its bit in the interrupt flags (which the
/// tick's handler reads, and so clears, near its end) and in `pending`, which the engine's dispatcher
/// takes: at one dispatch it calls the tick's handler and then timer B's; what comes due meanwhile
/// waits for the next dispatch.
struct CiaB {
    ta: CiaTimer,
    tb: CiaTimer,
    /// The E-clock of the last look.
    e: u64,
    /// The interrupt flags since the handler's last read of them (timer A 1, timer B 2).
    flags: u8,
    /// The underflows no dispatch has taken yet.
    pending: u8,
}

impl Default for CiaB {
    fn default() -> Self {
        let t = CiaTimer { latch: 0xffff, counter: 0xffff, running: false, one_shot: false };
        CiaB { ta: t, tb: CiaTimer { one_shot: true, ..t }, e: 0, flags: 0, pending: 0 }
    }
}

impl CiaB {
    fn sync(&mut self, now: u64) {
        let e = now / ECLOCK;
        if e > self.e {
            let ticks = e - self.e;
            self.e = e;
            let fired = self.ta.advance(ticks) as u8 | (self.tb.advance(ticks) as u8) << 1;
            self.flags |= fired;
            self.pending |= fired;
        }
    }

    /// The colour clock of the next underflow as the timers stood at the last look (it is in the
    /// past when a timer has underflowed since and nothing has looked).
    fn due(&self) -> Option<u64> {
        let until = |t: &CiaTimer| t.running.then(|| t.counter.max(0) as u64 + 1);
        match (until(&self.ta), until(&self.tb)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
        .map(|n| (self.e + n) * ECLOCK)
    }
}

pub struct Musicline {
    tab: u32,
    data: u32,
    mix: [u32; 2],
    tunes: u16,
    subsong: u16,
    tune: u32,
    eight: bool,
    /// 8 voices: the clock of StartInt's acknowledge, set by InitSound (`eight_row_times`), and those of
    /// EndSound's three writes when the tune ends in its pre-roll row.
    eight_ack_at: u64,
    eight_end_at: Option<[u64; 3]>,
    first: bool,
    /// Colour clocks from the start's DMA-on write to the first tick.
    used: std::cell::Cell<u64>,
    master: u16,
    tune_speed: u8,
    tune_groove: u8,
    tempo: u16,
    /// The CIA timer's latch (the engine reloads it at the next tick).
    timer: u16,
    /// The timer has been started once (by `start_int`).
    started: bool,
    /// The latch a tempo effect of the pre-roll row wrote while the timer was stopped: the
    /// write of its high byte loaded the counter, so the timer counts from there.
    preload: Option<u16>,
    mix_length: u16,
    half: u16,
    dma_mask: u16,
    /// CIA-B: timer A, the tick, and timer B, the DMA restart's two waits; and the step timer B's
    /// interrupt takes next (1 after a tick's DMA routine, 2 once the first has run, 3 after the second:
    /// nothing).
    cia: CiaB,
    isr_step: u8,
    /// The tick's handler has signalled the player's process (the first tick does).
    signalled: bool,
    /// The colour clock at which the engine next calls the interrupt handlers that are due.
    grid: Option<u64>,
    prev_period: [u16; 4],
    /// Each channel buffer's "written this tick" flag.
    chan_written: [bool; 4],
    voices: [Voice; 8],
    vol_table: Vec<u8>,
    title: String,
    /// 4 voices: the 68000 cycles the original's code has run since the call's entry (the clock is at
    /// `call_start` + `cyc` / 2 when a register is written), and the clock at that entry.
    cyc: u64,
    call_start: u64,
}

fn detect(d: &[u8]) -> bool {
    d.len() >= 12 && &d[..8] == b"MLEDMODL"
}

/// The Eagleplayer's InitPlayer: the mix buffers, then the module, loaded
/// chunk by chunk as the original's loader does (its AllocMem's in order).
fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let mix_chip = m.alloc(MIX_SIZE);
    let mix_fast = m.alloc(MIX_SIZE);
    let tab = m.alloc(TAB_SIZE);
    // dos.library's Lock and Open, and the read buffer
    let lock = m.alloc(20);
    let fh = m.alloc(44);
    let rbuf = m.alloc(0x1000);
    if mix_chip == 0 || mix_fast == 0 || tab == 0 || rbuf == 0 {
        return None;
    }
    let zero = tab + T_ZERO;
    for k in 0..0x800 {
        m.bus.w32(tab + 4 * k, zero);
    }
    for k in 0..0x80 {
        m.bus.w32(tab + T_ZERO_CHANNEL + 4 * k, 0x0010_0010);
    }
    let mut f = File { d, pos: 0 };
    let mut cost = Cost { base: m.bus.clock, ..Cost::default() };
    let ok = load(&mut f, m, tab, &mut cost);
    for a in [rbuf, fh, lock] {
        m.heap.free(a);
    }
    let tunes = ok?;
    if m.bus.r32(tab + T_TUNES) == zero {
        return None;
    }
    // the voices' buffers: the data hunk and the BSS
    let top = std::mem::replace(&mut m.alloc_top, true);
    let data = m.alloc(DATA_SIZE);
    let bss = m.alloc(8 * 0x500);
    m.alloc_top = top;
    if data == 0 || bss == 0 {
        return None;
    }
    log::debug!("musicline: data hunk at {data:06x}");
    let mut voices = [Voice::default(); 8];
    for (k, v) in voices.iter_mut().enumerate() {
        v.bufs = bss + 0x500 * k as u32;
        v.wave_buffer = if k < 4 { data + 2 + 0x100 * k as u32 } else { 0 };
    }
    let mut vol_table = vec![0u8; 0x10000 + 0x100];
    for v in 0..65 {
        for s in 0..256 {
            vol_table[v * 256 + s] = ((s as u8 as i8 as i32 * v as i32) / 128) as u8;
        }
    }
    // past it in the original: its waveforms and its period table
    let mut o = 65 * 256;
    for w in [&SINE, &DOWN_RAMP, &SAWTOOTH, &SQUARE] {
        for &b in w.iter() {
            vol_table[o] = b as u8;
            o += 1;
        }
    }
    for &p in PITCH[32..].iter() {
        if o + 1 < vol_table.len() {
            vol_table[o] = (p >> 8) as u8;
            vol_table[o + 1] = p as u8;
        }
        o += 2;
    }
    let mut ml = Musicline {
        tab,
        data,
        mix: [mix_fast, mix_chip],
        tunes,
        subsong: 0,
        tune: 0,
        eight: false,
        eight_ack_at: 0,
        eight_end_at: None,
        first: true,
        used: std::cell::Cell::new(0),
        master: 64 << 4,
        tune_speed: 0,
        tune_groove: 0,
        tempo: 125,
        timer: 0,
        started: false,
        preload: None,
        mix_length: 0,
        half: 0,
        dma_mask: 0,
        cia: CiaB::default(),
        isr_step: 1,
        signalled: false,
        grid: None,
        prev_period: [0; 4],
        chan_written: [false; 4],
        voices,
        vol_table,
        cyc: 0,
        call_start: 0,
        title: String::new(),
    };
    let mut hw = Hw { bus: &mut m.bus };
    // the original's loading and init take a time that follows the module: the
    // timer starts then (4 voices), or the first write is made (8 voices, at the
    // engine's start phase after it; InitSound's first writes)
    if hw.bus.r8(hw.bus.r32(tab + T_TUNES) + TU_PLAY_MODE) != 0 {
        let groove = if hw.bus.r8(hw.bus.r32(tab + T_TUNES) + TU_GROOVE) != 0 { EIGHT_GROOVE_INIT } else { 0 };
        hw.wait((cost.base + cost.clk8 + EIGHT_FIRST_WRITE + groove).saturating_sub(hw.bus.clock));
        ml.init_sound(&mut hw, 0, false);
    } else {
        ml.init_sound(&mut hw, 0, true);
        // the pre-roll's row is counted in `cyc`; the same stretch of the init sets up the four
        // voices (longer with a groove) and, when the tune ends in that row, runs EndSound
        let ended = ml.voices[0].ended;
        let mut init = ml.cyc;
        if ml.tune_groove != 0 {
            init += GROOVE_INIT;
        }
        if ended {
            init += END_SOUND_INIT;
        }
        let at = cost.four_clocks(init);
        if ended {
            // EndSound's writes come 579 colour clocks before the timer starts, 10 apart
            hw.wait(at.saturating_sub(END_BEFORE_START + hw.bus.clock));
            ml.end_sound(&mut hw, 10);
        }
        hw.wait(at.saturating_sub(hw.bus.clock));
    }
    ml.start_int(&mut hw);
    Some(Box::new(ml))
}

struct File<'a> {
    d: &'a [u8],
    pos: usize,
}

impl File<'_> {
    /// dos Read: what is left, at most `n` bytes, into guest memory.
    fn read(&mut self, m: &mut Machine, to: u32, n: u32) -> u32 {
        let n = (n as usize).min(self.d.len() - self.pos);
        m.bus.write_bytes(to, &self.d[self.pos..self.pos + n]);
        self.pos += n;
        n as u32
    }
    fn skip(&mut self, n: usize) -> bool {
        if self.pos + n > self.d.len() {
            return false;
        }
        self.pos += n;
        true
    }
    fn get(&self, o: usize) -> u32 {
        super::be32(self.d, o)
    }
}

/// The original's loader: the number of tunes, or None where it fails.
fn load(f: &mut File, m: &mut Machine, tab: u32, cost: &mut Cost) -> Option<u16> {
    let zero = tab + T_ZERO;
    // MLEDMODL, the MODL chunk (or none), VERS
    f.pos = 8;
    if f.get(8) != 0x5645_5253 {
        let n = f.get(8) as usize;
        if !f.skip(4 + n) {
            return None;
        }
        if f.get(f.pos) != 0x5645_5253 {
            return None;
        }
    }
    f.pos += 4;
    let n = f.get(f.pos) as usize;
    if !f.skip(4 + n) {
        return None;
    }
    let (mut tunes, mut samples, mut insts) = (0u16, 0u16, 0u16);
    loop {
        cost.header();
        if f.pos + 8 > f.d.len() {
            break;
        }
        let (id, len) = (f.get(f.pos), f.get(f.pos + 4));
        f.pos += 8;
        match &id.to_be_bytes() {
            b"TUNE" => {
                cost.call(126);
                let t = m.alloc(0x48);
                if t == 0 {
                    return None;
                }
                m.bus.w32(tab + T_TUNES + 4 * (tunes as u32 & 0xff), t);
                if f.read(m, t, 0x28) == 0 {
                    return None;
                }
                let ch = (m.bus.r8(t + TU_CHANNELS) as u32).min(8);
                cost.call(145);
                let (mut k, mut empty, mut first_seq) = (0, 0u64, true);
                if ch == 0 {
                    cost.run(366);
                } else {
                    cost.call(166);
                    if f.read(m, t + TU_SEQ, ch * 4) == 0 {
                        return None;
                    }
                    while k < ch {
                        let size = m.bus.r32(t + TU_SEQ + 4 * k);
                        if size == 0 {
                            m.bus.w32(t + TU_SEQ + 4 * k, tab + T_ZERO_CHANNEL);
                            empty += 1;
                        } else {
                            cost.call(if first_seq { 102 } else { 119 } + 76 * empty);
                            (first_seq, empty) = (false, 0);
                            cost.call(2941);
                            let s = m.alloc(0x200);
                            if s == 0 {
                                return None;
                            }
                            for j in 0..0x80 {
                                m.bus.w32(s + 4 * j, 0x0010_0010);
                            }
                            m.bus.w32(t + TU_SEQ + 4 * k, s);
                            if f.read(m, s, size) == 0 {
                                return None;
                            }
                        }
                        k += 1;
                    }
                    cost.run(if ch == 8 { 95 } else { 115 + 29 * (8 - ch as u64) } + 76 * empty);
                }
                while k < 8 {
                    m.bus.w32(t + TU_SEQ + 4 * k, tab + T_ZERO_CHANNEL);
                    k += 1;
                }
                tunes += 1;
            }
            b"PART" => {
                cost.call(165);
                let p = m.alloc(0x600);
                if p == 0 {
                    return None;
                }
                let start = f.pos;
                if !f.skip(len as usize) || len < 2 {
                    return None;
                }
                let c = &f.d[start..start + len as usize];
                let num = super::be16(c, 0) as u32;
                m.bus.w32(tab + T_PARTS + 4 * (num & 0x3ff), p);
                cost.call(117);
                let (mut i, mut line) = (2usize, p);
                let mut part_cost = 154u64;
                while let Some(&flag) = c.get(i) {
                    i += 1;
                    if flag & 0x80 != 0 {
                        break;
                    }
                    part_cost += 241 + 28 * (flag & 0x3f).count_ones() as u64;
                    let mut fl = flag;
                    for j in 0..6 {
                        if fl & 1 != 0 {
                            let a = c.get(i).copied().unwrap_or(0);
                            let b = c.get(i + 1).copied().unwrap_or(0);
                            i += 2;
                            m.bus.w8(line + 2 * j, a);
                            m.bus.w8(line + 2 * j + 1, b);
                        }
                        fl >>= 1;
                    }
                    line += 12;
                }
                cost.run(part_cost);
            }
            b"ARPG" => {
                cost.call(196);
                cost.call(93);
                cost.call(177);
                cost.run(61);
                let a = m.alloc(0x300);
                if a == 0 {
                    return None;
                }
                if f.read(m, a, 2) == 0 {
                    return None;
                }
                let num = m.bus.r16(a) as u32;
                m.bus.w32(tab + T_ARPS + 4 * (num & 0xff), a);
                if f.read(m, a, len.wrapping_sub(2)) == 0 && len > 2 {
                    return None;
                }
            }
            b"INST" => {
                cost.call(227);
                cost.call(193);
                cost.run(41);
                let a = m.alloc(0xce);
                if a == 0 {
                    return None;
                }
                insts += 1;
                m.bus.w32(tab + T_INSTS + 4 * (insts as u32 & 0xff), a);
                if f.read(m, a, len) == 0 {
                    return None;
                }
            }
            b"SMPL" => {
                cost.call(278);
                // the original length and the delta command; then the header
                if f.pos + 6 > f.d.len() {
                    return None;
                }
                let orig = f.get(f.pos);
                let cmd = f.d[f.pos + 4];
                f.pos += 6;
                let extra = if orig == 0x100 { 0xf0 } else { 0 };
                cost.call(if orig == 0x100 { 164 } else { 148 });
                cost.call(89);
                let s = m.alloc((orig + extra).wrapping_add(0x32));
                if s == 0 {
                    return None;
                }
                if f.read(m, s, 0x32) == 0 {
                    return None;
                }
                let crunched = len.wrapping_sub(0x32);
                if crunched == orig {
                    cost.call(176);
                    if f.read(m, s + 0x32, orig) == 0 && orig != 0 {
                        return None;
                    }
                } else {
                    cost.call(160);
                    cost.call(356);
                    let tmp = m.alloc(crunched);
                    let start = f.pos;
                    let n = (crunched as usize).min(f.d.len() - start);
                    f.pos += n;
                    decrunch(&f.d[start..start + n], cmd, m, s + 0x32, cost);
                    if tmp != 0 {
                        m.heap.free(tmp);
                    }
                }
                // the stretch that follows runs on to the next chunk's header read
                let mut tail = if crunched == orig { 364 } else { 349 } + if orig == 0x100 { 15_176 } else { 0 };
                let data = s + 0x32;
                if orig == 0x100 {
                    for k in 0..0x100 {
                        if m.bus.r8(data + k) == 0x80 {
                            m.bus.w8(data + k, 0x81);
                        }
                    }
                    for k in 0..0xf0 {
                        let b = m.bus.r8(data + 2 * k);
                        m.bus.w8(data + 0x100 + k, b);
                    }
                }
                samples += 1;
                log::debug!("musicline: sample {samples} at {data:06x} size {:x}", orig + extra);
                m.bus.w32(tab + T_SAMPLES + 4 * (samples as u32 & 0xff), s);
                // the instruments of this sample
                for k in 1..256 {
                    let inst = m.bus.r32(tab + T_INSTS + 4 * k);
                    if inst == zero {
                        break;
                    }
                    tail += 58;
                    if m.bus.r8(inst + IS_NUMBER) == samples as u8 {
                        tail += 104;
                        let st = m.bus.r16(inst + I_START) as u32;
                        let rs = m.bus.r16(inst + I_REPEAT_START) as u32;
                        m.bus.w32(inst + IS_POINTER, data + 2 * st);
                        m.bus.w32(inst + IS_REPEAT, data + 2 * rs);
                    }
                }
                cost.run(tail);
                let off = m.bus.r32(s + IS_REPEAT).wrapping_sub(m.bus.r32(s + IS_POINTER)) as i32;
                m.bus.w32(s + IS_POINTER, data);
                m.bus.w32(s + IS_REPEAT, data.wrapping_add(off.max(0) as u32));
            }
            b"INFO" => {
                cost.call(317);
                cost.run(41);
                if !f.skip(len as usize) {
                    break;
                }
            }
            _ => {
                cost.clk += UNKNOWN_CHUNK;
                cost.clk8 += UNKNOWN_CHUNK8;
                break;
            }
        }
    }
    Some(tunes)
}

/// What the original's loading took, counted while the port loads the module.
///
/// The original's loader runs on the engine's 68000 clock, which counts, between two calls into
/// the host library (an A-line call costs nothing and restarts the count), half of the
/// stretch's cycles rounded down: the loading time is the sum of floor(stretch / 2) over the
/// stretches the loader's code runs between its calls. The loader prices below are the
/// stretches measured for each kind of chunk (a chunk's last stretch runs on into the next
/// chunk's header read, with its own 70 cycles), checked against the call times of 183 of 184
/// four-voice modules.
#[derive(Default)]
struct Cost {
    /// The clock at which the loader starts.
    base: u64,
    /// The 4-voice loading time so far, in colour clocks from `base` (the stretches ended by a call).
    clk: u64,
    /// The same with the reference's CPU running eight times as fast (8 voices): floor(cycles / 16).
    clk8: u64,
    /// Cycles run since the last call.
    pending: u64,
    /// A chunk header has been read.
    started: bool,
}

impl Cost {
    /// Code runs on for `cycles`, up to the next call.
    fn run(&mut self, cycles: u64) {
        self.pending += cycles;
    }

    /// Code runs for `cycles` and a host call ends the stretch.
    fn call(&mut self, cycles: u64) {
        self.clk += (self.pending + cycles) / 2;
        self.clk8 += (self.pending + cycles) / 16;
        self.pending = 0;
    }

    /// The loader's reading of a chunk header (or of the end of the file): the first one's
    /// stretch is the init's, and counted in `LOAD_START`.
    fn header(&mut self) {
        if self.started {
            self.call(CHUNK_ENTRY);
        } else {
            self.started = true;
            self.clk = LOAD_START;
        }
    }

    /// Colour clocks from the start to the original's timer start with 4 voices: the
    /// loading (`clk`), the calls that close it and the init's stretch, which is its fixed part
    /// and `row`: the 68000 cycles the tune's routine runs for the first row, as in a tick, and
    /// what the init adds for a groove and for a tune that ends there.
    fn four_clocks(&self, row: u64) -> u64 {
        self.base + self.clk + AFTER_LOAD + (row + 37) / 2
    }
}

/// The delta decruncher: a byte, or the command, a start byte and a 16-bit
/// count of 4-bit deltas.
fn decrunch(c: &[u8], cmd: u8, m: &mut Machine, mut to: u32, cost: &mut Cost) {
    let mut i = 0;
    let mut next = || {
        let b = c.get(i).copied();
        i += 1;
        b
    };
    let ext = |n: u8| if n & 8 != 0 { n | 0xf0 } else { n };
    // the stretch's cycles: the literals, the records (and their deltas, by half-byte and
    // sign) and a fixed part, all between the two calls around the decoding
    let mut cyc = 237u64;
    while let Some(b) = next() {
        if b != cmd {
            cyc += 64;
            m.bus.w8(to, b);
            to += 1;
            continue;
        }
        cyc += 164;
        let (Some(mut s), Some(hi), Some(lo)) = (next(), next(), next()) else { break };
        let mut n = (hi as u16) << 8 | lo as u16;
        // an even, non-empty run costs 10 more
        if n != 0 && n % 2 == 0 {
            cyc += 10;
        }
        m.bus.w8(to, s);
        to += 1;
        while n != 0 {
            let Some(d) = next() else {
                cost.call(cyc);
                return;
            };
            cyc += 83 + 8 * (d >> 7) as u64;
            s = s.wrapping_add(ext(d >> 4));
            m.bus.w8(to, s);
            to += 1;
            n -= 1;
            if n != 0 {
                cyc += 64 + 8 * ((d >> 3) & 1) as u64;
                s = s.wrapping_add(ext(d & 15));
                m.bus.w8(to, s);
                to += 1;
                n -= 1;
            }
        }
    }
    cost.call(cyc);
}

fn rb(hw: &Hw, a: u32) -> u8 {
    hw.bus.r8(a)
}
fn rw(hw: &Hw, a: u32) -> u16 {
    hw.bus.r16(a)
}
fn rl(hw: &Hw, a: u32) -> u32 {
    hw.bus.r32(a)
}
fn rs8(hw: &Hw, a: u32) -> i32 {
    hw.bus.r8(a) as i8 as i32
}

fn period_of(note: i32) -> u16 {
    PITCH[(note.clamp(-32, 5 * 12 * 32) + 32) as usize]
}

impl Replayer for Musicline {
    fn format(&self) -> &'static str {
        "Musicline Editor"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick(&mut self, hw: &mut Hw) {
        // EndSound's register writes do not stop the CIA: the original plays on
        if self.eight {
            return;
        }
        // One dispatch of the reference's engine: it looks at the CIA, takes the underflows found so
        // far and calls their handlers one after the other, timer A's (the tick) first.
        let now = hw.bus.clock;
        self.cia.sync(now);
        let fired = std::mem::take(&mut self.cia.pending);
        if fired & 1 != 0 {
            self.call_start = hw.bus.clock;
            self.cyc = 0;
            self.handler4(hw);
        }
        if fired & 2 != 0 {
            self.timer_b_interrupt(hw);
        }
        // the first tick wakes the player's process, which the engine runs after the handlers
        if std::mem::take(&mut self.signalled) {
            hw.wait(FIRST_TICK_PROCESS);
        }
        // The next dispatch. The engine lets the time pass to the next underflow as the timers stood
        // at the CIA's last look, so one that has come since (unseen) is dispatched at once. When that
        // underflow is still to come but a handler's own look at the CIA has already found another
        // (it is pending), the dispatch comes where the engine stops next: at the end of the output
        // sample, or at once when a vertical blank has passed since this dispatch began.
        let end = hw.bus.clock;
        let due = self.cia.due().unwrap_or(end + (self.timer as u64 + 1) * ECLOCK);
        self.grid = Some(if self.cia.pending != 0 && due > end {
            hw.bus.paula.sync(end, &hw.bus.ram);
            let sample = end + hw.bus.paula.out.until_sample() as u64;
            let frame = hw.bus.frame;
            let vblank = FRAME + if now < FRAME { 0 } else { ((now - FRAME) / frame + 1) * frame };
            sample.min(vblank).min(due)
        } else {
            due
        });
    }

    fn next_tick_at(&self) -> Option<u64> {
        if self.eight {
            None
        } else {
            self.grid
        }
    }

    fn tick_clocks(&self) -> u64 {
        if self.eight {
            // the audio interrupt is the tick
            1 << 40
        } else {
            // the first period is shortened by what the start's tick took
            match self.used.replace(0) {
                0 => (self.timer as u64 + 1) * ECLOCK,
                first => first,
            }
        }
    }

    fn songend(&self) -> bool {
        false
    }

    fn needs_fast_reference_cpu(&self) -> bool {
        self.eight
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.tunes.saturating_sub(1), self.subsong)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.end_sound(hw, 0);
        self.init_sound(hw, n, false);
        self.start_int(hw);
    }

    fn audio_irq(&mut self, hw: &mut Hw) -> bool {
        if self.eight {
            hw.wait(EIGHT_CALL_TO_ACK);
            self.handler(hw);
        } else {
            hw.bus.write_word(0xdff09c, 0x0080);
        }
        true
    }
}

impl Musicline {
    fn sample(&self, hw: &Hw, n: u8) -> u32 {
        rl(hw, self.tab + T_SAMPLES + 4 * n as u32)
    }

    fn voice_count(&self) -> usize {
        if self.eight {
            8
        } else {
            4
        }
    }

    /// DTP_InitSound.
    /// With `defer_end` the caller makes EndSound's writes when a tune ends in its first row.
    fn init_sound(&mut self, hw: &mut Hw, sub: u16, defer_end: bool) {
        self.subsong = sub;
        self.tune = rl(hw, self.tab + T_TUNES + 4 * (sub as u32 & 0xff));
        let t = self.tune;
        self.eight = rb(hw, t + TU_PLAY_MODE) != 0;
        self.title = (0..32).map(|k| rb(hw, t + k)).take_while(|&c| c != 0).map(|c| c as char).collect();
        for v in 0..8 {
            let (bufs, wb) = (self.voices[v].bufs, self.voices[v].wave_buffer);
            self.voices[v] = Voice { bufs, wave_buffer: wb, ..Voice::default() };
        }
        for b in self.mix {
            for k in 0..MIX_SIZE / 4 {
                hw.bus.w32(b + 4 * k, 0);
            }
        }
        self.tempo = rw(hw, t + TU_TEMPO);
        self.tune_speed = rb(hw, t + TU_SPEED);
        self.tune_groove = rb(hw, t + TU_GROOVE);
        for v in 0..self.voice_count() {
            let x = &mut self.voices[v];
            x.channel_volume = 0x400;
            x.speed = self.tune_speed;
            x.groove = self.tune_groove;
            x.part_groove = self.tune_groove != 0;
            x.speed_counter = 1;
            x.pitch_slide_to = -1;
        }
        self.dma_mask = 0;
        self.first = true;
        if self.eight {
            self.half = 0;
            self.mix_length = self.buffer_length();
            let len = self.mix_length >> 1;
            for ch in 0..4 {
                hw.loc(ch, self.mix[1] + 0x1400 * ch as u32);
                hw.wait(1);
                hw.len(ch, len);
                hw.wait(1);
                hw.per(ch, 0x7e);
                hw.wait(2);
            }
            // the pre-roll row's time, and EndSound's writes at theirs when the tune ends in it
            self.eight_row_times(hw, hw.bus.clock - 2);
        }
        // the pre-roll: one row
        self.play_tune(hw);
        if self.eight {
            self.eight_end_sound(hw);
        } else if self.voices[0].ended && !defer_end {
            self.end_sound(hw, 0);
        }
    }

    /// The mix buffers' length: a tick at period $7E (PAL), even.
    fn buffer_length(&self) -> u16 {
        let tempo = (self.tempo as u32).max(1);
        let d1 = (0x361f0f / 0x7e) & 0xffff;
        (((d1 * 0x7d) / ((tempo * 50) & 0xffff).max(1)) & 0xfffe) as u16
    }

    /// DTP_StartInt.
    fn start_int(&mut self, hw: &mut Hw) {
        self.first = true;
        if !self.eight {
            self.timer = (0x1b0f88 / (self.tempo as u32).max(1)) as u16;
            // the original starts the timer before writing its latch, so the
            // timer counts down from $FFFF: its first interrupt comes 65536
            // E-clocks after the start (the tick's first write 47 colour clocks
            // later) and the ticks are regular from there. The DMA-on write is
            // 131 colour clocks after the timer starts. A tempo effect in the
            // pre-roll row has written the latch before (the timer being stopped,
            // that loaded the counter): the first interrupt then comes after
            // that latch's period.
            // the timer started at `t0` (its first underflow comes at the first E-clock edge
            // after it, (latch + 1) E-clocks on, the latch being the $FFFF of the stopped timer or
            // the one a tempo effect of the pre-roll row wrote)
            let t0 = hw.bus.clock;
            let latch = match self.preload.take() {
                Some(latch) if !self.started => latch,
                _ => 0xffff,
            };
            self.started = true;
            self.cia = CiaB::default();
            self.cia.e = t0 / ECLOCK;
            self.cia.ta = CiaTimer { latch: self.timer, counter: latch as i64, running: true, one_shot: false };
            let entry = self.cia.due().unwrap_or(t0);
            hw.wait(131);
            hw.dmacon(0x8000 | self.dma_mask);
            // the song starts (Paula's grids restart) this long after the DMA-on write in the
            // reference; the first interrupt keeps its time
            hw.wait(START_LAG);
            self.used.set(entry.saturating_sub(hw.bus.clock).max(1));
            self.grid = None;
            self.isr_step = 1;
        } else {
            // the pre-roll row's work and the calls up to StartInt
            hw.wait(self.eight_ack_at.saturating_sub(hw.bus.clock));
            hw.bus.write_word(0xdff09c, 0x0080);
            hw.wait(1);
            hw.bus.write_word(0xdff09a, 0x8080);
            hw.wait(1);
            hw.dmacon(0x800f);
            for ch in 0..4 {
                hw.wait(1);
                hw.vol(ch, 0x40);
            }
            hw.wait(EIGHT_ACK_TO_START);
        }
    }

    /// DTP_EndSound.
    /// Its three writes come `gap` colour clocks apart.
    fn end_sound(&mut self, hw: &mut Hw, gap: u64) {
        hw.dmacon(0x000f);
        hw.wait(gap);
        hw.bus.write_word(0xdff09a, 0x0080);
        hw.wait(gap);
        hw.bus.write_word(0xdff09c, 0x0080);
        for v in self.voices.iter_mut() {
            v.wave_number_old = 0;
        }
    }

    /// The interrupt handler with 8 voices: one tick.
    fn handler(&mut self, hw: &mut Hw) {
        hw.bus.write_word(0xdff09c, 0x0080);
        self.half ^= 0xa00;
        let mut at = 0;
        for ch in 0..4 {
            hw.wait(EIGHT_LOC_AT[ch] - at);
            at = EIGHT_LOC_AT[ch];
            hw.loc(ch, self.mix[1] + self.half as u32 + 0x1400 * ch as u32);
        }
        let first = std::mem::take(&mut self.first);
        if first {
            for ch in 0..4 {
                hw.wait(EIGHT_FIRST_VOL_AT[ch] - at);
                at = EIGHT_FIRST_VOL_AT[ch];
                hw.vol(ch, 0x40);
            }
        } else {
            self.play_tune(hw);
        }
        hw.wait(if first { FIRST_PLAY_TIME } else { PLAY_TIME });
        for v in 0..8 {
            self.play_effects(hw, v);
        }
        for v in 0..8 {
            self.period(v);
        }
        self.chan_written = [false; 4];
        for v in 0..8 {
            self.mix_voice(hw, v);
        }
        for v in 0..8 {
            self.mix_retrigger(hw, v);
        }
        // the fast buffer to the chip one
        let n = ((self.mix_length >> 4) as i32 - 1).max(0) as u32 + 1;
        for ch in 0..4u32 {
            let from = self.mix[0] + self.half as u32 + 0x1400 * ch;
            let to = self.mix[1] + self.half as u32 + 0x1400 * ch;
            for k in 0..8 * n {
                let x = rl(hw, from + 4 * k);
                hw.bus.w32(to + 4 * k, x);
            }
        }
    }

    /// The interrupt handler with 4 voices. The time of each register write is the 68000 cycles the
    /// original's code runs from the call's entry to it (`cyc`: every branch of the code the port follows adds
    /// what the original's takes; measured on the Eagleplayer's runs under the engine, whose instruction times
    /// do not depend on the data).
    fn handler4(&mut self, hw: &mut Hw) {
        self.cyc += 100;
        self.reach(hw, 0);
        hw.bus.write_word(0xdff09c, 0x0080);
        self.cyc += 81;
        if std::mem::take(&mut self.first) {
            // the first tick writes the four volumes instead of reading the tune
            for ch in 0..4 {
                self.cyc += if ch == 0 { 40 } else { 24 };
                self.reach(hw, 0);
                hw.vol(ch, 0);
            }
            self.cyc += 10;
        } else {
            self.cyc += 18;
            self.play_tune(hw);
            self.cyc += 37;
        }
        self.cyc += 18;
        for v in 0..4 {
            self.cyc += if v == 3 { 45 } else { 42 };
            self.play_effects(hw, v);
        }
        self.cyc += 18;
        for v in 0..4 {
            self.cyc += 30;
            self.period(v);
        }
        self.cyc += 37 + 18;
        self.period_volume_play4(hw);
        self.cyc += 18;
        self.dma4(hw);
    }

    /// The periods and volumes of the voices that do not restart.
    fn period_volume_play4(&mut self, hw: &mut Hw) {
        for v in 0..4 {
            self.cyc += [51, 30, 30, 12][v] + 25;
            if self.voices[v].play & P_RETRIGGER == 0 {
                self.cyc += 273;
                self.reach(hw, 241);
                hw.per(v, self.voices[v].period);
                if !self.voices[v].muted {
                    self.reach(hw, 12);
                    hw.vol(v, self.amiga_volume(v));
                }
            }
            self.cyc += 16;
        }
    }

    /// The DMA routine, at a tick's end: the voices that restart (and those of earlier
    /// ticks whose restart has not run yet) are switched off, timer B starts its wait (100
    /// E-clocks, or the longest period those voices played on the last tick) and the
    /// timer B interrupt will take its first step.
    fn dma4(&mut self, hw: &mut Hw) {
        let mut mask = self.dma_mask;
        let mut wait = 100u16;
        for v in 0..4 {
            self.cyc += if v == 0 { 92 } else { 37 };
            let x = &mut self.voices[v];
            if x.play & P_RETRIGGER == 0 {
                continue;
            }
            self.cyc += 57;
            let (n, old) = (x.wave_number, x.wave_number_old);
            x.wave_number_old = n;
            if x.wave_or_sample != SAMPLE {
                self.cyc += 13;
                if n == old {
                    continue;
                }
            }
            self.cyc += 29;
            mask |= 1 << v;
            if self.prev_period[v] > wait {
                self.cyc += 12;
                wait = self.prev_period[v];
            }
        }
        self.cyc += 242;
        self.reach(hw, 230);
        hw.dmacon(mask);
        self.dma_mask = mask;
        for v in 0..4 {
            self.prev_period[v] = self.voices[v].period;
        }
        self.reach(hw, 16);
        self.cia.sync(hw.bus.clock);
        self.cia.tb.write_high(wait);
        self.isr_step = 1;
        // the handler's end: it reads the CIA's interrupt flags 63 cycles after starting timer B and
        // returns 152 cycles later; 29 sooner when timer A's flag is not set (the tick was dispatched
        // for an underflow an earlier read had already seen), 90 later when timer B's is not (the
        // first tick)
        self.cyc += 47;
        self.cia.sync(self.call_start + self.cyc / 2);
        let flags = std::mem::take(&mut self.cia.flags);
        self.cyc += 152;
        if flags & 1 == 0 {
            self.cyc -= 29;
        }
        if flags & 2 == 0 {
            self.cyc += 90;
            self.signalled = true;
        }
        Self::until(hw, self.call_start + self.cyc / 2);
    }

    /// 4 voices: lets the clock reach the time of the original's code `back` cycles before the point
    /// counted so far.
    fn reach(&self, hw: &mut Hw, back: u64) {
        Self::until(hw, self.call_start + (self.cyc - back) / 2);
    }

    fn amiga_volume(&self, v: usize) -> u16 {
        let x = &self.voices[v];
        let w = ((x.volume3 as u32 * x.channel_volume as u32) << 6) >> 16 & 0xffff;
        ((((w * self.master as u32) << 2) >> 16) & 0xffff) as u16
    }

    /// Lets the time pass to `t`, if it has not.
    fn until(hw: &mut Hw, t: u64) {
        let now = hw.bus.clock;
        if t > now {
            hw.wait(t - now);
        }
    }

    /// The CIA-B timer B interrupt: the first step starts the voices, the second sets their
    /// loops, a third does nothing. The DMA routine of every tick sets the first step again, so a
    /// timer B interrupt that waited for a tick to end runs the first step again, and the second
    /// (which clears the DMA mask) never comes while the ticks overrun.
    fn timer_b_interrupt(&mut self, hw: &mut Hw) {
        // the 68000 cycles from the interrupt's entry, as in the tick
        let c = hw.bus.clock;
        let mut cyc: u64 = 181;
        if self.isr_step == 1 {
            cyc += 56;
            self.isr_step = 2;
            for v in 0..4 {
                cyc += 41;
                if self.voices[v].play & P_RETRIGGER == 0 {
                    continue;
                }
                self.voices[v].play &= !P_RETRIGGER;
                if !self.voices[v].muted {
                    Self::until(hw, c + (cyc + 209) / 2);
                    let vol = self.amiga_volume(v);
                    hw.vol(v, vol);
                }
                let x = &self.voices[v];
                Self::until(hw, c + (cyc + 337) / 2);
                hw.loc(v, x.wave_pointer);
                Self::until(hw, c + (cyc + 357) / 2);
                hw.len(v, x.wave_length);
                cyc += 377;
                Self::until(hw, c + cyc / 2);
                hw.per(v, x.period);
            }
            Self::until(hw, c + (cyc + 36) / 2);
            hw.dmacon(0x8000 | self.dma_mask);
            Self::until(hw, c + (cyc + 118) / 2);
            self.cia.sync(hw.bus.clock);
            self.cia.tb.write_high(150);
            cyc += 134;
        } else if self.isr_step == 2 {
            cyc += 56;
            self.isr_step = 3;
            for v in 0..4 {
                cyc += if v == 0 { 31 } else { 19 };
                if self.dma_mask & 1 << v == 0 {
                    continue;
                }
                let x = &self.voices[v];
                Self::until(hw, c + (cyc + 108) / 2);
                hw.loc(v, x.repeat_pointer);
                cyc += 128;
                Self::until(hw, c + cyc / 2);
                hw.len(v, x.repeat_length);
            }
            cyc += 48;
            self.dma_mask = 0;
        }
        Self::until(hw, c + (cyc + 148) / 2);
    }

    // ---- the tune ----

    fn play_tune(&mut self, hw: &mut Hw) {
        for v in 0..self.voice_count() {
            self.cyc += 101;
            self.play_voice(hw, v);
        }
    }

    fn play_voice(&mut self, hw: &mut Hw, v: usize) {
        let seq = rl(hw, self.tune + TU_SEQ + 4 * v as u32);
        let x = &mut self.voices[v];
        if x.voice_off {
            return;
        }
        self.cyc += 27;
        x.speed_counter = x.speed_counter.wrapping_sub(1);
        if x.speed_counter != 0 {
            return;
        }
        self.cyc += 117;
        let mut spd = x.speed;
        x.part_groove = !x.part_groove;
        self.cyc += 21 * x.part_groove as u64;
        if x.part_groove && x.groove != 0 {
            self.cyc += 6;
            spd = x.groove;
        }
        x.speed_counter = spd;
        let x = &mut self.voices[v];
        x.speed_part = 0;
        x.groove_part = 0;
        for _ in 0..256 {
            self.cyc += 125;
            let x = &mut self.voices[v];
            let pos = x.tune_position;
            let data = rw(hw, seq + 2 * pos as u32);
            self.cyc += 20 * (pos == 0) as u64;
            let arg = (data & 0x1f) as u8;
            let target = (data >> 8) as u8;
            if data & 0x20 != 0 {
                self.cyc += 81;
                match (data & 0xc0) >> 6 {
                    1 => {
                        self.cyc += 12;
                        x.voice_off = true;
                        x.ended = true;
                        return;
                    }
                    2 => {
                        self.cyc += 55;
                        if x.tune_jump_counter != 0 {
                            x.tune_jump_counter -= 1;
                            if x.tune_jump_counter != 0 {
                                x.tune_position = target;
                            } else {
                                x.tune_position = x.tune_position.wrapping_add(1);
                            }
                        } else if pos > target {
                            self.cyc += 13;
                            x.tune_jump_counter = arg;
                            x.tune_position = target;
                        } else {
                            x.tune_position = x.tune_position.wrapping_add(1);
                        }
                        continue;
                    }
                    3 => {
                        self.cyc += 21;
                        if x.tune_wait != 0 {
                            self.cyc += 25;
                            x.tune_wait -= 1;
                            if x.tune_wait != 0 {
                                self.cyc += 10;
                                return;
                            }
                            self.cyc += 26;
                            x.tune_position = x.tune_position.wrapping_add(1);
                            continue;
                        }
                        self.cyc += 178;
                        x.tune_wait = target;
                        if target == 0 {
                            x.tune_position = x.tune_position.wrapping_add(1);
                            continue;
                        }
                        x.pt_pitch_slide = 0;
                        x.pitch_slide = 0;
                        x.volume_slide = 0;
                        x.part_note = 0;
                        x.part_instrument = 0;
                        if arg != 0 {
                            x.speed = arg;
                            x.speed_counter = arg;
                        }
                        return;
                    }
                    _ => {}
                }
            }
            self.cyc += 150;
            if self.parse_part(hw, v, data) {
                self.cyc += 509;
                return;
            }
        }
    }

    /// A part's line; true when the voice is done for this row.
    fn parse_part(&mut self, hw: &mut Hw, v: usize, data: u16) -> bool {
        let x = &mut self.voices[v];
        x.part_number = ((data << 2) & 0x300) | (data >> 8);
        x.transpose_number = ((data & 0x1f) as u8).wrapping_sub(16);
        let part = rl(hw, self.tab + T_PARTS + 4 * x.part_number as u32);
        loop {
            self.cyc += 236;
            let x = &mut self.voices[v];
            let pos = x.part_position;
            x.part_position = x.part_position.wrapping_add(1) & 0x7f;
            if x.part_position == 0 {
                self.cyc += 56;
                x.tune_position = x.tune_position.wrapping_add(1);
                x.speed = self.tune_speed;
                x.groove = self.tune_groove;
            }
            let line = part + 12 * pos as u32;
            x.part_note = rb(hw, line);
            x.part_instrument = rb(hw, line + 1);
            for k in 0..5 {
                x.part_effects[k] = (rb(hw, line + 2 + 2 * k as u32), rb(hw, line + 3 + 2 * k as u32));
            }
            if x.part_note == 61 {
                self.cyc += 131;
                if pos == 0 {
                    self.end_sound(hw, 0);
                    return true;
                }
                let x = &mut self.voices[v];
                x.part_position = 0;
                x.speed = self.tune_speed;
                x.groove = self.tune_groove;
                x.speed_counter = x.speed;
                x.tune_position = x.tune_position.wrapping_add(1);
                return false;
            }
            self.cyc += 23;
            let jump = x.part_note & 0x7f;
            if x.part_note & 0x80 != 0 {
                self.cyc += 21;
                if x.part_jump_counter != 0 {
                    self.cyc += 25;
                    x.part_jump_counter -= 1;
                    if x.part_jump_counter != 0 {
                        self.cyc += 34;
                        x.part_position = jump;
                    }
                } else if pos > jump {
                    self.cyc += 78;
                    x.part_jump_counter = x.part_instrument;
                    x.part_position = jump;
                }
                continue;
            }
            if jump != 0 {
                self.cyc += 54;
                x.transpose = ((x.transpose_number as i8 as i16) << 5) as i16;
            }
            self.check_instrument(hw, v);
            return true;
        }
    }

    fn check_instrument(&mut self, hw: &mut Hw, v: usize) {
        let pi = self.voices[v].part_instrument;
        if pi != 0 {
            self.cyc += 167;
        }
        if pi != 0 {
            let inst = rl(hw, self.tab + T_INSTS + 4 * pi as u32);
            let x = &mut self.voices[v];
            x.instrument = inst;
            if pi != x.old_instrument {
                self.cyc += 44;
                x.arp = 0;
                x.instrument_pitch_slide = 0;
                x.old_instrument = pi;
            }
        }
        self.play_part_fx(hw, v);
        self.play_arpeggio(hw, v);
        self.play_instrument(hw, v);
    }

    fn play_arpeggio(&mut self, hw: &mut Hw, v: usize) {
        let inst = self.voices[v].instrument;
        self.cyc += 25 * (self.voices[v].part_note != 0 && inst == 0) as u64;
        if self.voices[v].part_note == 0 || inst == 0 {
            self.cyc += 16;
            return;
        }
        self.cyc += 142;
        let x = &mut self.voices[v];
        x.arp_wait = false;
        if x.arp & (A_USE_TABLE | A_ENABLED) == 0 {
            self.cyc += 25;
            if rb(hw, inst + I_EFFECT1) & E1_ARPEGGIO == 0 {
                return;
            }
            self.cyc += 30;
            x.arp |= A_ENABLED;
        }
        self.cyc += 592;
        if x.restart == 0 && x.part_instrument == 0 {
            return;
        }
        x.arp |= A_WILL_SET_NOTE;
        let mut num = rw(hw, inst + I_ARP_TABLE) as u32;
        if x.arp & A_USE_TABLE != 0 {
            self.cyc -= 13;
            num = x.arp_table as u32;
        }
        let arp = rl(hw, self.tab + T_ARPS + 4 * (num & 0xff));
        x.arp_position = 0;
        x.arp_wait = false;
        x.arp_volume_slide = 0;
        x.arp_pitch_slide = 0;
        x.arp_pitch_slide_note = 0;
        x.arp_speed_counter = rb(hw, inst + I_ARP_SPEED);
        loop {
            let x = &mut self.voices[v];
            let pos = x.arp_position;
            x.arp_position = x.arp_position.wrapping_add(1) & 0x7f;
            let line = arp + 6 * pos as u32;
            x.arp_note = x.part_note;
            let mut tr = rb(hw, line) as i8;
            if tr == 0 {
                x.arp_wait = true;
                return;
            }
            if tr == 61 {
                x.effect1 &= !E1_ARPEGGIO;
                return;
            }
            if tr == 62 {
                continue;
            }
            let sn = rb(hw, line + 1);
            x.wave_number = if sn == 0 { rb(hw, inst + IS_NUMBER) } else { sn };
            for k in 0..2 {
                self.cyc += 103;
                let e = (rb(hw, line + 2 + 2 * k), rb(hw, line + 3 + 2 * k));
                self.arpeggio_effect(v, e);
            }
            let x = &mut self.voices[v];
            if tr < 0 {
                self.cyc += 20;
                tr = tr.wrapping_add((61u8.wrapping_add(x.arp_note)) as i8);
            } else {
                self.cyc += 30;
                x.arp |= A_NOTE_ABSOLUTE;
            }
            x.note = ((tr as i16) << 5) as u16;
            x.arp_calc_note = x.note;
            self.arpeggio_wait_start(hw, v, sn);
            return;
        }
    }

    fn arpeggio_wait_start(&mut self, hw: &Hw, v: usize, sn: u8) {
        if sn == 0 {
            return;
        }
        let s = self.sample(hw, sn);
        let x = &mut self.voices[v];
        x.wave_sample = s;
        x.arp |= A_USE_WAVE_SAMPLE;
    }

    fn play_instrument(&mut self, hw: &mut Hw, v: usize) {
        self.cyc += 80;
        let inst = self.voices[v].instrument;
        if self.voices[v].arp_wait || inst == 0 {
            return;
        }
        self.cyc += 177;
        let mut is = inst;
        let mut st = rb(hw, inst + IS_TYPE);
        let x = &mut self.voices[v];
        let mut setup = true;
        if x.restart & (R_PART_FX | R_ARP) != 0 {
            self.cyc += 69;
            x.wave_number = rb(hw, inst + IS_NUMBER);
        } else if x.part_instrument == 0 {
            setup = false;
        } else if x.pitch_slide == FX_PORTAMENTO || x.part_note == 0 {
            if x.pitch_slide == FX_PORTAMENTO {
                self.cyc -= 37;
            } else {
                self.cyc += 11;
            }
            x.volume1 = rw(hw, inst + I_VOLUME).wrapping_mul(16);
            setup = false;
        } else if x.arp & A_USE_WAVE_SAMPLE != 0 {
            is = x.wave_sample;
            if is == 0 {
                return;
            }
            if rb(hw, is + IS_TYPE) == SAMPLE {
                st = SAMPLE;
            }
        } else {
            self.cyc -= 91;
            x.wave_number = rb(hw, inst + IS_NUMBER);
        }
        if setup {
            let mut ptr = rl(hw, is + IS_POINTER);
            if ptr == 0 {
                return;
            }
            let x = &mut self.voices[v];
            if rb(hw, inst + I_TRANSPOSABLE) == 0 {
                self.cyc += 16;
                x.transpose = 0;
            }
            let mut flag = rb(hw, inst + I_ETPF_BITS);
            if x.effect1 & E1_HOLD_SUSTAIN != 0 {
                self.cyc += 6;
                flag &= !F_HOLD_SUSTAIN;
                x.arg1 &= F_HOLD_SUSTAIN;
                flag |= x.arg1;
            }
            x.arg1 = flag;
            x.arg2 = rb(hw, inst + I_MRL_BITS);
            x.effect1 = rb(hw, inst + I_EFFECT1);
            x.effect2 = rb(hw, inst + I_EFFECT2);
            x.wave_or_sample = st;
            if st != SAMPLE {
                self.cyc += 28;
                let len = rw(hw, inst + IS_LENGTH);
                Self::fix_wave_length(x, len, st, ptr, &mut self.cyc);
            } else {
                self.cyc += 200;
                let mut len = rw(hw, is + IS_LENGTH);
                if x.sample_offset_active {
                    let off = (x.sample_offset as u16) << 7;
                    if off < len {
                        len -= off;
                        ptr = ptr.wrapping_add(2 * off as u32);
                    } else {
                        len = 1;
                    }
                }
                x.wave_pointer = ptr;
                x.wave_length = len;
                let (mut rp, mut rl_) = (rl(hw, is + IS_REPEAT), rw(hw, is + IS_REPEAT_LENGTH));
                if x.arp & A_USE_WAVE_SAMPLE == 0 && rb(hw, inst + I_EFFECT1) & E1_WAVE_SAMPLE_LOOP == 0 || rl_ == 0 {
                    self.cyc += 16;
                    // no loop: the empty sample
                    rp = self.data;
                    rl_ = 1;
                }
                x.repeat_pointer = rp;
                x.repeat_original = rp;
                x.repeat_length = rl_;
            }
            x.restart &= R_PART_FX | R_ARP | R_ARP_VOLUME;
            if x.restart == 0 {
                self.cyc += 38;
                x.volume1 = rw(hw, inst + I_VOLUME).wrapping_mul(16);
            }
        }
        // PlayNote
        let x = &mut self.voices[v];
        let mut vol = if x.volume != 0 { self.cyc += 12; x.volume_set as i16 } else { x.volume1 as i16 };
        if x.volume_add != 0 {
            self.cyc += 38;
            vol = vol.wrapping_add(x.volume_add_number).clamp(0, 1024);
        }
        x.volume1 = vol as u16;
        x.volume2 = vol as u16;
        x.volume3 = vol as u16;
        if x.pitch_slide == FX_PORTAMENTO {
            return;
        }
        self.cyc += 21;
        let slide = rb(hw, inst + I_SLIDE_SPEED);
        self.cyc += 25 * (slide != 0) as u64;
        if slide != 0 && x.part_note != 0 {
            self.cyc += 107;
            x.pitch_slide_speed = slide as u16;
            let new_note = x.note.wrapping_add(x.pitch_slide_note);
            x.pitch_slide_to = (x.part_note as i16) << 5;
            x.pitch_slide_down = (x.pitch_slide_to as i32) < new_note as i32;
            if x.instrument_pitch_slide != 0 {
                self.inst_play(hw, v);
                return;
            }
            self.cyc += 16;
            x.instrument_pitch_slide = FX_PORTAMENTO;
        }
        self.cyc += 29;
        let x = &mut self.voices[v];
        let had = x.arp & A_WILL_SET_NOTE != 0;
        x.arp &= !A_WILL_SET_NOTE;
        if !had {
            self.cyc += 25;
            if x.part_note == 0 {
                return;
            }
            self.cyc += 76;
            x.note = (x.part_note as u16) << 5;
            x.arp_volume_slide = 0;
            x.arp_pitch_slide = 0;
            x.arp_pitch_slide_note = 0;
        }
        self.cyc += 216;
        x.semi_tone = ((rw(hw, is + IS_SEMI_TONE) as i16) << 5) as i16;
        if x.play & P_KEEP_FINE_TUNE == 0 {
            self.cyc += 20;
            x.fine_tune = rw(hw, is + IS_FINE_TUNE) as i16;
        }
        x.pt_pitch_slide_note = 0;
        x.pitch_slide_note = 0;
        x.pt_vibrato_note = 0;
        x.pt_tremolo_position = 0;
        x.pt_vibrato_position = 0;
        x.vibrato_note = 0;
        x.pt_pitch_add = 0;
        x.pitch_add = 0;
        self.cyc += 31 * (x.part_instrument == 0 && x.restart & (R_PART_FX | R_ARP) != 0) as u64;
        if x.part_instrument != 0 || x.restart & (R_PART_FX | R_ARP) != 0 {
            self.inst_play(hw, v);
        }
    }

    fn fix_wave_length(x: &mut Voice, len: u16, st: u8, ptr: u32, cyc: &mut u64) {
        *cyc += 50;
        let (off, sh) = match st {
            WAVE16 => { *cyc += 77; (256 + 128 + 64 + 32, 4) },
            WAVE32 => { *cyc += 105; (256 + 128 + 64, 3) },
            WAVE64 => { *cyc += 137; (256 + 128, 2) },
            WAVE128 => { *cyc += 169; (256, 1) },
            _ => { *cyc += 152; (0, 0) },
        };
        let p = ptr.wrapping_add(off);
        x.wave_pointer = p;
        x.repeat_pointer = p;
        x.repeat_original = p;
        x.wave_length = len >> sh;
        x.repeat_length = len >> sh;
    }

    /// An instrument effect's counter, as InstPlay sets it up.
    fn init_counter(hw: &Hw, inst: u32, o: u32, step: bool, init_flag: bool, init: &mut u8, pi: u8, c: &mut Counter, speed: &mut u16, cyc: &mut u64) -> bool {
        let w = |k: u32| rw(hw, inst + o + 2 * k);
        let mut fresh = false;
        let mut check = false;
        c.step = step;
        if step {
            *cyc += 46;
            *speed = w(4);
            c.turns = 0;
            check = true;
        } else {
            *cyc += 45;
            c.turns = w(4) as i16;
            if !init_flag {
                *cyc += 16;
                *init = 0;
                fresh = true;
            } else {
                check = true;
            }
        }
        if check {
            *cyc += 53;
            let old = *init;
            *init = pi;
            if old == pi {
                *cyc += 10;
                c.delay = w(5);
            } else {
                fresh = true;
            }
        }
        if fresh {
            // (the resonance's and the filter's are counted with their flag, in InstPlay)
            *cyc += match o {
                I_TRANSFORM => 97,
                I_PHASE | I_MIX => 101,
                _ => 0,
            };
            c.counter = w(0);
            c.speed = w(3) as i16;
            c.repeat = w(1);
            c.repeat_end = w(2);
            if w(0) > w(1) {
                *cyc += 16;
                c.speed = c.speed.wrapping_neg();
            }
            c.delay = w(5);
        }
        fresh
    }

    fn inst_play(&mut self, hw: &mut Hw, v: usize) {
        let inst = self.voices[v].instrument;
        let x = &mut self.voices[v];
        if x.pitch_slide != FX_PORTAMENTO && !x.arp_wait {
            // (the phase, resonance, filter and mix flags' tests)
            self.cyc += 4 * 168;
            if x.effect1 & E1_ENVELOPE != 0 {
                self.cyc += 152;
            }
            x.play |= P_RETRIGGER;
            let w = |o: u32| rw(hw, inst + o);
            if x.vibrato == 0 && x.effect1 & E1_VIBRATO != 0 {
                self.cyc += 172;
                x.vibrato_counter = 0;
                x.vibrato_depth_now = 0;
                x.vibrato_speed = w(I_VIBRATO + 2);
                x.vibrato_delay = w(I_VIBRATO + 4);
                x.vibrato_attack_speed = w(I_VIBRATO + 6);
                x.vibrato_attack_length = w(I_VIBRATO + 8);
                x.vibrato_depth = w(I_VIBRATO + 10);
                x.vibrato_wave = rb(hw, inst + I_VIBRATO + 1);
                x.vibrato_down = rb(hw, inst + I_VIBRATO) == 0;
            }
            self.cyc += 25 * (x.tremolo == 0) as u64;
            if x.tremolo == 0 && x.effect1 & E1_TREMOLO != 0 {
                self.cyc += 172;
                x.tremolo_counter = 0;
                x.tremolo_depth_now = 0;
                x.tremolo_speed = w(I_TREMOLO + 2);
                x.tremolo_delay = w(I_TREMOLO + 4);
                x.tremolo_attack_speed = w(I_TREMOLO + 6);
                x.tremolo_attack_length = w(I_TREMOLO + 8);
                x.tremolo_depth = w(I_TREMOLO + 10);
                x.tremolo_wave = rb(hw, inst + I_TREMOLO + 1);
                x.tremolo_down = rb(hw, inst + I_TREMOLO) == 0;
            }
            if x.effect1 & E1_ENVELOPE != 0 {
                x.envelope_volume = 0;
                for k in 0..12 {
                    x.envelope[k] = w(I_ENVELOPE + 2 * k as u32);
                }
            }
            let pi = x.part_instrument;
            if x.effect2 & E2_PHASE != 0 {
                self.cyc += 99;
                x.phase_type = w(I_PHASE_TYPE);
                let (s, i) = (x.arg1 & F_PHASE_STEP != 0, x.arg1 & F_PHASE_INIT != 0);
                Self::init_counter(hw, inst, I_PHASE, s, i, &mut x.phase_init, pi, &mut x.phase_counter, &mut x.phase_speed, &mut self.cyc);
            }
            x.boost = rb(hw, inst + I_BOOST);
            if x.effect2 & E2_RESONANCE != 0 {
                self.cyc += 99;
                x.resonance_amp = rb(hw, inst + I_RESONANCE_AMP);
                let (s, i) = (x.arg2 & M_RESONANCE_STEP != 0, x.arg2 & M_RESONANCE_INIT != 0);
                if Self::init_counter(hw, inst, I_RESONANCE, s, i, &mut x.resonance_init, pi, &mut x.resonance_counter, &mut x.resonance_speed, &mut self.cyc) {
                    self.cyc += 117;
                    x.resonance_last_init = true;
                }
            }
            if x.effect2 & E2_FILTER != 0 {
                self.cyc += 99;
                x.filter_type = rb(hw, inst + I_FILTER_TYPE);
                let (s, i) = (x.arg1 & F_FILTER_STEP != 0, x.arg1 & F_FILTER_INIT != 0);
                if Self::init_counter(hw, inst, I_FILTER, s, i, &mut x.filter_init, pi, &mut x.filter_counter, &mut x.filter_speed, &mut self.cyc) {
                    self.cyc += 117;
                    x.filter_last_init = true;
                }
            }
            if x.effect2 & E2_MIX != 0 {
                self.cyc += 99;
                x.mix_wave = rb(hw, inst + I_MIX_WAVE);
                let (s, i) = (x.arg2 & M_MIX_STEP != 0, x.arg2 & M_MIX_INIT != 0);
                Self::init_counter(hw, inst, I_MIX, s, i, &mut x.mix_init, pi, &mut x.mix_counter, &mut x.mix_speed, &mut self.cyc);
            }
            if x.effect2 & E2_TRANSFORM != 0 {
                self.cyc += 216;
                x.transform_waves[0] = rb(hw, inst + IS_NUMBER);
                for k in 0..5 {
                    self.cyc -= 9;
                    x.transform_waves[k + 1] = rb(hw, inst + I_TRANSFORM_WAVES + k as u32);
                }
                let (s, i) = (x.arg1 & F_TRANSFORM_STEP != 0, x.arg1 & F_TRANSFORM_INIT != 0);
                Self::init_counter(hw, inst, I_TRANSFORM, s, i, &mut x.transform_init, pi, &mut x.transform_counter, &mut x.transform_speed, &mut self.cyc);
            }
            if x.effect1 & E1_LOOP != 0 {
                self.cyc += 275;
                let ll = w(I_LOOP + 6);
                self.cyc += 21 * (rb(hw, inst + IS_TYPE) == SAMPLE && ll == 0) as u64;
                if rb(hw, inst + IS_TYPE) != SAMPLE || ll == 0 {
                    self.cyc -= 224;
                    x.effect1 &= !E1_LOOP;
                } else {
                    let (mut init, mut part_init, mut check) = (false, false, false);
                    if x.arg2 & M_LOOP_STEP != 0 {
                        self.cyc += 105;
                        x.loop_speed = w(I_LOOP + 14) as i16;
                        x.loop_turns = 0;
                        check = true;
                    } else {
                        x.loop_turns = w(I_LOOP + 14) as i16;
                        if x.arg2 & M_LOOP_INIT == 0 {
                            self.cyc += 334;
                            x.loop_init = 0;
                            init = true;
                        } else {
                            self.cyc += 85;
                            check = true;
                        }
                    }
                    if check {
                        self.cyc += 17;
                        let old = x.loop_init;
                        x.loop_init = pi;
                        if old == pi {
                            x.play &= !P_RETRIGGER;
                            part_init = true;
                        } else {
                            self.cyc += 237;
                            init = true;
                        }
                    }
                    if init {
                        self.cyc += 17;
                        let s = rl(hw, self.tab + T_SAMPLES + 4 * rb(hw, inst + IS_NUMBER) as u32);
                        x.loop_sample = rl(hw, s + IS_POINTER);
                        x.loop_counter = w(I_LOOP);
                        x.loop_counter_save = w(I_LOOP);
                        x.loop_counter_max = rw(hw, s + IS_LENGTH).wrapping_sub(ll);
                        x.loop_length = ll;
                        x.loop_repeat_end = w(I_LOOP + 4);
                        x.loop_wait = w(I_LOOP + 10);
                        x.loop_step = w(I_LOOP + 8) as i32;
                        x.loop_repeat = w(I_LOOP + 2);
                        if w(I_LOOP) > w(I_LOOP + 2) {
                            self.cyc += 24;
                            x.loop_step = -x.loop_step;
                        }
                    }
                    if init || part_init {
                        x.loop_delay = w(I_LOOP + 12) as i16;
                        x.loop_wait_counter = 0;
                        let p = x.loop_sample.wrapping_add(2 * x.loop_counter_save as u32);
                        x.wave_pointer = p;
                        x.repeat_pointer = p;
                        x.repeat_original = p;
                        x.wave_length = x.loop_length;
                        x.repeat_length = x.loop_length;
                    }
                }
            }
        }
        let x = &mut self.voices[v];
        if !matches!(x.repeat_length, 8 | 16 | 32 | 64 | 128) {
            self.cyc += 33;
            x.effect2 = 0;
        }
    }

    fn period(&mut self, v: usize) {
        let x = &mut self.voices[v];
        self.cyc += 283 + 21 * (x.arp & A_NOTE_ABSOLUTE == 0) as u64;
        let mut note = x
            .note
            .wrapping_add(x.vibrato_note)
            .wrapping_add(x.pitch_slide_note)
            .wrapping_add(x.arp_pitch_slide_note)
            .wrapping_add(x.semi_tone as u16)
            .wrapping_add(x.fine_tune as u16)
            .wrapping_add(x.pitch_add as u16) as i16;
        if x.arp & A_NOTE_ABSOLUTE == 0 && x.transpose != 0 {
            self.cyc += 4;
            note = note.wrapping_add(x.transpose);
        }
        self.cyc += 8 * (note > 5 * 12 * 32) as u64;
        let p = period_of(note as i32)
            .wrapping_add(x.pt_pitch_slide_note)
            .wrapping_add(x.pt_vibrato_note)
            .wrapping_add(x.pt_pitch_add as u16);
        x.period = (p as i16).clamp(106, 3591) as u16;
    }

    fn get_period(x: &Voice, note: u16) -> u16 {
        let mut n = note
            .wrapping_add(x.vibrato_note)
            .wrapping_add(x.pitch_slide_note)
            .wrapping_add(x.arp_pitch_slide_note)
            .wrapping_add(x.semi_tone as u16)
            .wrapping_add(x.fine_tune as u16)
            .wrapping_add(x.pitch_add as u16) as i16;
        if x.transpose != 0 {
            n = n.wrapping_add(x.transpose);
        }
        period_of(n as i32)
    }

    // ---- the part effects ----

    fn play_part_fx(&mut self, hw: &mut Hw, v: usize) {
        let x = &mut self.voices[v];
        x.play |= P_FIRST_ROW_TICK;
        x.effect1 &= !E1_HOLD_SUSTAIN;
        x.vibrato = 0;
        x.vibrato_note = 0;
        x.pt_vibrato_note = 0;
        x.tremolo = 0;
        x.volume = 0;
        x.volume_add = 0;
        x.volume_slide = 0;
        x.channel_volume_slide = 0;
        x.master_volume_slide = 0;
        x.pitch_slide = 0;
        x.pt_pitch_slide = 0;
        x.sample_offset = 0;
        x.restart = 0;
        x.arp &= !(A_WILL_SET_NOTE | A_USE_WAVE_SAMPLE);
        if x.arp & A_ONE_STEP != 0 {
            self.cyc += 64;
            x.arp = 0;
            x.arp_volume_slide = 0;
            x.arp_pitch_slide = 0;
            x.arp_pitch_slide_note = 0;
        }
        if x.part_note != 0 {
            x.pitch_slide_to = -1;
            x.pt_pitch_slide_to = -1;
        }
        for k in 0..5 {
            self.cyc += 88;
            let (e, a) = self.voices[v].part_effects[k];
            self.part_effect(hw, v, e, a);
        }
    }

    fn part_effect(&mut self, hw: &mut Hw, v: usize, e: u8, a: u8) {
        let nv = self.voice_count();
        let x = &mut self.voices[v];
        let a16 = a as u16;
        match e {
            0x01 => {
                self.cyc += 65;
                x.pitch_slide = e;
                x.pitch_slide_down = false;
                if a != 0 {
                    self.cyc += 12;
                    x.pitch_slide_speed = a16;
                }
                x.pitch_slide_to = 59 * 32 + 32;
            }
            0x02 => {
                self.cyc += 65;
                x.pitch_slide = e;
                x.pitch_slide_down = true;
                if a != 0 {
                    self.cyc += 12;
                    x.pitch_slide_speed = a16;
                }
                x.pitch_slide_to = 0;
            }
            0x03 => {
                self.cyc += 107;
                x.pitch_slide = e;
                if a != 0 {
                    self.cyc += 12;
                    x.pitch_slide_speed = a16;
                }
                let note = x.note.wrapping_add(x.pitch_slide_note);
                let mut to = x.part_note as u16;
                if to != 0 {
                    self.cyc += 73;
                    to <<= 5;
                    x.part_note = 0;
                    x.pitch_slide_to = to as i16;
                    if to != note {
                        x.pitch_slide_down = to < note;
                        return;
                    }
                    x.pitch_slide_to = -1;
                }
                if x.pitch_slide_to == -1 {
                    self.cyc += 16;
                    x.pitch_slide = 0;
                }
            }
            0x04 => { self.cyc += 16; x.instrument_pitch_slide = 0 },
            0x05 => {
                if a != 0 {
                    x.pitch_add = x.pitch_add.wrapping_add(a as i16);
                }
            }
            0x06 => {
                if a != 0 {
                    x.pitch_add = x.pitch_add.wrapping_sub(a as i16);
                }
            }
            0x07 => {
                if a != 0 {
                    x.vibrato_speed = a16;
                }
            }
            0x08 | 0x09 => {
                self.cyc += 112;
                x.vibrato = e;
                if x.part_note != 0 {
                    x.vibrato_down = e == 0x09;
                    x.vibrato_counter = 0;
                    x.vibrato_depth_now = 0;
                    x.vibrato_delay = 0;
                    x.vibrato_attack_speed = 0;
                    x.vibrato_attack_length = 0;
                }
                if a != 0 {
                    self.cyc += 17;
                    x.vibrato_depth = a16 << 8;
                }
                x.vibrato_wave = x.part_vibrato_wave;
            }
            0x0a => {
                if a <= 3 {
                    x.vibrato_wave = a;
                    x.part_vibrato_wave = a;
                }
            }
            0x0b => {
                self.cyc += 74;
                x.fine_tune = (a as i8).clamp(-31, 31) as i16;
                x.play |= P_KEEP_FINE_TUNE;
            }
            0x10 => {
                self.cyc += 50;
                x.volume = e;
                // no clamp in the original (NostalgicPlayer clamps to 1024)
                x.volume_set = a16 << 4;
            }
            0x11 | 0x12 => {
                self.cyc += 33;
                x.volume_slide = e;
                if a != 0 {
                    self.cyc += 12;
                    x.volume_slide_speed = a16;
                }
            }
            0x13 => x.volume_slide_to = a16 << 4,
            0x14 => {
                self.cyc += 110;
                x.volume_slide = e;
                if a != 0 {
                    x.volume_slide_speed = a16;
                }
                let vol = x.volume1;
                if x.volume_slide_to != vol {
                    self.cyc += 16;
                    x.volume_slide_down = x.volume_slide_to < vol;
                    x.volume_slide_off = false;
                } else {
                    x.volume_slide_off = true;
                }
            }
            0x15 => {
                self.cyc += 59;
                x.volume_add = e;
                if a != 0 {
                    x.volume_add_number = (a16 << 4) as i16;
                }
            }
            0x16 => {
                self.cyc += 63;
                x.volume_add = e;
                if a != 0 {
                    x.volume_add_number = -((a16 << 4) as i16);
                }
            }
            0x17 => {
                self.cyc += 145;
                if a != 0 {
                    x.tremolo_speed = a16;
                }
            }
            0x18 | 0x19 => {
                x.tremolo = e;
                if x.part_note != 0 {
                    self.cyc += 92;
                    x.tremolo_down = e == 0x19;
                    x.tremolo_counter = 0;
                    x.tremolo_depth_now = 0;
                    x.tremolo_delay = 0;
                    x.tremolo_attack_speed = 0;
                    x.tremolo_attack_length = 0;
                }
                if a != 0 {
                    x.tremolo_depth = a16 << 8;
                }
                x.tremolo_wave = x.part_tremolo_wave;
            }
            0x1a => {
                if a <= 3 {
                    x.tremolo_wave = a;
                    x.part_tremolo_wave = a;
                }
            }
            // no clamp in the original
            0x20 => { self.cyc += 38; x.channel_volume = a16 << 4 },
            0x21 | 0x22 => {
                self.cyc += 33;
                x.channel_volume_slide = e;
                if a != 0 {
                    self.cyc += 12;
                    x.channel_volume_slide_speed = a16;
                }
            }
            0x23 => x.channel_volume_slide_to = a16 << 4,
            0x24 => {
                x.channel_volume_slide = e;
                if a != 0 {
                    x.channel_volume_slide_speed = a16;
                }
                let vol = x.channel_volume;
                if x.channel_volume_slide_to != vol {
                    x.channel_volume_slide_down = x.channel_volume_slide_to < vol;
                    x.channel_volume_slide_off = false;
                } else {
                    x.channel_volume_slide_off = true;
                }
            }
            0x25 => {
                self.cyc += 100;
                if a != 0 {
                    x.channel_volume_add_number = (a16 << 4) as i16;
                }
                let n = (x.channel_volume as i16).wrapping_add(x.channel_volume_add_number);
                x.channel_volume = n.min(1024) as u16;
            }
            0x26 => {
                if a != 0 {
                    x.channel_volume_add_number = (a16 << 4) as i16;
                }
                let n = (x.channel_volume as i16).wrapping_sub(x.channel_volume_add_number);
                x.channel_volume = n.max(0) as u16;
            }
            0x27 => {
                self.cyc += 159;
                for k in 0..nv {
                    self.cyc -= 4;
                    self.voices[k].channel_volume = a16 << 4;
                }
            }
            0x30 => {
                self.cyc += 38;
                self.master = a16 << 4;
            }
            0x31 | 0x32 => {
                x.master_volume_slide = e;
                if a != 0 {
                    x.master_volume_slide_speed = a16;
                }
            }
            0x33 => x.master_volume_slide_to = a16 << 4,
            0x34 => {
                x.master_volume_slide = e;
                if a != 0 {
                    x.master_volume_slide_speed = a16;
                }
                // The original compares with d1, still the effect's jump-table
                // offset (0x34 * 4), not with the master volume.
                let vol = 0x34 * 4;
                if x.master_volume_slide_to != vol {
                    x.master_volume_slide_down = x.master_volume_slide_to < vol;
                    x.master_volume_slide_off = false;
                } else {
                    x.master_volume_slide_off = true;
                }
            }
            0x35 => {
                if a != 0 {
                    x.master_volume_add_number = (a16 << 4) as i16;
                }
                self.master = (self.master as i16).wrapping_add(x.master_volume_add_number).min(1024) as u16;
            }
            0x36 => {
                if a != 0 {
                    x.master_volume_add_number = (a16 << 4) as i16;
                }
                self.master = (self.master as i16).wrapping_sub(x.master_volume_add_number).max(0) as u16;
            }
            0x40 => {
                self.cyc += 104;
                if a != 0 {
                    let s = a.min(31);
                    x.speed_part = 1;
                    x.speed = s;
                    if x.groove == 0 || !x.part_groove {
                        self.cyc -= 9;
                        x.speed_counter = s;
                    }
                }
            }
            0x41 => {
                self.cyc += 104;
                if a != 0 {
                    let g = a.min(31);
                    x.groove_part = 1;
                    x.groove = g;
                    if x.part_groove {
                        x.speed_counter = g;
                    }
                }
            }
            0x42 => {
                self.cyc += 24;
                if a == 0 {
                } else if a < 32 {
                    self.cyc += 463;
                    self.tune_speed = a;
                    for k in 0..nv {
                        let w = &mut self.voices[k];
                        if v >= k {
                            if w.speed_part == 0 {
                                w.speed = a;
                                if w.groove == 0 || !w.part_groove {
                                    w.speed_counter = a;
                                }
                            }
                        } else {
                            self.cyc -= 64;
                            w.speed = a;
                        }
                    }
                } else {
                    self.cyc += 226;
                    self.tempo = a16;
                    self.timer = (0x1b0f88 / a as u32) as u16;
                    if !self.started && !self.eight {
                        self.preload = Some(self.timer);
                    } else if !self.eight {
                        // the write of timer A's latch (the timer runs: it counts from it after its
                        // next underflow)
                        self.cia.sync(self.call_start + self.cyc / 2);
                        self.cia.ta.write_high(self.timer);
                    }
                }
            }
            0x43 => {
                if a != 0 {
                    let g = a & 0x1f;
                    self.tune_groove = g;
                    for k in 0..nv {
                        let w = &mut self.voices[k];
                        if v >= k {
                            if w.groove_part == 0 {
                                w.groove = g;
                                if g != 0 && !w.part_groove {
                                    w.speed_counter = g;
                                }
                            }
                        } else {
                            w.groove = g;
                        }
                    }
                }
            }
            0x44 => {
                self.cyc += 36;
                x.arp |= A_USE_TABLE;
                x.arp_table = a;
            }
            0x45 => {
                self.cyc += 36;
                x.arp |= A_ONE_STEP | A_USE_TABLE;
                x.arp_table = a;
            }
            0x46 => {
                self.cyc += 57;
                x.effect1 |= E1_HOLD_SUSTAIN;
                x.arg1 &= !F_HOLD_SUSTAIN;
                if a != 0 {
                    x.arg1 |= F_HOLD_SUSTAIN;
                }
            }
            0x47 => {
                self.cyc += 41;
                // the LED filter, through the CIA
                let b = hw.bus.read_byte(0xbfe001);
                hw.bus.write_byte(0xbfe001, if a != 0 { b & !2 } else { b | 2 });
            }
            0x48 => {
                self.cyc += 45;
                x.sample_offset_active = true;
                if a != 0 {
                    x.sample_offset = a;
                }
            }
            0x49 => {
                if x.part_instrument == 0 {
                    x.restart = R_PART_FX;
                }
            }
            0x4a => {
                self.cyc += 45;
                if a != 0 {
                    let s = rl(hw, self.tab + T_SAMPLES + 4 * a as u32);
                    let x = &mut self.voices[v];
                    x.wave_sample = s;
                    x.arp |= A_USE_WAVE_SAMPLE;
                }
            }
            0x4b => {
                self.cyc += 96;
                x.phase_init = 0;
                x.resonance_init = 0;
                x.filter_init = 0;
                x.transform_init = 0;
                x.mix_init = 0;
                x.loop_init = 0;
            }
            0xe1 => {
                x.pt_pitch_slide = e;
                x.pt_pitch_slide_down = false;
                if a != 0 {
                    x.pt_pitch_slide_speed = a16;
                }
                x.pt_pitch_slide_to = 106;
            }
            0xe2 => {
                x.pt_pitch_slide = e;
                x.pt_pitch_slide_down = true;
                if a != 0 {
                    x.pt_pitch_slide_speed = a16;
                }
                x.pt_pitch_slide_to = 3591;
            }
            0xe3 => {
                self.cyc += 161;
                x.pitch_slide = FX_PORTAMENTO;
                x.pt_pitch_slide = e;
                if a != 0 {
                    x.pt_pitch_slide_speed2 = a16;
                }
                let p = Self::get_period(x, x.note).wrapping_add(x.pt_pitch_slide_note);
                self.cyc += 226;
                if x.part_note != 0 {
                    let p2 = Self::get_period(x, (x.part_note as u16) << 5);
                    self.cyc += 226;
                    x.part_note = 0;
                    x.pt_pitch_slide_to = p2 as i16;
                    if p != p2 {
                        x.pt_pitch_slide_down = p < p2;
                        return;
                    }
                    x.pt_pitch_slide_to = -1;
                }
                if x.pt_pitch_slide_to == -1 {
                    x.pt_pitch_slide = 0;
                    x.pitch_slide = 0;
                }
            }
            0xe4 => {
                if a != 0 {
                    x.pt_pitch_add = x.pt_pitch_add.wrapping_sub((a & 15) as i16);
                }
            }
            0xe5 => {
                if a != 0 {
                    x.pt_pitch_add = x.pt_pitch_add.wrapping_add((a & 15) as i16);
                }
            }
            0xe6 | 0xe7 => {
                self.cyc += 80;
                x.volume_slide = e;
                if a != 0 {
                    x.volume_slide_speed = a16;
                }
            }
            0xe8 => {
                self.cyc += 107;
                x.tremolo = e;
                x.pt_tremolo_command = Self::pt_command(x.pt_tremolo_command, a);
            }
            0xe9 => x.pt_tremolo_wave = a & 15,
            0xea => {
                x.vibrato = e;
                x.pt_vibrato_command = Self::pt_command(x.pt_vibrato_command, a);
            }
            0xeb => x.pt_vibrato_wave = a & 15,
            _ => {}
        }
    }

    fn pt_command(mut c: u8, a: u8) -> u8 {
        if a != 0 {
            if a & 15 != 0 {
                c = (c & 0xf0) | (a & 15);
            }
            if a & 0xf0 != 0 {
                c = (c & 15) | (a & 0xf0);
            }
        }
        c
    }

    // ---- the effects, each tick ----

    fn play_effects(&mut self, hw: &mut Hw, v: usize) {
        let ch = v & 3;
        {
            let x = &mut self.voices[v];
            x.repeat_pointer = x.repeat_original;
        }
        self.cyc += 572;
        if self.voices[v].play & P_RETRIGGER == 0 {
            self.cyc += 622;
            self.slide_volume(v);
            self.slide_channel_volume(v);
            self.slide_master_volume(v);
            self.slide_arp_volume(v);
            self.slide_note(v);
            self.slide_arp_note(v);
            self.arpeggio_play(hw, v);
            self.vibrato_play(v);
            self.tremolo_play(v);
        }
        self.envelope_play(v);
        self.move_loop(hw, v);
        self.transform_play(hw, v);
        self.phase_play(hw, v);
        self.mix_play(hw, v);
        self.resonance_play(hw, v);
        self.filter_play(hw, v);
        let eight = self.eight;
        let x = &mut self.voices[v];
        if !eight {
            let rp = x.repeat_pointer;
            self.cyc += 15 * (rp >= x.bufs) as u64;
            if rp >= x.bufs && rp <= x.bufs + B_FILT {
                self.cyc += 97;
                let wb = x.wave_buffer;
                x.repeat_pointer = wb;
                self.cyc += [25, 10][(x.wave_or_sample != SAMPLE) as usize];
                if x.wave_or_sample != SAMPLE || x.effect1 & E1_LOOP != 0 {
                    x.wave_pointer = wb;
                }
                let n = (x.repeat_length >> 3) as u32 * 16;
                self.cyc += 90 * (n as u64 / 16);
                // Paula is not run up to the copy: the original's code touches no register
                // while its effects run, so under the engine Paula catches up at the next
                // register access (the next tick's acknowledge, or the tick's first period
                // write) and plays the new wave from the last access (measured on the
                // original's DMA fetches: the words Paula fetches are the same ones
                // with this, not with the copy's own time)
                for k in 0..n / 4 {
                    let b = rl(hw, rp + 4 * k);
                    hw.bus.w32(wb + 4 * k, b);
                }
            }
        }
        let x = &mut self.voices[v];
        self.cyc += 11 * (x.play & P_RETRIGGER == 0) as u64;
        if x.play & P_RETRIGGER == 0 && x.effect1 & E1_LOOP != 0 {
            self.cyc += 137;
            if !eight {
                let (rp, ll) = (x.repeat_pointer, x.loop_length);
                self.reach(hw, 72);
                hw.loc(ch, rp);
                self.reach(hw, 44);
                hw.len(ch, ll);
            }
            let x = &mut self.voices[v];
            x.repeat_length = x.loop_length;
        }
        self.cyc += 73;
        let x = &mut self.voices[v];
        if !x.arp_wait {
            x.part_note = 0;
        }
        x.play &= P_RETRIGGER;
    }

    fn slide_volume(&mut self, v: usize) {
        let x = &mut self.voices[v];
        let first = x.play & P_FIRST_ROW_TICK != 0;
        let mut vol;
        self.cyc += 25 * (x.volume_slide != FX_VOLUME_UP) as u64;
        if x.volume_slide == FX_SLIDE_TO_VOLUME {
            if x.volume_slide_off {
                self.cyc -= 8;
                return;
            }
            self.cyc += 64;
            let mut n;
            if x.volume_slide_down {
                n = (x.volume1 as i16).wrapping_sub(x.volume_slide_speed as i16);
                if (n as i32) < x.volume_slide_to as i32 {
                    n = x.volume_slide_to as i16;
                    x.volume_slide_off = true;
                }
            } else {
                n = (x.volume1 as i16).wrapping_add(x.volume_slide_speed as i16);
                if n as i32 > x.volume_slide_to as i32 {
                    n = x.volume_slide_to as i16;
                    x.volume_slide_off = true;
                }
            }
            vol = n;
        } else if x.volume_slide == FX_VOLUME_UP || (x.volume_slide == FX_PT_VOLUME_UP && !first) {
            self.cyc += 68;
            self.cyc += 29 * (x.volume_slide != FX_VOLUME_UP) as u64;
            vol = (x.volume1 as i16).wrapping_add(x.volume_slide_speed as i16).min(1024);
        } else if x.volume_slide == FX_VOLUME_DOWN || (x.volume_slide == FX_PT_VOLUME_DOWN && !first) {
            self.cyc += 85;
            self.cyc += 50 * (x.volume_slide != FX_VOLUME_DOWN) as u64;
            vol = (x.volume1 as i16).wrapping_sub(x.volume_slide_speed as i16);
            if vol < 0 {
                self.cyc += 4;
                vol = 0;
            }
        } else {
            self.cyc += 71;
            return;
        }
        self.cyc += 30;
        x.volume1 = vol as u16;
        x.volume2 = vol as u16;
        x.volume3 = vol as u16;
    }

    fn slide_channel_volume(&mut self, v: usize) {
        let x = &mut self.voices[v];
        let vol;
        if x.channel_volume_slide == FX_SLIDE_TO_CHANNEL {
            if x.channel_volume_slide_off {
                return;
            }
            let mut n;
            if x.channel_volume_slide_down {
                n = (x.channel_volume as i16).wrapping_sub(x.channel_volume_slide_speed as i16);
                if (n as i32) < x.channel_volume_slide_to as i32 {
                    n = x.channel_volume_slide_to as i16;
                    x.channel_volume_slide_off = true;
                }
            } else {
                n = (x.channel_volume as i16).wrapping_add(x.channel_volume_slide_speed as i16);
                if n as i32 > x.channel_volume_slide_to as i32 {
                    n = x.channel_volume_slide_to as i16;
                    x.channel_volume_slide_off = true;
                }
            }
            vol = n;
        } else if x.channel_volume_slide == FX_CHANNEL_UP {
            self.cyc += 69;
            vol = (x.channel_volume as i16).wrapping_add(x.channel_volume_slide_speed as i16).min(1024);
        } else if x.channel_volume_slide == FX_CHANNEL_DOWN {
            self.cyc += 86;
            vol = (x.channel_volume as i16).wrapping_sub(x.channel_volume_slide_speed as i16).max(0);
        } else {
            self.cyc += 41;
            return;
        }
        x.channel_volume = vol as u16;
    }

    fn slide_master_volume(&mut self, v: usize) {
        let master = self.master;
        let x = &mut self.voices[v];
        let vol;
        if x.master_volume_slide == FX_SLIDE_TO_MASTER {
            if x.master_volume_slide_off {
                return;
            }
            let mut n;
            if x.master_volume_slide_down {
                n = (master as i16).wrapping_sub(x.master_volume_slide_speed as i16);
                if (n as i32) < x.master_volume_slide_to as i32 {
                    n = x.master_volume_slide_to as i16;
                    x.master_volume_slide_off = true;
                }
            } else {
                n = (master as i16).wrapping_add(x.master_volume_slide_speed as i16);
                if n as i32 > x.master_volume_slide_to as i32 {
                    n = x.master_volume_slide_to as i16;
                    x.master_volume_slide_off = true;
                }
            }
            vol = n;
        } else if x.master_volume_slide == FX_MASTER_UP {
            vol = (master as i16).wrapping_add(x.master_volume_slide_speed as i16).min(1024);
        } else if x.master_volume_slide == FX_MASTER_DOWN {
            vol = (master as i16).wrapping_sub(x.master_volume_slide_speed as i16).max(0);
        } else {
            return;
        }
        self.master = vol as u16;
    }

    fn slide_arp_volume(&mut self, v: usize) {
        let x = &mut self.voices[v];
        let vol = match x.arp_volume_slide {
            AE_VOLUME_UP => (x.volume1 as i16).wrapping_add(x.arp_volume_slide_speed as i16).min(1024),
            AE_VOLUME_DOWN => (x.volume1 as i16).wrapping_sub(x.arp_volume_slide_speed as i16).max(0),
            _ => return,
        };
        x.volume1 = vol as u16;
        x.volume2 = vol as u16;
        x.volume3 = vol as u16;
    }

    fn slide_note(&mut self, v: usize) {
        let x = &mut self.voices[v];
        if x.pt_pitch_slide == 0 {
            self.cyc += 37;
            self.cyc += 21 * (x.pitch_slide == 0) as u64;
            if x.pitch_slide == 0 && x.instrument_pitch_slide == 0 {
                return;
            }
            self.cyc += 21;
            if x.pitch_slide_to < 0 {
                return;
            }
            self.cyc += 33;
            if x.pitch_slide_down {
                self.cyc += 61;
                x.pitch_slide_note = x.pitch_slide_note.wrapping_sub(x.pitch_slide_speed);
                let n = x.note.wrapping_add(x.pitch_slide_note).wrapping_sub(x.pitch_slide_to as u16) as i16;
                if n <= 0 {
                    self.cyc += 32;
                    x.pitch_slide_note = x.pitch_slide_note.wrapping_sub(n as u16);
                    x.pitch_slide_to = -1;
                }
            } else {
                self.cyc += 61;
                x.pitch_slide_note = x.pitch_slide_note.wrapping_add(x.pitch_slide_speed);
                let n = x.note.wrapping_add(x.pitch_slide_note).wrapping_sub(x.pitch_slide_to as u16) as i16;
                if n >= 0 {
                    self.cyc += 32;
                    x.pitch_slide_note = x.pitch_slide_note.wrapping_sub(n as u16);
                    x.pitch_slide_to = -1;
                }
            }
        } else if x.play & P_FIRST_ROW_TICK == 0 && x.pt_pitch_slide_to >= 0 {
            self.cyc += 138;
            let speed = if x.pt_pitch_slide == FX_PT_PORTAMENTO { self.cyc += 12; x.pt_pitch_slide_speed2 } else { x.pt_pitch_slide_speed };
            if x.pt_pitch_slide_down {
                x.pt_pitch_slide_note = x.pt_pitch_slide_note.wrapping_add(speed);
                let p = Self::get_period(x, x.note).wrapping_add(x.pt_pitch_slide_note).wrapping_sub(x.pt_pitch_slide_to as u16) as i16;
                self.cyc += 226;
                if p >= 0 {
                    x.pt_pitch_slide_note = x.pt_pitch_slide_note.wrapping_sub(p as u16);
                    x.pt_pitch_slide_to = -1;
                }
            } else {
                x.pt_pitch_slide_note = x.pt_pitch_slide_note.wrapping_sub(speed);
                let p = Self::get_period(x, x.note).wrapping_add(x.pt_pitch_slide_note).wrapping_sub(x.pt_pitch_slide_to as u16) as i16;
                self.cyc += 226;
                if p <= 0 {
                    x.pt_pitch_slide_note = x.pt_pitch_slide_note.wrapping_sub(p as u16);
                    x.pt_pitch_slide_to = -1;
                }
            }
        }
    }

    fn slide_arp_note(&mut self, v: usize) {
        let x = &mut self.voices[v];
        if x.arp_pitch_slide == 0 || x.arp_pitch_slide_to < 0 {
            return;
        }
        self.cyc += 115;
        if x.arp_pitch_down {
            x.arp_pitch_slide_note = x.arp_pitch_slide_note.wrapping_sub(x.arp_pitch_slide_speed);
            let n = x.arp_calc_note.wrapping_add(x.arp_pitch_slide_note).wrapping_sub(x.arp_pitch_slide_to as u16) as i16;
            if n <= 0 {
                x.arp_pitch_slide_note = x.arp_pitch_slide_note.wrapping_sub(n as u16);
                x.arp_pitch_slide_to = -1;
            }
        } else {
            x.arp_pitch_slide_note = x.arp_pitch_slide_note.wrapping_add(x.arp_pitch_slide_speed);
            let n = x.arp_calc_note.wrapping_add(x.arp_pitch_slide_note).wrapping_sub(x.arp_pitch_slide_to as u16) as i16;
            if n >= 0 {
                x.arp_pitch_slide_note = x.arp_pitch_slide_note.wrapping_sub(n as u16);
                x.arp_pitch_slide_to = -1;
            }
        }
    }

    fn arpeggio_effect(&mut self, v: usize, (e, a): (u8, u8)) {
        self.cyc += 8;
        let x = &mut self.voices[v];
        match e {
            AE_SLIDE_UP => {
                self.cyc += 77;
                x.arp_pitch_slide = e;
                x.arp_pitch_down = false;
                if a != 0 {
                    x.arp_pitch_slide_speed = a as u16;
                    x.arp_pitch_slide_to = 59 * 32 + 32;
                }
            }
            AE_SLIDE_DOWN => {
                self.cyc += 77;
                x.arp_pitch_slide = e;
                x.arp_pitch_down = true;
                if a != 0 {
                    x.arp_pitch_slide_speed = a as u16;
                    x.arp_pitch_slide_to = 0;
                }
            }
            AE_SET_VOLUME => {
                self.cyc += 82;
                // no clamp in the original
                let vol = (a as u16) << 4;
                x.restart |= R_ARP_VOLUME;
                x.volume1 = vol;
                x.volume2 = vol;
                x.volume3 = vol;
            }
            AE_VOLUME_UP | AE_VOLUME_DOWN => {
                x.arp_volume_slide = e;
                if a != 0 {
                    x.arp_volume_slide_speed = a as u16;
                }
            }
            _ => {}
        }
    }

    fn arpeggio_restart(&mut self, v: usize) {
        let x = &mut self.voices[v];
        x.restart |= R_ARP;
        if x.arg1 & F_PHASE_INIT != 0 {
            self.cyc += 16;
            x.phase_init = 0;
        }
        // tested on effect 1's flags, as the original does
        if x.arg1 & F_PHASE_STEP != 0 {
            self.cyc += 16;
            x.resonance_init = 0;
        }
        if x.arg1 & F_FILTER_INIT != 0 {
            x.filter_init = 0;
        }
        if x.arg1 & F_TRANSFORM_INIT != 0 {
            x.transform_init = 0;
        }
        if x.arg1 & F_HOLD_SUSTAIN != 0 {
            x.mix_init = 0;
        }
        if x.arg2 & M_LOOP_INIT != 0 {
            x.loop_init = 0;
        }
    }

    fn arpeggio_play(&mut self, hw: &mut Hw, v: usize) {
        let inst = self.voices[v].instrument;
        {
            let x = &mut self.voices[v];
            self.cyc += 27 * (x.arp & A_USE_TABLE == 0) as u64;
            if x.arp & (A_USE_TABLE | A_ENABLED) == 0 {
                return;
            }
            self.cyc += 27;
            x.arp_speed_counter = x.arp_speed_counter.wrapping_sub(1);
            if x.arp_speed_counter != 0 {
                return;
            }
            self.cyc += 161;
            let mut spd = rb(hw, inst + I_ARP_SPEED);
            x.arp_groove = !x.arp_groove;
            let g = rb(hw, inst + I_ARP_GROOVE);
            self.cyc += 21 * x.arp_groove as u64;
            if x.arp_groove && g != 0 {
                spd = g;
            }
            x.arp_speed_counter = spd;
        }
        let x = &self.voices[v];
        let mut num = rw(hw, inst + I_ARP_TABLE) as u32;
        if x.arp & A_USE_TABLE != 0 {
            self.cyc += 12;
            num = x.arp_table as u32;
        }
        let arp = rl(hw, self.tab + T_ARPS + 4 * (num & 0xff));
        loop {
            self.cyc += 176;
            let x = &mut self.voices[v];
            let pos = x.arp_position;
            x.arp_position = x.arp_position.wrapping_add(1) & 0x7f;
            let line = arp + 6 * pos as u32;
            let mut tr = rb(hw, line) as i8;
            let sn = rb(hw, line + 1);
            if x.arp_wait && tr == 0 {
                return;
            }
            if tr == 61 {
                self.cyc += 46;
                x.arp = 0;
                x.note = x.arp_calc_note;
                return;
            }
            self.cyc += 17;
            if tr == 62 {
                self.cyc += 43;
                if sn != pos {
                    x.arp_position = sn;
                }
                continue;
            }
            self.cyc += 124;
            x.wave_number = if sn == 0 { self.cyc += 12; rb(hw, inst + IS_NUMBER) } else { self.cyc += 309; sn };
            x.restart = 0;
            x.arp_pitch_slide = 0;
            x.arp_volume_slide = 0;
            for k in 0..2 {
                self.cyc += 109;
                let e = (rb(hw, line + 2 + 2 * k), rb(hw, line + 3 + 2 * k));
                if e.0 == AE_RESTART {
                    self.arpeggio_restart(v);
                } else {
                    self.arpeggio_effect(v, e);
                }
            }
            let x = &mut self.voices[v];
            x.arp &= !A_NOTE_ABSOLUTE;
            if tr == 0 {
                return;
            }
            if tr < 0 {
                self.cyc += 162;
                tr = tr.wrapping_add(61u8.wrapping_add(x.arp_note) as i8);
            } else {
                x.arp |= A_NOTE_ABSOLUTE;
            }
            x.note = ((tr as i16) << 5) as u16;
            x.arp_calc_note = x.note;
            x.arp_pitch_slide_note = 0;
            let waited = x.arp_wait;
            x.arp_wait = false;
            if waited {
                self.arpeggio_wait_start(hw, v, sn);
                self.play_instrument(hw, v);
                return;
            }
            if x.restart & R_ARP != 0 {
                x.arp |= A_WILL_SET_NOTE;
                if sn != 0 {
                    let s = self.sample(hw, sn);
                    let x = &mut self.voices[v];
                    x.wave_sample = s;
                    x.arp |= A_USE_WAVE_SAMPLE;
                }
                self.play_instrument(hw, v);
                return;
            }
            if sn == 0 {
                self.cyc += 97;
                x.semi_tone = ((rw(hw, inst + IS_SEMI_TONE) as i16) << 5) as i16;
                if x.play & P_KEEP_FINE_TUNE == 0 {
                    x.fine_tune = rw(hw, inst + IS_FINE_TUNE) as i16;
                }
                return;
            }
            x.play |= P_RETRIGGER;
            let s = self.sample(hw, sn);
            let x = &mut self.voices[v];
            x.semi_tone = ((rw(hw, s + IS_SEMI_TONE) as i16) << 5) as i16;
            if x.play & P_KEEP_FINE_TUNE == 0 {
                x.fine_tune = rw(hw, s + IS_FINE_TUNE) as i16;
            }
            x.wave_or_sample = rb(hw, s + IS_TYPE);
            if x.wave_or_sample != SAMPLE {
                let mut st = rb(hw, inst + IS_TYPE);
                if st == SAMPLE {
                    st = WAVE64;
                }
                x.wave_or_sample = st;
                let len = rw(hw, inst + IS_LENGTH);
                Self::fix_wave_length(x, len, st, rl(hw, s + IS_POINTER), &mut self.cyc);
            } else {
                self.cyc += 114;
                x.wave_pointer = rl(hw, s + IS_POINTER);
                x.wave_length = rw(hw, s + IS_LENGTH);
                let mut rp = rl(hw, s + IS_REPEAT);
                let rlen = rw(hw, s + IS_REPEAT_LENGTH);
                let mut rlen = rlen;
                if rlen == 0 {
                    rp = self.data;
                    rlen = 1;
                }
                x.repeat_pointer = rp;
                x.repeat_original = rp;
                x.repeat_length = rlen;
            }
            x.effect2 = if matches!(x.repeat_length, 8 | 16 | 32 | 64 | 128) { rb(hw, inst + I_EFFECT2) } else { 0 };
            return;
        }
    }

    fn envelope_play(&mut self, v: usize) {
        let x = &mut self.voices[v];
        if x.effect1 & E1_ENVELOPE == 0 {
            return;
        }
        self.cyc += 119;
        let set3 = |x: &mut Voice, d: u16| {
            x.volume3 = ((d as u32 * x.volume2 as u32) >> 6).min(1024) as u16;
        };
        let mut i = 0usize;
        let mut d = x.envelope[i];
        i += 1;
        if d == 0 {
            self.cyc += 17;
            d = x.envelope[i];
            i += 1;
            if d == 0 {
                self.cyc += 17;
                d = x.envelope[i];
                i += 1;
                if d == 0 {
                    self.cyc += 25;
                    if x.arg1 & F_HOLD_SUSTAIN != 0 {
                        self.cyc += 22;
                        let s = x.envelope[i + 7];
                        set3(x, s);
                        return;
                    }
                    self.cyc += 17;
                    d = x.envelope[i];
                    i += 1;
                    if d == 0 {
                        self.cyc += 22;
                        let s = x.envelope[i + 7];
                        set3(x, s);
                        return;
                    }
                }
            }
        }
        self.cyc += 79;
        let mut vol = x.envelope_volume.wrapping_add(x.envelope[i + 3]);
        x.envelope_volume = vol;
        vol >>= 8;
        d -= 1;
        i -= 1;
        x.envelope[i] = d;
        if d == 0 {
            self.cyc += 12;
            vol = x.envelope[i + 8];
        }
        set3(x, vol);
    }

    fn tremolo_play(&mut self, v: usize) {
        let x = &mut self.voices[v];
        if x.tremolo == FX_PT_TREMOLO {
            self.cyc += 43;
            if x.play & P_FIRST_ROW_TICK != 0 {
                return;
            }
            self.cyc += 311;
            let wd = Self::pt_wave(x.pt_tremolo_wave, x.pt_tremolo_position);
            let mut t = ((x.pt_tremolo_command & 15) as i32 * wd) >> 2;
            if x.pt_tremolo_position < 0 {
                self.cyc += 27;
                t = -t.abs();
            } else {
                self.cyc += 13;
                t = t.abs();
            }
            let mut vol = x.volume1 as i32;
            if vol != 0 {
                vol = (vol + t).clamp(0, 1024);
            }
            x.volume2 = vol as u16;
            x.volume3 = vol as u16;
            x.pt_tremolo_position = x.pt_tremolo_position.wrapping_add(((x.pt_tremolo_command >> 2) & 0x3c) as i8);
            return;
        }
        self.cyc += 37;
        if x.tremolo == 0 {
            self.cyc += 27;
            if x.effect1 & E1_TREMOLO == 0 {
                return;
            }
            self.cyc += 21;
            if x.tremolo_delay != 0 {
                self.cyc += 16;
                x.tremolo_delay -= 1;
                return;
            }
        }
        self.cyc += 338;
        if x.tremolo_attack_length == 0 {
            self.cyc += 30;
            x.tremolo_depth_now = x.tremolo_depth;
        } else {
            self.cyc += 53;
            x.tremolo_depth_now = x.tremolo_depth_now.wrapping_add(x.tremolo_attack_speed);
            x.tremolo_attack_length -= 1;
            if x.tremolo_attack_length == 0 {
                self.cyc += 20;
                x.tremolo_depth_now = x.tremolo_depth;
            }
        }
        let mut wd = wave_table(x.tremolo_wave)[(x.tremolo_counter >> 2) as usize & 0x7f] as i32;
        if x.tremolo_down {
            wd = -(wd as i8 as i32);
            wd = wd as i8 as i32;
        }
        let mut t = ((wd * (x.tremolo_depth_now >> 8) as i32) >> 1) as i16;
        if t < 0 {
            self.cyc += 8;
            t = t.wrapping_add(16);
        }
        let mut vol = x.volume1 as i32;
        if x.volume1 != 0 {
            self.cyc += 34;
            vol += t as i32;
            // (measured: the volume held at its top)
            self.cyc += 8 * (vol > 1024) as u64;
            vol = vol.clamp(0, 1024);
        }
        x.volume2 = vol as u16;
        x.volume3 = vol as u16;
        x.tremolo_counter = x.tremolo_counter.wrapping_add(x.tremolo_speed) & 0x1ff;
    }

    fn pt_wave(wave: u8, pos: i8) -> i32 {
        let p = ((pos >> 2) & 0x1f) as u8;
        match wave {
            0 => PT_VIBRATO[p as usize] as i32,
            1 => {
                let p = p << 3;
                if pos < 0 {
                    (255 - p) as i32
                } else {
                    p as i32
                }
            }
            _ => 255,
        }
    }

    fn vibrato_play(&mut self, v: usize) {
        let x = &mut self.voices[v];
        if x.vibrato == FX_PT_VIBRATO {
            if x.play & P_FIRST_ROW_TICK != 0 {
                return;
            }
            let wd = Self::pt_wave(x.pt_vibrato_wave, x.pt_vibrato_position);
            let mut t = ((x.pt_vibrato_command & 15) as i32 * wd) >> 7;
            if x.pt_vibrato_position < 0 {
                t = -t.abs();
            } else {
                t = t.abs();
            }
            x.pt_vibrato_note = t as u16;
            x.pt_vibrato_position = x.pt_vibrato_position.wrapping_add(((x.pt_vibrato_command >> 2) & 0x3c) as i8);
            return;
        }
        if x.vibrato == 0 {
            self.cyc += 21;
            if x.effect1 & E1_VIBRATO == 0 {
                return;
            }
            self.cyc += 21;
            if x.vibrato_delay != 0 {
                self.cyc += 16;
                x.vibrato_delay -= 1;
                return;
            }
        }
        self.cyc += 311;
        if x.vibrato_attack_length == 0 {
            self.cyc += 30;
            x.vibrato_depth_now = x.vibrato_depth;
        } else {
            self.cyc += 53;
            x.vibrato_depth_now = x.vibrato_depth_now.wrapping_add(x.vibrato_attack_speed);
            x.vibrato_attack_length -= 1;
            if x.vibrato_attack_length == 0 {
                self.cyc += 20;
                x.vibrato_depth_now = x.vibrato_depth;
            }
        }
        let mut wd = wave_table(x.vibrato_wave)[(x.vibrato_counter >> 2) as usize & 0x7f] as i32;
        if x.vibrato_down {
            self.cyc += 4;
            wd = (-wd) as i8 as i32;
        }
        let mut n = ((wd * (x.vibrato_depth_now >> 8) as i32) >> 4) as i16;
        if n < 0 {
            self.cyc += 4;
            n = n.wrapping_add(1);
        }
        x.vibrato_note = n as u16;
        x.vibrato_counter = x.vibrato_counter.wrapping_add(x.vibrato_speed) & 0x1ff;
    }

    // ---- the waveform effects ----

    fn move_loop(&mut self, hw: &mut Hw, v: usize) {
        let eight = self.eight;
        let data = self.data;
        let x = &mut self.voices[v];
        if x.effect1 & E1_LOOP == 0 {
            return;
        }
        self.cyc += 25;
        let step = x.arg2 & M_LOOP_STEP != 0;
        if !step {
            self.cyc += 64;
            x.loop_delay = x.loop_delay.wrapping_sub(1);
            if x.loop_delay >= 0 {
                self.cyc += 39;
                let p = x.loop_sample.wrapping_add(2 * x.loop_counter_save as u32);
                x.repeat_pointer = p;
                x.repeat_original = p;
                return;
            }
            x.loop_delay = 0;
            if x.loop_wait != 0 {
                self.cyc += 27;
                x.loop_wait_counter = x.loop_wait_counter.wrapping_sub(1);
                if x.loop_wait_counter >= 0 {
                    return;
                }
                self.cyc += 30;
                x.loop_wait_counter = x.loop_wait as i16;
            }
            self.cyc += 202;
            x.loop_counter_save = x.loop_counter;
            Self::loop_count(x, &mut self.cyc);
        } else if x.arg2 & M_LOOP_INIT == 0 {
            self.cyc += 300;
            if x.part_note != 0 {
                x.loop_counter_save = x.loop_counter;
                Self::loop_count(x, &mut self.cyc);
            }
        } else {
            if x.part_note != 0 {
                x.loop_counter_save = x.loop_counter;
            }
            Self::loop_count(x, &mut self.cyc);
        }
        self.cyc += 122;
        let prev = x.loop_counter_save;
        if step {
            x.loop_delay = x.loop_delay.wrapping_sub(1);
            if x.loop_delay < 0 {
                x.loop_delay = 0;
                let mut go = true;
                if x.loop_wait != 0 {
                    x.loop_wait_counter = x.loop_wait_counter.wrapping_sub(1);
                    if x.loop_wait_counter >= 0 {
                        go = false;
                    } else {
                        x.loop_wait_counter = x.loop_wait as i16;
                    }
                }
                if go {
                    // (measured: the step's sign and a counter already at its end)
                    self.cyc = self.cyc + 4 * (x.loop_step < 0) as u64 + 4 * (x.loop_counter_save == x.loop_counter_max) as u64 - 4;
                    let st = x.loop_step.abs();
                    let c = x.loop_counter_save as i32 + (x.loop_speed as i8 as i32) * st;
                    if (x.loop_speed as i8) < 0 {
                        x.loop_counter_save = if c < 0 { 0 } else { c as u16 };
                    } else {
                        x.loop_counter_save = if c > x.loop_counter_max as i32 { x.loop_counter_max } else { c as u16 };
                    }
                }
            }
        }
        let p = x.loop_sample.wrapping_add(2 * prev as u32);
        x.repeat_pointer = p;
        x.repeat_original = p;
        if x.loop_turns >= 0 || x.effect1 & E1_LOOP_STOP == 0 {
            return;
        }
        self.cyc += 179;
        x.effect1 &= !E1_LOOP;
        x.mix_loop = false;
        if !eight {
            hw.loc(v & 3, data);
            hw.len(v & 3, 1);
        }
    }

    fn loop_count(x: &mut Voice, cyc: &mut u64) {
        if x.loop_turns < 0 {
            return;
        }
        let mut c = x.loop_counter as i32;
        c += x.loop_step;
        let (lo, hi) = if x.loop_repeat < x.loop_repeat_end {
            (x.loop_repeat, x.loop_repeat_end)
        } else {
            *cyc += 10;
            (x.loop_repeat_end, x.loop_repeat)
        };
        if x.loop_step < 0 {
            if c >= lo as i32 {
                x.loop_counter = c as u16;
                return;
            }
        } else if c <= hi as i32 {
            x.loop_counter = c as u16;
            return;
        }
        *cyc += 73;
        if x.loop_turns != 0 {
            x.loop_turns -= 1;
            if x.loop_turns == 0 {
                x.loop_turns = -1;
            }
        }
        c -= x.loop_step;
        x.loop_step = -x.loop_step;
        x.loop_counter = c as u16;
    }

    /// The counter step of an effect: whether it counts, then its saved value.
    fn effect_counter(c: &mut Counter, step: bool, init: bool, part_note: u8, one_way: bool, speed: u16, phase: bool, cyc: &mut u64) -> u16 {
        let count = if step {
            *cyc += 21;
            if !init {
                if part_note == 0 {
                    false
                } else {
                    *cyc += 30;
                    c.save = c.counter;
                    true
                }
            } else {
                if part_note != 0 {
                    *cyc += 20;
                    c.save = c.counter;
                }
                true
            }
        } else {
            *cyc += 20;
            c.save = c.counter;
            true
        };
        if count {
            if one_way {
                c.one_way(cyc);
            } else {
                *cyc += 18;
                c.count(cyc);
            }
        }
        let prev = c.save;
        if step {
            *cyc += 46;
            if c.delay != 0 {
                *cyc += 26;
                c.delay -= 1;
            } else {
                *cyc += 25;
                let s = speed as i8 as i16;
                if phase {
                    *cyc += 41;
                    c.save = c.save.wrapping_sub(s as u16);
                    if s < 0 {
                        if c.save as i16 > 512 {
                            *cyc += 16;
                            c.save = 512;
                        }
                    } else if (c.save as i16) < 2 {
                        c.save = 2;
                    }
                } else {
                    c.save = c.save.wrapping_add(s as u16);
                    if s < 0 {
                        *cyc += 25;
                        if (c.save as i16) < 0 {
                            *cyc += 26;
                            c.save = 0;
                        }
                    } else {
                        *cyc += 41;
                        if c.save > 510 {
                            *cyc += 16;
                            c.save = 510;
                        }
                    }
                }
            }
        }
        prev
    }

    fn set_effect_output(x: &mut Voice, buf: u32, with_loop: bool) {
        x.repeat_pointer = buf;
        if (with_loop && x.effect1 & E1_LOOP != 0) || x.wave_or_sample != SAMPLE {
            x.wave_pointer = buf;
        }
    }

    fn transform_play(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].effect2 & E2_TRANSFORM == 0 {
            return;
        }
        self.cyc += 459;
        let x = &mut self.voices[v];
        let len = x.repeat_length as u32 * 2;
        let off = match len {
            256 => 0,
            128 => 256,
            64 => { self.cyc += 10; 256 + 128 },
            32 => { self.cyc += 25; 256 + 128 + 64 },
            _ => { self.cyc += 33; 256 + 128 + 64 + 32 },
        };
        // (the tests of the length, one after the other)
        self.cyc += match len { 256 => 0, 128 => 25, 64 => 40, _ => 50 };
        let (s, i) = (x.arg1 & F_TRANSFORM_STEP != 0, x.arg1 & F_TRANSFORM_INIT != 0);
        let mut prev = Self::effect_counter(&mut x.transform_counter, s, i, x.part_note, false, x.transform_speed, false, &mut self.cyc);
        prev >>= 1;
        let mut octave = 0usize;
        let mut rem = prev as i16;
        for _ in 0..5 {
            rem = rem.wrapping_sub(256);
            if rem > 0 {
                self.cyc += 21;
                octave += 1;
                prev = rem as u16;
            } else {
                break;
            }
        }
        let factor = prev as i32;
        let sn = x.transform_waves[octave];
        if sn == 0 {
            return;
        }
        let mut src1 = rl(hw, self.sample(hw, sn) + IS_REPEAT).wrapping_add(off);
        let x = &self.voices[v];
        if octave == 0 {
            self.cyc += 16;
            src1 = x.repeat_pointer;
        }
        let sn2 = x.transform_waves[octave + 1];
        if sn2 == 0 {
            return;
        }
        let src2 = rl(hw, self.sample(hw, sn2) + IS_REPEAT).wrapping_add(off);
        let x = &mut self.voices[v];
        let buf = x.bufs + B_TRANSFORM;
        Self::set_effect_output(x, buf, true);
        self.cyc += 21 * (x.effect1 & E1_LOOP == 0 && x.wave_or_sample != SAMPLE) as u64;
        for k in 0..len {
            self.cyc += 126;
            let s1 = rs8(hw, src1.wrapping_add(k));
            let s2 = rs8(hw, src2.wrapping_add(k));
            hw.bus.w8(buf + k, ((((s2 - s1) * factor) >> 8) + s1) as u8);
        }
    }

    fn phase_mix(ptype: u16, s1: i32, s2: i32) -> i32 {
        match ptype {
            1 => ((s1 + s2) + s1 * 2) >> 2,
            2 => (s1 + s2) >> 1,
            3 => ((s1 + s2) + s2 * 2) >> 2,
            _ => s1,
        }
    }

    fn phase_play(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].effect2 & E2_PHASE == 0 {
            self.cyc += 16;
            return;
        }
        self.cyc += 230;
        let x = &mut self.voices[v];
        let (s, i) = (x.arg1 & F_PHASE_STEP != 0, x.arg1 & F_PHASE_INIT != 0);
        let mut prev = Self::effect_counter(&mut x.phase_counter, s, i, x.part_note, false, x.phase_speed, true, &mut self.cyc);
        let len = x.repeat_length as u32 * 2;
        let n: u32 = if len > 128 {
            self.cyc += 46;
            256
        } else if len > 64 {
            self.cyc += 48;
            128
        } else if len > 32 {
            self.cyc += 50;
            64
        } else if len > 16 {
            self.cyc += 73;
            32
        } else {
            self.cyc += 65;
            16
        };
        // (the tests of the length, one after the other)
        self.cyc += 17 * ((n < 256) as u64 + (n < 128) as u64);
        let sh = 512 / n;
        prev = prev.wrapping_add(sh as u16 - 1) >> sh.trailing_zeros();
        let src = x.repeat_pointer;
        let buf = x.bufs + B_PHASE;
        Self::set_effect_output(x, buf, true);
        self.cyc += 21 * (x.effect1 & E1_LOOP == 0 && x.wave_or_sample != SAMPLE) as u64;
        let fill = x.arg1 & F_PHASE_FILL != 0;
        let ptype = x.phase_type;
        let prev = prev as u32;
        if prev >= len || prev == 0 {
            self.cyc += 20;
        } else {
            self.cyc += 92;
            // the type's tests and pass, the interpolated bytes, then the rest's set-up, passes and bytes
            let t = [0, 1, 2, 3, 0][(ptype as usize).min(4)];
            let left = (len - prev) as u64;
            self.cyc += [52, 62, 89, 37][t] + [36, 78, 68, 78][t] * prev as u64;
            if t != 0 && fill {
                self.cyc += 33 + 17 * left.div_ceil(prev as u64) + [0, 82, 0, 82][t] * left;
            } else if t != 0 {
                self.cyc += [0, 59, 47, 47][t] + [0, 0, 42, 56][t] * left;
            }
        }
        let rd = |hw: &Hw, k: u32| rs8(hw, src.wrapping_add(k));
        if prev >= len || prev == 0 {
            for k in 0..len {
                self.cyc += 22;
                let b = rb(hw, src.wrapping_add(k));
                hw.bus.w8(buf + k, b);
            }
            return;
        }
        let todo = prev;
        let mut left = len - prev;
        let so = (prev - 1) * prev / 2;
        let sizer = |k: u32| (((k + 1) * n) / prev - 1) as u32;
        let mut o = 0u32;
        let mut w = |hw: &mut Hw, b: i32| {
            hw.bus.w8(buf + o, b as u8);
            o += 1;
        };
        let mut idx = 0u32;
        let mut rc = 0u32;
        for k in 0..todo {
            idx = sizer(k);
            let s1 = rd(hw, idx);
            let s2 = rd(hw, rc);
            rc += 1;
            w(hw, Self::phase_mix(ptype, s1, s2) as i8 as i32);
        }
        if ptype == 0 {
            self.cyc += 33;
            if fill {
                self.cyc += 33;
                // the start of the phase buffer again
                for k in 0..left {
                    self.cyc += 22;
                    let b = rb(hw, buf + k);
                    w(hw, b as i32);
                }
            } else if left > 0 {
                self.cyc += 43;
                let s = rd(hw, idx);
                for _ in 0..left {
                    self.cyc += 18;
                    w(hw, s);
                }
            }
            return;
        }
        if fill {
            // High and Low keep stepping through the sizer table across the passes
            // (Med restarts its row each pass)
            let mut fk = 0u32;
            while left > 0 {
                let c = prev.min(left);
                for k in 0..c {
                    let i = if ptype == 1 || ptype == 3 {
                        let t = so + fk;
                        fk += 1;
                        // the table is the rows of 1, 2, 3 ... entries one after the other
                        let mut r = (((8 * t + 1) as f64).sqrt() as u32 + 1) / 2;
                        while r * (r - 1) / 2 > t {
                            r -= 1;
                        }
                        while (r + 1) * r / 2 <= t {
                            r += 1;
                        }
                        (((t - r * (r - 1) / 2 + 1) * n) / r).wrapping_sub(1)
                    } else {
                        self.cyc += 72;
                        sizer(k)
                    };
                    let s1 = rd(hw, i);
                    let s2 = rd(hw, rc);
                    rc += 1;
                    w(hw, Self::phase_mix(ptype, s1, s2) as i8 as i32);
                }
                left -= c;
            }
        } else if left > 0 {
            let s1 = rd(hw, idx);
            for _ in 0..left {
                let s2 = rd(hw, rc);
                rc += 1;
                let out = match ptype {
                    1 => { self.cyc += 44; ((s1 + s1 * 2) as i16 as i32 + s2) >> 2 },
                    _ => Self::phase_mix(ptype, s1, s2),
                };
                w(hw, out as i8 as i32);
            }
        }
    }

    fn mix_play(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].effect2 & E2_MIX == 0 {
            return;
        }
        self.cyc += 302;
        let x = &mut self.voices[v];
        let (s, i) = (x.arg2 & M_MIX_STEP != 0, x.arg2 & M_MIX_INIT != 0);
        let one_way = x.arg2 & M_MIX_COUNTER != 0;
        // (the counter's kind is tested when it counts; the mixing loop takes 60 cycles a byte)
        let counts = !(s && !i && x.part_note == 0);
        let mut prev = Self::effect_counter(&mut x.mix_counter, s, i, x.part_note, one_way, x.mix_speed, false, &mut self.cyc) as u32;
        let mut len = x.repeat_length as u32 * 2;
        self.cyc += 25 * counts as u64 + 60 * len as u64 + 16;
        if s && !i {
            // (the mix has its own way through the counter's start)
            if x.part_note == 0 {
                self.cyc -= 21;
            } else {
                self.cyc += 170;
            }
        }
        let off;
        if len > 128 {
            self.cyc += 22;
            prev >>= 1;
            off = 0;
        } else if len > 64 {
            self.cyc += 32;
            prev >>= 2;
            off = 256;
        } else if len > 32 {
            self.cyc += 34;
            prev >>= 3;
            off = 256 + 128;
        } else if len > 16 {
            self.cyc += 53;
            prev >>= 4;
            off = 256 + 128 + 64;
        } else {
            self.cyc += 45;
            prev >>= 5;
            off = 256 + 128 + 64 + 32;
        }
        self.cyc += 17 * ((len <= 128) as u64 + (len <= 64) as u64);
        if x.boost & B_MIX != 0 {
            self.cyc += 4;
        }
        let src = x.repeat_pointer;
        let dest = x.bufs + B_MIXB;
        let mut mixb = dest;
        if x.arg2 & M_MIX_BUFF == 0 {
            self.cyc += 29;
            mixb = src;
            if x.mix_wave != 0 {
                self.cyc += 66;
                let s = rl(hw, self.tab + T_SAMPLES + 4 * x.mix_wave as u32);
                mixb = rl(hw, s + IS_REPEAT).wrapping_add(off);
            }
        }
        let x = &mut self.voices[v];
        let src2 = mixb;
        x.repeat_pointer = dest;
        if x.wave_or_sample != SAMPLE {
            self.cyc += 16;
            x.wave_pointer = dest;
        }
        let mixb = mixb.wrapping_add(prev);
        self.cyc += 8 * (prev.min(len) > 0) as u64;
        len -= prev.min(len);
        let boost = if x.boost & B_MIX != 0 { 0 } else { 1 };
        for k in 0..len {
            let a = rs8(hw, src.wrapping_add(k));
            let b = rs8(hw, mixb.wrapping_add(k));
            hw.bus.w8(dest + k, ((a + b) >> boost) as u8);
        }
        for k in 0..prev {
            let a = rs8(hw, src.wrapping_add(len + k));
            let b = rs8(hw, src2.wrapping_add(k));
            hw.bus.w8(dest + len + k, ((a + b) >> boost) as u8);
        }
    }

    fn resonance_play(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].effect2 & E2_RESONANCE == 0 {
            return;
        }
        self.cyc += 444;
        let x = &mut self.voices[v];
        let (s, i) = (x.arg2 & M_RESONANCE_STEP != 0, x.arg2 & M_RESONANCE_INIT != 0);
        let prev = Self::effect_counter(&mut x.resonance_counter, s, i, x.part_note, false, x.resonance_speed, false, &mut self.cyc);
        let src = x.repeat_pointer;
        let buf = x.bufs + B_RES;
        Self::set_effect_output(x, buf, true);
        self.cyc += 21 * (x.effect1 & E1_LOOP == 0 && x.wave_or_sample != SAMPLE) as u64;
        let len = x.repeat_length as u32 * 2;
        let mut last = x.resonance_last;
        if x.resonance_last_init {
            self.cyc += 40;
            x.resonance_last_init = false;
            last = (rb(hw, src.wrapping_add(len).wrapping_sub(1)) as i8) >> 2;
        }
        let mut state = (last as i16) << 7;
        let cutoff = RESONANCE_LIST[((prev & 0xfffe) / 2) as usize & 0xff] as i32;
        let mut damp: u16 = 0x8000u16.wrapping_sub(RESAMPLE_LIST[x.resonance_amp as usize]);
        damp = ((damp as u32 * 0xe666) >> 16) as u16;
        let mut vel: i16 = 0;
        let mut out = last;
        let boost = if x.boost & B_RESONANCE != 0 { self.cyc += 4; 6 } else { 7 };
        for k in 0..len {
            self.cyc += 310;
            // the original subtracts in 16 bits, and its divs.w leaves the
            // dividend as it was when the quotient overflows 16 bits
            let diff = ((rs8(hw, src.wrapping_add(k)) << 5) as i16).wrapping_sub(state) as i32;
            let dividend = diff << 7;
            let q = dividend / cutoff.max(1);
            let acc = if q == q as i16 as i32 { q as i16 } else { dividend as i16 };
            vel = vel.wrapping_add(acc);
            state = state.wrapping_add(vel);
            out = (state >> boost) as i8;
            hw.bus.w8(buf + k, out as u8);
            vel = ((vel as i32 * damp as i16 as i32) >> 15) as i16;
        }
        self.voices[v].resonance_last = out;
    }

    fn filter_play(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].effect2 & E2_FILTER == 0 {
            self.cyc += 16;
            return;
        }
        self.cyc += 229;
        let x = &mut self.voices[v];
        let (s, i) = (x.arg1 & F_FILTER_STEP != 0, x.arg1 & F_FILTER_INIT != 0);
        let prev = Self::effect_counter(&mut x.filter_counter, s, i, x.part_note, false, x.filter_speed, false, &mut self.cyc);
        let src = x.repeat_pointer;
        let buf = x.bufs + B_FILT;
        self.cyc += x.repeat_length as u64 * 2 * [218, 220][(x.filter_type == 0) as usize] + 218 + 4 * (x.boost & B_FILTER != 0) as u64;
        Self::set_effect_output(x, buf, true);
        self.cyc += 21 * (x.effect1 & E1_LOOP == 0 && x.wave_or_sample != SAMPLE) as u64;
        let len = x.repeat_length as u32 * 2;
        let mut last = x.filter_last;
        let normal = x.filter_type == 0;
        if x.filter_last_init {
            x.filter_last_init = false;
            let b = rb(hw, src.wrapping_add(len).wrapping_sub(1)) as i8;
            last = if normal { self.cyc += 30; b } else { self.cyc += 38; b >> 1 };
        }
        let mut state = (last as i16) << 7;
        let idx = ((prev & 0xfffe) / 2) as usize & 0xff;
        let mut cutoff = if normal { FILTER_LIST[idx] } else { RESONANCE_FILTER_LIST[idx] };
        let mut damp = 0x8000u16.wrapping_sub(cutoff);
        cutoff >>= 1;
        damp = if normal {
            ((damp as i16 as i32 * 0xf000u16 as i16 as i32) >> 16) as u16
        } else {
            ((damp as u32 * 0xe666) >> 16) as u16
        };
        let mut vel: i16 = 0;
        let mut out = last;
        let boost = if x.boost & B_FILTER != 0 { 6 } else { 7 };
        let sh = if normal { 7 } else { 6 };
        for k in 0..len {
            let acc = (((rs8(hw, src.wrapping_add(k)) << sh) - state as i32) * cutoff as i32) >> 14;
            vel = vel.wrapping_add(acc as i16);
            state = state.wrapping_add(vel);
            out = (state >> boost) as i8;
            hw.bus.w8(buf + k, out as u8);
            vel = ((vel as i32 * damp as i16 as i32) >> 15) as i16;
        }
        self.voices[v].filter_last = out;
    }

    // ---- the 8-voice mixer ----

    fn mix_row(&self, v: usize) -> u32 {
        let x = &self.voices[v];
        if x.muted {
            return 0;
        }
        let w = ((x.volume3 as u32 * x.channel_volume as u32) << 6) >> 16 & 0xffff;
        ((w * self.master as u32) >> 6) & 0xff00
    }

    /// The bytes left of the voice's sample, as mix-rate bytes.
    fn mix_setup(&mut self, v: usize, len: u16, add_frac: bool) {
        let x = &mut self.voices[v];
        x.mix_len = len;
        let bytes = 2 * len as u32;
        let left = bytes.wrapping_sub(if add_frac { x.mix_pos } else { 0 });
        let ratio = ((x.period as u32) << 8) / 0x7e & 0xffff;
        let mut d0 = (left & 0xffff) * ratio;
        if add_frac {
            d0 = d0.wrapping_add(x.mix_frac_acc);
        }
        x.mix_frac_acc = (x.mix_frac_acc & !0xff) | (d0 & 0xff);
        d0 >>= 8;
        if left >> 16 != 0 {
            d0 = d0.wrapping_add(ratio << 8);
        }
        x.mix_count = d0;
    }

    fn mix_step(&mut self, v: usize) {
        let x = &mut self.voices[v];
        let step = ((0x7e * 0x8000u32) / x.period as u32) & 0xffff;
        x.mix_step = step * 2;
    }

    /// PeriodVolumePlay for 8 voices: mixes the voices that go on playing.
    fn mix_voice(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].play & P_RETRIGGER != 0 {
            return;
        }
        let ch = v & 3;
        if self.voices[v].mix_done {
            if !self.chan_written[ch] {
                let to = self.mix[0] + 0x1400 * ch as u32 + self.half as u32;
                for k in 0..self.mix_length as u32 {
                    hw.bus.w8(to + k, 0);
                }
            }
            return;
        }
        self.voices[v].mix_table = self.mix_row(v);
        if self.voices[v].period == 0 {
            return;
        }
        self.mix_step(v);
        let len = self.voices[v].mix_len;
        self.mix_setup(v, len, true);
        self.mix_run(hw, v);
    }

    /// The DMA routine for 8 voices: the restarted voices start mixing.
    fn mix_retrigger(&mut self, hw: &mut Hw, v: usize) {
        let x = &mut self.voices[v];
        if x.play & P_RETRIGGER == 0 {
            return;
        }
        x.play &= !P_RETRIGGER;
        let row = self.mix_row(v);
        let x = &mut self.voices[v];
        x.mix_table = row;
        x.mix_sample = x.wave_pointer;
        x.mix_pos = 0;
        x.mix_frac_acc = 0;
        x.mix_frac = 0;
        x.mix_done = false;
        if x.period == 0 {
            return;
        }
        self.mix_step(v);
        let len = self.voices[v].wave_length;
        self.mix_setup(v, len, false);
        let x = &mut self.voices[v];
        x.mix_loop = x.effect1 & (E1_LOOP | E1_WAVE_SAMPLE_LOOP) != 0;
        self.mix_run(hw, v);
    }

    fn mix_run(&mut self, hw: &mut Hw, v: usize) {
        let ch = v & 3;
        let first = !self.chan_written[ch];
        self.chan_written[ch] = true;
        let mut to = self.mix[0] + 0x1400 * ch as u32 + self.half as u32;
        let mut left = self.mix_length as u32;
        let x = &mut self.voices[v];
        let int = x.mix_step >> 16;
        let frac = x.mix_step as u16;
        let table = x.mix_table as usize;
        let mut pos = x.mix_pos;
        let mut acc = x.mix_frac;
        let put = |hw: &mut Hw, x: &Voice, pos: u32, to: &mut u32| {
            let s = hw.bus.r8(x.mix_sample.wrapping_add(pos)) as usize;
            let b = self.vol_table.get(table + s).copied().unwrap_or(0);
            if first {
                hw.bus.w8(*to, b);
            } else {
                let o = hw.bus.r8(*to);
                hw.bus.w8(*to, o.wrapping_add(b));
            }
            *to += 1;
        };
        loop {
            let end = 2 * x.mix_len as u32;
            let count = x.mix_count;
            // the samples until the end, or the buffer's
            let n = if count == 0 { 0 } else { count.min(left) };
            let mut k = 0;
            let mut ended = count == 0 || count < left;
            while k < n {
                if pos >= end {
                    ended = true;
                    break;
                }
                let (a, c) = acc.overflowing_add(frac);
                acc = a;
                put(hw, x, pos, &mut to);
                pos = pos.wrapping_add(int + c as u32);
                k += 1;
                left -= 1;
            }
            if !ended && left == 0 {
                x.mix_pos = pos;
                x.mix_frac = acc;
                return;
            }
            if left == 0 && !ended {
                break;
            }
            if !x.mix_loop {
                if first {
                    for _ in 0..left {
                        hw.bus.w8(to, 0);
                        to += 1;
                    }
                }
                x.mix_done = true;
                x.mix_pos = pos;
                x.mix_frac = acc;
                return;
            }
            // the loop
            x.mix_sample = x.repeat_pointer;
            pos = 0;
            acc = 0;
            let len = x.repeat_length;
            x.mix_len = len;
            let bytes = 2 * len as u32;
            let ratio = ((x.period as u32) << 8) / 0x7e & 0xffff;
            let mut d0 = (bytes & 0xffff) * ratio;
            d0 = d0.wrapping_add(x.mix_frac_acc);
            x.mix_frac_acc = (x.mix_frac_acc & !0xff) | (d0 & 0xff);
            d0 >>= 8;
            if bytes >> 16 != 0 {
                d0 = d0.wrapping_add(ratio << 8);
            }
            x.mix_count = d0;
            if left == 0 {
                x.mix_pos = pos;
                x.mix_frac = acc;
                return;
            }
        }
        x.mix_pos = pos;
        x.mix_frac = acc;
    }
}

fn wave_table(w: u8) -> &'static [i8; 128] {
    match w & 3 {
        0 => &SINE,
        1 => &DOWN_RAMP,
        2 => &SAWTOOTH,
        _ => &SQUARE,
    }
}

// ---- 8 voices: the time of the pre-roll row ----
//
// The 68000 cycles the original's InitSound runs from its last register write to the host call that
// ends its stretch: the row of an untouched player (every voice's first sequence entries and first
// part line). All of it was measured on the Eagleplayer with rows made for the purpose, one change at
// a time (the cycle count of the stretch is exact, a colour clock being 16 cycles there): the costs
// add up, do not depend on the voice except where said, and are given as the difference from a voice
// that reads an empty line.

/// A row of eight empty lines; less for each voice when the tune has a groove.
const ROW_EMPTY: i64 = 15_188;
const ROW_GROOVE_VOICE: i64 = -21;
/// Sequence entries: the voice is switched off; it waits (a count, and a new speed with it); a wait of
/// zero and a jump not taken go on to the next entry, as a jump taken does (to entry 0, or 20 cycles
/// less to another; the first time, and each time its counter is not used up); an entry with the
/// command bit and no command plays its part; and the voice's last entry is 8 cycles cheaper when a
/// jump's counter is still running.
const SEQ_OFF: i64 = -1361;
const SEQ_WAIT: i64 = -1174;
const SEQ_WAIT_SPEED: i64 = 34;
const SEQ_WAIT_NONE: i64 = 278;
const SEQ_JUMP_PAST: i64 = 253;
const SEQ_JUMP_FIRST: i64 = 302;
const SEQ_JUMP_AGAIN: i64 = 281;
const SEQ_JUMP_ELSEWHERE: i64 = -20;
const SEQ_FLAGGED: i64 = 81;
const SEQ_JUMP_RUNNING: i64 = -8;
/// Jumps that never reach a part: the voice gives up after 256 entries (when the last one jumped to
/// entry 0, without the 20 cycles of that).
const SEQ_NO_PART: i64 = -1552;
/// Part lines: a jump line that does not jump, the part's end on its first line (EndSound's writes),
/// a note without an instrument, an instrument without a note, both (a sample without a loop and
/// with no effects), both where the instrument has no sample.
const LINE_JUMP: i64 = 295;
const LINE_END: i64 = -566;
const LINE_NOTE: i64 = 79;
const LINE_INSTRUMENT: i64 = 474;
const LINE_INSTRUMENT_SLIDE: i64 = 25;
const LINE_START: i64 = 1873;
const LINE_NO_SAMPLE: i64 = 502;
/// A note with a portamento ($03, $E3) starts nothing: the line costs this besides the effect; and what
/// the line's one portamento adds to a note without an instrument, or to an instrument without a note
/// (lines with two portamentos were not measured).
const LINE_PORTAMENTO: i64 = 462;
const LINE_PT_PORTAMENTO: i64 = 669;
const LINE_NOTE_PORTAMENTO: i64 = 7;
const LINE_NOTE_PT_PORTAMENTO: i64 = 214;
const LINE_INSTRUMENT_PT_PORTAMENTO: i64 = -98;
/// Part effects with a fixed cost: the code, with a zero argument, with another.
const ROW_FX: [(u8, i64, i64); 47] = [
    (0x01, 65, 77), (0x02, 65, 77), (0x03, 123, 135), (0x04, 16, 16), (0x05, 21, 37), (0x06, 21, 37), (0x07, 21, 33),
    (0x08, 88, 122), (0x09, 78, 112), (0x10, 50, 50), (0x11, 33, 45), (0x12, 33, 45), (0x13, 38, 38), (0x14, 98, 110),
    (0x15, 33, 59), (0x16, 33, 63), (0x17, 21, 33), (0x18, 88, 122), (0x19, 78, 112), (0x20, 38, 38), (0x21, 33, 45),
    (0x22, 33, 45), (0x23, 38, 38), (0x24, 114, 126), (0x27, 239, 239), (0x30, 38, 38),
    (0x31, 33, 45), (0x32, 33, 45), (0x33, 38, 38), (0x34, 102, 114), (0x44, 36, 36),
    (0x45, 36, 36), (0x46, 57, 77), (0x47, 41, 41), (0x48, 33, 45), (0x49, 37, 37), (0x4a, 21, 112), (0x4b, 96, 96),
    (0xe1, 65, 77), (0xe2, 65, 77), (0xe3, 318, 330), (0xe4, 17, 41), (0xe5, 17, 41), (0xe6, 43, 55), (0xe7, 43, 55),
    (0xe9, 28, 28), (0xeb, 28, 28),
];
/// The channel's and the master volume ($25, $35 up, $26, $36 down, by the voice's last number when the
/// effect has none; both start at 64, $20, $27 and $30 set them, and the master volume is one for all
/// the voices): no argument, an argument, and the clamping at 64 or at zero; and what the slide to a
/// volume ($24, $34) costs less when it is there already (as the port's effects decide it).
const ROW_VOLUME_UP: (i64, i64) = (74, 100);
const ROW_VOLUME_DOWN: (i64, i64) = (58, 84);
const ROW_VOLUME_CLAMP: i64 = 16;
const ROW_VOLUME_SLIDE_THERE: i64 = -16;
/// What a line with a note adds to a vibrato or tremolo effect ($08, $09, $18, $19), what an instrument
/// takes off the restart effect ($49), and a note with an instrument off the fine tune effect ($0B).
const FX_NOTE_VIBRATO: i64 = 92;
const FX_RESTART_INSTRUMENT: i64 = -16;
const FX_FINE_TUNE_START: i64 = -20;
/// The instrument's start: not transposable; the hold-sustain effect ($46); the sample offset effect
/// ($48) on a sample, inside it or past its end; the volume effect ($10); a volume added ($15, $16),
/// and clamped at the top or at zero; a slide speed.
const START_FIXED_NOTE: i64 = 16;
const START_HOLD: i64 = 40;
const START_OFFSET: i64 = 69;
const START_OFFSET_PAST: i64 = 63;
const START_VOLUME: i64 = 12;
const START_VOLUME_ADD: i64 = 38;
const START_VOLUME_TOP: i64 = 8;
const START_VOLUME_ZERO: i64 = 4;
const START_SLIDE: i64 = 148;
/// ... a sample with the loop flag, the wave types 1 to 4 and the others, and the played length when it
/// is one a wave effect works on (8, 16, 32, 64, 128 words; the others pay the clearing of the effects).
const START_SAMPLE_LOOP: i64 = -16;
const START_WAVE: [i64; 4] = [-14, 1, 16, 31];
const START_WAVE_OTHER: i64 = -3;
const START_LENGTH: [(u16, i64); 5] = [(8, -80), (16, -67), (32, -50), (64, -33), (128, -16)];
/// ... the envelope; the vibrato or tremolo (and what a part effect that set one takes off instead);
/// the loop effect: of a wave, of a sample with no loop length, and its set-up.
const START_ENVELOPE: i64 = 152;
const START_VIBRATO: i64 = 172;
const START_VIBRATO_SET: i64 = -25;
const START_LOOP_WAVE: i64 = 51;
const START_LOOP_NONE: i64 = 72;
const START_LOOP: i64 = 609;
const START_LOOP_DOWN: i64 = 24;
/// ... the wave effects' counters, in the order phase, resonance, filter, mix, transform; a counter in
/// step mode, one with its init flag, and one that starts above its repeat value cost more.
const START_COUNTER: [i64; 5] = [261, 277, 277, 261, 329];
const COUNTER_STEP: i64 = 38;
const COUNTER_INIT: i64 = 37;
const LOOP_INIT: i64 = 39;
const COUNTER_DOWN: i64 = 16;
/// The arpeggio's start: through the instrument, or (less) through the table effects ($44, $45); a
/// skipped line; its first line empty (the note waits: the instrument does not start); the table's end;
/// an absolute note; a wave sample (one measurement); the line's two effects: slides (1, 2), volume (3),
/// volume slides (4, 5), with a zero argument and another, and any other code; both of them the volume.
const ARP_START_ROW: i64 = 763;
const ARP_BY_TABLE: i64 = -68;
const ARP_SKIP: i64 = 201;
const ARP_WAITS: i64 = -1129;
const ARP_ENDS: i64 = 389;
const ARP_ABSOLUTE: i64 = 10;
const ARP_SAMPLE: i64 = 91;
const ARP_FX: [(i64, i64); 6] = [(0, 0), (49, 77), (49, 77), (44, 44), (33, 45), (33, 45)];
const ARP_FX_OTHER: i64 = -68;
const ARP_VOLUME_TWICE: i64 = 38;
/// A tune that ends in its first row (voice 0 switched off): EndSound's DMA-off write comes this many
/// cycles before the row's end, the interrupt disable and the acknowledge 20 cycles apart after it, and
/// the stretch ends `ROW_END_SOUND` after the row.
const ROW_END_WRITE: i64 = -53;
const ROW_END_STEP: i64 = 20;
const ROW_END_SOUND: i64 = 518;

/// What the row's effects have set so far, as far as the time depends on it.
struct RowState {
    groove: [u8; 8],
    speed_part: [bool; 8],
    groove_part: [bool; 8],
    part_groove: [bool; 8],
    volume: [u16; 8],
    master: u16,
    add_number: [[i16; 8]; 2],
    slide_to: [[u16; 8]; 2],
}

impl Musicline {
    /// 8 voices, InitSound: the clock of StartInt's acknowledge (`last` is the last period write's),
    /// and EndSound's writes when the tune ends in its first row. Called before the row is played.
    fn eight_row_times(&mut self, hw: &Hw, last: u64) {
        let (row, ended) = self.eight_row_cycles(hw);
        let at = |cycles: i64| last + (EIGHT_LAST_WRITE_CYCLE as i64 + cycles).max(0) as u64 / 16;
        self.eight_end_at = ended.then(|| [0, 1, 2].map(|k| at(row + ROW_END_WRITE + k * ROW_END_STEP)));
        self.eight_ack_at = at(row + if ended { ROW_END_SOUND } else { 0 }) + EIGHT_ROW_TO_ACK;
    }

    /// 8 voices: EndSound's writes of a tune that ended in its pre-roll row, at their times.
    fn eight_end_sound(&mut self, hw: &mut Hw) {
        let Some(at) = self.eight_end_at.take() else { return };
        if !self.voices[0].ended {
            return;
        }
        for (k, (reg, v)) in [(0xdff096, 0x000f), (0xdff09a, 0x0080), (0xdff09c, 0x0080)].into_iter().enumerate() {
            hw.wait(at[k].saturating_sub(hw.bus.clock));
            hw.bus.write_word(reg, v);
        }
        for v in self.voices.iter_mut() {
            v.wave_number_old = 0;
        }
    }

    /// The cycles of the pre-roll row of an untouched player, and whether the tune ends in it.
    fn eight_row_cycles(&self, hw: &Hw) -> (i64, bool) {
        let g = self.tune_groove;
        let mut st = RowState {
            groove: [g; 8],
            speed_part: [false; 8],
            groove_part: [false; 8],
            part_groove: [g != 0; 8],
            volume: [1024; 8],
            master: 1024,
            add_number: [[0; 8]; 2],
            slide_to: [[0; 8]; 2],
        };
        let mut total = ROW_EMPTY + if g != 0 { 8 * ROW_GROOVE_VOICE } else { 0 };
        let mut ended = false;
        for v in 0..8 {
            st.part_groove[v] = !st.part_groove[v];
            let (cycles, off) = self.eight_voice_cycles(hw, v, &mut st);
            total += cycles;
            ended |= v == 0 && off;
        }
        (total, ended)
    }

    /// One voice's row: its cycles more than an empty line's, and whether it was switched off.
    fn eight_voice_cycles(&self, hw: &Hw, v: usize, st: &mut RowState) -> (i64, bool) {
        let seq = rl(hw, self.tune + TU_SEQ + 4 * v as u32);
        let mut d = 0;
        let (mut pos, mut jumps) = (0u8, 0u8);
        let mut entry = None;
        let mut to_first = false;
        for _ in 0..256 {
            let data = rw(hw, seq + 2 * pos as u32);
            let (arg, target) = ((data & 0x1f) as u8, (data >> 8) as u8);
            to_first = false;
            if data & 0x20 != 0 {
                match (data & 0xc0) >> 6 {
                    1 => return (d + SEQ_OFF + if jumps != 0 { SEQ_JUMP_RUNNING } else { 0 }, true),
                    2 => {
                        let elsewhere = if target != 0 { SEQ_JUMP_ELSEWHERE } else { 0 };
                        if jumps != 0 {
                            jumps -= 1;
                            if jumps != 0 {
                                d += SEQ_JUMP_AGAIN + elsewhere;
                                pos = target;
                                to_first = target == 0;
                            } else {
                                d += SEQ_JUMP_PAST;
                                pos = pos.wrapping_add(1);
                            }
                        } else if pos > target {
                            d += SEQ_JUMP_FIRST + elsewhere;
                            jumps = arg;
                            pos = target;
                            to_first = target == 0;
                        } else {
                            d += SEQ_JUMP_PAST;
                            pos = pos.wrapping_add(1);
                        }
                        continue;
                    }
                    3 => {
                        if target != 0 {
                            let running = if jumps != 0 { SEQ_JUMP_RUNNING } else { 0 };
                            return (d + SEQ_WAIT + if arg != 0 { SEQ_WAIT_SPEED } else { 0 } + running, false);
                        }
                        d += SEQ_WAIT_NONE;
                        pos = pos.wrapping_add(1);
                        continue;
                    }
                    _ => d += SEQ_FLAGGED,
                }
            }
            entry = Some(data);
            break;
        }
        let Some(data) = entry else {
            // 256 entries and no part: the voice plays nothing
            let running = if jumps != 0 { SEQ_JUMP_RUNNING } else { 0 };
            return (d + SEQ_NO_PART + running + if to_first { SEQ_JUMP_ELSEWHERE } else { 0 }, false);
        };
        if jumps != 0 {
            d += SEQ_JUMP_RUNNING;
        }
        let part = rl(hw, self.tab + T_PARTS + 4 * (((data << 2) & 0x300) | (data >> 8)) as u32);
        let mut line = part;
        for _ in 0..128 {
            let note = rb(hw, line);
            if note == 61 {
                return (d + LINE_END, false);
            }
            if note & 0x80 == 0 {
                break;
            }
            d += LINE_JUMP;
            line += 12;
        }
        (d + self.eight_line_cycles(hw, v, line, st), false)
    }

    /// A part line's cycles more than an empty line's.
    fn eight_line_cycles(&self, hw: &Hw, v: usize, line: u32, st: &mut RowState) -> i64 {
        let (note, pi) = (rb(hw, line), rb(hw, line + 1));
        let mut d = 0;
        // what the effects leave for the instrument's start
        let (mut vibrato, mut tremolo, mut hold, mut keep_fine_tune) = (false, false, false, false);
        let (mut volume, mut add, mut add_number) = (None, false, 0i16);
        let (mut offset, mut offset_by) = (false, 0u8);
        let (mut table, mut portamento, mut portamentos) = (None, 0, 0);
        for k in 0..5 {
            let (e, a) = (rb(hw, line + 2 + 2 * k), rb(hw, line + 3 + 2 * k));
            d += Self::eight_fx_cycles(e, a, v, st);
            let a16 = (a as u16) << 4;
            if matches!(e, 0x03 | 0xe3) {
                portamentos += 1;
            }
            match e {
                0x03 | 0xe3 if portamento == 0 => portamento = e,
                0x08 | 0x09 | 0xea => vibrato = true,
                0x18 | 0x19 | 0xe8 => tremolo = true,
                0x0b => keep_fine_tune = true,
                0x10 => volume = Some(a16),
                0x15 | 0x16 => {
                    add = true;
                    if a != 0 {
                        add_number = if e == 0x15 { a16 as i16 } else { -(a16 as i16) };
                    }
                }
                0x44 | 0x45 => table = Some(a),
                0x46 => hold = true,
                0x48 => {
                    offset = true;
                    if a != 0 {
                        offset_by = a;
                    }
                }
                _ => {}
            }
            if note != 0 && matches!(e, 0x08 | 0x09 | 0x18 | 0x19) {
                d += FX_NOTE_VIBRATO;
            }
            if pi != 0 && e == 0x49 {
                d += FX_RESTART_INSTRUMENT;
            }
        }
        // the line's one portamento
        let one = if portamentos == 1 { portamento } else { 0 };
        if pi == 0 {
            return d + match (note != 0, one) {
                (false, _) => 0,
                (true, 0x03) => LINE_NOTE + LINE_NOTE_PORTAMENTO,
                (true, 0xe3) => LINE_NOTE + LINE_NOTE_PT_PORTAMENTO,
                (true, _) => LINE_NOTE,
            };
        }
        let inst = rl(hw, self.tab + T_INSTS + 4 * pi as u32);
        let w = |o: u32| rw(hw, inst + o);
        // the volume's path, from the instrument's or the one a line effect set
        let volume_cycles = |from: u16| {
            let mut c = if volume.is_some() { START_VOLUME } else { 0 };
            if add {
                let vol = (volume.unwrap_or(from) as i16).wrapping_add(add_number);
                c += START_VOLUME_ADD + if vol > 1024 { START_VOLUME_TOP } else if vol < 0 { START_VOLUME_ZERO } else { 0 };
            }
            c
        };
        let inst_volume = w(I_VOLUME).wrapping_mul(16);
        if note == 0 {
            return d
                + LINE_INSTRUMENT
                + volume_cycles(inst_volume)
                + if rb(hw, inst + I_SLIDE_SPEED) != 0 { LINE_INSTRUMENT_SLIDE } else { 0 }
                + if one == 0xe3 { LINE_INSTRUMENT_PT_PORTAMENTO } else { 0 };
        }
        if portamento != 0 {
            return d + if portamento == 0x03 { LINE_PORTAMENTO } else { LINE_PT_PORTAMENTO };
        }
        let e1 = rb(hw, inst + I_EFFECT1);
        let mut start_volume = inst_volume;
        // the arpeggio's first line
        if table.is_some() || e1 & E1_ARPEGGIO != 0 {
            let num = table.map_or(w(I_ARP_TABLE) as u32, |t| t as u32);
            let arp = rl(hw, self.tab + T_ARPS + 4 * (num & 0xff));
            if table.is_some() {
                d += ARP_BY_TABLE;
            }
            let mut at = arp;
            for _ in 0..128 {
                if rb(hw, at) != 62 {
                    break;
                }
                d += ARP_SKIP;
                at += 6;
            }
            let tr = rb(hw, at) as i8;
            if tr == 0 {
                return d + LINE_START + ARP_WAITS;
            }
            if tr == 61 {
                d += ARP_ENDS;
            } else {
                d += ARP_START_ROW + if tr > 0 { ARP_ABSOLUTE } else { 0 };
                if rb(hw, at + 1) != 0 {
                    d += ARP_SAMPLE;
                }
                for k in 0..2 {
                    let (e, a) = (rb(hw, at + 2 + 2 * k), rb(hw, at + 3 + 2 * k));
                    d += match ARP_FX.get(e as usize) {
                        Some(&(zero, other)) => if a == 0 { zero } else { other },
                        None => ARP_FX_OTHER,
                    };
                    if e == AE_SET_VOLUME {
                        start_volume = (a as u16) << 4;
                    }
                }
                if rb(hw, at + 2) == AE_SET_VOLUME && rb(hw, at + 4) == AE_SET_VOLUME {
                    d += ARP_VOLUME_TWICE;
                }
            }
        }
        if rl(hw, inst + IS_POINTER) == 0 {
            return d + LINE_NO_SAMPLE;
        }
        d += LINE_START + volume_cycles(start_volume);
        if keep_fine_tune {
            d += FX_FINE_TUNE_START;
        }
        let st_ = rb(hw, inst + IS_TYPE);
        if rb(hw, inst + I_TRANSPOSABLE) == 0 {
            d += START_FIXED_NOTE;
        }
        if hold {
            d += START_HOLD;
        }
        if offset && st_ == SAMPLE {
            d += if ((offset_by as u16) << 7) < w(IS_LENGTH) { START_OFFSET } else { START_OFFSET_PAST };
        }
        if rb(hw, inst + I_SLIDE_SPEED) != 0 {
            d += START_SLIDE;
        }
        // the played length: the sample's loop or none, or the wave's
        let mut length = match st_ {
            SAMPLE => {
                if e1 & E1_WAVE_SAMPLE_LOOP != 0 {
                    d += START_SAMPLE_LOOP;
                }
                if e1 & E1_WAVE_SAMPLE_LOOP != 0 && w(IS_REPEAT_LENGTH) != 0 { w(IS_REPEAT_LENGTH) } else { 1 }
            }
            WAVE16..=WAVE128 => {
                d += START_WAVE[st_ as usize - 1];
                w(IS_LENGTH) >> (5 - st_)
            }
            _ => {
                d += START_WAVE_OTHER;
                w(IS_LENGTH)
            }
        };
        if e1 & E1_ENVELOPE != 0 {
            d += START_ENVELOPE;
        }
        for (set, flag) in [(vibrato, E1_VIBRATO), (tremolo, E1_TREMOLO)] {
            d += if set { START_VIBRATO_SET } else if e1 & flag != 0 { START_VIBRATO } else { 0 };
        }
        let (e2, arg1, arg2) = (rb(hw, inst + I_EFFECT2), rb(hw, inst + I_ETPF_BITS), rb(hw, inst + I_MRL_BITS));
        let counters = [
            (E2_PHASE, I_PHASE, arg1 & F_PHASE_STEP, arg1 & F_PHASE_INIT),
            (E2_RESONANCE, I_RESONANCE, arg2 & M_RESONANCE_STEP, arg2 & M_RESONANCE_INIT),
            (E2_FILTER, I_FILTER, arg1 & F_FILTER_STEP, arg1 & F_FILTER_INIT),
            (E2_MIX, I_MIX, arg2 & M_MIX_STEP, arg2 & M_MIX_INIT),
            (E2_TRANSFORM, I_TRANSFORM, arg1 & F_TRANSFORM_STEP, arg1 & F_TRANSFORM_INIT),
        ];
        for (k, (flag, o, step, init)) in counters.into_iter().enumerate() {
            if e2 & flag != 0 {
                d += START_COUNTER[k] + if step != 0 { COUNTER_STEP } else if init != 0 { COUNTER_INIT } else { 0 };
                if w(o) > w(o + 2) {
                    d += COUNTER_DOWN;
                }
            }
        }
        if e1 & E1_LOOP != 0 {
            let ll = w(I_LOOP + 6);
            if st_ != SAMPLE {
                d += START_LOOP_WAVE;
            } else if ll == 0 {
                d += START_LOOP_NONE;
            } else {
                d += START_LOOP + if arg2 & M_LOOP_STEP != 0 { COUNTER_STEP } else if arg2 & M_LOOP_INIT != 0 { LOOP_INIT } else { 0 };
                if w(I_LOOP) > w(I_LOOP + 2) {
                    d += START_LOOP_DOWN;
                }
                length = ll;
            }
        }
        d + START_LENGTH.iter().find(|(n, _)| *n == length).map_or(0, |(_, c)| *c)
    }

    /// What a speed effect costs more on a voice with a groove: its counter set, or not.
    fn eight_groove_cycles(k: usize, st: &RowState, set: i64, kept: i64) -> i64 {
        match (st.groove[k] != 0, st.part_groove[k]) {
            (false, _) => 0,
            (true, false) => set,
            (true, true) => kept,
        }
    }

    /// A part effect's cycles.
    fn eight_fx_cycles(e: u8, a: u8, v: usize, st: &mut RowState) -> i64 {
        // the channel's volume (index 0) and the master volume (1), as `part_effects` keeps them
        let a16 = (a as u16) << 4;
        let m = (e >= 0x30) as usize;
        match e {
            0x20 => st.volume[v] = a16,
            0x27 => st.volume = [a16; 8],
            0x30 => st.master = a16,
            0x23 | 0x33 => st.slide_to[m][v] = a16,
            0x25 | 0x26 | 0x35 | 0x36 => {
                if a != 0 {
                    st.add_number[m][v] = a16 as i16;
                }
                let (up, cost) = if e & 15 == 5 { (true, ROW_VOLUME_UP) } else { (false, ROW_VOLUME_DOWN) };
                let volume = if m == 0 { &mut st.volume[v] } else { &mut st.master };
                let n = if up { (*volume as i16).wrapping_add(st.add_number[m][v]) } else { (*volume as i16).wrapping_sub(st.add_number[m][v]) };
                let clamp = if up { n > 1024 } else { n < 0 };
                *volume = if up { n.min(1024) } else { n.max(0) } as u16;
                return if a == 0 { cost.0 } else { cost.1 } + if clamp { ROW_VOLUME_CLAMP } else { 0 };
            }
            _ => {}
        }
        if let Some(&(_, zero, other)) = ROW_FX.iter().find(|(code, _, _)| *code == e) {
            let there = match e {
                0x24 => st.slide_to[0][v] == st.volume[v],
                0x34 => st.slide_to[1][v] == 0x34 * 4,
                _ => false,
            };
            return if a == 0 { zero } else { other } + if there { ROW_VOLUME_SLIDE_THERE } else { 0 };
        }
        match e {
            // the vibrato and tremolo waves: 0 to 3, or not one
            0x0a | 0x1a => if a <= 3 { 49 } else { 25 },
            // the fine tune, clamped above 31 or below -31
            0x0b => match a as i8 {
                32.. => 88,
                ..=-32 => 78,
                _ => 74,
            },
            // ProTracker tremolo and vibrato: each nibble given
            0xe8 | 0xea => if a == 0 { 29 } else { 95 + 12 * ((a & 15 != 0) as i64 + (a & 0xf0 != 0) as i64) },
            // the voice's speed (above 31: clamped; with a groove its counter is set on the ticks
            // the groove is not used)
            0x40 if a != 0 => {
                st.speed_part[v] = true;
                95 + if a > 31 { 8 } else { 0 } + Self::eight_groove_cycles(v, st, 21, 9)
            }
            // the voice's groove (its counter is set on the tick the groove is used)
            0x41 if a != 0 => {
                st.groove_part[v] = true;
                st.groove[v] = a.min(31);
                (if st.part_groove[v] { 104 } else { 92 }) + if a > 31 { 8 } else { 0 }
            }
            0x40 | 0x41 => 17,
            // the tune's speed: each voice up to this one takes it (unless a part effect set its own;
            // more with a groove), the others only store it; from 32 on it is the tempo
            0x42 => match a {
                0 => 23,
                32.. => 250,
                _ => 387 + (0..=v).map(|k| 64 + if st.speed_part[k] { -55 } else { Self::eight_groove_cycles(k, st, 21, -1) }).sum::<i64>(),
            },
            // the tune's groove, likewise: the voices up to this one (less where a part effect set the
            // voice's own groove), and 6 cycles less for each later voice that is on its groove tick
            // and gets a groove, or is not and had one
            0x43 => {
                let g = a & 0x1f;
                let later = |k: usize| if st.part_groove[k] { g != 0 } else { st.groove[k] != 0 };
                let c = if a == 0 {
                    19
                } else {
                    (if g == 0 { 374 } else { 422 })
                        + (0..8)
                            .map(|k| match (k <= v, g != 0, st.groove_part[k], st.part_groove[k]) {
                                (true, false, true, _) => 9,
                                (true, false, false, _) => 30,
                                (true, true, true, _) => 3,
                                (true, true, false, true) => 45,
                                (true, true, false, false) => 67,
                                (false, ..) => if later(k) { -6 } else { 0 },
                            })
                            .sum::<i64>()
                };
                if a != 0 {
                    for k in 0..8 {
                        if k > v || !st.groove_part[k] {
                            st.groove[k] = g;
                        }
                    }
                }
                c
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PITCH;
    use crate::engine::Engine;
    use std::path::Path;

    fn chunk(d: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
        d.extend(id);
        d.extend((body.len() as u32).to_be_bytes());
        d.extend(body);
    }

    /// One 4-voice tune "TEST" (tempo 125, speed 6): voice 0 plays part 0, whose
    /// first line is note 25 with instrument 1, a 64-byte sample without a loop
    /// at volume 64; the other voices are empty.
    fn module() -> Vec<u8> {
        let mut d = b"MLEDMODL".to_vec();
        chunk(&mut d, b"VERS", &[0, 1, 0, 0]);
        let mut t = vec![0u8; 0x28];
        t[..4].copy_from_slice(b"TEST");
        t[0x20..0x22].copy_from_slice(&125u16.to_be_bytes());
        t[0x22] = 6; // speed
        t[0x24..0x26].copy_from_slice(&64u16.to_be_bytes()); // volume
        t[0x27] = 4; // channels, play mode 0 (Paula)
        t.extend(2u32.to_be_bytes()); // voice 0's sequence: one entry
        t.extend([0u8; 12]);
        t.extend(0x0010u16.to_be_bytes()); // part 0, no transpose
        chunk(&mut d, b"TUNE", &t);
        // part 0: line 0 note 25 instrument 1; the end flag
        chunk(&mut d, b"PART", &[0, 0, 1, 25, 1, 0x80]);
        let mut i = vec![0u8; 0xce];
        i[..4].copy_from_slice(b"INST");
        i[0x20] = 1; // sample 1
        i[0x26..0x28].copy_from_slice(&32u16.to_be_bytes()); // length (words)
        i[0x3a..0x3c].copy_from_slice(&64u16.to_be_bytes()); // volume
        chunk(&mut d, b"INST", &i);
        d.extend(b"SMPL");
        d.extend((0x32u32 + 64).to_be_bytes());
        d.extend(64u32.to_be_bytes()); // original length
        d.extend([0, 0]); // no delta packing
        let mut h = vec![0u8; 0x32];
        h[0x26..0x28].copy_from_slice(&32u16.to_be_bytes());
        d.extend(h);
        d.extend((0..64).map(|k| if k < 32 { 0x40u8 } else { 0xc0 }));
        d
    }

    #[test]
    fn plays_a_sample_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("ml.test"), module()).unwrap();
        let mut out = vec![0f32; 2 * 24000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let p = e.playing.as_ref().unwrap();
        assert_eq!(p.format, "Musicline Editor");
        assert_eq!((p.sub_min, p.sub_max), (0, 0));
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the note: the sample's 32 words at note 25's period, volume 64
        let (hi, lo) = (reg(0xDFF0A0), reg(0xDFF0A2));
        assert!(!hi.is_empty());
        let loc = (hi[0] as u32) << 16 | lo[0] as u32;
        assert_eq!(e.m.bus.r8(loc), 0x40);
        assert_eq!(e.m.bus.r8(loc + 32), 0xc0);
        assert_eq!(reg(0xDFF0A4)[0], 32);
        assert_eq!(reg(0xDFF0A6)[0], PITCH[(25 << 5) + 32]);
        assert!(reg(0xDFF0A8).contains(&64));
        // no loop: then the one-word empty sample
        assert_eq!(reg(0xDFF0A4)[1], 1);
        // voices 1 to 3 have no sequence: the zero channel's entries ($0010)
        // are part 0 too, so they play the same note
        assert!((1..4).all(|ch| reg(0xDFF0A4 + 16 * ch).first() == Some(&32)));
    }

    /// One 8-voice tune (tempo 125, speed 6, the groove given): every voice plays part 0, an empty
    /// line, but `voice`, whose sequence is `seq`; part 1's first line is `line` (note, instrument,
    /// effects); instrument 1 is a 64-byte sample without a loop.
    fn module8(groove: u8, voice: usize, seq: &[u16], line: (u8, u8, &[(u8, u8)])) -> Vec<u8> {
        let mut d = b"MLEDMODL".to_vec();
        d.extend(0x18u32.to_be_bytes());
        d.extend([0u8; 0x18]);
        chunk(&mut d, b"VERS", &[1, 0x14, 0x31, 0x2e, 0x31, 0x34]);
        let mut t = vec![0u8; 0x28];
        t[..4].copy_from_slice(b"PROB");
        t[0x20..0x22].copy_from_slice(&125u16.to_be_bytes());
        t[0x22] = 6;
        t[0x23] = groove;
        t[0x24..0x26].copy_from_slice(&64u16.to_be_bytes());
        t[0x26] = 1; // play mode: 8 voices
        t[0x27] = 8;
        let seqs: Vec<&[u16]> = (0..8).map(|v| if v == voice { seq } else { &[0x0010][..] }).collect();
        for s in &seqs {
            t.extend((2 * s.len() as u32).to_be_bytes());
        }
        for s in &seqs {
            t.extend(s.iter().flat_map(|x| x.to_be_bytes()));
        }
        chunk(&mut d, b"TUNE", &t);
        chunk(&mut d, b"PART", &[0, 0, 0, 0x80]);
        // part 1: the line's pairs that are not empty, behind their flags
        let mut p = vec![0u8, 1, 0];
        let pairs = std::iter::once((line.0, line.1)).chain(line.2.iter().copied());
        for (k, (x, y)) in pairs.enumerate() {
            if x != 0 || y != 0 {
                p[2] |= 1 << k;
                p.extend([x, y]);
            }
        }
        p.push(0x80);
        chunk(&mut d, b"PART", &p);
        let mut i = vec![0u8; 0xce];
        i[..4].copy_from_slice(b"INST");
        i[0x20] = 1;
        i[0x26..0x28].copy_from_slice(&32u16.to_be_bytes());
        i[0x3a..0x3c].copy_from_slice(&64u16.to_be_bytes());
        i[0x3c] = 1; // transposable
        chunk(&mut d, b"INST", &i);
        d.extend(b"SMPL");
        d.extend((0x32u32 + 64).to_be_bytes());
        d.extend(64u32.to_be_bytes());
        d.extend([0, 0]);
        let mut h = vec![0u8; 0x32];
        h[..4].copy_from_slice(b"SMPL");
        h[0x26..0x28].copy_from_slice(&32u16.to_be_bytes());
        d.extend(h);
        d.extend((0..64u32).map(|k| ((k * 7) & 0x7f) as u8));
        d
    }

    /// The start of an 8-voice module: the clock of InitSound's first write, the colour clocks from
    /// its last period write to StartInt's acknowledge, and the custom chip writes between the two
    /// (register, colour clocks after the period write).
    fn start8(data: Vec<u8>) -> (u64, u64, Vec<(u32, u64)>) {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("ml.test"), data).unwrap();
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log: Vec<_> = e.m.bus.write_log.take().unwrap().into_iter().filter(|w| w.0 >> 12 == 0xdff).collect();
        let first = log.iter().position(|w| w.0 == 0xDFF0A0).unwrap();
        let last = log[first + 15];
        assert_eq!(last.0, 0xDFF0D6);
        let rest = &log[first + 16..];
        let ack = (0..rest.len() - 1).find(|&k| rest[k].0 == 0xDFF09C && (rest[k + 1].0, rest[k + 1].1) == (0xDFF09A, 0x8080)).unwrap();
        (log[first].2, rest[ack].2 - last.2, rest[..ack].iter().map(|w| (w.0, w.2 - last.2)).collect())
    }

    /// 8 voices: the pre-roll row's time is the Eagleplayer's (its CPU eight times as fast), measured
    /// on these very modules: an empty row, a tune with a groove (which also starts 8 colour clocks
    /// later), a note, sequence jumps with effects that depend on what the row did before, and a tune
    /// that ends in its first row (EndSound's three writes at their times).
    #[test]
    fn eight_voice_start_times() {
        let empty: (u8, u8, &[(u8, u8)]) = (0, 0, &[]);
        let (first, gap, between) = start8(module8(0, 2, &[0x0110], empty));
        assert_eq!((gap, between.len()), (995, 0));
        let (first_groove, gap, _) = start8(module8(3, 2, &[0x0110], empty));
        assert_eq!((first_groove - first, gap), (8, 984));
        assert_eq!(start8(module8(0, 2, &[0x0110], (25, 1, &[(0x10, 0x20)]))).1, 1116);
        assert_eq!(start8(module8(0, 2, &[0x03a1, 0x00e0, 0x00a2, 0x0110], (0, 0, &[(0x26, 0x50), (0x43, 5)]))).1, 1104);
        let (_, gap, between) = start8(module8(0, 0, &[0x0060], empty));
        assert_eq!(gap, 942);
        assert_eq!(between, [(0xDFF096, 861), (0xDFF09A, 862), (0xDFF09C, 863)]);
    }
}

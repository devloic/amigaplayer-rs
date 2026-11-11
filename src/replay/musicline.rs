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
//! Late ticks (4 voices): a tune whose effects (256-byte phase, mix and resonance voices)
//! take a tick's whole period on a 68000 makes the original's next tick late. The
//! original's CIA-B has two timers in play: timer A, the tick (its underflows go on
//! while the code runs and are one pending interrupt when the tick overran), and
//! timer B, the DMA restart's two waits (100 E-clocks, or the longest period those
//! voices played, and then 150), whose interrupts are taken after a tick that
//! overran, in the order of the CIA's bits (the tick, then timer B). The port keeps
//! both timers (`grid`, `TimerB`) and takes the interrupts at those moments:
//! * the DMA routine at a tick's end (DMA off for the voices that restart and those
//!   still waiting, timer B started, the interrupt's step set to the first) and the
//!   interrupt's two steps (the voices' registers and DMA on, then their loops and
//!   the DMA mask cleared) are separate; a timer B interrupt that waited for a tick
//!   to end runs the first step again (the routine set it), so a note starts two ticks
//!   late, the second step never comes and the DMA mask stays set (the voice
//!   restarts every tick with no LOC/LEN writes);
//! * the interrupt that runs while timer B counts again only changes its latch (the
//!   timer is not restarted), as the CIA does;
//! * the engine takes the next tick's time from `Replayer::next_tick_at`, the first
//!   underflow after the tick that was called.
//! The time the original's code takes on a 68000 is priced by what a tick does, not by
//! averages: a pass of each effect (per byte of its wave, and a setup by its type: the
//! phase types and fill, the mix sources, the filter types), the lines and instruments
//! of a row, and the slides, envelope, arpeggio and loop steps of each voice, each with
//! a price in colour clocks (`PHASE_PASS`, `RES_BYTE`, `SEQ_ENTRY` ...) that was measured
//! on the Eagleplayer's runs on the engine's 68000 clock (the instruction cycles the
//! code ran on 174 four-voice modules, regressed on these counts; 130 of them fitted
//! and 44 held out: a tick is right to 0.2% on average and 0.9% at worst on those, and
//! to 0.1% on corpus lines 244 and the list's synthmix, which were drifting by 0.1 s over
//! 20 s). The costs are charged where the port does the work and paid after the row and
//! after each voice's effects (`pay`); the time from the last voice's volume write to the
//! DMA routine depends on how many voices write (`TAIL`). The effects' output is copied
//! to the played wave without running Paula up to the copy, as the original's code
//! does under the engine (it touches no register meanwhile). UADE has none of this
//! (a faster CPU: its ticks stay on the timer's period).
//!
//! Inside a 4-voice tick (all of it measured on the Eagleplayer's runs with half-cycle write times,
//! 192 four-voice corpus modules): the tick's fixed work (`PLAY_TIME`) is not all before the voices'
//! effects: a voice's loop location and length writes come at the end of its effects, `LOOP_PRE` after
//! the row and `VOICE_GAP` more for each voice after the first, and the rest of the fixed work comes
//! after the last voice, before the first period write, which does not move. An idle tick costs `VOICE_PLAY`
//! for each voice plus `VOICE_PLAYING` for each voice that plays an instrument, except a sample with
//! the sample-loop flag (E1 bit 7; a wave with it pays); the volume slides measured (`SLIDE_*`) add
//! theirs; the instrument start is priced by the voice that starts (`INST_START_BY_VOICE`); the time
//! that the last voice's tail takes off its effects (`TAIL`, which depends on which voices write) is
//! not carried to the next tick if it was not used up. What is still off, by module and not
//! by what the tick's counts say (a regression on those counts does not hold on modules left out):
//! the first write after the effect passes of the filter, mix and phase types (errors from 5 to 100
//! colour clocks, constant in a module and tick class), the other slides, tremolo and ProTracker
//! slides, the start ticks (the error is a step of about 33 or 42 colour clocks, by voice), the
//! first tick's total, and the voice's play cost where the instrument is blank or its sample
//! is long.
//! 8 voices (the CPU of the reference eight times as fast, `DELI_CPU_SPEED`): InitSound's writes come
//! `EIGHT_FIRST_WRITE` after the loading, the first tick's acknowledge `EIGHT_ROW_BASE` + the pre-roll
//! row's price / 8 after the last of them, the song starts `EIGHT_ACK_TO_START` after the first tick's
//! four volume writes, and each tick's handler writes the four mix buffer locations (`EIGHT_LOC_AT`)
//! after its acknowledge (the first tick also the four volumes, `EIGHT_FIRST_VOL_AT`). Of an 8-voice
//! module only the writes of the start-up and the tick's handler are exact in time (seven writes);
//! the gap from the last InitSound write to the first acknowledge varies by +-30 colour clocks with
//! the tune, and nine tunes start 8 colour clocks later than the rest.
//!
//! The time before the first tick (4 voices) is exact except for the first row. The original's
//! loader runs on the engine's 68000 clock, which counts floor(cycles / 2) of each stretch of
//! code between two host-library calls, so the loading time is the sum of the stretches of each
//! chunk kind (per channel of a tune, per line and voice bit of a part, per instrument, and per
//! sample: the wave's, the decrunch's records, literals and deltas): it equals the original's
//! call times on 183 of 184 four-voice modules (`Cost`). The timer starts a fixed time after
//! the last call plus the first row's work, which is calibrated (`INIT_ROW`), and its first
//! tick comes 65536 E-clocks after, from the E-clock edge the start fell in (`start_int`); the
//! timer's later ticks follow its latch. A tune whose first row sets the tempo (effect $42) has
//! the timer's latch loaded before the player starts, so the first tick comes after that
//! period (`preload`) rather than the default one.
//!
//! The tables (`musicline_tables.rs`) come from NostalgicPlayer's Tables.cs; the
//! 68k player was used only to check them, none of its code or data is copied.

use super::{Hw, Open, Replayer};
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

// Colour clocks the original's code takes between its writes (68000, from
// the Eagleplayer's write log) and the CIA's wait.
const ECLOCK: u64 = 5;
/// The tick's fixed work before the effects.
const PLAY_TIME: u64 = 2669;
/// ... of which this long passes before the end of the first voice's effects (its loop writes) with none to
/// price, and this long more before the end of each next voice's (measured on the loop writes of 3 modules).
const LOOP_PRE: u64 = 650;
const VOICE_GAP: u64 = 375;
/// The first tick's (the volume writes instead of the tune's reading).
const FIRST_PLAY_TIME: u64 = 2683;
/// EndSound's writes of a 4-voice tune that ends in its first row, before the timer starts.
const END_BEFORE_START: u64 = 579;
/// 8 voices (the reference's CPU running eight times as fast as the 68000's: it counts floor(cycles / 16) of
/// each stretch between two host calls, `Cost::clk8`): the first write of InitSound comes this many
/// colour clocks after the loading (the fixed init stretches), on 120 of 129 tunes; nine others (12, 75, 125, 234,
/// 237, 263, 274, 289, 9 of the corpus) 8 later, for a reason not found.
const EIGHT_FIRST_WRITE: u64 = 27_617;
/// 8 voices: the time from InitSound's last write to the first tick's acknowledge is, in colour
/// clocks, a fit on 129 tunes (standard deviation 37, at most 200) of the pre-roll row's price:
/// the price (in 68000 colour clocks) over 8, as the reference's CPU is 8 times as fast, and a constant.
const EIGHT_ROW_DIV: f64 = 8.0;
const EIGHT_ROW_BASE: f64 = 130.2;
/// 8 voices (the reference's CPU running eight times as fast): InitSound's writes of a channel
/// (location, length, period) come 0, 1 and 2 colour clocks after the first, the next channel's 4
/// after; the first tick's acknowledge is `EIGHT_ROW_BASE` + the row's price / 8 after the last, the interrupt
/// enable, DMA-on and the four volumes 1 colour clock apart after it, and the song starts 22
/// after the last volume write (the writes before it are the engine's time before the start).
const EIGHT_ACK_TO_START: u64 = 22;
/// 8 voices: a tick's four location writes come these colour clocks after its acknowledge
/// (each channel's location and length words in one move) and the four volumes of the first
/// tick after the last location.
const EIGHT_LOC_AT: [u64; 4] = [9, 11, 12, 14];
const EIGHT_FIRST_VOL_AT: [u64; 4] = [19, 20, 22, 23];
/// 4 voices: the first tick's four volume writes (0) come these colour clocks after its acknowledge;
/// they come out of the tick's fixed work (`FIRST_PLAY_TIME`).
const FIRST_VOL_AT: [u64; 4] = [60, 72, 84, 96];

// Prices, in colour clocks, of what the original's code does (see PLAY_TIME).
// A row:
const SEQ_ENTRY: f64 = 196.2;
const PART_LINE: f64 = 153.7;
const CHECK_INSTRUMENT: f64 = 514.2;
const CHECK_NOTE: f64 = 0.0;
const CHECK_NEW_INSTRUMENT: f64 = 98.5;
const PART_FX: f64 = 32.0;
const ARP_START: f64 = 365.9;
const PLAY_INSTRUMENT: f64 = 122.3;
const INST_PLAY: f64 = 756.2;
/// A tick in which instruments start takes less than the prices say. 8 voices: per instrument started,
/// and once for the tick (a fit on the 623 such ticks of 15 modules).
const INST_START_VOICE: f64 = -124.3;
const INST_START_TICK: f64 = 105.1;
/// 4 voices: the error of the prices on a start tick (the DMA-on write of 563 ticks of 18 modules) is
/// better described by a term per started voice, growing with the voice's index (a start of voice 0 or 1
/// is priced right, one of voice 3 takes 190 colour clocks less), than by terms per instrument and tick:
/// about half of those ticks come within 37 colour clocks, against 40% with no term and fewer with those.
const INST_START_BY_VOICE: [f64; 4] = [-15.0, 0.0, -65.0, -190.0];
const INST_ENVELOPE: f64 = 104.1;
const INST_FILTER: f64 = 199.9;
const INST_MIX: f64 = 80.5;
const INST_PHASE: f64 = 116.7;
const INST_RESONANCE: f64 = 164.0;
// Each tick and voice:
/// (An idle tick takes 4411.25 + 7.5 per voice that plays an instrument to the first period write: 4418, 4426,
/// 4433 with one, two, three; 15 cycles of the 68000 for each; 814 idle ticks of 3 modules.)
const VOICE_PLAY: f64 = 453.8125;
/// (not for a sample with the sample-loop flag, which takes 7.5 less)
const VOICE_PLAYING: f64 = 7.5;
const SLIDE_NOTE: f64 = 51.8;
/// The volume slides that were measured (the others take what the idle tick does): a volume slide down
/// (FX $12) and its ProTracker form ($E7), a channel volume slide up and down. Each is constant to a
/// colour clock on its tick (151, 87, 180, 109, 223, 160 of the corpus: 23, 47, 16.5, 22.5 per voice).
const SLIDE_VOLUME_DOWN: f64 = 23.0;
const SLIDE_PT_VOLUME_DOWN: f64 = 47.0;
const SLIDE_CHANNEL_UP: f64 = 16.5;
const SLIDE_CHANNEL_DOWN: f64 = 22.5;
const ARP_WAIT: f64 = 25.6;
const ARP_STEP: f64 = 313.1;
const ARP_LINE: f64 = 170.4;
const ENVELOPE: f64 = 78.5;
const ENVELOPE_END1: f64 = 4.2;
const ENVELOPE_END2: f64 = 30.5;
const ENVELOPE_STEP: f64 = 20.3;
const VIBRATO: f64 = 191.7;
const TREMOLO: f64 = 165.5;
const LOOP_MOVE: f64 = 120.3;
const LOOP_COUNT: f64 = 148.6;
const WAVE_COPY: f64 = 58.9;
const WAVE_COPY_BYTE: f64 = 2.88;
// The effects: a pass and a byte of it.
const RES_PASS: f64 = 316.4;
const RES_BYTE: f64 = 155.26;
const TRANSFORM_PASS: f64 = 442.5;
const TRANSFORM_BYTE: f64 = 62.49;
const FILTER_PASS: f64 = 315.5;
const FILTER_BYTE: f64 = 110.12;
const RFILTER_PASS: f64 = 311.3;
const RFILTER_BYTE: f64 = 109.13;
const MIX_BYTE: f64 = 29.82;
const MIX_BYTE_BOOST: f64 = 29.83;
const PHASE_EARLY_PASS: f64 = 284.5;
const PHASE_EARLY_BYTE: f64 = 10.69;
/// A mix pass by its source (own buffer or not), whether it uses a wave of its own and
/// whether its offset is 0: [buffer][wave of its own][offset 0].
const MIX_PASS: [[[f64; 2]; 2]; 2] = [[[333.2, 350.6], [351.6, 370.9]], [[346.4, 336.4], [337.9, 319.4]]];
/// A phase pass by type (0 to 3) and fill: (pass, per interpolated byte, per filled byte).
const PHASE_PASS: [[(f64, f64, f64); 2]; 4] = [[(386.4, 17.65, 8.64), (414.3, 17.82, 10.49)],
    [(391.9, 38.89, 21.84), (384.1, 39.04, 41.05)],
    [(732.2, 27.97, 14.68), (375.1, 34.05, 34.95)],
    [(348.7, 39.06, 28.01), (423.3, 38.24, 41.29)]];
/// ... and per restart of the row of a fill (types 1 and 2).
const PHASE_RESTART: [f64; 4] = [0.0, 0.0, 11.0, 0.0];
/// Colour clocks from the last volume write to the DMA routine's first write, by the last
/// voice that wrote and how many voices wrote (the rest restart; measured on the original).
const TAIL: [[u64; 4]; 4] = [[347, 0, 0, 0], [320, 263, 0, 0], [276, 241, 192, 0], [250, 201, 166, 131]];
const PER_TO_VOL: u64 = 114;
const VOL_TO_PER: u64 = 58;
// The writes of the voices that do not restart (measured on the original): from a voice's
// volume write to the next written voice's period write, by that voice (the last one comes
// 9 sooner), plus a price for each voice skipped between; from the last volume write to the
// DMA-off write, by the mask of the voices written, plus a price for each voice that
// restarts and sets its DMA bit.
const VOL_TO_NEXT: [u64; 4] = [58, 58, 58, 49];
const SKIPPED_VOICE: u64 = 35;
const TAIL_TO_DMA: [u64; 16] = [0, 333, 298, 263, 262, 227, 227, 192, 236, 200, 200, 166, 201, 165, 165, 131];
const TAIL_ADD: u64 = 14;
// The tick's entry to its first write (the acknowledge), after the timer's underflow.
const CALL_TO_ACK: u64 = 50;
// The DMA routine at a tick's end: DMA off, 90 colour clocks to the low byte of timer B's
// latch and 17 more to the high byte, which starts the timer.
const OFF_TO_TB_LOW: u64 = 90;
const TB_LOW_TO_HIGH: u64 = 17;
// ... and the tick returns 107 colour clocks after that.
const TICK_RETURN: u64 = 107;
const VOL_TO_LOC: u64 = 64;
const LOC_TO_LEN: u64 = 10;
const LEN_TO_PER: u64 = 10;
// Timer B's first interrupt (the voices' registers and DMA on), measured from its entry
// (the timer's underflow, or the end of the tick it waited for): the first voice's
// first write by the voice (the skipped voices take about 20 colour clocks each); from
// a voice's period write to the next voice's first write by the voices skipped between
// (125 and about 20 more for each); from the last voice's period write to the DMA-on
// write by the voices after it; with no voice to start, the DMA-on write comes at 218.
// The routine then writes timer B's latch (24 colour clocks after the DMA-on write, and 17
// more for the high byte, which starts the 150 wait) and returns 82 after that.
const START_FIRST: [u64; 4] = [243, 264, 284, 305];
const START_NEXT: [u64; 3] = [125, 145, 166];
const START_LAST_TO_ON: [u64; 4] = [80, 59, 39, 18];
const START_NONE_ON: u64 = 218;
const ON_TO_TB_LOW: u64 = 24;
const TB_HIGH_TO_END: u64 = 82;
// The second interrupt (the loop registers of the voices in the mask): the first voice's
// location write by the voice, the length 10 later, the next voice's location by the voices
// skipped (63, 10 more each), the return by the voices after the last (98, 9 more each);
// with an empty mask it returns at 260.
const LOOP_FIRST: [u64; 4] = [188, 197, 207, 216];
const LOOP_NEXT: [u64; 3] = [63, 73, 82];
const LOOP_LAST_TO_END: [u64; 4] = [126, 117, 107, 98];
const LOOP_NONE_END: u64 = 260;

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
/// The first row's work in the init (colour clocks). Its cost is known only as a range (the
/// instrument starts of the voices), so it is calibrated, not exact: a fixed part, a part of the
/// port's price of the pre-roll row, and for each voice that starts an instrument what its
/// effects add (a fit on 123 four-voice modules; median error 47 colour clocks on them and 61
/// on 60 held out, against 100 for the price alone).
const INIT_ROW_BASE: f64 = 638.3;
const INIT_ROW: f64 = 0.93;
const INIT_START_E2: [(u8, f64); 5] = [(E2_TRANSFORM, 158.8), (E2_PHASE, 55.1), (E2_MIX, 43.5), (E2_RESONANCE, -4.8), (E2_FILTER, -40.2)];
const INIT_START_E1: [(u8, f64); 5] = [(E1_ENVELOPE, -6.6), (E1_VIBRATO, 79.3), (E1_ARPEGGIO, 36.7), (E1_LOOP, 257.2), (E1_WAVE_SAMPLE_LOOP, -19.4)];

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
    fn one_way(&mut self) {
        if self.step || self.delay == 0 {
            self.counter = self.counter.wrapping_add(self.speed as u16) & 0x1ff;
        } else {
            self.delay -= 1;
        }
    }

    fn count(&mut self) {
        if !(self.step || self.delay == 0) {
            self.delay -= 1;
            return;
        }
        if self.turns < 0 {
            return;
        }
        let mut c = self.counter as i16;
        c = c.wrapping_add(self.speed);
        let (lo, hi) = if self.repeat < self.repeat_end {
            (self.repeat, self.repeat_end)
        } else {
            (self.repeat_end, self.repeat)
        };
        if self.speed < 0 {
            if c as i32 >= lo as i32 {
                self.counter = c as u16;
                return;
            }
        } else if c as i32 <= hi as i32 {
            self.counter = c as u16;
            return;
        }
        if self.turns != 0 {
            self.turns -= 1;
            if self.turns == 0 {
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

/// CIA-B timer B in one-shot mode, as the engine's CIA counts it: a write of the high byte
/// of the latch starts it when it is stopped (while it runs only the latch changes), it
/// underflows (latch + 1) E-clocks after that and then stops; the interrupt it raises is
/// taken by whatever runs the interrupt handlers, which can be late.
#[derive(Default)]
struct TimerB {
    latch: u16,
    running: bool,
    /// The colour clock of the underflow.
    fire: u64,
    /// An underflow no interrupt has taken yet.
    pending: bool,
}

impl TimerB {
    fn sync(&mut self, now: u64) {
        if self.running && self.fire <= now {
            self.running = false;
            self.pending = true;
        }
    }

    fn write_high(&mut self, now: u64, latch: u16) {
        self.sync(now);
        self.latch = latch;
        if !self.running {
            self.running = true;
            self.fire = (now / ECLOCK + latch as u64 + 1) * ECLOCK;
        }
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
    /// CIA-B timer B, the DMA restart's two waits, and the interrupt step it takes next
    /// (1 after a tick's DMA routine, 2 once the first interrupt has run).
    tb: TimerB,
    isr_step: u8,
    /// The tick timer's next underflow (CIA-B timer A, which runs on whatever the code does).
    grid: Option<u64>,
    prev_period: [u16; 4],
    /// Each channel buffer's "written this tick" flag.
    chan_written: [bool; 4],
    voices: [Voice; 8],
    vol_table: Vec<u8>,
    title: String,
    /// The prices of the work done since the last `pay` (colour clocks, with fractions).
    owed: f64,
    /// An instrument has started on this tick (the tick's share of the start's price is paid).
    inst_started: bool,
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
        tb: TimerB::default(),
        isr_step: 1,
        grid: None,
        prev_period: [0; 4],
        chan_written: [false; 4],
        voices,
        vol_table,
        owed: 0.0,
        inst_started: false,
        title: String::new(),
    };
    let mut hw = Hw { bus: &mut m.bus };
    // the original's loading and init take a time that follows the module: the
    // timer starts then (4 voices), or the first write is made (8 voices, at the
    // engine's start phase after it; InitSound's first writes)
    if hw.bus.r8(hw.bus.r32(tab + T_TUNES) + TU_PLAY_MODE) != 0 {
        hw.wait((cost.base + cost.clk8 + EIGHT_FIRST_WRITE).saturating_sub(hw.bus.clock));
        ml.init_sound(&mut hw, 0, false);
    } else {
        ml.init_sound(&mut hw, 0, true);
        // the pre-roll's row is priced in `owed`
        let at = cost.four_clocks(ml.owed, &ml.voices[..4]);
        if ml.voices[0].ended {
            // a tune that ends in its first row: the init makes EndSound's writes 579 colour
            // clocks before it starts the timer
            hw.wait(at.saturating_sub(END_BEFORE_START + hw.bus.clock));
            ml.end_sound(&mut hw);
        }
        hw.wait(at.saturating_sub(hw.bus.clock));
    }
    ml.start_int(&mut hw);
    // what the init's row cost is in the start time already
    ml.owed = 0.0;
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
            _ => break,
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
    /// loading (`clk`), the calls that close it and the init's fixed stretch, and the
    /// first row's work (`preroll`, in the port's price, which fits the init's time to a
    /// median of about 100 colour clocks).
    fn four_clocks(&self, preroll: f64, starts: &[Voice]) -> u64 {
        let mut row = INIT_ROW_BASE + INIT_ROW * preroll;
        for x in starts.iter().filter(|x| x.play & P_RETRIGGER != 0) {
            row += INIT_START_E2.iter().filter(|(b, _)| x.effect2 & b != 0).map(|(_, c)| c).sum::<f64>();
            row += INIT_START_E1.iter().filter(|(b, _)| x.effect1 & b != 0).map(|(_, c)| c).sum::<f64>();
        }
        self.base + self.clk + AFTER_LOAD + row.max(0.0) as u64
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
        // The timer's underflows go on while the code runs: the next one is the first after
        // this call (one that came in a tick that ran too long is one pending interrupt,
        // not several).
        let now = hw.bus.clock;
        let period = (self.timer as u64 + 1) * ECLOCK;
        let mut next = self.grid.unwrap_or(now);
        while next <= now {
            next += period;
        }
        self.grid = Some(next);
        // the interrupts pending at this point are taken after the tick's, in the
        // order of the CIA's bits (timer A, timer B)
        self.tb.sync(now);
        let take_b = std::mem::take(&mut self.tb.pending);
        hw.wait(CALL_TO_ACK);
        self.handler(hw);
        if take_b {
            self.timer_b_interrupt(hw);
        }
        self.until_next_tick(hw, next);
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
        self.end_sound(hw);
        self.init_sound(hw, n, false);
        self.start_int(hw);
    }

    fn audio_irq(&mut self, hw: &mut Hw) -> bool {
        if self.eight {
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
        }
        // the pre-roll: one row
        self.play_tune(hw);
        if self.voices[0].ended && !defer_end {
            self.end_sound(hw);
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
                Some(latch) if !self.started => latch as u64,
                _ => 0xffff,
            };
            self.started = true;
            let entry = (t0 / ECLOCK + latch + 1) * ECLOCK;
            hw.wait(131);
            hw.dmacon(0x8000 | self.dma_mask);
            // the song starts (Paula's grids restart) this long after the DMA-on write in the
            // reference; the first interrupt keeps its time
            hw.wait(START_LAG);
            self.used.set(entry.saturating_sub(hw.bus.clock).max(1));
            self.grid = None;
            self.tb = TimerB::default();
            self.isr_step = 1;
        } else {
            // the pre-roll row's work, from the last period write (2 colour clocks ago)
            let row = std::mem::take(&mut self.owed);
            hw.wait(((EIGHT_ROW_BASE + row / EIGHT_ROW_DIV) as u64).saturating_sub(2));
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
    fn end_sound(&mut self, hw: &mut Hw) {
        hw.dmacon(0x000f);
        hw.bus.write_word(0xdff09a, 0x0080);
        hw.bus.write_word(0xdff09c, 0x0080);
        for v in self.voices.iter_mut() {
            v.wave_number_old = 0;
        }
    }

    /// The interrupt handler: one tick.
    fn handler(&mut self, hw: &mut Hw) {
        hw.bus.write_word(0xdff09c, 0x0080);
        if self.eight {
            self.half ^= 0xa00;
            let mut at = 0;
            for ch in 0..4 {
                hw.wait(EIGHT_LOC_AT[ch] - at);
                at = EIGHT_LOC_AT[ch];
                hw.loc(ch, self.mix[1] + self.half as u32 + 0x1400 * ch as u32);
            }
            if self.first {
                for ch in 0..4 {
                    hw.wait(EIGHT_FIRST_VOL_AT[ch] - at);
                    at = EIGHT_FIRST_VOL_AT[ch];
                    hw.vol(ch, 0x40);
                }
            }
        }
        let mut spent = 0;
        self.inst_started = false;
        // what the last tick took off its time (the tail) does not come off the next one's
        self.owed = self.owed.max(0.0);
        let first = self.first;
        if self.first {
            self.first = false;
            if !self.eight {
                // the first tick writes the four volumes, a move apart, instead of reading the tune
                for ch in 0..4 {
                    hw.wait(FIRST_VOL_AT[ch] - spent);
                    spent = FIRST_VOL_AT[ch];
                    hw.vol(ch, 0);
                }
            }
        } else {
            self.play_tune(hw);
            self.pay(hw);
        }
        // 4 voices, not the first tick: the tick's fixed work is spread over the voices' loop (a voice's
        // location and length writes come at the end of its effects), `LOOP_PRE` before the first voice's
        // and `VOICE_GAP` more before each next; the rest comes after the last voice
        let spread = !first && !self.eight;
        hw.wait(if first { FIRST_PLAY_TIME } else if spread { LOOP_PRE } else { PLAY_TIME } - spent);
        for v in 0..self.voice_count() {
            if spread && v > 0 {
                hw.wait(VOICE_GAP);
            }
            self.play_effects(hw, v);
            self.pay(hw);
        }
        if spread {
            hw.wait(PLAY_TIME - LOOP_PRE - 3 * VOICE_GAP);
        }
        for v in 0..self.voice_count() {
            self.period(v);
        }
        if self.eight {
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
        } else {
            self.period_volume_play(hw);
            self.dma(hw);
        }
    }

    fn amiga_volume(&self, v: usize) -> u16 {
        let x = &self.voices[v];
        let w = ((x.volume3 as u32 * x.channel_volume as u32) << 6) >> 16 & 0xffff;
        ((((w * self.master as u32) << 2) >> 16) & 0xffff) as u16
    }

    fn period_volume_play(&mut self, hw: &mut Hw) {
        let mut prev: Option<usize> = None;
        let mut written = 0;
        for v in 0..4 {
            if self.voices[v].play & P_RETRIGGER != 0 {
                continue;
            }
            if let Some(p) = prev {
                hw.wait(VOL_TO_NEXT[v] + SKIPPED_VOICE * (v - p - 1) as u64);
            }
            hw.per(v, self.voices[v].period);
            if !self.voices[v].muted {
                hw.wait(PER_TO_VOL);
                hw.vol(v, self.amiga_volume(v));
            }
            prev = Some(v);
            written |= 1 << v;
        }
        if prev.is_some() {
            // the voices that restart cost more when their DMA bit is set
            let mut adds = 0;
            for v in 0..4 {
                let x = &self.voices[v];
                if x.play & P_RETRIGGER != 0 && (x.wave_or_sample == SAMPLE || x.wave_number != x.wave_number_old) {
                    adds += 1;
                }
            }
            hw.wait(TAIL_TO_DMA[written] + TAIL_ADD * adds);
        }
    }

    /// The time from the last voice's volume write to the DMA routine's first write is
    /// longer than a voice's (VOL_TO_PER) by what the original spends on the voices that
    /// restart, which write nothing here. It is taken off the effects' time, so that the
    /// first write comes when the original's does and the tick is as long as before.
    fn tail_extra(&self) -> f64 {
        let mut last = None;
        let mut written = 0;
        for v in 0..4 {
            if self.voices[v].play & P_RETRIGGER == 0 {
                last = Some(v);
                written += 1;
            }
        }
        match last {
            Some(l) => (TAIL[l][written - 1] - VOL_TO_PER) as f64,
            None => 0.0,
        }
    }

    /// The DMA routine, at a tick's end: the voices that restart (and those of earlier
    /// ticks whose restart has not run yet) are switched off, timer B starts its wait (100
    /// E-clocks, or the longest period those voices played on the last tick) and the
    /// timer B interrupt will take its first step.
    fn dma(&mut self, hw: &mut Hw) {
        let mut mask = self.dma_mask;
        let mut wait = 100u16;
        for v in 0..4 {
            let x = &mut self.voices[v];
            if x.play & P_RETRIGGER == 0 {
                continue;
            }
            let (n, old) = (x.wave_number, x.wave_number_old);
            x.wave_number_old = n;
            if x.wave_or_sample != SAMPLE && n == old {
                continue;
            }
            mask |= 1 << v;
            wait = wait.max(self.prev_period[v]);
        }
        hw.dmacon(mask);
        self.dma_mask = mask;
        for v in 0..4 {
            self.prev_period[v] = self.voices[v].period;
        }
        hw.wait(OFF_TO_TB_LOW + TB_LOW_TO_HIGH);
        self.tb.write_high(hw.bus.clock, wait);
        self.isr_step = 1;
        hw.wait(TICK_RETURN);
    }

    /// Lets the time pass to `t`, if it has not.
    fn until(hw: &mut Hw, t: u64) {
        let now = hw.bus.clock;
        if t > now {
            hw.wait(t - now);
        }
    }

    /// What happens before the tick timer's next underflow `due`: a timer B interrupt
    /// that is pending is taken (when the tick is not yet due again, and also when it is,
    /// by the tick's own call), and a timer B that underflows before `due` is waited for.
    fn until_next_tick(&mut self, hw: &mut Hw, due: u64) {
        loop {
            let now = hw.bus.clock;
            self.tb.sync(now);
            if now >= due {
                return;
            }
            if self.tb.pending {
                self.tb.pending = false;
                self.timer_b_interrupt(hw);
            } else if self.tb.running && self.tb.fire < due {
                hw.wait(self.tb.fire - now);
            } else {
                return;
            }
        }
    }

    /// The CIA-B timer B interrupt: the first step starts the voices, the second sets their
    /// loops. The DMA routine of every tick sets the first step again, so a timer B
    /// interrupt that waited for a tick to end runs the first step again, and the second
    /// (which clears the DMA mask) never comes while the ticks overrun.
    fn timer_b_interrupt(&mut self, hw: &mut Hw) {
        let c = hw.bus.clock;
        if self.isr_step == 1 {
            self.isr_step = 2;
            let mut last: Option<(usize, u64)> = None;
            for v in 0..4 {
                if self.voices[v].play & P_RETRIGGER == 0 {
                    continue;
                }
                self.voices[v].play &= !P_RETRIGGER;
                let mut at = match last {
                    None => START_FIRST[v],
                    Some((l, per)) => per + START_NEXT[v - l - 1],
                };
                if !self.voices[v].muted {
                    Self::until(hw, c + at);
                    let vol = self.amiga_volume(v);
                    hw.vol(v, vol);
                    at += VOL_TO_LOC;
                }
                Self::until(hw, c + at);
                let x = &self.voices[v];
                hw.loc(v, x.wave_pointer);
                at += LOC_TO_LEN;
                Self::until(hw, c + at);
                hw.len(v, x.wave_length);
                at += LEN_TO_PER;
                Self::until(hw, c + at);
                hw.per(v, x.period);
                last = Some((v, at));
            }
            let on = last.map_or(START_NONE_ON, |(l, per)| per + START_LAST_TO_ON[l]);
            Self::until(hw, c + on);
            hw.dmacon(0x8000 | self.dma_mask);
            Self::until(hw, c + on + ON_TO_TB_LOW + TB_LOW_TO_HIGH);
            self.tb.write_high(hw.bus.clock, 150);
            Self::until(hw, c + on + ON_TO_TB_LOW + TB_LOW_TO_HIGH + TB_HIGH_TO_END);
        } else {
            let mut last: Option<(usize, u64)> = None;
            for v in 0..4 {
                if self.dma_mask & 1 << v == 0 {
                    continue;
                }
                let at = match last {
                    None => LOOP_FIRST[v],
                    Some((l, len)) => len + LOOP_NEXT[v - l - 1],
                };
                Self::until(hw, c + at);
                let x = &self.voices[v];
                hw.loc(v, x.repeat_pointer);
                Self::until(hw, c + at + LOC_TO_LEN);
                hw.len(v, x.repeat_length);
                last = Some((v, at + LOC_TO_LEN));
            }
            let end = last.map_or(LOOP_NONE_END, |(l, len)| len + LOOP_LAST_TO_END[l]);
            Self::until(hw, c + end);
            self.dma_mask = 0;
        }
    }

    // ---- the tune ----

    fn play_tune(&mut self, hw: &mut Hw) {
        for v in 0..self.voice_count() {
            self.play_voice(hw, v);
        }
    }

    fn play_voice(&mut self, hw: &mut Hw, v: usize) {
        let seq = rl(hw, self.tune + TU_SEQ + 4 * v as u32);
        let x = &mut self.voices[v];
        if x.voice_off {
            return;
        }
        x.speed_counter = x.speed_counter.wrapping_sub(1);
        if x.speed_counter != 0 {
            return;
        }
        let mut spd = x.speed;
        x.part_groove = !x.part_groove;
        if x.part_groove && x.groove != 0 {
            spd = x.groove;
        }
        x.speed_counter = spd;
        let x = &mut self.voices[v];
        x.speed_part = 0;
        x.groove_part = 0;
        for _ in 0..256 {
            self.owed += SEQ_ENTRY;
            let x = &mut self.voices[v];
            let pos = x.tune_position;
            let data = rw(hw, seq + 2 * pos as u32);
            let arg = (data & 0x1f) as u8;
            let target = (data >> 8) as u8;
            if data & 0x20 != 0 {
                match (data & 0xc0) >> 6 {
                    1 => {
                        x.voice_off = true;
                        x.ended = true;
                        return;
                    }
                    2 => {
                        if x.tune_jump_counter != 0 {
                            x.tune_jump_counter -= 1;
                            if x.tune_jump_counter != 0 {
                                x.tune_position = target;
                            } else {
                                x.tune_position = x.tune_position.wrapping_add(1);
                            }
                        } else if pos > target {
                            x.tune_jump_counter = arg;
                            x.tune_position = target;
                        } else {
                            x.tune_position = x.tune_position.wrapping_add(1);
                        }
                        continue;
                    }
                    3 => {
                        if x.tune_wait != 0 {
                            x.tune_wait -= 1;
                            if x.tune_wait != 0 {
                                return;
                            }
                            x.tune_position = x.tune_position.wrapping_add(1);
                            continue;
                        }
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
            if self.parse_part(hw, v, data) {
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
            self.owed += PART_LINE;
            let x = &mut self.voices[v];
            let pos = x.part_position;
            x.part_position = x.part_position.wrapping_add(1) & 0x7f;
            if x.part_position == 0 {
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
                if pos == 0 {
                    self.end_sound(hw);
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
            let jump = x.part_note & 0x7f;
            if x.part_note & 0x80 != 0 {
                if x.part_jump_counter != 0 {
                    x.part_jump_counter -= 1;
                    if x.part_jump_counter != 0 {
                        x.part_position = jump;
                    }
                } else if pos > jump {
                    x.part_jump_counter = x.part_instrument;
                    x.part_position = jump;
                }
                continue;
            }
            if jump != 0 {
                x.transpose = ((x.transpose_number as i8 as i16) << 5) as i16;
            }
            self.check_instrument(hw, v);
            return true;
        }
    }

    fn check_instrument(&mut self, hw: &mut Hw, v: usize) {
        let pi = self.voices[v].part_instrument;
        self.owed += CHECK_INSTRUMENT;
        if pi != 0 {
            self.owed += CHECK_NEW_INSTRUMENT;
        }
        if self.voices[v].part_note != 0 {
            self.owed += CHECK_NOTE;
        }
        if pi != 0 {
            let inst = rl(hw, self.tab + T_INSTS + 4 * pi as u32);
            let x = &mut self.voices[v];
            x.instrument = inst;
            if pi != x.old_instrument {
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
        if self.voices[v].part_note == 0 || inst == 0 {
            return;
        }
        let x = &mut self.voices[v];
        x.arp_wait = false;
        if x.arp & (A_USE_TABLE | A_ENABLED) == 0 {
            if rb(hw, inst + I_EFFECT1) & E1_ARPEGGIO == 0 {
                return;
            }
            x.arp |= A_ENABLED;
        }
        if x.restart == 0 && x.part_instrument == 0 {
            return;
        }
        x.arp |= A_WILL_SET_NOTE;
        self.owed += ARP_START;
        let mut num = rw(hw, inst + I_ARP_TABLE) as u32;
        if x.arp & A_USE_TABLE != 0 {
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
                let e = (rb(hw, line + 2 + 2 * k), rb(hw, line + 3 + 2 * k));
                self.arpeggio_effect(v, e);
            }
            let x = &mut self.voices[v];
            if tr < 0 {
                tr = tr.wrapping_add((61u8.wrapping_add(x.arp_note)) as i8);
            } else {
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
        let inst = self.voices[v].instrument;
        if self.voices[v].arp_wait || inst == 0 {
            return;
        }
        self.owed += PLAY_INSTRUMENT;
        let mut is = inst;
        let mut st = rb(hw, inst + IS_TYPE);
        let x = &mut self.voices[v];
        let mut setup = true;
        if x.restart & (R_PART_FX | R_ARP) != 0 {
            x.wave_number = rb(hw, inst + IS_NUMBER);
        } else if x.part_instrument == 0 {
            setup = false;
        } else if x.pitch_slide == FX_PORTAMENTO || x.part_note == 0 {
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
            x.wave_number = rb(hw, inst + IS_NUMBER);
        }
        if setup {
            let mut ptr = rl(hw, is + IS_POINTER);
            if ptr == 0 {
                return;
            }
            let x = &mut self.voices[v];
            if rb(hw, inst + I_TRANSPOSABLE) == 0 {
                x.transpose = 0;
            }
            let mut flag = rb(hw, inst + I_ETPF_BITS);
            if x.effect1 & E1_HOLD_SUSTAIN != 0 {
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
                let len = rw(hw, inst + IS_LENGTH);
                Self::fix_wave_length(x, len, st, ptr);
            } else {
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
                x.volume1 = rw(hw, inst + I_VOLUME).wrapping_mul(16);
            }
        }
        // PlayNote
        let x = &mut self.voices[v];
        let mut vol = if x.volume != 0 { x.volume_set as i16 } else { x.volume1 as i16 };
        if x.volume_add != 0 {
            vol = vol.wrapping_add(x.volume_add_number).clamp(0, 1024);
        }
        x.volume1 = vol as u16;
        x.volume2 = vol as u16;
        x.volume3 = vol as u16;
        if x.pitch_slide == FX_PORTAMENTO {
            return;
        }
        let slide = rb(hw, inst + I_SLIDE_SPEED);
        if slide != 0 && x.part_note != 0 {
            x.pitch_slide_speed = slide as u16;
            let new_note = x.note.wrapping_add(x.pitch_slide_note);
            x.pitch_slide_to = (x.part_note as i16) << 5;
            x.pitch_slide_down = (x.pitch_slide_to as i32) < new_note as i32;
            if x.instrument_pitch_slide != 0 {
                self.inst_play(hw, v);
                return;
            }
            x.instrument_pitch_slide = FX_PORTAMENTO;
        }
        let x = &mut self.voices[v];
        let had = x.arp & A_WILL_SET_NOTE != 0;
        x.arp &= !A_WILL_SET_NOTE;
        if !had {
            if x.part_note == 0 {
                return;
            }
            x.note = (x.part_note as u16) << 5;
            x.arp_volume_slide = 0;
            x.arp_pitch_slide = 0;
            x.arp_pitch_slide_note = 0;
        }
        x.semi_tone = ((rw(hw, is + IS_SEMI_TONE) as i16) << 5) as i16;
        if x.play & P_KEEP_FINE_TUNE == 0 {
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
        if x.part_instrument != 0 || x.restart & (R_PART_FX | R_ARP) != 0 {
            self.inst_play(hw, v);
        }
    }

    fn fix_wave_length(x: &mut Voice, len: u16, st: u8, ptr: u32) {
        let (off, sh) = match st {
            WAVE16 => (256 + 128 + 64 + 32, 4),
            WAVE32 => (256 + 128 + 64, 3),
            WAVE64 => (256 + 128, 2),
            WAVE128 => (256, 1),
            _ => (0, 0),
        };
        let p = ptr.wrapping_add(off);
        x.wave_pointer = p;
        x.repeat_pointer = p;
        x.repeat_original = p;
        x.wave_length = len >> sh;
        x.repeat_length = len >> sh;
    }

    /// An instrument effect's counter, as InstPlay sets it up.
    fn init_counter(hw: &Hw, inst: u32, o: u32, step: bool, init_flag: bool, init: &mut u8, pi: u8, c: &mut Counter, speed: &mut u16) -> bool {
        let w = |k: u32| rw(hw, inst + o + 2 * k);
        let mut fresh = false;
        let mut check = false;
        c.step = step;
        if step {
            *speed = w(4);
            c.turns = 0;
            check = true;
        } else {
            c.turns = w(4) as i16;
            if !init_flag {
                *init = 0;
                fresh = true;
            } else {
                check = true;
            }
        }
        if check {
            let old = *init;
            *init = pi;
            if old == pi {
                c.delay = w(5);
            } else {
                fresh = true;
            }
        }
        if fresh {
            c.counter = w(0);
            c.speed = w(3) as i16;
            c.repeat = w(1);
            c.repeat_end = w(2);
            if w(0) > w(1) {
                c.speed = c.speed.wrapping_neg();
            }
            c.delay = w(5);
        }
        fresh
    }

    fn inst_play(&mut self, hw: &mut Hw, v: usize) {
        let inst = self.voices[v].instrument;
        self.owed += INST_PLAY;
        if self.eight {
            self.owed += INST_START_VOICE;
            if !std::mem::replace(&mut self.inst_started, true) {
                self.owed += INST_START_TICK;
            }
        } else {
            self.owed += INST_START_BY_VOICE[v & 3];
        }
        let x = &mut self.voices[v];
        if x.pitch_slide != FX_PORTAMENTO && !x.arp_wait {
            for (flag, price) in [(E2_PHASE, INST_PHASE), (E2_RESONANCE, INST_RESONANCE), (E2_FILTER, INST_FILTER), (E2_MIX, INST_MIX)] {
                if x.effect2 & flag != 0 {
                    self.owed += price;
                }
            }
            if x.effect1 & E1_ENVELOPE != 0 {
                self.owed += INST_ENVELOPE;
            }
            x.play |= P_RETRIGGER;
            let w = |o: u32| rw(hw, inst + o);
            if x.vibrato == 0 && x.effect1 & E1_VIBRATO != 0 {
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
            if x.tremolo == 0 && x.effect1 & E1_TREMOLO != 0 {
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
                x.phase_type = w(I_PHASE_TYPE);
                let (s, i) = (x.arg1 & F_PHASE_STEP != 0, x.arg1 & F_PHASE_INIT != 0);
                Self::init_counter(hw, inst, I_PHASE, s, i, &mut x.phase_init, pi, &mut x.phase_counter, &mut x.phase_speed);
            }
            x.boost = rb(hw, inst + I_BOOST);
            if x.effect2 & E2_RESONANCE != 0 {
                x.resonance_amp = rb(hw, inst + I_RESONANCE_AMP);
                let (s, i) = (x.arg2 & M_RESONANCE_STEP != 0, x.arg2 & M_RESONANCE_INIT != 0);
                if Self::init_counter(hw, inst, I_RESONANCE, s, i, &mut x.resonance_init, pi, &mut x.resonance_counter, &mut x.resonance_speed) {
                    x.resonance_last_init = true;
                }
            }
            if x.effect2 & E2_FILTER != 0 {
                x.filter_type = rb(hw, inst + I_FILTER_TYPE);
                let (s, i) = (x.arg1 & F_FILTER_STEP != 0, x.arg1 & F_FILTER_INIT != 0);
                if Self::init_counter(hw, inst, I_FILTER, s, i, &mut x.filter_init, pi, &mut x.filter_counter, &mut x.filter_speed) {
                    x.filter_last_init = true;
                }
            }
            if x.effect2 & E2_MIX != 0 {
                x.mix_wave = rb(hw, inst + I_MIX_WAVE);
                let (s, i) = (x.arg2 & M_MIX_STEP != 0, x.arg2 & M_MIX_INIT != 0);
                Self::init_counter(hw, inst, I_MIX, s, i, &mut x.mix_init, pi, &mut x.mix_counter, &mut x.mix_speed);
            }
            if x.effect2 & E2_TRANSFORM != 0 {
                x.transform_waves[0] = rb(hw, inst + IS_NUMBER);
                for k in 0..5 {
                    x.transform_waves[k + 1] = rb(hw, inst + I_TRANSFORM_WAVES + k as u32);
                }
                let (s, i) = (x.arg1 & F_TRANSFORM_STEP != 0, x.arg1 & F_TRANSFORM_INIT != 0);
                Self::init_counter(hw, inst, I_TRANSFORM, s, i, &mut x.transform_init, pi, &mut x.transform_counter, &mut x.transform_speed);
            }
            if x.effect1 & E1_LOOP != 0 {
                let ll = w(I_LOOP + 6);
                if rb(hw, inst + IS_TYPE) != SAMPLE || ll == 0 {
                    x.effect1 &= !E1_LOOP;
                } else {
                    let (mut init, mut part_init, mut check) = (false, false, false);
                    if x.arg2 & M_LOOP_STEP != 0 {
                        x.loop_speed = w(I_LOOP + 14) as i16;
                        x.loop_turns = 0;
                        check = true;
                    } else {
                        x.loop_turns = w(I_LOOP + 14) as i16;
                        if x.arg2 & M_LOOP_INIT == 0 {
                            x.loop_init = 0;
                            init = true;
                        } else {
                            check = true;
                        }
                    }
                    if check {
                        let old = x.loop_init;
                        x.loop_init = pi;
                        if old == pi {
                            x.play &= !P_RETRIGGER;
                            part_init = true;
                        } else {
                            init = true;
                        }
                    }
                    if init {
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
            x.effect2 = 0;
        }
    }

    fn period(&mut self, v: usize) {
        let x = &mut self.voices[v];
        let mut note = x
            .note
            .wrapping_add(x.vibrato_note)
            .wrapping_add(x.pitch_slide_note)
            .wrapping_add(x.arp_pitch_slide_note)
            .wrapping_add(x.semi_tone as u16)
            .wrapping_add(x.fine_tune as u16)
            .wrapping_add(x.pitch_add as u16) as i16;
        if x.arp & A_NOTE_ABSOLUTE == 0 && x.transpose != 0 {
            note = note.wrapping_add(x.transpose);
        }
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
            let (e, a) = self.voices[v].part_effects[k];
            if e != 0 || a != 0 {
                self.owed += PART_FX;
            }
            self.part_effect(hw, v, e, a);
        }
    }

    fn part_effect(&mut self, hw: &mut Hw, v: usize, e: u8, a: u8) {
        let nv = self.voice_count();
        let x = &mut self.voices[v];
        let a16 = a as u16;
        match e {
            0x01 => {
                x.pitch_slide = e;
                x.pitch_slide_down = false;
                if a != 0 {
                    x.pitch_slide_speed = a16;
                }
                x.pitch_slide_to = 59 * 32 + 32;
            }
            0x02 => {
                x.pitch_slide = e;
                x.pitch_slide_down = true;
                if a != 0 {
                    x.pitch_slide_speed = a16;
                }
                x.pitch_slide_to = 0;
            }
            0x03 => {
                x.pitch_slide = e;
                if a != 0 {
                    x.pitch_slide_speed = a16;
                }
                let note = x.note.wrapping_add(x.pitch_slide_note);
                let mut to = x.part_note as u16;
                if to != 0 {
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
                    x.pitch_slide = 0;
                }
            }
            0x04 => x.instrument_pitch_slide = 0,
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
                x.fine_tune = (a as i8).clamp(-31, 31) as i16;
                x.play |= P_KEEP_FINE_TUNE;
            }
            0x10 => {
                x.volume = e;
                // no clamp in the original (NostalgicPlayer clamps to 1024)
                x.volume_set = a16 << 4;
            }
            0x11 | 0x12 => {
                x.volume_slide = e;
                if a != 0 {
                    x.volume_slide_speed = a16;
                }
            }
            0x13 => x.volume_slide_to = a16 << 4,
            0x14 => {
                x.volume_slide = e;
                if a != 0 {
                    x.volume_slide_speed = a16;
                }
                let vol = x.volume1;
                if x.volume_slide_to != vol {
                    x.volume_slide_down = x.volume_slide_to < vol;
                    x.volume_slide_off = false;
                } else {
                    x.volume_slide_off = true;
                }
            }
            0x15 => {
                x.volume_add = e;
                if a != 0 {
                    x.volume_add_number = (a16 << 4) as i16;
                }
            }
            0x16 => {
                x.volume_add = e;
                if a != 0 {
                    x.volume_add_number = -((a16 << 4) as i16);
                }
            }
            0x17 => {
                if a != 0 {
                    x.tremolo_speed = a16;
                }
            }
            0x18 | 0x19 => {
                x.tremolo = e;
                if x.part_note != 0 {
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
            0x20 => x.channel_volume = a16 << 4,
            0x21 | 0x22 => {
                x.channel_volume_slide = e;
                if a != 0 {
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
                for k in 0..nv {
                    self.voices[k].channel_volume = a16 << 4;
                }
            }
            0x30 => self.master = a16 << 4,
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
                if a != 0 {
                    let s = a.min(31);
                    x.speed_part = 1;
                    x.speed = s;
                    if x.groove == 0 || !x.part_groove {
                        x.speed_counter = s;
                    }
                }
            }
            0x41 => {
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
                if a == 0 {
                } else if a < 32 {
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
                            w.speed = a;
                        }
                    }
                } else {
                    self.tempo = a16;
                    self.timer = (0x1b0f88 / a as u32) as u16;
                    if !self.started && !self.eight {
                        self.preload = Some(self.timer);
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
                x.arp |= A_USE_TABLE;
                x.arp_table = a;
            }
            0x45 => {
                x.arp |= A_ONE_STEP | A_USE_TABLE;
                x.arp_table = a;
            }
            0x46 => {
                x.effect1 |= E1_HOLD_SUSTAIN;
                x.arg1 &= !F_HOLD_SUSTAIN;
                if a != 0 {
                    x.arg1 |= F_HOLD_SUSTAIN;
                }
            }
            0x47 => {
                // the LED filter, through the CIA
                let b = hw.bus.read_byte(0xbfe001);
                hw.bus.write_byte(0xbfe001, if a != 0 { b & !2 } else { b | 2 });
            }
            0x48 => {
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
                if a != 0 {
                    let s = rl(hw, self.tab + T_SAMPLES + 4 * a as u32);
                    let x = &mut self.voices[v];
                    x.wave_sample = s;
                    x.arp |= A_USE_WAVE_SAMPLE;
                }
            }
            0x4b => {
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
                x.pitch_slide = FX_PORTAMENTO;
                x.pt_pitch_slide = e;
                if a != 0 {
                    x.pt_pitch_slide_speed2 = a16;
                }
                let p = Self::get_period(x, x.note).wrapping_add(x.pt_pitch_slide_note);
                if x.part_note != 0 {
                    let p2 = Self::get_period(x, (x.part_note as u16) << 5);
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
                x.volume_slide = e;
                if a != 0 {
                    x.volume_slide_speed = a16;
                }
            }
            0xe8 => {
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
        if self.voices[v].play & P_RETRIGGER == 0 {
            self.owed += VOICE_PLAY;
            if self.voices[v].instrument != 0 && !(self.voices[v].effect1 & E1_WAVE_SAMPLE_LOOP != 0 && self.voices[v].wave_or_sample == SAMPLE) {
                self.owed += VOICE_PLAYING;
            }
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
        // the writes at the end of the voice's effects come after their time
        if v + 1 == self.voice_count() {
            self.owed -= self.tail_extra();
        }
        self.pay(hw);
        let eight = self.eight;
        let x = &mut self.voices[v];
        if !eight {
            let rp = x.repeat_pointer;
            if rp >= x.bufs && rp <= x.bufs + B_FILT {
                let wb = x.wave_buffer;
                x.repeat_pointer = wb;
                if x.wave_or_sample != SAMPLE || x.effect1 & E1_LOOP != 0 {
                    x.wave_pointer = wb;
                }
                let n = (x.repeat_length >> 3) as u32 * 16;
                self.owed += WAVE_COPY + WAVE_COPY_BYTE * n as f64;
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
        if x.play & P_RETRIGGER == 0 && x.effect1 & E1_LOOP != 0 {
            if !eight {
                hw.loc(ch, x.repeat_pointer);
                hw.len(ch, x.loop_length);
            }
            x.repeat_length = x.loop_length;
        }
        if !x.arp_wait {
            x.part_note = 0;
        }
        x.play &= P_RETRIGGER;
    }

    fn slide_volume(&mut self, v: usize) {
        let x = &mut self.voices[v];
        let first = x.play & P_FIRST_ROW_TICK != 0;
        let vol;
        let mut cost = 0.0;
        if x.volume_slide == FX_SLIDE_TO_VOLUME {
            if x.volume_slide_off {
                return;
            }
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
            vol = (x.volume1 as i16).wrapping_add(x.volume_slide_speed as i16).min(1024);
        } else if x.volume_slide == FX_VOLUME_DOWN || (x.volume_slide == FX_PT_VOLUME_DOWN && !first) {
            vol = (x.volume1 as i16).wrapping_sub(x.volume_slide_speed as i16).max(0);
            cost = if x.volume_slide == FX_VOLUME_DOWN { SLIDE_VOLUME_DOWN } else { SLIDE_PT_VOLUME_DOWN };
        } else {
            return;
        }
        x.volume1 = vol as u16;
        x.volume2 = vol as u16;
        x.volume3 = vol as u16;
        self.owed += cost;
    }

    fn slide_channel_volume(&mut self, v: usize) {
        let x = &mut self.voices[v];
        let vol;
        let mut cost = 0.0;
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
            vol = (x.channel_volume as i16).wrapping_add(x.channel_volume_slide_speed as i16).min(1024);
            cost = SLIDE_CHANNEL_UP;
        } else if x.channel_volume_slide == FX_CHANNEL_DOWN {
            vol = (x.channel_volume as i16).wrapping_sub(x.channel_volume_slide_speed as i16).max(0);
            cost = SLIDE_CHANNEL_DOWN;
        } else {
            return;
        }
        x.channel_volume = vol as u16;
        self.owed += cost;
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
            if x.pitch_slide == 0 && x.instrument_pitch_slide == 0 {
                return;
            }
            if x.pitch_slide_to < 0 {
                return;
            }
            self.owed += SLIDE_NOTE;
            if x.pitch_slide_down {
                x.pitch_slide_note = x.pitch_slide_note.wrapping_sub(x.pitch_slide_speed);
                let n = x.note.wrapping_add(x.pitch_slide_note).wrapping_sub(x.pitch_slide_to as u16) as i16;
                if n <= 0 {
                    x.pitch_slide_note = x.pitch_slide_note.wrapping_sub(n as u16);
                    x.pitch_slide_to = -1;
                }
            } else {
                x.pitch_slide_note = x.pitch_slide_note.wrapping_add(x.pitch_slide_speed);
                let n = x.note.wrapping_add(x.pitch_slide_note).wrapping_sub(x.pitch_slide_to as u16) as i16;
                if n >= 0 {
                    x.pitch_slide_note = x.pitch_slide_note.wrapping_sub(n as u16);
                    x.pitch_slide_to = -1;
                }
            }
        } else if x.play & P_FIRST_ROW_TICK == 0 && x.pt_pitch_slide_to >= 0 {
            let speed = if x.pt_pitch_slide == FX_PT_PORTAMENTO { x.pt_pitch_slide_speed2 } else { x.pt_pitch_slide_speed };
            if x.pt_pitch_slide_down {
                x.pt_pitch_slide_note = x.pt_pitch_slide_note.wrapping_add(speed);
                let p = Self::get_period(x, x.note).wrapping_add(x.pt_pitch_slide_note).wrapping_sub(x.pt_pitch_slide_to as u16) as i16;
                if p >= 0 {
                    x.pt_pitch_slide_note = x.pt_pitch_slide_note.wrapping_sub(p as u16);
                    x.pt_pitch_slide_to = -1;
                }
            } else {
                x.pt_pitch_slide_note = x.pt_pitch_slide_note.wrapping_sub(speed);
                let p = Self::get_period(x, x.note).wrapping_add(x.pt_pitch_slide_note).wrapping_sub(x.pt_pitch_slide_to as u16) as i16;
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
        let x = &mut self.voices[v];
        match e {
            AE_SLIDE_UP => {
                x.arp_pitch_slide = e;
                x.arp_pitch_down = false;
                if a != 0 {
                    x.arp_pitch_slide_speed = a as u16;
                    x.arp_pitch_slide_to = 59 * 32 + 32;
                }
            }
            AE_SLIDE_DOWN => {
                x.arp_pitch_slide = e;
                x.arp_pitch_down = true;
                if a != 0 {
                    x.arp_pitch_slide_speed = a as u16;
                    x.arp_pitch_slide_to = 0;
                }
            }
            AE_SET_VOLUME => {
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
            x.phase_init = 0;
        }
        // tested on effect 1's flags, as the original does
        if x.arg1 & F_PHASE_STEP != 0 {
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
            if x.arp & (A_USE_TABLE | A_ENABLED) == 0 {
                return;
            }
            x.arp_speed_counter = x.arp_speed_counter.wrapping_sub(1);
            if x.arp_speed_counter != 0 {
                self.owed += ARP_WAIT;
                return;
            }
            self.owed += ARP_STEP;
            let mut spd = rb(hw, inst + I_ARP_SPEED);
            x.arp_groove = !x.arp_groove;
            let g = rb(hw, inst + I_ARP_GROOVE);
            if x.arp_groove && g != 0 {
                spd = g;
            }
            x.arp_speed_counter = spd;
        }
        let x = &self.voices[v];
        let mut num = rw(hw, inst + I_ARP_TABLE) as u32;
        if x.arp & A_USE_TABLE != 0 {
            num = x.arp_table as u32;
        }
        let arp = rl(hw, self.tab + T_ARPS + 4 * (num & 0xff));
        loop {
            self.owed += ARP_LINE;
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
                x.arp = 0;
                x.note = x.arp_calc_note;
                return;
            }
            if tr == 62 {
                if sn != pos {
                    x.arp_position = sn;
                }
                continue;
            }
            x.wave_number = if sn == 0 { rb(hw, inst + IS_NUMBER) } else { sn };
            x.restart = 0;
            x.arp_pitch_slide = 0;
            x.arp_volume_slide = 0;
            for k in 0..2 {
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
                Self::fix_wave_length(x, len, st, rl(hw, s + IS_POINTER));
            } else {
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
        self.owed += ENVELOPE;
        let set3 = |x: &mut Voice, d: u16| {
            x.volume3 = ((d as u32 * x.volume2 as u32) >> 6).min(1024) as u16;
        };
        let mut i = 0usize;
        let mut d = x.envelope[i];
        i += 1;
        if d == 0 {
            self.owed += ENVELOPE_END1;
            d = x.envelope[i];
            i += 1;
            if d == 0 {
                d = x.envelope[i];
                i += 1;
                if d == 0 {
                    self.owed += ENVELOPE_END2;
                    if x.arg1 & F_HOLD_SUSTAIN != 0 {
                        let s = x.envelope[i + 7];
                        set3(x, s);
                        return;
                    }
                    d = x.envelope[i];
                    i += 1;
                    if d == 0 {
                        let s = x.envelope[i + 7];
                        set3(x, s);
                        return;
                    }
                }
            }
        }
        self.owed += ENVELOPE_STEP;
        let mut vol = x.envelope_volume.wrapping_add(x.envelope[i + 3]);
        x.envelope_volume = vol;
        vol >>= 8;
        d -= 1;
        i -= 1;
        x.envelope[i] = d;
        if d == 0 {
            vol = x.envelope[i + 8];
        }
        set3(x, vol);
    }

    fn tremolo_play(&mut self, v: usize) {
        let x = &mut self.voices[v];
        if x.tremolo == FX_PT_TREMOLO {
            if x.play & P_FIRST_ROW_TICK != 0 {
                return;
            }
            let wd = Self::pt_wave(x.pt_tremolo_wave, x.pt_tremolo_position);
            let mut t = ((x.pt_tremolo_command & 15) as i32 * wd) >> 2;
            if x.pt_tremolo_position < 0 {
                t = -t.abs();
            } else {
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
        if x.tremolo == 0 {
            if x.effect1 & E1_TREMOLO == 0 {
                return;
            }
            if x.tremolo_delay != 0 {
                x.tremolo_delay -= 1;
                return;
            }
        }
        self.owed += TREMOLO;
        if x.tremolo_attack_length == 0 {
            x.tremolo_depth_now = x.tremolo_depth;
        } else {
            x.tremolo_depth_now = x.tremolo_depth_now.wrapping_add(x.tremolo_attack_speed);
            x.tremolo_attack_length -= 1;
            if x.tremolo_attack_length == 0 {
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
            t = t.wrapping_add(16);
        }
        let mut vol = x.volume1 as i32;
        if x.volume1 != 0 {
            vol = (vol + t as i32).clamp(0, 1024);
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
            if x.effect1 & E1_VIBRATO == 0 {
                return;
            }
            if x.vibrato_delay != 0 {
                x.vibrato_delay -= 1;
                return;
            }
        }
        self.owed += VIBRATO;
        if x.vibrato_attack_length == 0 {
            x.vibrato_depth_now = x.vibrato_depth;
        } else {
            x.vibrato_depth_now = x.vibrato_depth_now.wrapping_add(x.vibrato_attack_speed);
            x.vibrato_attack_length -= 1;
            if x.vibrato_attack_length == 0 {
                x.vibrato_depth_now = x.vibrato_depth;
            }
        }
        let mut wd = wave_table(x.vibrato_wave)[(x.vibrato_counter >> 2) as usize & 0x7f] as i32;
        if x.vibrato_down {
            wd = (-wd) as i8 as i32;
        }
        let mut n = ((wd * (x.vibrato_depth_now >> 8) as i32) >> 4) as i16;
        if n < 0 {
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
        let step = x.arg2 & M_LOOP_STEP != 0;
        if !step {
            self.owed += LOOP_MOVE;
            x.loop_delay = x.loop_delay.wrapping_sub(1);
            if x.loop_delay >= 0 {
                let p = x.loop_sample.wrapping_add(2 * x.loop_counter_save as u32);
                x.repeat_pointer = p;
                x.repeat_original = p;
                return;
            }
            x.loop_delay = 0;
            if x.loop_wait != 0 {
                x.loop_wait_counter = x.loop_wait_counter.wrapping_sub(1);
                if x.loop_wait_counter >= 0 {
                    return;
                }
                x.loop_wait_counter = x.loop_wait as i16;
            }
            self.owed += LOOP_COUNT;
            x.loop_counter_save = x.loop_counter;
            Self::loop_count(x);
        } else if x.arg2 & M_LOOP_INIT == 0 {
            if x.part_note != 0 {
                x.loop_counter_save = x.loop_counter;
                Self::loop_count(x);
            }
        } else {
            if x.part_note != 0 {
                x.loop_counter_save = x.loop_counter;
            }
            Self::loop_count(x);
        }
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
        x.effect1 &= !E1_LOOP;
        x.mix_loop = false;
        if !eight {
            hw.loc(v & 3, data);
            hw.len(v & 3, 1);
        }
    }

    fn loop_count(x: &mut Voice) {
        if x.loop_turns < 0 {
            return;
        }
        let mut c = x.loop_counter as i32;
        c += x.loop_step;
        let (lo, hi) = if x.loop_repeat < x.loop_repeat_end {
            (x.loop_repeat, x.loop_repeat_end)
        } else {
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
    fn effect_counter(c: &mut Counter, step: bool, init: bool, part_note: u8, one_way: bool, speed: u16, phase: bool) -> u16 {
        let count = if step {
            if !init {
                if part_note == 0 {
                    false
                } else {
                    c.save = c.counter;
                    true
                }
            } else {
                if part_note != 0 {
                    c.save = c.counter;
                }
                true
            }
        } else {
            c.save = c.counter;
            true
        };
        if count {
            if one_way {
                c.one_way();
            } else {
                c.count();
            }
        }
        let prev = c.save;
        if step {
            if c.delay != 0 {
                c.delay -= 1;
            } else {
                let s = speed as i8 as i16;
                if phase {
                    c.save = c.save.wrapping_sub(s as u16);
                    if s < 0 {
                        if c.save as i16 > 512 {
                            c.save = 512;
                        }
                    } else if (c.save as i16) < 2 {
                        c.save = 2;
                    }
                } else {
                    c.save = c.save.wrapping_add(s as u16);
                    if s < 0 {
                        if (c.save as i16) < 0 {
                            c.save = 0;
                        }
                    } else if c.save > 510 {
                        c.save = 510;
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
        let x = &mut self.voices[v];
        let len = x.repeat_length as u32 * 2;
        let off = match len {
            256 => 0,
            128 => 256,
            64 => 256 + 128,
            32 => 256 + 128 + 64,
            _ => 256 + 128 + 64 + 32,
        };
        let (s, i) = (x.arg1 & F_TRANSFORM_STEP != 0, x.arg1 & F_TRANSFORM_INIT != 0);
        let mut prev = Self::effect_counter(&mut x.transform_counter, s, i, x.part_note, false, x.transform_speed, false);
        prev >>= 1;
        let mut octave = 0usize;
        let mut rem = prev as i16;
        for _ in 0..5 {
            rem = rem.wrapping_sub(256);
            if rem > 0 {
                octave += 1;
                prev = rem as u16;
            } else {
                break;
            }
        }
        let factor = prev as i32;
        self.owed += TRANSFORM_PASS + TRANSFORM_BYTE * len as f64;
        let sn = x.transform_waves[octave];
        if sn == 0 {
            return;
        }
        let mut src1 = rl(hw, self.sample(hw, sn) + IS_REPEAT).wrapping_add(off);
        let x = &self.voices[v];
        if octave == 0 {
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
        for k in 0..len {
            let s1 = rs8(hw, src1.wrapping_add(k));
            let s2 = rs8(hw, src2.wrapping_add(k));
            hw.bus.w8(buf + k, ((((s2 - s1) * factor) >> 8) + s1) as u8);
        }
    }

    /// Lets the time of the work counted since the last call pass (4-voice tunes).
    fn pay(&mut self, hw: &mut Hw) {
        let due = self.owed.floor().max(0.0);
        self.owed -= due;
        if !self.eight {
            hw.wait(due as u64);
        }
    }

    fn phase_play(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].effect2 & E2_PHASE == 0 {
            return;
        }
        let x = &mut self.voices[v];
        let (s, i) = (x.arg1 & F_PHASE_STEP != 0, x.arg1 & F_PHASE_INIT != 0);
        let mut prev = Self::effect_counter(&mut x.phase_counter, s, i, x.part_note, false, x.phase_speed, true);
        let len = x.repeat_length as u32 * 2;
        let n: u32 = if len > 128 {
            256
        } else if len > 64 {
            128
        } else if len > 32 {
            64
        } else if len > 16 {
            32
        } else {
            16
        };
        let sh = 512 / n;
        prev = prev.wrapping_add(sh as u16 - 1) >> sh.trailing_zeros();
        let src = x.repeat_pointer;
        let buf = x.bufs + B_PHASE;
        Self::set_effect_output(x, buf, true);
        let fill = x.arg1 & F_PHASE_FILL != 0;
        let ptype = x.phase_type;
        let prev = prev as u32;
        if prev >= len || prev == 0 {
            self.owed += PHASE_EARLY_PASS + PHASE_EARLY_BYTE * len as f64;
        } else {
            let (pass, interpolated, filled) = PHASE_PASS[(ptype as usize).min(3)][fill as usize];
            self.owed += pass + interpolated * prev as f64 + filled * (len - prev) as f64;
            if fill {
                self.owed += PHASE_RESTART[(ptype as usize).min(3)] * ((len - prev + prev - 1) / prev) as f64;
            }
        }
        let rd = |hw: &Hw, k: u32| rs8(hw, src.wrapping_add(k));
        if prev >= len || prev == 0 {
            for k in 0..len {
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
        let mix = |s1: i32, s2: i32| -> i32 {
            match ptype {
                1 => ((s1 + s2) + s1 * 2) >> 2,
                2 => (s1 + s2) >> 1,
                3 => ((s1 + s2) + s2 * 2) >> 2,
                _ => s1,
            }
        };
        let mut idx = 0u32;
        let mut rc = 0u32;
        for k in 0..todo {
            idx = sizer(k);
            let s1 = rd(hw, idx);
            let s2 = rd(hw, rc);
            rc += 1;
            w(hw, mix(s1, s2) as i8 as i32);
        }
        if ptype == 0 {
            if fill {
                // the start of the phase buffer again
                for k in 0..left {
                    let b = rb(hw, buf + k);
                    w(hw, b as i32);
                }
            } else if left > 0 {
                let s = rd(hw, idx);
                for _ in 0..left {
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
                        sizer(k)
                    };
                    let s1 = rd(hw, i);
                    let s2 = rd(hw, rc);
                    rc += 1;
                    w(hw, mix(s1, s2) as i8 as i32);
                }
                left -= c;
            }
        } else if left > 0 {
            let s1 = rd(hw, idx);
            for _ in 0..left {
                let s2 = rd(hw, rc);
                rc += 1;
                let out = match ptype {
                    1 => ((s1 + s1 * 2) as i16 as i32 + s2) >> 2,
                    _ => mix(s1, s2),
                };
                w(hw, out as i8 as i32);
            }
        }
    }

    fn mix_play(&mut self, hw: &mut Hw, v: usize) {
        if self.voices[v].effect2 & E2_MIX == 0 {
            return;
        }
        let x = &mut self.voices[v];
        let (s, i) = (x.arg2 & M_MIX_STEP != 0, x.arg2 & M_MIX_INIT != 0);
        let one_way = x.arg2 & M_MIX_COUNTER != 0;
        let mut prev = Self::effect_counter(&mut x.mix_counter, s, i, x.part_note, one_way, x.mix_speed, false) as u32;
        let mut len = x.repeat_length as u32 * 2;
        let off;
        if len > 128 {
            prev >>= 1;
            off = 0;
        } else if len > 64 {
            prev >>= 2;
            off = 256;
        } else if len > 32 {
            prev >>= 3;
            off = 256 + 128;
        } else if len > 16 {
            prev >>= 4;
            off = 256 + 128 + 64;
        } else {
            prev >>= 5;
            off = 256 + 128 + 64 + 32;
        }
        let pass = MIX_PASS[(x.arg2 & M_MIX_BUFF != 0) as usize][(x.mix_wave != 0) as usize][(prev == 0) as usize];
        let byte = if x.boost & B_MIX != 0 { MIX_BYTE_BOOST } else { MIX_BYTE };
        self.owed += pass + byte * len as f64;
        let src = x.repeat_pointer;
        let dest = x.bufs + B_MIXB;
        let mut mixb = dest;
        if x.arg2 & M_MIX_BUFF == 0 {
            mixb = src;
            if x.mix_wave != 0 {
                let s = rl(hw, self.tab + T_SAMPLES + 4 * x.mix_wave as u32);
                mixb = rl(hw, s + IS_REPEAT).wrapping_add(off);
            }
        }
        let x = &mut self.voices[v];
        let src2 = mixb;
        x.repeat_pointer = dest;
        if x.wave_or_sample != SAMPLE {
            x.wave_pointer = dest;
        }
        let mixb = mixb.wrapping_add(prev);
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
        let x = &mut self.voices[v];
        let (s, i) = (x.arg2 & M_RESONANCE_STEP != 0, x.arg2 & M_RESONANCE_INIT != 0);
        let prev = Self::effect_counter(&mut x.resonance_counter, s, i, x.part_note, false, x.resonance_speed, false);
        let src = x.repeat_pointer;
        let buf = x.bufs + B_RES;
        Self::set_effect_output(x, buf, true);
        let len = x.repeat_length as u32 * 2;
        self.owed += RES_PASS + RES_BYTE * len as f64;
        let mut last = x.resonance_last;
        if x.resonance_last_init {
            x.resonance_last_init = false;
            last = (rb(hw, src.wrapping_add(len).wrapping_sub(1)) as i8) >> 2;
        }
        let mut state = (last as i16) << 7;
        let cutoff = RESONANCE_LIST[((prev & 0xfffe) / 2) as usize & 0xff] as i32;
        let mut damp: u16 = 0x8000u16.wrapping_sub(RESAMPLE_LIST[x.resonance_amp as usize]);
        damp = ((damp as u32 * 0xe666) >> 16) as u16;
        let mut vel: i16 = 0;
        let mut out = last;
        let boost = if x.boost & B_RESONANCE != 0 { 6 } else { 7 };
        for k in 0..len {
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
            return;
        }
        let x = &mut self.voices[v];
        let (s, i) = (x.arg1 & F_FILTER_STEP != 0, x.arg1 & F_FILTER_INIT != 0);
        let prev = Self::effect_counter(&mut x.filter_counter, s, i, x.part_note, false, x.filter_speed, false);
        let src = x.repeat_pointer;
        let buf = x.bufs + B_FILT;
        Self::set_effect_output(x, buf, true);
        let len = x.repeat_length as u32 * 2;
        self.owed += if x.filter_type == 0 { FILTER_PASS + FILTER_BYTE * len as f64 } else { RFILTER_PASS + RFILTER_BYTE * len as f64 };
        let mut last = x.filter_last;
        let normal = x.filter_type == 0;
        if x.filter_last_init {
            x.filter_last_init = false;
            let b = rb(hw, src.wrapping_add(len).wrapping_sub(1)) as i8;
            last = if normal { b } else { b >> 1 };
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
}

//! Fred Editor, ported from NostalgicPlayer's Fred Editor player and its Fred
//! Editor (Final) converter (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! NostalgicPlayer plays the editor's own files ("Fred Editor " modules) and
//! converts the "final" ones, the editor's replayer with the song after it, to
//! them. The original player, the Fred Eagleplayer (V2.3, 25 Aug 97), plays only
//! the final ones, and with a replayer of its own: it finds the song's tables and
//! the replayer's variables in the module through the code's pc-relative
//! references, and plays with the voices' state where the replayer keeps it, in
//! the module, synth waveforms included. So does this port: it works on the
//! module in guest memory, at the addresses the original computes, starting from
//! the state the editor left there.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * voices are played from the fourth to the first; a note writes location,
//!   length, volume 0 and period, then DMA on (only for the voices the module's
//!   byte at $10 enables), and the loop follows on the next tick, as location
//!   `sample + repeat` and length `length - repeat` (words less bytes), the synth
//!   waveform for a synth sound; a repeat of 0 plays the module's empty sample
//!   (50 words), one with bit 15 set nothing;
//! * a voice is muted one tick before its next row when that row's byte is in
//!   $20..$9F (a signed compare with $A0, so notes below $20 are not), and only
//!   a voice whose DMA is on (as the player last set it) runs its effects;
//! * no instrument checks: a note before any instrument plays what the voice's
//!   instrument pointer holds, an instrument past the last or unused is played,
//!   and any type but 0 and 2 is a pulse sound;
//! * the volume is `envelope * instrument volume / 512` (not 256);
//! * the arpeggio wraps only when its position equals the count; pulse
//!   positions compare signed; a pulse waveform is built from the length's low
//!   byte doubled (a start of 0 writes 256 bytes); the blend multiplies by the
//!   sample's second half read as words, shifting logically;
//! * the portamento's step is `divs.w`'s quotient (the product when it
//!   overflows);
//! * a position list's negative entry is a jump that ends the song, and $FFFF
//!   stops the player for good (no song end), even in the middle of a tick;
//! * the writes fall at the original's colour clocks (see `cyc`): the original writes each
//!   register through the host (ENPP_PokeAdr, ENPP_PokeLen, ...), so each write is a host call
//!   at the 68000 cycles its code took since the last one, halved and rounded down, and the
//!   host's ENPP_DMAMask waits 10 lines after a DMA on.

use super::{be16, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "Fred Editor", detect, new };

/// The replayer's pc-relative references the original player follows: offset of
/// the instruction from the init routine, and its opcode. The target of each is
/// at the instruction + 2 + its displacement.
const REFS: [(u16, u16); 14] = [
    (0x062, 0x47fa), // stopped flag
    (0x070, 0x47fa), // fading flag
    (0x068, 0x47fa), // fade volume
    (0x078, 0x47fa), // fade speed
    (0x1ca, 0x45fa), // periods
    (0x00a, 0x47fa), // current subsong
    (0x000, 0x123a), // last subsong
    (0x014, 0x49fa), // tempo
    (0x010, 0x45fa), // the subsongs' start tempos
    (0x194, 0xdbfa), // offset of the instruments
    (0x046, 0xd7fa), // offset of the tracks
    (0x10a, 0x45fa), // the empty sample
    (0x022, 0x41fa), // the voices
    (0x030, 0x43fa), // the subsongs' position lists
];
const STOPPED: usize = 0;
const FADING: usize = 1;
const FADE_VOLUME: usize = 2;
const PERIODS: usize = 4;
const SUBSONG: usize = 5;
const LAST_SUBSONG: usize = 6;
const TEMPO: usize = 7;
const START_TEMPOS: usize = 8;
const INSTRUMENTS: usize = 9;
const TRACKS: usize = 10;
const EMPTY: usize = 11;
const VOICES: usize = 12;
const POSITIONS: usize = 13;

/// A loop of the original that never ends on some instruments (a pulse whose
/// start is past its end, a blend of 0): the port gives up after this many turns.
const MAX_TURNS: u32 = 256;

/// Colour clocks from the start of the song (the host's call of InitPlayer) to its end
/// (InitSound returned): the time the original's InitPlayer and InitSound take on the 68000,
/// measured in the reference runs (song start 6736 against the port's 100). A module with a
/// name (its init routine past $12; one in the reference set, voodoo-2) takes 27 more, the
/// cost of the original's ending of the name at the init routine. It sets the beam's phase
/// against the writes, which the waits after a DMA on depend on, and the render's sample grid.
/// The later replayer (`REFS_V2`), which no Eagleplayer plays, starts without it.
const INIT_CLOCKS: u64 = 6636;
const INIT_NAME_CLOCKS: u64 = 27;

/// 68000 cycles of the original's code between its host calls, by the path it takes. The
/// engine counts a fixed figure per instruction (crates/m68k-rc/src/cycles.rs), so a path's
/// time does not depend on the data, only the branches do; a register write is a host call,
/// which lands at the clock the cycles since the last host call reach (halved, rounded
/// down: a half cycle left over is lost at each call). Fitted as integers to every write
/// gap of the reference's write logs of the nine Fred modules the Eagleplayer plays (60 562
/// gaps over 20 s each, all exact), and of 40 modified copies of four of them (one change
/// each: a sample or synth loop, a blend sound, a voice the module disables, tempo commands,
/// a song end), which take the paths the nine do not (290 477 gaps, all exact). The fit fixes
/// only the sums along each path, so where a cost could move to its neighbour (the voice's
/// entry against what follows the loop check, the effects against the portamento and
/// vibrato) one split was chosen. UNSURE: a path no module took.
mod cyc {
    /// From the tick (the host's call of the interrupt) to the fourth voice.
    pub const START: u64 = 25;
    /// A voice's entry, then its loop check: nothing to write, or the loop of the note
    /// started last tick (the empty sample, a sample or synth loop, or none: repeat bit 15).
    pub const ENTRY: u64 = 100;
    pub const LOOP_DONE: u64 = 152;
    pub const LOOP_EMPTY: u64 = 287;
    pub const LOOP_SAMPLE: u64 = 363;
    pub const LOOP_SYNTH: u64 = 371;
    pub const LOOP_NONE: u64 = 206;
    /// From the loop's location to its length (the empty sample's, or one computed from the
    /// instrument), and from the length on.
    pub const LOOP_LEN: u64 = 218;
    pub const LOOP_LEN_REPEAT: u64 = 222;
    pub const LOOP_END_REPEAT: u64 = 128;
    pub const LOOP_END: u64 = 118;
    /// The duration counted down: more to wait, the tick before a row (no mute), the mute.
    pub const DUR_OTHER: u64 = 0;
    pub const DUR_LAST: u64 = 23;
    pub const MUTE: u64 = 266;
    /// After the DMA off of a mute or a pause (the host's return).
    pub const HOST_OFF: u64 = 70;
    /// A track's commands, each from its byte read on.
    pub const INST: u64 = 208;
    pub const TEMPO: u64 = 135;
    pub const PORT: u64 = 546;
    pub const PAUSE: u64 = 331;
    pub const PAUSE_RET: u64 = 69;
    pub const END: u64 = 304;
    pub const JUMP: u64 = 166;
    pub const WAIT: u64 = 255;
    /// A note, to its location: a sample, or a synth sound (with or without an instrument
    /// command before it in the row).
    pub const NOTE_SAMPLE: u64 = 495;
    pub const NOTE_SYNTH: u64 = 508;
    pub const NOTE_SYNTH_INST: u64 = 481;
    pub const PULSE_CREATE: u64 = 212;
    pub const PULSE_BYTE: u64 = 26;
    pub const BLEND_CREATE: u64 = 893;
    /// Between a note's writes; a portamento running, and one starting from this note, add
    /// to the period's; from the DMA on (after the host's wait) to the effects.
    pub const NOTE_LEN: u64 = 230;
    pub const NOTE_VOL: u64 = 214;
    pub const NOTE_PER: u64 = 362;
    pub const NOTE_PORT_RUNNING: u64 = 23;
    pub const NOTE_PORT_START: u64 = 72;
    pub const NOTE_DMA: u64 = 357;
    pub const NOTE_FX: u64 = 86;
    pub const NOTE_NO_DMA: u64 = 109;
    /// The effects of a voice whose DMA is off; of one whose DMA is on, then its steps.
    pub const FX_SKIP: u64 = 0;
    pub const FX: u64 = 400;
    pub const ARP_NEXT: u64 = 71;
    pub const ARP_WRAP: u64 = 87;
    pub const PORT_OFF: u64 = 62;
    pub const PORT_DELAY: u64 = 111;
    pub const PORT_STEP: u64 = 384;
    pub const PORT_DONE: u64 = 36;
    pub const VIB_DELAY: u64 = 0;
    pub const VIB_OFF: u64 = 0; // UNSURE
    /// The vibrato's step: up, or down; up turning at the amplitude, reaching 0 (down: and
    /// turning); then less when the period goes down.
    pub const VIB_UP: u64 = 139;
    pub const VIB_DOWN: u64 = 123;
    pub const VIB_UP_TURN: u64 = 30;
    pub const VIB_UP_ZERO: u64 = 20;
    pub const VIB_DOWN_ZERO: u64 = 40;
    pub const VIB_SUB_LESS: u64 = 10;
    /// From the period to the volume, by the envelope's state before and after.
    pub fn env(from: u8, to: u8) -> u64 {
        match (from, to) {
            (0, 0) => 577,
            (0, _) => 615,
            (1, 1) => 604,
            (1, _) => 643,
            (2, _) => 600,
            (3, 3) => 659,
            (3, _) => 696,
            _ => 589,
        }
    }
    /// After the volume: a sample; a pulse waiting, at its speed, blocked, or written (in the
    /// direction it went, or after a turn: plus `PULSE_TURN` each).
    pub const SAMPLE: u64 = 38;
    pub const PULSE_DELAY: u64 = 104;
    pub const PULSE_SPEED: u64 = 127;
    pub const PULSE_BLOCKED: u64 = 161;
    pub const PULSE_WRITE: u64 = 294;
    pub const PULSE_FWD_TURNED: u64 = 215;
    pub const PULSE_BACK_TURNED: u64 = 205;
    pub const PULSE_TURN: u64 = 144;
    /// A blend's steps: waiting, blocked, each turn, the waveform built; a step up without a
    /// turn takes `BLEND_UP` more.
    pub const BLEND_DELAY: u64 = 104;
    pub const BLEND_BLOCKED: u64 = 128;
    pub const BLEND_TURN: u64 = 155;
    pub const BLEND_BUILD: u64 = 3617;
    pub const BLEND_UP: u64 = 24;
}

/// The original's voice structure ($80 bytes): field offsets.
mod v {
    pub const POSITIONS: u32 = 0x00; // l the position list
    pub const TRACK: u32 = 0x08; // l the next track byte
    pub const INSTRUMENT: u32 = 0x0c; // l
    pub const VIB_DELAY: u32 = 0x10;
    pub const VIB_SPEED: u32 = 0x12;
    pub const VIB_AMPL: u32 = 0x13;
    pub const VIB_VALUE: u32 = 0x14;
    pub const PORT_RUNNING: u32 = 0x15;
    pub const PORT_DELAY: u32 = 0x16; // w
    pub const NOTE: u32 = 0x19;
    pub const PORT_LIMIT: u32 = 0x1a; // w
    pub const PERIOD: u32 = 0x1c; // w
    pub const LOOP_DONE: u32 = 0x1e; // 0: the loop is written next tick
    pub const DURATION: u32 = 0x20; // w
    pub const VIB_FLAGS: u32 = 0x22; // bit 7 up, bit 0 period up
    pub const PORT_NOTE: u32 = 0x23;
    pub const PORT_START: u32 = 0x24; // w
    pub const PORT_DIFF: u32 = 0x26; // w
    pub const PORT_COUNTER: u32 = 0x28; // w
    pub const PORT_SPEED: u32 = 0x2a; // w
    pub const VOLUME: u32 = 0x2c;
    pub const ENV_STATE: u32 = 0x2d;
    pub const SUSTAIN: u32 = 0x2e;
    pub const ARP_POS: u32 = 0x2f;
    pub const ARP_SPEED: u32 = 0x30;
    pub const PULSE_POS: u32 = 0x31;
    pub const PULSE_DELAY: u32 = 0x32;
    pub const PULSE_WAY: u32 = 0x33; // bit 2
    pub const PULSE_SPEED: u32 = 0x34;
    pub const BLEND_POS: u32 = 0x36; // w
    pub const BLEND_WAY: u32 = 0x38;
    pub const BLEND_DELAY: u32 = 0x39;
    pub const POSITION: u32 = 0x3a; // w byte offset in the position list
    pub const PULSE_SHOT: u32 = 0x3c;
    pub const BLEND_SHOT: u32 = 0x3d;
    pub const WAVE: u32 = 0x40; // the synth waveform (64 bytes)
}

/// An instrument ($40 bytes): field offsets.
mod ins {
    pub const SAMPLE: u32 = 0x00; // l offset in the module
    pub const REPEAT: u32 = 0x04; // w
    pub const LENGTH: u32 = 0x06; // w words
    pub const PERIOD: u32 = 0x08; // w
    pub const VIB_DELAY: u32 = 0x0a;
    pub const VIB_SPEED: u32 = 0x0c;
    pub const VIB_AMPL: u32 = 0x0d;
    pub const ENV_VOL: u32 = 0x0e;
    pub const ATTACK_SPEED: u32 = 0x0f;
    pub const ATTACK_VOL: u32 = 0x10;
    pub const DECAY_SPEED: u32 = 0x11;
    pub const DECAY_VOL: u32 = 0x12;
    pub const SUSTAIN: u32 = 0x13;
    pub const RELEASE_SPEED: u32 = 0x14;
    pub const RELEASE_VOL: u32 = 0x15;
    pub const ARPEGGIO: u32 = 0x16; // 16 bytes
    pub const ARP_SPEED: u32 = 0x26;
    pub const TYPE: u32 = 0x27; // 0 sample, 1 pulse, 2 blend
    pub const PULSE_MIN: u32 = 0x28;
    pub const PULSE_PLUS: u32 = 0x29;
    pub const PULSE_SPEED: u32 = 0x2a;
    pub const PULSE_START: u32 = 0x2b;
    pub const PULSE_END: u32 = 0x2c;
    pub const PULSE_DELAY: u32 = 0x2d;
    pub const SYNC: u32 = 0x2e;
    pub const BLEND: u32 = 0x2f;
    pub const BLEND_DELAY: u32 = 0x30;
    pub const PULSE_SHOT: u32 = 0x31;
    pub const BLEND_SHOT: u32 = 0x32;
    pub const ARP_COUNT: u32 = 0x33;
}

/// The same references in the later version of the editor's replayer (a module that has a
/// word `$FFFF` after the four jumps and init at `$14`): the code between them grew, so
/// the offsets from the init routine differ. Found by aligning the two replayers' code
/// word by word and checking each opcode (the order is that of `REFS`).
const REFS_V2: [(u16, u16); 14] = [
    (0x066, 0x47fa),
    (0x074, 0x47fa),
    (0x06c, 0x47fa),
    (0x07c, 0x47fa),
    (0x1dc, 0x45fa),
    (0x00e, 0x47fa),
    (0x004, 0x123a),
    (0x018, 0x49fa),
    (0x014, 0x45fa),
    (0x1a6, 0xdbfa),
    (0x04a, 0xd7fa),
    (0x10e, 0x45fa),
    (0x026, 0x41fa),
    (0x034, 0x43fa),
];

/// Offsets in the module of the original's targets, and of its init routine.
fn refs(d: &[u8]) -> Option<(u16, [u32; 14])> {
    refs_with(d, &REFS).or_else(|| refs_with(d, &REFS_V2))
}

fn refs_with(d: &[u8], table: &[(u16, u16); 14]) -> Option<(u16, [u32; 14])> {
    // EP_Check: four jumps, and no fifth
    if (0..4).any(|i| be16(d, 4 * i) != 0x4efa) || be16(d, 16) == 0x4efa {
        return None;
    }
    let init = be16(d, 2).wrapping_add(2);
    let mut a = [0; 14];
    for (t, &(off, op)) in a.iter_mut().zip(table) {
        // the original indexes the module with words, sign extended
        let o = off.wrapping_add(init) as i16 as i64;
        if o < 0 || be16(d, o as usize) != op {
            return None;
        }
        let o = (o as u16).wrapping_add(be16(d, o as usize + 2)) as i16 as i64 + 2;
        *t = o as u32;
    }
    Some((init, a))
}

fn detect(d: &[u8]) -> bool {
    refs(d).is_some()
}

pub struct Fred {
    module: u32,
    /// The guest addresses of the original's targets (`REFS`).
    p: [u32; 14],
    /// The DMA bits as the player last set them (the original's shadow word).
    dma: u16,
    sub: u16,
    last_subsong: u16,
    songend: bool,
    /// What the offsets the module keeps (tracks, instruments, samples) are relative to: the
    /// module's start, and in the later replayer (`REFS_V2`) a point 0x44 bytes before it.
    base: u32,
    /// Where the byte that enables the voices lies (after the later replayer's extra word).
    voice_mask_at: u32,
    /// The clock of the last host call (the tick's start, or a write), and the 68000 cycles
    /// spent since (`cyc`).
    seg: u64,
    cyc: u64,
}

/// What the voice loop does after a voice.
enum Next {
    Voice,
    /// The same voice again (a new track).
    Again,
    /// The player stopped.
    Stop,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (init, a) = refs(d)?;
    let v2 = refs_with(d, &REFS).is_none();
    if !v2 {
        m.bus.clock += INIT_CLOCKS + if init > 0x12 { INIT_NAME_CLOCKS } else { 0 };
    }
    let b = &mut m.bus;
    // InitPlayer ends the module name at the init routine
    if init > 0x12 {
        b.w8(base + init as u32 - 1, 0);
    }
    let last_subsong = b.r8(base.wrapping_add(a[LAST_SUBSONG])) as u16;
    let p = a.map(|o| base.wrapping_add(o));
    let mut f = Fred {
        module: base,
        p,
        dma: 0,
        sub: 0,
        last_subsong,
        songend: false,
        base: if v2 { base.wrapping_sub(0x44) } else { base },
        voice_mask_at: if v2 { 0x12 } else { 0x10 },
        seg: b.clock,
        cyc: 0,
    };
    f.init_sound(0, &mut Hw { bus: b });
    Some(Box::new(f))
}

impl Replayer for Fred {
    fn format(&self) -> &'static str {
        "Fred Editor"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.last_subsong, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.init_sound(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if hw.bus.r8(self.p[STOPPED]) != 0 {
            return;
        }
        self.seg = hw.bus.clock;
        self.cyc = cyc::START;
        let mut ch = 3;
        loop {
            match self.voice(ch, hw) {
                Next::Voice if ch > 0 => ch -= 1,
                Next::Again => {}
                _ => break,
            }
        }
        hw.bus.clock = hw.bus.clock.max(self.seg + self.cyc / 2);
    }
}

impl Fred {
    fn voice_at(&self, ch: usize) -> u32 {
        self.p[VOICES] + 0x80 * ch as u32
    }

    /// InitSound: subsong `n`'s start tempo and position lists.
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        let b = &mut *hw.bus;
        self.sub = n;
        if n > b.r8(self.p[LAST_SUBSONG]) as u16 {
            b.w8(self.p[STOPPED], 0xff);
            self.dma_mask(0x000f, hw);
            return;
        }
        b.w8(self.p[SUBSONG], n as u8);
        b.w8(self.p[TEMPO], b.r8(self.p[START_TEMPOS] + n as u32));
        for ch in 0..4 {
            let va = self.voice_at(ch);
            b.w16(va + v::DURATION, 1);
            b.w16(va + v::POSITION, 0);
            let list = self.p[POSITIONS];
            let pos = list.wrapping_add(b.r16(list + 8 * n as u32 + 2 * ch as u32) as i16 as u32);
            b.w32(va + v::POSITIONS, pos);
            let tracks = self.base.wrapping_add(b.r32(self.p[TRACKS]));
            b.w32(va + v::TRACK, tracks.wrapping_add(b.r16(pos) as i16 as u32));
            b.w8(va + v::LOOP_DONE, 0xff);
        }
        b.w8(self.p[STOPPED], 0);
        b.w16(self.p[FADE_VOLUME], 0x1000);
        b.w8(self.p[FADING], 0);
    }

    fn spend(&mut self, cycles: u64) {
        self.cyc += cycles;
    }

    /// A host call: it runs at the clock the cycles since the last one reach.
    fn host(&mut self, hw: &mut Hw) {
        hw.bus.clock = hw.bus.clock.max(self.seg + self.cyc / 2);
    }

    /// The code goes on from the host call's return.
    fn rebase(&mut self, hw: &Hw) {
        self.seg = hw.bus.clock;
        self.cyc = 0;
    }

    /// ENPP_PokeAdr, ENPP_PokeLen, ENPP_PokePer, ENPP_PokeVol.
    fn loc(&mut self, ch: usize, a: u32, hw: &mut Hw) {
        self.host(hw);
        hw.loc(ch, a);
        self.rebase(hw);
    }

    fn len(&mut self, ch: usize, n: u16, hw: &mut Hw) {
        self.host(hw);
        hw.len(ch, n);
        self.rebase(hw);
    }

    fn per(&mut self, ch: usize, p: u16, hw: &mut Hw) {
        self.host(hw);
        hw.per(ch, p);
        self.rebase(hw);
    }

    fn vol(&mut self, ch: usize, v: u16, hw: &mut Hw) {
        self.host(hw);
        hw.vol(ch, v);
        self.rebase(hw);
    }

    /// The player's DMA switch, through ENPP_DMAMask: bit 15 of `d0` on or off.
    fn dma_mask(&mut self, d0: u16, hw: &mut Hw) {
        let bits = d0 & 0xf;
        if d0 & 0x8000 == 0 {
            self.dma &= !bits;
        } else {
            self.dma |= bits;
        }
        self.host(hw);
        hw.dmacon(bits | (d0 & 0x8000));
        if d0 & 0x8000 != 0 {
            // the host's ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does:
            // to 10 lines past the current one
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        }
        self.rebase(hw);
    }

    fn tempo(&self, hw: &Hw) -> u16 {
        hw.bus.r8(self.p[TEMPO]) as u16
    }

    /// `period table[note] * instrument period / 1024`, as words.
    fn period(&self, hw: &Hw, note: u8, inst: u32) -> u16 {
        let p = hw.bus.r16(self.p[PERIODS] + 2 * note as u32) as u32;
        ((p * hw.bus.r16(inst + ins::PERIOD) as u32) >> 10) as u16
    }

    fn voice(&mut self, ch: usize, hw: &mut Hw) -> Next {
        let va = self.voice_at(ch);
        let bit = 1u16 << ch;
        let mut inst_change = false;
        self.spend(cyc::ENTRY);
        let b = &mut *hw.bus;
        let track = b.r32(va + v::TRACK);
        // the loop of the note started in the last tick
        if b.r8(va + v::LOOP_DONE) == 0 {
            b.w8(va + v::LOOP_DONE, 0xff);
            let inst = b.r32(va + v::INSTRUMENT);
            let repeat = b.r16(inst + ins::REPEAT);
            if repeat == 0 {
                self.spend(cyc::LOOP_EMPTY);
                self.loc(ch, self.p[EMPTY], hw);
                self.spend(cyc::LOOP_LEN);
                self.len(ch, 0x32, hw);
                self.spend(cyc::LOOP_END);
            } else if repeat & 0x8000 == 0 {
                let mut a = self.base.wrapping_add(b.r32(inst + ins::SAMPLE)).wrapping_add(repeat as u32);
                let len = b.r16(inst + ins::LENGTH).wrapping_sub(repeat);
                if b.r8(inst + ins::TYPE) != 0 {
                    a = va + v::WAVE;
                    self.spend(cyc::LOOP_SYNTH);
                } else {
                    self.spend(cyc::LOOP_SAMPLE);
                }
                self.loc(ch, a, hw);
                self.spend(cyc::LOOP_LEN_REPEAT);
                self.len(ch, len, hw);
                self.spend(cyc::LOOP_END_REPEAT);
            } else {
                self.spend(cyc::LOOP_NONE);
            }
        } else {
            self.spend(cyc::LOOP_DONE);
        }

        let b = &mut *hw.bus;
        let duration = b.r16(va + v::DURATION).wrapping_sub(1);
        b.w16(va + v::DURATION, duration);
        if duration == 0 {
            if let Some(next) = self.new_line(ch, &mut inst_change, hw) {
                return next;
            }
        } else if duration == 1 && (b.r8(track).wrapping_sub(0xa0) as i8) < 0 {
            self.spend(cyc::MUTE);
            self.dma_mask(bit, hw);
            self.spend(cyc::HOST_OFF);
        } else if duration == 1 {
            self.spend(cyc::DUR_LAST);
        } else {
            self.spend(cyc::DUR_OTHER);
        }
        self.effects(ch, hw)
    }

    /// The next track row(s) of voice `ch`. None goes on to the effects.
    fn new_line(&mut self, ch: usize, inst_change: &mut bool, hw: &mut Hw) -> Option<Next> {
        let va = self.voice_at(ch);
        let bit = 1u16 << ch;
        loop {
            let b = &mut *hw.bus;
            let mut inst = b.r32(va + v::INSTRUMENT);
            let mut t = b.r32(va + v::TRACK);
            let cmd = b.r8(t);
            t += 1;
            let tempo = self.tempo(hw);
            self.spend(match cmd {
                0x00..=0x7f => 0,
                0x83 => cyc::INST,
                0x82 => cyc::TEMPO,
                0x81 => cyc::PORT,
                0x84 => cyc::PAUSE,
                0x80 => cyc::END,
                _ => cyc::WAIT,
            });
            let b = &mut *hw.bus;
            match cmd {
                0x00..=0x7f => return self.note(ch, cmd, t, inst, *inst_change, hw),
                // instrument
                0x83 => {
                    let n = b.r8(t) as u32;
                    inst = self.base.wrapping_add(b.r32(self.p[INSTRUMENTS])).wrapping_add(n << 6);
                    b.w32(va + v::INSTRUMENT, inst);
                    *inst_change = true;
                    b.w32(va + v::TRACK, t + 1);
                }
                // tempo
                0x82 => {
                    b.w8(self.p[TEMPO], b.r8(t));
                    b.w32(va + v::TRACK, t + 1);
                }
                // portamento: speed, note, delay
                0x81 => {
                    let speed = b.r8(t) as u16;
                    b.w16(va + v::PORT_SPEED, speed.wrapping_mul(tempo));
                    let note = b.r8(t + 1);
                    b.w8(va + v::PORT_NOTE, note);
                    let limit = self.period(hw, note, inst);
                    let b = &mut *hw.bus;
                    b.w16(va + v::PORT_LIMIT, limit);
                    b.w16(va + v::PORT_START, 0);
                    b.w16(va + v::PORT_DELAY, (b.r8(t + 2) as u16).wrapping_mul(tempo));
                    b.w8(va + v::PORT_RUNNING, 0xff);
                    b.w32(va + v::TRACK, t + 3);
                }
                // pause: the voice is muted, and its effects do not run
                0x84 => {
                    self.dma_mask(bit, hw);
                    self.spend(cyc::HOST_OFF + cyc::PAUSE_RET);
                    let b = &mut *hw.bus;
                    b.w16(va + v::DURATION, tempo);
                    b.w8(va + v::LOOP_DONE, 0xff);
                    b.w32(va + v::TRACK, t);
                    return Some(Next::Voice);
                }
                // end of track: the next position
                0x80 => return Some(self.next_position(ch, hw)),
                // wait
                _ => {
                    b.w16(va + v::DURATION, (cmd.wrapping_neg() as u16).wrapping_mul(tempo));
                    b.w32(va + v::TRACK, t);
                    return None;
                }
            }
        }
    }

    fn next_position(&mut self, ch: usize, hw: &mut Hw) -> Next {
        let va = self.voice_at(ch);
        let mut off = hw.bus.r16(va + v::POSITION).wrapping_add(2) as u32;
        for _ in 0..MAX_TURNS {
            let b = &mut *hw.bus;
            let e = b.r16(b.r32(va + v::POSITIONS).wrapping_add(off));
            if e == 0xffff {
                // the original stops for good, in the middle of the tick
                b.w8(self.p[STOPPED], 0xff);
                return Next::Stop;
            }
            if e & 0x8000 != 0 {
                // a jump: dtg_SongEnd
                self.spend(cyc::JUMP);
                self.host(hw);
                self.rebase(hw);
                self.songend = true;
                off = (e & 0x7fff) as u32;
                continue;
            }
            b.w16(va + v::POSITION, off as u16);
            let tracks = self.base.wrapping_add(b.r32(self.p[TRACKS]));
            b.w32(va + v::TRACK, tracks.wrapping_add(e as u32));
            b.w16(va + v::DURATION, 1);
            return Next::Again;
        }
        Next::Voice
    }

    fn note(&mut self, ch: usize, note: u8, t: u32, inst: u32, inst_change: bool, hw: &mut Hw) -> Option<Next> {
        let va = self.voice_at(ch);
        let bit = 1u16 << ch;
        let b = &mut *hw.bus;
        b.w32(va + v::TRACK, t);
        b.w8(va + v::NOTE, note);
        b.w8(va + v::ARP_POS, 0);
        b.w8(va + v::ARP_SPEED, b.r8(inst + ins::ARP_SPEED));
        b.w8(va + v::VIB_DELAY, b.r8(inst + ins::VIB_DELAY));
        b.w8(va + v::VIB_SPEED, b.r8(inst + ins::VIB_SPEED));
        b.w8(va + v::VIB_AMPL, b.r8(inst + ins::VIB_AMPL));
        b.w8(va + v::VIB_FLAGS, 0xff);
        b.w8(va + v::VIB_VALUE, 0);
        let kind = b.r8(inst + ins::TYPE);
        let sync = b.r8(inst + ins::SYNC);
        self.spend(match (kind, inst_change) {
            (0, _) => cyc::NOTE_SAMPLE,
            (_, false) => cyc::NOTE_SYNTH,
            (_, true) => cyc::NOTE_SYNTH_INST,
        });
        if kind == 1 && (inst_change || sync & 2 != 0) {
            self.create_pulse(va, inst, hw);
        } else if kind == 2 && (inst_change || sync & 8 != 0) {
            self.create_blend(va, inst, hw);
        }
        let tempo = self.tempo(hw);
        let b = &mut *hw.bus;
        b.w16(va + v::DURATION, tempo);
        let mut a = self.base.wrapping_add(b.r32(inst + ins::SAMPLE));
        if kind != 0 {
            a = va + v::WAVE;
        }
        let len = b.r16(inst + ins::LENGTH);
        self.loc(ch, a, hw);
        self.spend(cyc::NOTE_LEN);
        self.len(ch, len, hw);
        self.spend(cyc::NOTE_VOL);
        hw.bus.w8(va + v::VOLUME, 0);
        self.vol(ch, 0, hw);
        self.spend(cyc::NOTE_PER);
        let b = &mut *hw.bus;
        b.w8(va + v::ENV_STATE, 0);
        b.w8(va + v::SUSTAIN, b.r8(inst + ins::SUSTAIN));
        let period = self.period(hw, note, inst);
        self.per(ch, period, hw);
        let b = &mut *hw.bus;
        b.w16(va + v::PERIOD, period);
        if b.r8(va + v::PORT_RUNNING) != 0 {
            self.cyc += cyc::NOTE_PORT_RUNNING;
            if b.r16(va + v::PORT_START) == 0 {
                self.cyc += cyc::NOTE_PORT_START;
                b.w16(va + v::PORT_DIFF, b.r16(va + v::PORT_LIMIT).wrapping_sub(period));
                b.w16(va + v::PORT_COUNTER, 1);
                b.w16(va + v::PORT_START, period);
            }
        }
        if b.r8(self.module + self.voice_mask_at) & bit as u8 == 0 {
            // a voice the module does not enable: no DMA, no effects
            self.spend(cyc::NOTE_NO_DMA);
            return Some(Next::Voice);
        }
        self.spend(cyc::NOTE_DMA);
        self.dma_mask(0x8200 | bit, hw);
        self.spend(cyc::NOTE_FX);
        hw.bus.w8(va + v::LOOP_DONE, 0);
        None
    }

    fn create_pulse(&mut self, va: u32, inst: u32, hw: &mut Hw) {
        self.spend(cyc::PULSE_CREATE);
        let b = &mut *hw.bus;
        b.w8(va + v::PULSE_SHOT, b.r8(inst + ins::PULSE_SHOT));
        b.w8(va + v::PULSE_DELAY, b.r8(inst + ins::PULSE_DELAY));
        b.w8(va + v::PULSE_SPEED, b.r8(inst + ins::PULSE_SPEED));
        b.w8(va + v::PULSE_WAY, 0);
        let start = b.r8(inst + ins::PULSE_START);
        b.w8(va + v::PULSE_POS, start);
        let (min, plus) = (b.r8(inst + ins::PULSE_MIN), b.r8(inst + ins::PULSE_PLUS));
        let mut a = va + v::WAVE;
        // dbra counts: the start less 1 as a byte; the length's low byte doubled,
        // less the start and 1, with the length's high byte above it
        for _ in 0..=start.wrapping_sub(1) as u16 {
            b.w8(a, min);
            a += 1;
        }
        let len = b.r16(inst + ins::LENGTH);
        let lo = (len as u8).wrapping_mul(2).wrapping_sub(start).wrapping_sub(1);
        for _ in 0..=((len & 0xff00) | lo as u16) as u32 {
            b.w8(a, plus);
            a += 1;
        }
        self.spend(cyc::PULSE_BYTE * (a - va - v::WAVE) as u64);
    }

    fn create_blend(&mut self, va: u32, inst: u32, hw: &mut Hw) {
        self.spend(cyc::BLEND_CREATE);
        let b = &mut *hw.bus;
        b.w8(va + v::BLEND_WAY, 0);
        b.w16(va + v::BLEND_POS, 1);
        b.w8(va + v::BLEND_SHOT, b.r8(inst + ins::BLEND_SHOT));
        b.w8(va + v::BLEND_DELAY, b.r8(inst + ins::BLEND_DELAY));
        let s = self.base.wrapping_add(b.r32(inst + ins::SAMPLE));
        for i in 0..32 {
            b.w8(va + v::WAVE + i, b.r8(s + i));
        }
    }

    /// Arpeggio, portamento, vibrato, envelope and the synth sounds of a voice
    /// whose DMA is on; writes its period and volume.
    fn effects(&mut self, ch: usize, hw: &mut Hw) -> Next {
        if self.dma & (1 << ch) == 0 {
            self.spend(cyc::FX_SKIP);
            return Next::Voice;
        }
        self.spend(cyc::FX);
        let va = self.voice_at(ch);
        let b = &mut *hw.bus;
        let inst = b.r32(va + v::INSTRUMENT);

        // arpeggio
        let pos = b.r8(va + v::ARP_POS);
        let note = b.r8(va + v::NOTE).wrapping_add(b.r8(inst + ins::ARPEGGIO + pos as u32));
        let speed = b.r8(va + v::ARP_SPEED).wrapping_sub(1);
        b.w8(va + v::ARP_SPEED, speed);
        if speed == 0 {
            b.w8(va + v::ARP_SPEED, b.r8(inst + ins::ARP_SPEED));
            let pos = pos.wrapping_add(1);
            let wrap = pos == b.r8(inst + ins::ARP_COUNT);
            self.cyc += if wrap { cyc::ARP_WRAP } else { cyc::ARP_NEXT };
            b.w8(va + v::ARP_POS, if wrap { 0 } else { pos });
        }
        let period = self.period(hw, note, inst);
        let b = &mut *hw.bus;
        b.w16(va + v::PERIOD, period);

        // portamento
        if b.r8(va + v::PORT_RUNNING) != 0 {
            let delay = b.r16(va + v::PORT_DELAY);
            if delay != 0 {
                self.cyc += cyc::PORT_DELAY;
                hw.bus.w16(va + v::PORT_DELAY, delay - 1);
            } else {
                self.cyc += cyc::PORT_STEP;
                let b = &mut *hw.bus;
                let counter = b.r16(va + v::PORT_COUNTER);
                let speed = b.r16(va + v::PORT_SPEED) as i16;
                let product = counter as i16 as i32 * b.r16(va + v::PORT_DIFF) as i16 as i32;
                // divs.w: the quotient, or the product left as it was on overflow
                // (and a speed of 0 would trap; the port adds nothing then)
                let step = match product.checked_div(speed as i32) {
                    Some(q) if q == q as i16 as i32 => q as u16,
                    Some(_) => product as u16,
                    None => 0,
                };
                b.w16(va + v::PERIOD, b.r16(va + v::PERIOD).wrapping_add(step));
                let counter = counter.wrapping_add(1);
                b.w16(va + v::PORT_COUNTER, counter);
                if counter as i16 > speed {
                    self.cyc += cyc::PORT_DONE;
                    b.w8(va + v::NOTE, b.r8(va + v::PORT_NOTE));
                    b.w8(va + v::PORT_RUNNING, 0);
                }
            }
        } else {
            self.spend(cyc::PORT_OFF);
        }

        // vibrato
        let b = &mut *hw.bus;
        let mut period = b.r16(va + v::PERIOD);
        let delay = b.r8(va + v::VIB_DELAY);
        if delay != 0 {
            self.cyc += cyc::VIB_DELAY;
            hw.bus.w8(va + v::VIB_DELAY, delay - 1);
        } else if b.r8(va + v::VIB_FLAGS) != 0 {
            let mut flags = b.r8(va + v::VIB_FLAGS);
            let value;
            let mut c;
            if flags & 0x80 != 0 {
                c = cyc::VIB_UP;
                value = b.r8(va + v::VIB_VALUE).wrapping_add(b.r8(va + v::VIB_SPEED));
                b.w8(va + v::VIB_VALUE, value);
                if value == b.r8(va + v::VIB_AMPL) {
                    c += cyc::VIB_UP_TURN;
                    flags ^= 0x80;
                }
                if value == 0 {
                    c += cyc::VIB_UP_ZERO;
                }
            } else {
                c = cyc::VIB_DOWN;
                value = b.r8(va + v::VIB_VALUE).wrapping_sub(b.r8(va + v::VIB_SPEED));
                b.w8(va + v::VIB_VALUE, value);
                if value == 0 {
                    c += cyc::VIB_DOWN_ZERO;
                    flags ^= 0x80;
                }
            }
            if value == 0 {
                flags ^= 1;
            }
            b.w8(va + v::VIB_FLAGS, flags);
            let d = value as i8 as u16;
            period = if flags & 1 != 0 {
                period.wrapping_add(d)
            } else {
                c -= cyc::VIB_SUB_LESS;
                period.wrapping_sub(d)
            };
            self.cyc += c;
        } else {
            self.cyc += cyc::VIB_OFF;
        }
        self.per(ch, period, hw);

        // envelope: the sums and differences are words
        let b = &mut *hw.bus;
        let vol = b.r8(va + v::VOLUME) as i16;
        let r = |o: u32| b.r8(inst + o) as i16;
        let (attack, decay, release) = (
            (r(ins::ATTACK_SPEED), r(ins::ATTACK_VOL)),
            (r(ins::DECAY_SPEED), r(ins::DECAY_VOL)),
            (r(ins::RELEASE_SPEED), r(ins::RELEASE_VOL)),
        );
        let state = b.r8(va + v::ENV_STATE);
        match state {
            0 => {
                let x = vol + attack.0;
                b.w8(va + v::VOLUME, x as u8);
                if x >= attack.1 {
                    b.w8(va + v::VOLUME, attack.1 as u8);
                    b.w8(va + v::ENV_STATE, 1);
                }
            }
            s @ (1 | 3) => {
                let (speed, end) = if s == 1 { decay } else { release };
                let x = vol - speed;
                b.w8(va + v::VOLUME, x as u8);
                if x <= end {
                    b.w8(va + v::VOLUME, end as u8);
                    b.w8(va + v::ENV_STATE, s + 1);
                }
            }
            2 => {
                let s = b.r8(va + v::SUSTAIN);
                if s == 0 {
                    b.w8(va + v::ENV_STATE, 3);
                } else {
                    b.w8(va + v::SUSTAIN, s - 1);
                }
            }
            _ => {}
        }
        // the fade volume ($1000 unless faded, which nothing starts here)
        let fade = b.r16(self.p[FADE_VOLUME]) as u32;
        let env_vol = (b.r8(inst + ins::ENV_VOL) as u32 * fade) >> 12;
        let vol = (b.r8(va + v::VOLUME) as u32 * (env_vol & 0xffff)) >> 9;
        self.cyc += cyc::env(state, b.r8(va + v::ENV_STATE));
        self.vol(ch, vol as u16, hw);

        let b = &mut *hw.bus;
        match b.r8(inst + ins::TYPE) {
            0 => self.spend(cyc::SAMPLE),
            2 => self.blend(va, inst, hw),
            _ => self.pulse(va, inst, hw),
        }
        Next::Voice
    }

    fn pulse(&mut self, va: u32, inst: u32, hw: &mut Hw) {
        let b = &mut *hw.bus;
        let delay = b.r8(va + v::PULSE_DELAY);
        if delay != 0 {
            b.w8(va + v::PULSE_DELAY, delay - 1);
            return self.spend(cyc::PULSE_DELAY);
        }
        let speed = b.r8(va + v::PULSE_SPEED);
        if speed != 0 {
            b.w8(va + v::PULSE_SPEED, speed - 1);
            return self.spend(cyc::PULSE_SPEED);
        }
        if b.r8(inst + ins::SYNC) & 1 != 0 && b.r8(va + v::PULSE_SHOT) == 0 {
            return self.spend(cyc::PULSE_BLOCKED);
        }
        b.w8(va + v::PULSE_SPEED, b.r8(inst + ins::PULSE_SPEED));
        let mut pos = b.r8(va + v::PULSE_POS);
        let (start, end) = (b.r8(inst + ins::PULSE_START) as i8, b.r8(inst + ins::PULSE_END) as i8);
        let mut back = b.r8(va + v::PULSE_WAY) & 4 != 0;
        let was_back = back;
        let shot = |b: &mut crate::bus::Bus| b.w8(va + v::PULSE_SHOT, b.r8(va + v::PULSE_SHOT).wrapping_sub(1));
        let mut turns = 0;
        while turns < MAX_TURNS {
            if !back {
                if pos as i8 <= end {
                    b.w8(va + v::WAVE + pos as u32, b.r8(inst + ins::PULSE_MIN));
                    b.w8(va + v::PULSE_POS, pos.wrapping_add(1));
                    break;
                }
                b.w8(va + v::PULSE_WAY, b.r8(va + v::PULSE_WAY) | 4);
                shot(b);
                pos = pos.wrapping_sub(1);
                turns += 1;
            }
            back = true;
            if pos as i8 >= start {
                b.w8(va + v::WAVE + pos as u32, b.r8(inst + ins::PULSE_PLUS));
                b.w8(va + v::PULSE_POS, pos.wrapping_sub(1));
                break;
            }
            b.w8(va + v::PULSE_WAY, b.r8(va + v::PULSE_WAY) & !4);
            shot(b);
            pos = pos.wrapping_add(1);
            back = false;
            turns += 1;
        }
        // the direction written: back unless the last pass wrote forward
        self.spend(match (was_back, back) {
            _ if turns == 0 => cyc::PULSE_WRITE,
            (_, false) => cyc::PULSE_FWD_TURNED,
            (_, true) => cyc::PULSE_BACK_TURNED,
        } + cyc::PULSE_TURN * turns as u64);
    }

    fn blend(&mut self, va: u32, inst: u32, hw: &mut Hw) {
        let b = &mut *hw.bus;
        let delay = b.r8(va + v::BLEND_DELAY);
        if delay != 0 {
            b.w8(va + v::BLEND_DELAY, delay - 1);
            return self.spend(cyc::BLEND_DELAY);
        }
        let blend = b.r8(inst + ins::BLEND) as u32;
        let mut turns = 0;
        let pos = loop {
            if b.r8(inst + ins::SYNC) & 4 != 0 && b.r8(va + v::BLEND_SHOT) == 0 {
                return self.spend(cyc::BLEND_TURN * turns as u64 + cyc::BLEND_BLOCKED);
            }
            let pos = b.r16(va + v::BLEND_POS);
            let turn = if b.r8(va + v::BLEND_WAY) == 0 {
                // lsl.l: a count of 32 or more leaves 0
                pos == 1u32.checked_shl(blend & 63).unwrap_or(0) as u16
            } else {
                pos == 1
            };
            if !turn {
                let pos = if b.r8(va + v::BLEND_WAY) == 0 {
                    if turns == 0 {
                        self.cyc += cyc::BLEND_UP;
                    }
                    pos.wrapping_add(1)
                } else {
                    pos.wrapping_sub(1)
                };
                b.w16(va + v::BLEND_POS, pos);
                break pos;
            }
            b.w8(va + v::BLEND_WAY, b.r8(va + v::BLEND_WAY) ^ 1);
            b.w8(va + v::BLEND_SHOT, b.r8(va + v::BLEND_SHOT).wrapping_sub(1));
            turns += 1;
            if turns == MAX_TURNS {
                return;
            }
        };
        self.spend(cyc::BLEND_TURN * turns as u64 + cyc::BLEND_BUILD);
        let b = &mut *hw.bus;
        // the sample's first half, plus the second half (read as words) times
        // the position, shifted down logically by the blend
        let s = self.base.wrapping_add(b.r32(inst + ins::SAMPLE));
        for i in 0..32 {
            let p = (pos as i16 as i32).wrapping_mul(b.r16(s + 0x20 + 2 * i) as i16 as i32) as u32;
            let p = p.checked_shr(blend & 63).unwrap_or(0) as u8;
            b.w8(va + v::WAVE + i, p.wrapping_add(b.r8(s + i)));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// The replayer's jumps and references (at the addresses the modules have
    /// them), one subsong whose voices all play one track: instrument 0, note 36,
    /// end. The position list then jumps back to its start (a song end).
    /// Instrument 0 is a 16-word sample repeating from byte 8, at period 428.
    fn module() -> Vec<u8> {
        build(false)
    }

    /// The same song in the later replayer's layout (`REFS_V2`): a word `$FFFF` after the
    /// four jumps, init at $14, the voices' mask after it, the code 0x46 bytes longer, and
    /// the offsets of tracks, instruments and samples counted from 0x44 bytes before the
    /// module.
    fn module_v2() -> Vec<u8> {
        build(true)
    }

    fn build(v2: bool) -> Vec<u8> {
        let (shift, bias, init) = if v2 { (0x46usize, 0x44u32, 0x14usize) } else { (0, 0, 0x12) };
        let targets = [0x7fe, 0x7ff, 0x800, 0x802, 0x804, 0x894, 0x895, 0x896, 0x897, 0x8a2, 0x8a6, 0x8aa, 0x90e, 0xb0e].map(|t| t + shift);
        let mut d = vec![0u8; 0xc60 + shift];
        for i in 0..4 {
            d[4 * i..4 * i + 2].copy_from_slice(&0x4efau16.to_be_bytes());
        }
        d[2..4].copy_from_slice(&((init - 2) as u16).to_be_bytes());
        if v2 {
            d[0x10..0x12].copy_from_slice(&[0xff, 0xff]);
        }
        d[init - 2] = 0x0f; // all voices enabled
        for (&(off, op), &t) in (if v2 { &super::REFS_V2 } else { &super::REFS }).iter().zip(&targets) {
            let o = init + off as usize;
            d[o..o + 2].copy_from_slice(&op.to_be_bytes());
            d[o + 2..o + 4].copy_from_slice(&((t - o - 2) as u16).to_be_bytes());
        }
        let list = 0xb0e + shift;
        d[0x804 + shift + 72..0x804 + shift + 74].copy_from_slice(&1024u16.to_be_bytes()); // note 36
        d[0x897 + shift] = 3; // start tempo
        let (ins_at, tracks_at) = (0xc00 + shift, 0xb40 + shift);
        d[0x8a2 + shift..0x8a6 + shift].copy_from_slice(&(ins_at as u32 + bias).to_be_bytes()); // instruments
        d[0x8a6 + shift..0x8aa + shift].copy_from_slice(&(tracks_at as u32 + bias).to_be_bytes()); // tracks
        for i in 0..4 {
            d[list + 2 * i + 1] = 8; // each voice's position list at +8
        }
        d[list + 8..list + 12].copy_from_slice(&[0x00, 0x00, 0x80, 0x00]); // track 0, jump to 0
        d[tracks_at..tracks_at + 4].copy_from_slice(&[0x83, 0, 36, 0x80]);
        let ins = &mut d[ins_at..ins_at + 0x40];
        ins[0..4].copy_from_slice(&(ins_at as u32 + 0x40 + bias).to_be_bytes());
        ins[4..10].copy_from_slice(&[0, 8, 0, 16, 0x01, 0xac]); // repeat 8, 16 words, period 428
        ins[0x0e..0x11].copy_from_slice(&[64, 64, 255]); // volume 64, attack by 64 to 255
        ins[0x26] = 1; // arpeggio speed
        ins[0x33] = 1; // arpeggio count
        for (i, b) in d[ins_at + 0x40..].iter_mut().enumerate() {
            *b = (i as u8) << 3;
        }
        d
    }

    #[test]
    fn the_later_replayer_plays_what_the_first_does() {
        assert!(super::detect(&module()));
        assert!(super::detect(&module_v2()));
        let play = |m: Vec<u8>| {
            let mut e = Engine::new(48000.0);
            e.open_replayers = true;
            e.m.bus.write_log = Some(Vec::new());
            e.play_data(Path::new("test.frd"), m).unwrap();
            assert_eq!(e.playing.as_ref().unwrap().format, "Fred Editor");
            let mut out = vec![0f32; 2 * 4800];
            e.render(&mut out);
            // the registers but the locations (the module's two layouts are not at one address)
            let log = e.m.bus.write_log.take().unwrap();
            let regs: Vec<(u32, u32)> =
                log.iter().filter(|w| !(0xDFF0A0..0xDFF0E0).contains(&w.0) || !matches!(w.0 & 0xf, 0 | 2)).map(|w| (w.0, w.1 as u32)).collect();
            (out, regs)
        };
        let (a, la) = play(module());
        let (b, lb) = play(module_v2());
        assert_eq!(la, lb, "the same register writes");
        assert!(a.iter().any(|&s| s != 0.0) && b.iter().any(|&s| s != 0.0), "no sound");
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.frd"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Fred Editor");
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // each voice's note, then its loop: 8 words from byte 8 (16 words less
        // 8 bytes)
        assert_eq!(reg(0xDFF0A4)[..2], [16, 8]);
        assert_eq!(reg(0xDFF0D4)[..2], [16, 8]);
        let lo = reg(0xDFF0D2);
        assert_eq!(lo[1].wrapping_sub(lo[0]), 8);
        // volume 0, then 64 * 64 / 512 and 128 * 64 / 512
        assert_eq!(&reg(0xDFF0D8)[..3], &[0, 8, 16]);
        assert_eq!(reg(0xDFF0D6)[..2], [428, 428]);
        // DMA on for each note from the fourth voice to the first, off a tick
        // before the track's end, on again at the next pass
        let dma = reg(0xDFF096);
        assert_eq!(&dma[..9], &[0x8008, 0x8004, 0x8002, 0x8001, 0x0008, 0x0004, 0x0002, 0x0001, 0x8008]);
        assert!(e.songend());
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

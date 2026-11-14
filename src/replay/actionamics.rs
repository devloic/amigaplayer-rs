//! Actionamics Sound Tool, ported from NostalgicPlayer's Actionamics player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! Checked against the Eagleplayer "Action Amics" (V1.1), which has the
//! replayer in its own code hunk. Like the original this plays from the module
//! in guest memory: InitSound writes each sample's address into its info in the
//! module, the sample effect inverts bytes of the sample data there and keeps
//! its counters in the sample info. The voices' and the song's variables are in
//! guest memory too, in the layout of the original's hunk, so that a period
//! index outside the table reads what the original reads.
//!
//! Where NostalgicPlayer differs from the original, this follows the original:
//! * every voice writes its location, length and period every tick (after the
//!   volume and period of its effects); a note's sample goes out on its first
//!   tick, and from the next tick on its loop, whatever its length (0 or 1);
//! * a row turns off the DMA of its notes' voices at its start (DMACON is
//!   written even when no note starts), and every tick ends with DMACON on for
//!   the voices that wrote their sample: a shadow word that is never cleared but
//!   on a row, and that a note delay takes its voice out of;
//! * the sample number list writes the new sample's location and length at
//!   once;
//! * the volume is masked to 7 bits; the subsongs are all 32 entries of the
//!   table (DeliTracker's 0-31), empty ones too; the module's tempo is not used
//!   (DeliTracker's default timer);
//! * the sample effect's counter restarts from its speed word (NostalgicPlayer's
//!   is never set), and its end test is the original's (a position past the end
//!   wraps or bounces);
//! * the mute effect cuts at the tick its argument gives, not on the row;
//! * a tone portamento that is at its end period returns before its step;
//! * the original's byte and word arithmetic and signed compares are kept, and
//!   nothing is range-checked; period 9 is 3616 (NostalgicPlayer has 3816).
//!
//! Each write waits the time the original's code takes since the one before
//! (`Cost`): code-path costs fitted by least squares to the gaps of the
//! Eagleplayer's write logs, so that the writes come within a few colour clocks
//! of the original's. The init writes (DTP_Volume and DTP_Balance, which write
//! the last volumes again, and InitSound's DMA off) and the first tick come when
//! the reference's do; InitSound's loops through the samples set the time to
//! the first tick (54 colour clocks a sample).

use super::{Hw, Open, Replayer};
use crate::machine::Machine;

pub const OPEN: Open = Open { name: "Actionamics", detect, new };

include!("actionamics_tables.rs");

const ID: &[u8] = b"ACTIONAMICS SOUND TOOL";

// Offsets in the original's code hunk.
const HUNK_SIZE: u32 = 0x12bc;
const VOICES: u32 = 0x1080;
const VOICE_SIZE: u32 = 0x62;
/// The song's variables (bytes): end position, speed, position, row, speed
/// counter, rows, measure counter, loop position; the tempo (word) and the
/// subsong.
const SONG: u32 = 0x120e;
const S_END: u32 = 0;
const S_SPEED: u32 = 1;
const S_POS: u32 = 2;
const S_ROW: u32 = 3;
const S_COUNTER: u32 = 4;
const S_ROWS: u32 = 5;
const S_MEASURE: u32 = 6;
const S_LOOP: u32 = 7;
const S_TEMPO: u32 = 8;
const S_SUBSONG: u32 = 10;
const MODULE: u32 = 0x1208;
const PERIOD_TABLE: u32 = 0x121a;
/// The sample infos the sample effect went through this time (longs).
const TAKEN: u32 = 0x12ac;

// A voice's variables.
const V_POSITIONS: u32 = 0x00;
const V_ADR: u32 = 0x04;
const V_LEN: u32 = 0x08;
const V_PER: u32 = 0x0a;
const V_VOL: u32 = 0x0c;
const V_DMA: u32 = 0x0e;
const V_ENV: u32 = 0x12;
const V_SUSTAIN: u32 = 0x14;
const V_INSTR: u32 = 0x16;
const V_LOOP: u32 = 0x1a;
const V_LOOP_LEN: u32 = 0x1e;
const V_NOTE_FINAL: u32 = 0x20;
const V_SL: u32 = 0x22;
const V_SL_COUNT: u32 = 0x23;
const V_SL_POS: u32 = 0x24;
const V_AL: u32 = 0x26;
const V_AL_COUNT: u32 = 0x27;
const V_AL_POS: u32 = 0x28;
const V_FL_COUNT: u32 = 0x2b;
const V_FL_POS: u32 = 0x2c;
const V_NOTE: u32 = 0x2e;
const V_INSTR_NR: u32 = 0x30;
const V_FX: u32 = 0x32;
const V_ARG: u32 = 0x34;
const V_PORTA_COUNT: u32 = 0x36;
const V_INSTR_TRANSPOSE: u32 = 0x39;
const V_PORTA: u32 = 0x3a;
const V_TP_END: u32 = 0x3c;
const V_TP_STEP: u32 = 0x3e;
const V_TRACK: u32 = 0x40;
const V_WAIT: u32 = 0x45;
const V_NOTE_TRANSPOSE: u32 = 0x47;
const V_SAMPLE: u32 = 0x48;
const V_VIB: u32 = 0x4a;
const V_VIB_POS: u32 = 0x4b;
const V_NOTE_PER: u32 = 0x4c;
const V_SFX: u32 = 0x4e;
const V_TREM: u32 = 0x50;
const V_TREM_POS: u32 = 0x51;
const V_TREM_VOL: u32 = 0x52;
const V_OFFSET: u32 = 0x54;
const V_DELAY: u32 = 0x56;
const V_RESTART: u32 = 0x58;
const V_RESTART_ADR: u32 = 0x5a;
const V_RESTART_LEN: u32 = 0x5e;
const V_MASTER: u32 = 0x60;

// A sample's info in the module (64 bytes).
const SI_ADR: u32 = 0x00;
const SI_LEN: u32 = 0x04;
const SI_LOOP: u32 = 0x06;
const SI_LOOP_LEN: u32 = 0x08;
const SI_FX_START: u32 = 0x0a;
/// Its high byte is also the sample's arpeggio list.
const SI_FX_LEN: u32 = 0x0c;
const SI_FX_SPEED: u32 = 0x0e;
const SI_FX_MODE: u32 = 0x10;
const SI_FX_STEP: u32 = 0x12;
const SI_FX_POS: u32 = 0x14;
const SI_FX_COUNT: u32 = 0x18;
const SI_FX_TAKEN: u32 = 0x1a;

// An instrument in the module (32 bytes): the sample number, arpeggio and
// frequency lists (list, values, counter step back, counter end), portamento,
// note transpose, the envelope.
const I_PORTA: u32 = 0x0c;
const I_PORTA_DELAY: u32 = 0x0d;
const I_TRANSPOSE: u32 = 0x0e;
const I_ENV: u32 = 0x10;
const I_ATTACK_SPEED: u32 = 0x11;

// The module's parts, in the original's table of their addresses.
const T_POS_TRACKS: usize = 2;
const T_POS_NOTE: usize = 3;
const T_POS_INSTR: usize = 4;
const T_INSTRUMENTS: usize = 5;
const T_SAMPLE_LISTS: usize = 6;
const T_ARP_LISTS: usize = 7;
const T_FREQ_LISTS: usize = 8;
const T_SUBSONGS: usize = 11;
const T_SAMPLES: usize = 13;
const T_TRACK_OFFSETS: usize = 14;
const T_TRACKS: usize = 15;

/// InitSound's time after its DMACON write, but for its sample loops (counted
/// from its code), and the host's from there to the first write at a subsong
/// change (measured, with it).
const INIT_REST: i64 = 2915;
const SUBSONG_LEAD: u64 = 219;
/// The song start's lag behind the end of InitSound (measured: the reference's song starts that
/// much later, its first tick does not).
const START_LATE: u64 = 60;

fn detect(d: &[u8]) -> bool {
    // the original checks the mark only
    d.get(62..62 + ID.len()) == Some(ID)
}

/// The code paths of the original whose time is counted (`Cost::clocks`), and
/// the register writes.
#[derive(Clone, Copy, Debug)]
enum Cost {
    Tick,
    Measure3,
    Row,
    RowVoice,
    RowWait,
    RowByte,
    RowFx,
    RowDelay,
    RowNote,
    RowInstr,
    RowNoteDma,
    Note,
    NoteOn,
    NoteInstr,
    Inv,
    InvVoice,
    InvTaken,
    InvCount,
    InvReload,
    InvFlip,
    InvEdge,
    InvEnd,
    RowEnd,
    NewPos,
    SongEnd,
    Track,
    Voice,
    Voice4,
    Restart,
    Restart0,
    Fx,
    EnvNone,
    EnvAttack,
    EnvAttackEnd,
    EnvDecay,
    EnvDecayEnd,
    EnvSustain,
    EnvSustainEnd,
    EnvRelease,
    EnvReleaseEnd,
    EnvSkip,
    EnvDone,
    VolTail,
    List,
    ListOn,
    ListCount,
    ListStep,
    ListWrap,
    ListNeg,
    ListSet,
    TrackFx,
    TrackFxOn,
    TrackFxJump,
    FxArp,
    FxSlide,
    FxVolSlideSet,
    FxVolSlide,
    FxVolSlideTest,
    FxVibArg,
    FxVib,
    FxMul,
    FxTrem,
    FxSmall,
    FxOffset,
    FxOffsetCut,
    FxArg,
    FxVolume,
    FxVolumeMax,
    Arp,
    ArpOn,
    ArpCount,
    ArpStep,
    ArpWrap,
    Freq,
    FreqOn,
    FreqCount,
    FreqStep,
    FreqWrap,
    Porta,
    PortaOn,
    PortaCount,
    PortaStep,
    Tone,
    ToneSet,
    ToneStep,
    ToneEnd,
    FxEnd,
    Clamp,
    ClampHigh,
    Delayed,
    Trig,
    TrigLen,
    TrigPer,
    End,
    WriteVol,
    WritePer,
    WriteLoc,
    WriteLen,
    WriteDmaOff,
    WriteDmaOn,
}

impl Cost {
    /// 68000 cycles (two a colour clock), fitted by least squares to the
    /// Eagleplayer's write logs (all subsongs of the two modules known); the
    /// paths those never take (the sample effect's inversion, the sample number,
    /// arpeggio and frequency lists, the arpeggio and tremolo effects,
    /// portamento, tone portamento, the decay's and release's ends, the high
    /// period clamp) are counted from the original's code.
    fn cycles(self) -> i64 {
        use Cost::*;
        match self {
            Tick => 657,
            Measure3 => 16,
            Row => 41,
            RowVoice => 164,
            RowWait => 82,
            RowByte => 64,
            RowFx => 47,
            RowDelay => 35,
            RowNote => -1,
            RowInstr => -18,
            RowNoteDma => 31,
            Note => 81,
            NoteOn => 597,
            NoteInstr => 74,
            Inv => 37,
            InvVoice => 150,
            InvTaken => 36,
            InvCount => 42,
            InvReload => 114,
            InvFlip => 224,
            InvEdge => 40,
            InvEnd => 37,
            RowEnd => 20,
            NewPos => 1263,
            SongEnd => 88,
            Track => 0,
            Voice => 126,
            Voice4 => 16,
            Restart => 29,
            Restart0 => 13,
            Fx => 179,
            EnvNone => -30,
            EnvAttack => 58,
            EnvAttackEnd => 58,
            EnvDecay => 18,
            EnvDecayEnd => 28,
            EnvSustain => 65,
            EnvSustainEnd => 65,
            EnvRelease => 68,
            EnvReleaseEnd => 28,
            EnvSkip => 87,
            EnvDone => -1,
            VolTail => 43,
            List => 43,
            ListOn => 34,
            ListCount => 32,
            ListStep => 180,
            ListWrap => 14,
            ListNeg => 32,
            ListSet => 112,
            TrackFx => 43,
            TrackFxOn => 63,
            TrackFxJump => 34,
            FxArp => 166,
            FxSlide => 36,
            FxVolSlideSet => 76,
            FxVolSlide => 112,
            FxVolSlideTest => 18,
            FxVibArg => 95,
            FxVib => 252,
            FxMul => -1,
            FxTrem => 360,
            FxSmall => 40,
            FxOffset => 148,
            FxOffsetCut => -44,
            FxArg => 46,
            FxVolume => 86,
            FxVolumeMax => 10,
            Arp => 86,
            ArpOn => 34,
            ArpCount => 32,
            ArpStep => 250,
            ArpWrap => 14,
            Freq => 86,
            FreqOn => 34,
            FreqCount => 32,
            FreqStep => 210,
            FreqWrap => 14,
            Porta => 86,
            PortaOn => 34,
            PortaCount => 32,
            PortaStep => 36,
            Tone => 86,
            ToneSet => 140,
            ToneStep => 80,
            ToneEnd => 70,
            FxEnd => 86,
            Clamp => -97,
            ClampHigh => 16,
            Delayed => -52,
            Trig => 56,
            TrigLen => 22,
            TrigPer => -23,
            End => 66,
            WriteVol => 264,
            WritePer => 63,
            WriteLoc => 56,
            WriteLen => 22,
            WriteDmaOff => 54,
            WriteDmaOn => 66,
        }
    }
}

/// The trace of code paths for fitting their costs (ACTIONAMICS_TRACE=file):
/// one line per register write, with the paths taken since the one before.
struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<Cost>,
}

impl Trace {
    fn open() -> Self {
        let out = std::env::var_os("ACTIONAMICS_TRACE")
            .and_then(|p| std::fs::File::create(p).ok())
            .map(std::io::BufWriter::new);
        Trace { out, pend: Vec::new() }
    }

    /// A tick starts.
    fn mark(&mut self, hw: &Hw) {
        if let Some(o) = &mut self.out {
            use std::io::Write;
            let _ = writeln!(o, "T {}", hw.bus.clock);
        }
    }

    fn write(&mut self, hw: &Hw) {
        if let Some(o) = &mut self.out {
            use std::io::Write;
            let _ = write!(o, "{}", hw.bus.clock);
            for e in self.pend.drain(..) {
                let _ = write!(o, " {:?}", e);
            }
            let _ = writeln!(o);
        }
    }
}

pub struct Actionamics {
    /// The original's code hunk, as far as its variables go.
    p: u32,
    base: u32,
    /// The addresses of the module's parts (the original's table at $466).
    t: [u32; 16],
    /// The DMACON shadow word ($FEC), which starts as $800F.
    dma: u16,
    /// The volumes of the voices' last writes (the UPS structure's), which
    /// DTP_Volume writes again.
    vols: [u16; 4],
    sub: u16,
    songend: bool,
    /// The odd 68000 cycle not yet waited (`spend`).
    cycles: i64,
    trace: Trace,
    /// The first tick's clock, taken by the engine at the start.
    first_tick: Option<u64>,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let p = m.alloc(HUNK_SIZE);
    if p == 0 {
        return None;
    }
    let bus = &mut m.bus;
    for (k, &v) in PERIODS.iter().enumerate() {
        bus.w16(p + PERIOD_TABLE + 2 * k as u32, v);
    }
    // InitPlayer
    bus.w32(p + MODULE, base);
    let mut a = Actionamics {
        p,
        base,
        t: [0; 16],
        dma: 0x800f,
        vols: [0; 4],
        sub: 0,
        songend: false,
        cycles: 0,
        trace: Trace::open(),
        first_tick: None,
    };
    let mut hw = Hw { bus: &mut m.bus };
    a.start_sound(0, &mut hw, 687);
    // the reference's start call ends START_LATE colour clocks later, its timer runs from where
    // it would have without them
    a.first_tick = Some(hw.bus.clock + super::DT_TICK);
    hw.wait(START_LATE);
    Some(Box::new(a))
}

impl Replayer for Actionamics {
    fn format(&self) -> &'static str {
        "Actionamics"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, 31, self.sub)
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.first_tick.take()
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // DeliTracker's StopSound and StartSound
        self.start_sound(n, hw, SUBSONG_LEAD);
    }

    /// The original's interrupt: the rows when the speed counter is up, then
    /// each voice's effects and sample, then the DMA.
    fn tick(&mut self, hw: &mut Hw) {
        let s = self.p + SONG;
        self.trace.mark(hw);
        self.at(hw, Cost::Tick);
        let m = hw.bus.r8(s + S_MEASURE).wrapping_add(1);
        hw.bus.w8(s + S_MEASURE, m);
        if m == 3 {
            self.at(hw, Cost::Measure3);
            hw.bus.w8(s + S_MEASURE, 0);
        }
        let c = hw.bus.r8(s + S_COUNTER).wrapping_add(1);
        hw.bus.w8(s + S_COUNTER, c);
        if c == hw.bus.r8(s + S_SPEED) {
            self.at(hw, Cost::Row);
            hw.bus.w8(s + S_COUNTER, 0);
            hw.bus.w8(s + S_MEASURE, 0);
            self.dma = 0;
            for i in 0..4 {
                self.read_row(hw, i);
            }
            self.write(hw, Cost::WriteDmaOff);
            hw.dmacon(self.dma);
            self.dma = self.dma.wrapping_add(0x8000);
            for i in 0..4 {
                self.setup_note(hw, i);
            }
            self.sample_fx(hw);
            self.at(hw, Cost::RowEnd);
            let row = hw.bus.r8(s + S_ROW).wrapping_add(1);
            hw.bus.w8(s + S_ROW, row);
            if row == hw.bus.r8(s + S_ROWS) {
                self.at(hw, Cost::NewPos);
                hw.bus.w8(s + S_ROW, 0);
                let pos = hw.bus.r8(s + S_POS);
                hw.bus.w8(s + S_POS, pos.wrapping_add(1));
                if pos == hw.bus.r8(s + S_END) {
                    self.at(hw, Cost::SongEnd);
                    self.songend = true;
                    hw.bus.w8(s + S_POS, hw.bus.r8(s + S_LOOP));
                }
                for i in 0..4 {
                    self.setup_track(hw, i);
                }
            }
        }

        for i in 0..4 {
            let v = self.voice(i);
            self.at(hw, Cost::Voice);
            // the fourth voice is skipped while the player's sound effects
            // (which the Eagleplayer never starts) have it
            if i == 3 {
                self.at(hw, Cost::Voice4);
                if (hw.bus.r16(v + V_SFX) as i16) < 0 {
                    continue;
                }
            }
            let r = hw.bus.r16(v + V_RESTART);
            if r != 0 {
                self.at(hw, Cost::Restart);
                hw.bus.w16(v + V_RESTART, r - 1);
                if r == 1 {
                    self.at(hw, Cost::Restart0);
                    hw.bus.w32(v + V_ADR, hw.bus.r32(v + V_RESTART_ADR));
                    hw.bus.w16(v + V_LEN, hw.bus.r16(v + V_RESTART_LEN));
                    // the voice's bit alone: its DMA off
                    self.write(hw, Cost::WriteDmaOff);
                    hw.dmacon(hw.bus.r16(v + V_DMA));
                }
            }
            self.effects(hw, i);
            let d = hw.bus.r16(v + V_DELAY).wrapping_sub(1);
            hw.bus.w16(v + V_DELAY, d);
            if d as i16 >= 0 {
                self.at(hw, Cost::Delayed);
                continue;
            }
            hw.bus.w16(v + V_DELAY, d.wrapping_add(1));
            self.at(hw, Cost::Trig);
            self.write(hw, Cost::WriteLoc);
            hw.loc(i, hw.bus.r32(v + V_ADR));
            self.at(hw, Cost::TrigLen);
            self.write(hw, Cost::WriteLen);
            hw.len(i, hw.bus.r16(v + V_LEN));
            self.at(hw, Cost::TrigPer);
            self.write(hw, Cost::WritePer);
            hw.per(i, hw.bus.r16(v + V_PER));
            hw.bus.w32(v + V_ADR, hw.bus.r32(v + V_LOOP));
            hw.bus.w16(v + V_LEN, hw.bus.r16(v + V_LOOP_LEN));
            self.dma |= hw.bus.r16(v + V_DMA) | if i == 0 { 0x8000 } else { 0 };
        }
        self.at(hw, Cost::End);
        if hw.bus.r8(s + S_COUNTER) != 0 {
            self.sample_fx(hw);
        }
        self.write(hw, Cost::WriteDmaOn);
        hw.dmacon(self.dma);
    }
}

impl Actionamics {
    fn voice(&self, i: usize) -> u32 {
        self.p + VOICES + VOICE_SIZE * i as u32
    }

    /// Counts `cycles` 68000 cycles of the original's code; they pass at the
    /// next write (in colour clocks, an odd cycle carried).
    fn spend(&mut self, cycles: i64) {
        self.cycles += cycles;
    }

    fn at(&mut self, _hw: &mut Hw, c: Cost) {
        self.spend(c.cycles());
        if self.trace.out.is_some() {
            self.trace.pend.push(c);
        }
    }

    /// The time of the instruction that writes a register: the cycles counted
    /// since the last write pass. (A path's cost alone can be negative, as the
    /// fit split the time of the original's branches over the paths; their sum
    /// is the time between two writes.)
    fn write(&mut self, hw: &mut Hw, c: Cost) {
        self.at(hw, c);
        let n = self.cycles.max(0);
        hw.wait(n as u64 / 2);
        self.cycles = n & 1;
        self.trace.write(hw);
    }

    /// The period table entry at byte offset `o` (a signed word), read as the
    /// original reads it, past the table too.
    fn period(&self, hw: &Hw, o: u16) -> u16 {
        hw.bus.r16(self.p.wrapping_add(PERIOD_TABLE).wrapping_add(o as i16 as u32))
    }

    /// The volume routine ($22A): the low 7 bits, scaled by DeliTracker's volume
    /// (full).
    fn vol_write(&mut self, hw: &mut Hw, i: usize, v: u16) {
        let v = v & 0x7f;
        self.vols[i] = v;
        self.write(hw, Cost::WriteVol);
        hw.vol(i, ((v as u32 * 64) as u16) >> 6);
    }

    /// DeliTracker's start of a song: DTP_Volume and DTP_Balance, which write
    /// the voices' last volumes again, then InitSound. The times are the
    /// reference's: the host's before the first write (`lead`, at the first
    /// start, or from the end of the last InitSound at a subsong change) and the
    /// volume routine's.
    fn start_sound(&mut self, n: u16, hw: &mut Hw, lead: u64) {
        for k in 0..2 {
            for (i, gap) in [if k == 0 { lead } else { 206 }, 149, 158, 152].into_iter().enumerate() {
                self.spend(2 * gap as i64 - Cost::WriteVol.cycles());
                let v = self.vols[i];
                self.vol_write(hw, i, v);
            }
        }
        self.init_sound(n, hw);
    }

    /// InitSound ($314).
    fn init_sound(&mut self, n: u16, hw: &mut Hw) {
        let (p, base) = (self.p, self.base);
        self.spend(2 * 198 - Cost::WriteDmaOff.cycles());
        self.write(hw, Cost::WriteDmaOff);
        hw.dmacon(0x000f);
        let bus = &mut *hw.bus;
        bus.w8(p + SONG + S_SUBSONG, (n & 0x1f) as u8);
        self.sub = n;
        for o in (0..4 * VOICE_SIZE).step_by(4) {
            bus.w32(p + VOICES + o, 0);
        }
        for i in 0..4u32 {
            let v = p + VOICES + VOICE_SIZE * i;
            bus.w16(v + V_DMA, 1 << i);
            bus.w16(v + 0x10, 0xa0 + 0x10 * i as u16);
            bus.w16(v + V_MASTER, 0x4041);
        }
        bus.w16(p + VOICES + 3 * VOICE_SIZE + V_SFX, 0);
        // the parts of the module follow each other after the header (the
        // tempo word, 15 lengths)
        self.t[0] = base.wrapping_add(62);
        bus.w16(p + SONG + S_TEMPO, bus.r16(base));
        for k in 0..15 {
            self.t[k + 1] = self.t[k].wrapping_add(bus.r32(base + 2 + 4 * k as u32));
        }
        let s = p + SONG;
        bus.w32(s, 0x0004_0000);
        bus.w32(s + 4, 0x0040_0000);
        let e = self.t[T_SUBSONGS].wrapping_add(4 * bus.r8(s + S_SUBSONG) as u32);
        bus.w8(s + S_POS, bus.r8(e));
        bus.w8(s + S_END, bus.r8(e + 1));
        bus.w8(s + S_LOOP, bus.r8(e + 2));
        bus.w8(s + S_SPEED, bus.r8(e + 3));
        self.songend = false;
        let count = self.t[T_POS_NOTE].wrapping_sub(self.t[T_POS_TRACKS]) >> 2;
        for i in 0..4 {
            let v = self.voice(i);
            hw.bus.w16(v + V_SAMPLE, 0);
            hw.bus.w32(v + V_POSITIONS, count.wrapping_mul(i as u32));
            self.setup_track(hw, i);
        }
        // the samples lie at the module's end, after each other
        let bus = &mut *hw.bus;
        let (first, n) = (self.t[T_SAMPLES], self.sample_count());
        let mut total = 0u32;
        for k in 0..n {
            total = total.wrapping_add(2 * bus.r16(first + 64 * k + SI_LEN) as u32);
        }
        let mut a = bus.r32(self.t[1]).wrapping_add(base).wrapping_sub(total);
        for k in 0..n {
            bus.w32(first + 64 * k + SI_ADR, a);
            a = a.wrapping_add(2 * bus.r16(first + 64 * k + SI_LEN) as u32);
        }
        // the rest of InitSound's time, whose loops through the samples take 54
        // colour clocks a sample (the reference's first tick comes that much
        // later), passes before the timer starts
        let rest = INIT_REST + 54 * n as i64;
        self.spend(2 * rest - 4 * Cost::Track.cycles());
        hw.wait((self.cycles.max(0) / 2) as u64);
        self.cycles = 0;
    }

    /// The samples the original's dbra loops go through: the low word of the
    /// sample infos' size over 64, as a dbra count.
    fn sample_count(&self) -> u32 {
        dbra_count((self.t[T_TRACK_OFFSETS].wrapping_sub(self.t[T_SAMPLES]) as u16) >> 6)
    }

    /// A voice's track for the current position ($884).
    fn setup_track(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        let bus = &mut *hw.bus;
        let pos = bus.r8(self.p + SONG + S_POS) as u32;
        let o = bus.r32(v + V_POSITIONS).wrapping_add(pos);
        let track = bus.r8(self.t[T_POS_TRACKS].wrapping_add(o)) as u32;
        let start = bus.r16(self.t[T_TRACK_OFFSETS].wrapping_add(2 * track)) as u32;
        bus.w32(v + V_TRACK, start.wrapping_add(self.t[T_TRACKS]));
        bus.w8(v + V_NOTE_TRANSPOSE, bus.r8(self.t[T_POS_NOTE].wrapping_add(o)));
        bus.w8(v + V_INSTR_TRANSPOSE, bus.r8(self.t[T_POS_INSTR].wrapping_add(o)));
        bus.w8(v + V_WAIT, 0);
        self.at(hw, Cost::Track);
    }

    /// A voice's row ($8C8, $902): note, instrument, effect and argument, or
    /// a wait of rows; a note's voice goes into the DMA off mask.
    fn read_row(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        self.at(hw, Cost::RowVoice);
        let mut a = hw.bus.r32(v + V_TRACK);
        hw.bus.w32(v + V_NOTE, 0);
        hw.bus.w32(v + V_FX, 0);
        let wait = hw.bus.r8(v + V_WAIT);
        if wait != 0 {
            self.at(hw, Cost::RowWait);
            hw.bus.w8(v + V_WAIT, wait - 1);
        } else {
            // up to three bytes: note, instrument, effect (and its argument);
            // an effect may come in place of the note or the instrument, and a
            // byte with bit 7 set ends the row with a wait
            for k in 0..3 {
                self.at(hw, Cost::RowByte);
                let b = hw.bus.r8(a);
                a = a.wrapping_add(1);
                if b & 0x80 != 0 {
                    self.at(hw, Cost::RowDelay);
                    hw.bus.w8(v + V_WAIT, !b);
                    break;
                }
                if k == 2 || b >= 0x70 {
                    self.at(hw, Cost::RowFx);
                    hw.bus.w8(v + V_FX + 1, b);
                    hw.bus.w8(v + V_ARG + 1, hw.bus.r8(a));
                    a = a.wrapping_add(1);
                    break;
                }
                if k == 0 {
                    self.at(hw, Cost::RowNote);
                    hw.bus.w8(v + V_NOTE + 1, b);
                } else {
                    self.at(hw, Cost::RowInstr);
                    hw.bus.w8(v + V_INSTR_NR + 1, b);
                }
            }
        }
        hw.bus.w32(v + V_TRACK, a);
        // (the sound effects' note, for a voice they have, never comes)
        let note = hw.bus.r16(v + V_NOTE);
        if note != 0 {
            self.at(hw, Cost::RowNoteDma);
            self.dma = self.dma.wrapping_add(hw.bus.r16(v + V_DMA));
            let n = (note as u8).wrapping_add(hw.bus.r8(v + V_NOTE_TRANSPOSE));
            hw.bus.w16(v + V_NOTE, note & 0xff00 | n as u16);
        }
    }

    /// A note's instrument, sample and period ($95E).
    fn setup_note(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        self.at(hw, Cost::Note);
        hw.bus.w16(v + V_PORTA, 0);
        let note = hw.bus.r16(v + V_NOTE);
        if note == 0 {
            return;
        }
        self.at(hw, Cost::NoteOn);
        let bus = &mut *hw.bus;
        bus.w16(v + V_VOL, 0);
        bus.w32(v + V_SL, 0);
        bus.w16(v + V_AL, 0);
        bus.w32(v + V_AL_POS, 0);
        bus.w16(v + V_FL_POS, 0);
        bus.w16(v + V_PORTA_COUNT, 0);
        bus.w16(v + V_TP_STEP, 0);
        bus.w16(v + V_ENV, 4);
        bus.w16(v + V_SUSTAIN, 0);
        let nr = bus.r16(v + V_INSTR_NR);
        if nr != 0 {
            let lo = (nr as u8).wrapping_sub(1).wrapping_add(bus.r8(v + V_INSTR_TRANSPOSE));
            let o = (nr & 0xff00 | lo as u16) << 5;
            bus.w32(v + V_INSTR, self.t[T_INSTRUMENTS].wrapping_add(o as i16 as u32));
        }
        let ins = bus.r32(v + V_INSTR);
        let mut d0 = note & 0xff00 | (note as u8).wrapping_add(bus.r8(ins.wrapping_add(I_TRANSPOSE))) as u16;
        bus.w16(v + V_NOTE_FINAL, d0);
        let list = (bus.r8(ins) as u32) << 4;
        let s = bus.r8(self.t[T_SAMPLE_LISTS].wrapping_add(list));
        bus.w8(v + V_SAMPLE + 1, s);
        let si = self.t[T_SAMPLES].wrapping_add(((s as u16) << 6) as i16 as u32);
        bus.w32(v + V_ADR, bus.r32(si + SI_ADR));
        bus.w16(v + V_LEN, bus.r16(si + SI_LEN));
        bus.w32(v + V_LOOP, bus.r32(v + V_ADR).wrapping_add(2 * bus.r16(si + SI_LOOP) as u32));
        bus.w16(v + V_LOOP_LEN, bus.r16(si + SI_LOOP_LEN));
        let arp = (bus.r8(si + SI_FX_LEN) as u32) << 4;
        d0 = d0.wrapping_add(bus.r8(self.t[T_ARP_LISTS].wrapping_add(arp)) as i8 as u16);
        let per = self.period(hw, d0.wrapping_add(d0));
        hw.bus.w16(v + V_NOTE_PER, per);
        hw.bus.w16(v + V_PER, per);
        if nr != 0 {
            self.at(hw, Cost::NoteInstr);
        }
    }

    /// The sample effect ($7CA): every few ticks it inverts a byte of a voice's
    /// sample, stepping through a range of it. Each sample once a time, however
    /// many voices play it.
    fn sample_fx(&mut self, hw: &mut Hw) {
        self.at(hw, Cost::Inv);
        let mut taken = [0u32; 4];
        for (i, tk) in taken.iter_mut().enumerate() {
            self.at(hw, Cost::InvVoice);
            let v = self.voice(i);
            let bus = &mut *hw.bus;
            let o = (bus.r16(v + V_SAMPLE) << 6) as i16 as u32;
            let a0 = self.t[T_SAMPLES].wrapping_add(o);
            *tk = a0;
            bus.w32(self.p + TAKEN + 4 * i as u32, a0);
            let tk = bus.r16(a0 + SI_FX_TAKEN);
            if tk != 0 {
                self.at(hw, Cost::InvTaken);
                continue;
            }
            bus.w16(a0 + SI_FX_TAKEN, !tk);
            let c = bus.r16(a0 + SI_FX_COUNT);
            if c != 0 {
                bus.w16(a0 + SI_FX_COUNT, c.wrapping_sub(1) & 0x1f);
                self.at(hw, Cost::InvCount);
                continue;
            }
            bus.w16(a0 + SI_FX_COUNT, bus.r16(a0 + SI_FX_SPEED));
            let mode = bus.r16(a0 + SI_FX_MODE);
            self.at(hw, Cost::InvReload);
            if mode == 0 {
                continue;
            }
            let bus = &mut *hw.bus;
            let at = bus
                .r32(a0 + SI_ADR)
                .wrapping_add(2 * bus.r16(a0 + SI_FX_START) as u32)
                .wrapping_add(bus.r32(a0 + SI_FX_POS));
            let end = (2 * bus.r16(a0 + SI_FX_LEN) as u32).wrapping_sub(1);
            bus.w8(at, !bus.r8(at));
            let step = bus.r16(a0 + SI_FX_STEP) as i16 as u32;
            let pos = bus.r32(a0 + SI_FX_POS).wrapping_add(step);
            bus.w32(a0 + SI_FX_POS, pos);
            self.at(hw, Cost::InvFlip);
            let bus = &mut *hw.bus;
            let edge = if (pos as i32) < 0 {
                Some(mode == 2)
            } else if end < pos {
                Some(mode == 1)
            } else {
                None
            };
            match edge {
                // wrap to the other end
                Some(true) => bus.w32(a0 + SI_FX_POS, if mode == 2 { end } else { 0 }),
                // bounce
                Some(false) => {
                    bus.w32(a0 + SI_FX_POS, pos.wrapping_sub(step));
                    bus.w16(a0 + SI_FX_STEP, (step as u16).wrapping_neg());
                }
                None => {}
            }
            if edge.is_some() {
                self.at(hw, Cost::InvEdge);
            }
        }
        self.at(hw, Cost::InvEnd);
        for a0 in taken {
            hw.bus.w16(a0 + SI_FX_TAKEN, 0);
        }
    }

    /// A voice's effects ($A1A): envelope and volume, sample number list, the
    /// track effect, arpeggio and frequency lists, portamento, tone portamento,
    /// and the period.
    fn effects(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        self.at(hw, Cost::Fx);
        self.envelope(hw, i);
        let vol = hw.bus.r16(v + V_VOL);
        hw.bus.w16(v + V_TREM, vol);
        let d0 = self.master(hw, i, vol);
        self.vol_write(hw, i, d0);
        self.at(hw, Cost::VolTail);
        self.sample_list(hw, i);
        self.track_fx(hw, i);
        self.arp_list(hw, i);
        self.freq_list(hw, i);
        let ins = self.porta(hw, i);
        self.tone_porta(hw, i, ins);
        self.at(hw, Cost::FxEnd);
        let per = hw.bus.r16(v + V_PER).wrapping_add(hw.bus.r16(v + V_PORTA));
        hw.bus.w16(v + V_PER, per);
        if per as i16 <= 0x5f {
            self.at(hw, Cost::Clamp);
            hw.bus.w16(v + V_PER, 0x5f);
        } else if per as i16 >= 0x1680 {
            self.at(hw, Cost::ClampHigh);
            hw.bus.w16(v + V_PER, 0x1680);
        }
        self.write(hw, Cost::WritePer);
        hw.per(i, hw.bus.r16(v + V_PER));
    }

    /// A volume times the voice's master volume ($4041: 255 is 64).
    fn master(&self, hw: &Hw, i: usize, vol: u16) -> u16 {
        ((vol as u32 * hw.bus.r16(self.voice(i) + V_MASTER) as u32) >> 16) as u16
    }

    /// The envelope ($F48): attack, decay, sustain, release.
    fn envelope(&mut self, hw: &mut Hw, i: usize) {
        use Cost::*;
        let v = self.voice(i);
        let ins = hw.bus.r32(v + V_INSTR);
        if ins == 0 {
            self.at(hw, EnvNone);
            return;
        }
        let e = ins.wrapping_add(I_ENV);
        let st = hw.bus.r16(v + V_ENV);
        let bus = &mut *hw.bus;
        let vol = bus.r16(v + V_VOL);
        // each phase: its end volume and speed, the cost of getting there and
        // of its end
        let (phase, end_cost) = match st {
            4 => {
                let (end, speed) = (bus.r8(e) as u16, bus.r8(e + 1) as u16);
                let nv = vol.wrapping_add(speed);
                bus.w16(v + V_VOL, nv);
                self.at(hw, EnvAttack);
                if end as i16 > nv as i16 {
                    return;
                }
                hw.bus.w16(v + V_VOL, end);
                (EnvAttackEnd, true)
            }
            3 | 1 => {
                let k = if st == 3 { 2 } else { 5 };
                let (end, speed) = (bus.r8(e + k) as u16, bus.r8(e + k + 1) as u16);
                self.at(hw, if st == 3 { EnvDecay } else { EnvRelease });
                if speed == 0 {
                    (EnvSkip, true)
                } else {
                    let nv = vol.wrapping_sub(speed);
                    hw.bus.w16(v + V_VOL, nv);
                    if (end as i16) < nv as i16 {
                        return;
                    }
                    hw.bus.w16(v + V_VOL, end);
                    (if st == 3 { EnvDecayEnd } else { EnvReleaseEnd }, true)
                }
            }
            2 => {
                let d = bus.r8(e + 4) as u16;
                let c = bus.r16(v + V_SUSTAIN);
                self.at(hw, EnvSustain);
                if d != c {
                    hw.bus.w16(v + V_SUSTAIN, c.wrapping_add(1));
                    return;
                }
                (EnvSustainEnd, true)
            }
            _ => (EnvDone, false),
        };
        self.at(hw, phase);
        if end_cost {
            hw.bus.w16(v + V_ENV, st - 1);
        }
    }

    /// The instrument's sample number list ($EB8): the next sample, written to
    /// Paula at once.
    fn sample_list(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        self.at(hw, Cost::List);
        let ins = hw.bus.r32(v + V_INSTR);
        if ins == 0 || hw.bus.r8(ins + 1) == 0 {
            return;
        }
        self.at(hw, Cost::ListOn);
        let bus = &mut *hw.bus;
        let end = bus.r8(ins + 3);
        let c = bus.r8(v + V_SL_COUNT);
        if end != c {
            bus.w8(v + V_SL_COUNT, c.wrapping_add(1));
            self.at(hw, Cost::ListCount);
            return;
        }
        bus.w8(v + V_SL_COUNT, end.wrapping_sub(bus.r8(ins + 2)));
        let wrap = bus.r8(ins + 1) == bus.r8(v + V_SL_POS + 1);
        if wrap {
            bus.w8(v + V_SL_POS + 1, 0xff);
        }
        bus.w8(v + V_SL_POS + 1, bus.r8(v + V_SL_POS + 1).wrapping_add(1));
        let o = ((bus.r8(ins) as u16) << 4).wrapping_add(bus.r16(v + V_SL_POS));
        let s = bus.r8(self.t[T_SAMPLE_LISTS].wrapping_add(o as i16 as u32));
        self.at(hw, Cost::ListStep);
        if wrap {
            self.at(hw, Cost::ListWrap);
        }
        if (s as i8) < 0 {
            self.at(hw, Cost::ListNeg);
            hw.bus.w8(v + V_SL_POS + 1, hw.bus.r8(v + V_SL_POS + 1).wrapping_sub(1));
            return;
        }
        let bus = &mut *hw.bus;
        bus.w8(v + V_SAMPLE + 1, s);
        let si = self.t[T_SAMPLES].wrapping_add(((s as u16) << 6) as i16 as u32);
        bus.w32(v + V_LOOP, bus.r32(si + SI_ADR));
        bus.w16(v + V_LOOP_LEN, bus.r16(si + SI_LEN));
        self.at(hw, Cost::ListSet);
        self.write(hw, Cost::WriteLoc);
        hw.loc(i, hw.bus.r32(v + V_LOOP));
        self.write(hw, Cost::WriteLen);
        hw.len(i, hw.bus.r16(v + V_LOOP_LEN));
    }

    /// The track effect ($A92): $70-$74 on the ticks after the row, $75-$7F on
    /// every tick.
    fn track_fx(&mut self, hw: &mut Hw, i: usize) {
        use Cost::*;
        let v = self.voice(i);
        let s = self.p + SONG;
        self.at(hw, TrackFx);
        let fx = hw.bus.r16(v + V_FX);
        if (fx as i16) < 0x70 {
            return;
        }
        let fx = fx & 0xf;
        self.at(hw, TrackFxOn);
        let counter = hw.bus.r8(s + S_COUNTER);
        if counter == 0 && fx < 5 {
            return;
        }
        self.at(hw, TrackFxJump);
        let arg = hw.bus.r16(v + V_ARG);
        let arg8 = arg as u8;
        match fx {
            0 => {
                // arpeggio: the note, plus the high nibble, the note, plus the
                // low nibble (and so on, by the measure counter)
                self.at(hw, FxArp);
                let m = hw.bus.r8(s + S_MEASURE);
                let add = [(arg >> 4) as u8, 0, arg8 & 0xf, 0][m as usize & 3];
                let n = (add as u16).wrapping_add(hw.bus.r16(v + V_NOTE_FINAL));
                let per = self.period(hw, n.wrapping_add(n));
                hw.bus.w16(v + V_PER, per);
            }
            1 | 2 => {
                self.at(hw, FxSlide);
                let d = if fx == 1 { arg.wrapping_neg() } else { arg };
                hw.bus.w16(v + V_PORTA, d);
                hw.bus.w16(v + V_NOTE_PER, hw.bus.r16(v + V_NOTE_PER).wrapping_add(d));
            }
            3 | 0xd | 0xe => {
                self.vol_slide(hw, i, fx, counter);
                if fx == 0xe {
                    self.vibrato(hw, i);
                }
            }
            4 => {
                if arg8 != 0 {
                    self.at(hw, FxVibArg);
                    let d = nibbles(hw.bus.r8(v + V_VIB), arg8);
                    hw.bus.w8(v + V_VIB, d);
                }
                self.vibrato(hw, i);
            }
            5 => {
                self.at(hw, FxSmall);
                hw.bus.w8(s + S_ROWS, arg8);
            }
            6 => {
                self.at(hw, FxOffset);
                if arg != 0 {
                    hw.bus.w16(v + V_OFFSET, arg);
                }
                let o = hw.bus.r16(v + V_OFFSET) << 7;
                let len = hw.bus.r16(v + V_LEN);
                if o as i16 >= len as i16 {
                    self.at(hw, FxOffsetCut);
                    hw.bus.w16(v + V_LEN, 1);
                } else {
                    hw.bus.w16(v + V_LEN, len.wrapping_sub(o));
                    let a = hw.bus.r32(v + V_ADR).wrapping_add(o.wrapping_add(o) as u32);
                    hw.bus.w32(v + V_ADR, a);
                }
            }
            7 => {
                // note delay: the voice's DMA stays off until its sample is
                // written
                self.at(hw, FxSmall);
                if arg != 0 {
                    self.at(hw, FxArg);
                    hw.bus.w32(v + V_FX, 0);
                    hw.bus.w16(v + V_DELAY, arg);
                    self.dma &= !hw.bus.r16(v + V_DMA);
                }
            }
            8 => {
                // mute at the tick the argument gives
                self.at(hw, FxSmall);
                if arg8 != 0 && arg8 == counter {
                    self.at(hw, FxArg);
                    hw.bus.w16(v + V_VOL, 0);
                    hw.bus.w16(v + V_TREM, 0);
                    self.vol_write(hw, i, 0);
                }
            }
            9 => {
                self.at(hw, FxSmall);
                if arg != 0 {
                    self.at(hw, FxArg);
                    hw.bus.w32(v + V_FX, 0);
                    hw.bus.w16(v + V_RESTART, arg);
                    hw.bus.w32(v + V_RESTART_ADR, hw.bus.r32(v + V_ADR));
                    hw.bus.w16(v + V_RESTART_LEN, hw.bus.r16(v + V_LEN));
                }
            }
            0xa => self.tremolo(hw, i, arg8),
            0xb => {
                self.at(hw, FxSmall);
                hw.bus.w8(s + S_ROW, hw.bus.r8(s + S_ROWS).wrapping_sub(1));
            }
            0xc => {
                self.at(hw, FxVolume);
                let mut d0 = arg.wrapping_mul(4);
                if d0 as i16 >= 0xff {
                    self.at(hw, FxVolumeMax);
                    d0 = 0xff;
                }
                hw.bus.w16(v + V_VOL, d0);
                let d = self.master(hw, i, d0);
                self.vol_write(hw, i, d);
            }
            _ => {
                self.at(hw, FxSmall);
                if arg8 < 0x1f {
                    hw.bus.w8(s + S_SPEED, arg8);
                }
            }
        }
    }

    /// Effects $73, $7D and $7E's volume slide ($BB4, $BBC): $73 and $7E only
    /// once the envelope is done; on the row the volume starts from the
    /// instrument's attack speed ($73 and $7E only with an instrument on the
    /// row).
    fn vol_slide(&mut self, hw: &mut Hw, i: usize, fx: u16, counter: u8) {
        let v = self.voice(i);
        if fx != 0xd {
            self.at(hw, Cost::FxVolSlideTest);
            if hw.bus.r16(v + V_ENV) != 0 {
                return;
            }
        }
        if counter == 0 && (fx == 0xd || hw.bus.r16(v + V_INSTR_NR) != 0) {
            self.at(hw, Cost::FxVolSlideSet);
            let ins = hw.bus.r32(v + V_INSTR);
            if ins == 0 {
                return;
            }
            hw.bus.w8(v + V_VOL + 1, hw.bus.r8(ins + I_ATTACK_SPEED));
        }
        self.at(hw, Cost::FxVolSlide);
        let arg = hw.bus.r16(v + V_ARG);
        let vol = hw.bus.r16(v + V_VOL);
        let d0 = if arg as u8 & 0xf != 0 {
            let d = vol.wrapping_sub(arg.wrapping_mul(4));
            if (d as i16) < 0 {
                0
            } else {
                d
            }
        } else {
            let d = vol.wrapping_add((arg >> 4).wrapping_mul(4));
            if d as i16 >= 0x100 {
                0xff
            } else {
                d
            }
        };
        hw.bus.w16(v + V_VOL, d0);
        hw.bus.w16(v + V_TREM, d0);
    }

    /// The vibrato ($C4E) on the note's period.
    fn vibrato(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        let pos = hw.bus.r8(v + V_VIB_POS);
        let vib = hw.bus.r8(v + V_VIB);
        let depth = (vib & 0xf) as u16;
        self.at(hw, Cost::FxVib);
        for _ in 0..depth.count_ones() {
            self.at(hw, Cost::FxMul);
        }
        let mut d = (SINUS[(pos >> 2) as usize & 0x1f] as u16 * depth) >> 7;
        if (pos as i8) >= 0 {
            d = d.wrapping_neg();
        }
        hw.bus.w16(v + V_PER, hw.bus.r16(v + V_NOTE_PER).wrapping_sub(d));
        hw.bus.w8(v + V_VIB_POS, pos.wrapping_add(vib >> 2));
    }

    /// The tremolo ($CA8), on a volume the player never sets (0).
    fn tremolo(&mut self, hw: &mut Hw, i: usize, arg: u8) {
        let v = self.voice(i);
        if arg != 0 {
            self.at(hw, Cost::FxVibArg);
            let d = nibbles(hw.bus.r8(v + V_TREM), arg);
            hw.bus.w8(v + V_TREM, d);
        }
        let pos = hw.bus.r8(v + V_TREM_POS);
        let trem = hw.bus.r8(v + V_TREM);
        let depth = (trem & 0xf) as u16;
        self.at(hw, Cost::FxTrem);
        for _ in 0..depth.count_ones() {
            self.at(hw, Cost::FxMul);
        }
        let mut d = (SINUS[(pos >> 2) as usize & 0x1f] as u16 * depth) >> 6;
        if (pos as i8) >= 0 {
            d = d.wrapping_neg();
        }
        let mut vol = hw.bus.r16(v + V_TREM_VOL).wrapping_sub(d);
        if (vol as i16) < 0 {
            vol = 0;
        }
        if vol as i16 >= 0x40 {
            vol = 0x40;
        }
        vol *= 4;
        if vol == 0x100 {
            vol = 0xff;
        }
        hw.bus.w16(v + V_VOL, vol);
        let d0 = self.master(hw, i, vol);
        self.vol_write(hw, i, d0);
        let step = (hw.bus.r8(v + V_TREM) >> 2) & 0x3c;
        hw.bus.w8(v + V_TREM_POS, hw.bus.r8(v + V_TREM_POS).wrapping_add(step));
    }

    /// The instrument's arpeggio list ($E50): the period of the final note plus
    /// the list's next value.
    fn arp_list(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        self.at(hw, Cost::Arp);
        let ins = hw.bus.r32(v + V_INSTR);
        if ins == 0 || hw.bus.r8(ins + 5) == 0 {
            return;
        }
        self.at(hw, Cost::ArpOn);
        let bus = &mut *hw.bus;
        let end = bus.r8(ins + 7);
        let c = bus.r8(v + V_AL_COUNT);
        if end != c {
            bus.w8(v + V_AL_COUNT, c.wrapping_add(1));
            self.at(hw, Cost::ArpCount);
            return;
        }
        bus.w8(v + V_AL_COUNT, end.wrapping_sub(bus.r8(ins + 6)));
        let wrap = bus.r8(ins + 5) == bus.r8(v + V_AL_POS + 1);
        if wrap {
            bus.w8(v + V_AL_POS + 1, 0xff);
        }
        bus.w8(v + V_AL_POS + 1, bus.r8(v + V_AL_POS + 1).wrapping_add(1));
        let o = ((bus.r8(ins + 4) as u16) << 4).wrapping_add(bus.r16(v + V_AL_POS));
        let d = bus.r8(self.t[T_ARP_LISTS].wrapping_add(o as i16 as u32)) as i8 as u16;
        let n = d.wrapping_add(bus.r16(v + V_NOTE_FINAL));
        let per = self.period(hw, n.wrapping_add(n));
        hw.bus.w16(v + V_PER, per);
        self.at(hw, Cost::ArpStep);
        if wrap {
            self.at(hw, Cost::ArpWrap);
        }
    }

    /// The instrument's frequency list ($DF6): the list's next value added to
    /// the period.
    fn freq_list(&mut self, hw: &mut Hw, i: usize) {
        let v = self.voice(i);
        self.at(hw, Cost::Freq);
        let ins = hw.bus.r32(v + V_INSTR);
        if ins == 0 || hw.bus.r8(ins + 9) == 0 {
            return;
        }
        self.at(hw, Cost::FreqOn);
        let bus = &mut *hw.bus;
        let end = bus.r8(ins + 0xb);
        let c = bus.r8(v + V_FL_COUNT);
        if end != c {
            bus.w8(v + V_FL_COUNT, c.wrapping_add(1));
            self.at(hw, Cost::FreqCount);
            return;
        }
        bus.w8(v + V_FL_COUNT, end.wrapping_sub(bus.r8(ins + 0xa)));
        let wrap = bus.r8(ins + 9) == bus.r8(v + V_FL_POS + 1);
        if wrap {
            bus.w8(v + V_FL_POS + 1, 0xff);
        }
        bus.w8(v + V_FL_POS + 1, bus.r8(v + V_FL_POS + 1).wrapping_add(1));
        let o = ((bus.r8(ins + 8) as u16) << 4).wrapping_add(bus.r16(v + V_FL_POS));
        let d = bus.r8(self.t[T_FREQ_LISTS].wrapping_add(o as i16 as u32)) as i8 as u16;
        bus.w16(v + V_PER, bus.r16(v + V_PER).wrapping_add(d));
        self.at(hw, Cost::FreqStep);
        if wrap {
            self.at(hw, Cost::FreqWrap);
        }
    }

    /// The instrument's portamento ($A6A), after its delay. Returns the
    /// instrument, which the tone portamento uses (address register a4).
    fn porta(&mut self, hw: &mut Hw, i: usize) -> u32 {
        let v = self.voice(i);
        self.at(hw, Cost::Porta);
        let ins = hw.bus.r32(v + V_INSTR);
        if ins == 0 {
            return ins;
        }
        let inc = hw.bus.r8(ins + I_PORTA);
        if inc == 0 {
            return ins;
        }
        self.at(hw, Cost::PortaOn);
        let c = hw.bus.r8(v + V_PORTA_COUNT + 1);
        if hw.bus.r8(ins + I_PORTA_DELAY) != c {
            self.at(hw, Cost::PortaCount);
            hw.bus.w8(v + V_PORTA_COUNT + 1, c.wrapping_add(1));
            return ins;
        }
        self.at(hw, Cost::PortaStep);
        hw.bus.w16(v + V_PORTA, hw.bus.r16(v + V_PORTA).wrapping_add(inc as i8 as u16));
        ins
    }

    /// The tone portamento ($AE0): an effect byte below $70 is the note to
    /// slide to, its argument the speed.
    fn tone_porta(&mut self, hw: &mut Hw, i: usize, ins: u32) {
        let v = self.voice(i);
        self.at(hw, Cost::Tone);
        let fx = hw.bus.r16(v + V_FX);
        let speed = hw.bus.r16(v + V_ARG);
        if fx != 0 && (fx as i16) < 0x70 && speed != 0 {
            self.at(hw, Cost::ToneSet);
            let n = (fx as u8)
                .wrapping_add(hw.bus.r8(v + V_NOTE_TRANSPOSE))
                .wrapping_add(hw.bus.r8(ins.wrapping_add(I_TRANSPOSE)));
            let n = fx & 0xff00 | n as u16;
            let end = self.period(hw, n.wrapping_add(n));
            hw.bus.w16(v + V_TP_END, end);
            let diff = end.wrapping_sub(hw.bus.r16(v + V_PER));
            if diff == 0 {
                return;
            }
            hw.bus.w16(v + V_TP_STEP, if (diff as i16) < 0 { speed.wrapping_neg() } else { speed });
        }
        let step = hw.bus.r16(v + V_TP_STEP);
        if step == 0 {
            return;
        }
        self.at(hw, Cost::ToneStep);
        let per = hw.bus.r16(v + V_PER).wrapping_add(step);
        hw.bus.w16(v + V_PER, per);
        let end = hw.bus.r16(v + V_TP_END);
        let done = if (step as i16) < 0 { per as i16 <= end as i16 } else { per as i16 >= end as i16 };
        if done {
            self.at(hw, Cost::ToneEnd);
            hw.bus.w16(v + V_TP_STEP, 0);
            hw.bus.w16(v + V_PER, end);
            hw.bus.w16(v + V_NOTE_PER, end);
        }
    }
}

/// The times a `subq #1; dbra` loop runs for `n` (65536 for 0).
fn dbra_count(n: u16) -> u32 {
    n.wrapping_sub(1) as u32 + 1
}

/// A vibrato or tremolo argument's nibbles replace the old ones where they are
/// not 0.
fn nibbles(old: u8, arg: u8) -> u8 {
    let mut d = old;
    if arg & 0xf != 0 {
        d = d & 0xf0 | arg & 0xf;
    }
    if arg & 0xf0 != 0 {
        d = d & 0xf | arg & 0xf0;
    }
    d
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::Path;

    /// Voice 0 plays note 25 with a looped sample, an instrument with a full
    /// attack, and the set volume effect; the other voices wait.
    fn module() -> Vec<u8> {
        let mut parts: Vec<Vec<u8>> = vec![Vec::new(); 15];
        parts[0] = b"ACTIONAMICS SOUND TOOL\0\0\0\0\0\0".to_vec();
        parts[1] = vec![0; 4]; // the total length, below
        parts[2] = vec![0, 1, 1, 1]; // the voices' tracks at position 0
        parts[3] = vec![0; 4];
        parts[4] = vec![0; 4];
        let mut ins = vec![0u8; 32];
        ins[0x10] = 255; // attack to 255 by 255
        ins[0x11] = 255;
        parts[5] = ins;
        parts[6] = vec![0; 16]; // sample list 0: sample 0
        parts[7] = vec![0; 16];
        parts[8] = vec![0; 16];
        let mut subsongs = vec![0u8; 128];
        subsongs[3] = 2; // position 0 to 0, speed 2
        parts[11] = subsongs;
        let mut sample = vec![0u8; 64];
        sample[4..10].copy_from_slice(&[0, 32, 0, 8, 0, 16]); // 32 words, loop from 8, 16
        parts[13] = sample;
        // note 25, instrument 1, set volume 32, then a wait of 63 rows; the
        // other track waits
        let tracks = [vec![25, 1, 0x7c, 32, !63], vec![!63]];
        let mut offsets = Vec::new();
        let mut o = 0u16;
        for t in &tracks {
            offsets.extend(o.to_be_bytes());
            o += t.len() as u16;
        }
        offsets.extend(o.to_be_bytes());
        parts[14] = offsets;

        let mut d = 125u16.to_be_bytes().to_vec();
        for p in &parts {
            d.extend((p.len() as u32).to_be_bytes());
        }
        for p in &parts {
            d.extend(p);
        }
        for t in &tracks {
            d.extend(t);
        }
        d.extend((0..64).map(|i| i as u8));
        let total = (d.len() as u32).to_be_bytes();
        d[62 + 28..62 + 32].copy_from_slice(&total);
        d
    }

    #[test]
    fn plays_a_note_on_paula() {
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.ast"), module()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "Actionamics");
        let mut out = vec![0f32; 2 * 4800];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // InitSound's DMA off; the first tick has no row, and ends with DMA on;
        // the row's DMA off for voice 0's note, DMA on at the end of each tick
        assert_eq!(&reg(0xDFF096)[..4], &[0x000f, 0x800f, 0x0001, 0x800f]);
        // DTP_Volume's and DTP_Balance's, the first tick's; on the row the
        // attack's (255 of the master volume: 64), then the effect's (128: 32)
        assert_eq!(&reg(0xDFF0A8)[..5], &[0, 0, 0, 64, 32]);
        // the lowest period at first (each voice writes it twice a tick), then
        // the note's
        assert_eq!(&reg(0xDFF0A6)[..4], &[0x5f, 0x5f, 1440, 1440]);
        // an empty sample at first, then the note's sample, then its loop
        assert_eq!(&reg(0xDFF0A4)[..3], &[0, 32, 16]);
        let adr: Vec<u32> =
            reg(0xDFF0A0).iter().zip(reg(0xDFF0A2)).map(|(&h, l)| (h as u32) << 16 | l as u32).collect();
        assert_eq!(adr[0], 0);
        assert_eq!(adr[2] - adr[1], 16);
        assert_eq!(e.m.bus.r8(adr[1] + 63), 63);
    }
}

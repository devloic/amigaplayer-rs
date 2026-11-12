//! The DMF loader: a port of libopenmpt 0.8.7's `soundlib/Load_dmf.cpp` (Johannes Schultz,
//! BSD-3-Clause; see `../mod.rs`), with the pieces of OpenMPT it calls: the effect writer
//! (`CPattern::WriteEffect`, `pattern.cpp`), the column combiner (`ModCommand::FillInTwoCommands`,
//! `ConvertToVolCommand`, `GetEffectWeight`, `modcommand.cpp`), the order reader
//! (`ReadOrderFromFile`, `Loaders.h`), the sample reader for 8/16-bit PCM and the DMF Huffman
//! compression (`SampleIO.cpp`, `DMFUnpack`) and `ModSample::SanitizeLoops`.
//!
//! Every order entry gets a pattern of its own, converted with the tempo and effect state the
//! previous entries left, as OpenMPT does. Beyond OpenMPT's own limits, two bounds keep a
//! hostile file cheap (they never bind on a real module): the converted patterns hold at most
//! `MAX_CELLS` cells (an entry past that is an end-of-song entry, as when OpenMPT runs out of
//! patterns), and the effect-memory search of the conversion has a step budget.

/// No note (`NOTE_NONE`).
pub const NOTE_NONE: u8 = 0;
/// `NOTE_MAX`: notes are 1..=128.
pub const NOTE_MAX: u8 = 128;
pub const NOTE_KEYOFF: u8 = 0xFF;
pub const NOTE_NOTECUT: u8 = 0xFE;
/// `NOTE_PCS`: the lowest special note (`NOTE_MIN_SPECIAL`).
pub const NOTE_MIN_SPECIAL: u8 = 0xFB;
/// An order entry that ends the song (`PATTERNINDEX_INVALID`).
pub const ORDER_END: u16 = 0xFFFF;

/// The most cells the converted patterns may hold together.
const MAX_CELLS: usize = 8 << 20;
/// The steps the effect-memory search may take over the whole load.
const MEMORY_STEPS: u64 = 64 << 20;
/// `MAX_PATTERNS`
const MAX_PATTERNS: usize = 4000;
/// `MAX_PATTERN_ROWS`
const MAX_PATTERN_ROWS: usize = 4096;
/// `MAX_SAMPLE_LENGTH`
const MAX_SAMPLE_LENGTH: u32 = 0x1000_0000;
/// The most sample frames all samples may hold together (zero-padded past the data they have).
const MAX_SAMPLE_FRAMES: u64 = 64 << 20;

/// The effect commands the loader writes (OpenMPT's `EffectCommand`, a subset).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Cmd {
    #[default]
    None,
    Arpeggio,
    PortaUp,
    PortaDown,
    TonePorta,
    Vibrato,
    Tremolo,
    Panning8,
    Offset,
    VolumeSlide,
    PositionJump,
    Volume,
    Speed,
    Tempo,
    Tremor,
    Retrig,
    S3mCmdEx,
    PanningSlide,
    Panbrello,
    Finetune,
}

impl Cmd {
    /// `GetEffectWeight`: the place in OpenMPT's table (only the order matters).
    fn weight(self) -> u8 {
        match self {
            Cmd::None => 0,
            Cmd::Tremolo => 6,
            Cmd::Vibrato => 8,
            Cmd::Finetune => 10,
            Cmd::Panbrello => 12,
            Cmd::S3mCmdEx => 13,
            Cmd::PanningSlide => 18,
            Cmd::Panning8 => 19,
            Cmd::PortaUp => 24,
            Cmd::PortaDown => 28,
            Cmd::VolumeSlide => 31,
            Cmd::Volume => 34,
            Cmd::Offset => 39,
            Cmd::Tremor => 40,
            Cmd::Retrig => 41,
            Cmd::Arpeggio => 43,
            Cmd::TonePorta => 45,
            Cmd::Tempo => 54,
            Cmd::Speed => 55,
            Cmd::PositionJump => 56,
        }
    }
}

/// The volume column commands (`VolumeCommand`), in OpenMPT's order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum VolCmd {
    #[default]
    None,
    Volume,
    Panning,
    VolSlideUp,
    VolSlideDown,
    FineVolUp,
    FineVolDown,
    VibratoSpeed,
    VibratoDepth,
    PanSlideLeft,
    PanSlideRight,
    TonePorta,
    PortaUp,
    PortaDown,
    PlayControl,
    Offset,
}

/// One cell (`ModCommand`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cell {
    pub note: u8,
    pub instr: u8,
    pub volcmd: VolCmd,
    pub vol: u8,
    pub cmd: Cmd,
    pub param: u8,
}

impl Cell {
    pub fn is_note(&self) -> bool {
        (1..=NOTE_MAX).contains(&self.note)
    }
    pub fn is_tone_porta(&self) -> bool {
        self.cmd == Cmd::TonePorta || self.volcmd == VolCmd::TonePorta
    }
}

pub struct Pattern {
    pub rows: usize,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug)]
pub enum Data {
    Empty,
    I8(Vec<i8>),
    I16(Vec<i16>),
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub data: Data,
    /// `nLength`
    pub len: u32,
    pub sustain: bool,
    pub sus_start: u32,
    pub sus_end: u32,
    pub c5: u32,
    /// default volume 0..=256
    pub volume: i32,
    /// `SMP_NODEFAULTVOLUME`
    pub no_default_volume: bool,
}

impl Sample {
    fn empty() -> Sample {
        Sample { data: Data::Empty, len: 0, sustain: false, sus_start: 0, sus_end: 0, c5: 8363, volume: 256, no_default_volume: false }
    }
    pub fn has_data(&self) -> bool {
        !matches!(self.data, Data::Empty) && self.len > 0
    }
}

pub struct Module {
    pub version: u8,
    pub tracker: &'static str,
    pub title: String,
    pub artist: String,
    /// the tracks + 1 (channel 0 carries the global track's tempo commands)
    pub channels: usize,
    pub patterns: Vec<Pattern>,
    /// pattern indices; `ORDER_END` ends the song, other values past `patterns` are skipped
    pub order: Vec<u16>,
    /// sample 0 is the empty one
    pub samples: Vec<Sample>,
}

/// A read cursor with OpenMPT `FileReader` semantics: reads past the end give 0 and do not move.
#[derive(Clone, Copy)]
struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn new(d: &'a [u8]) -> Rd<'a> {
        Rd { d, p: 0 }
    }
    fn left(&self) -> usize {
        self.d.len() - self.p
    }
    fn can(&self, n: usize) -> bool {
        self.left() >= n
    }
    fn u8(&mut self) -> u8 {
        if self.can(1) {
            self.p += 1;
            self.d[self.p - 1]
        } else {
            0
        }
    }
    fn u16(&mut self) -> u16 {
        if self.can(2) {
            self.p += 2;
            u16::from_le_bytes([self.d[self.p - 2], self.d[self.p - 1]])
        } else {
            0
        }
    }
    fn u32(&mut self) -> u32 {
        if self.can(4) {
            self.p += 4;
            u32::from_le_bytes(self.d[self.p - 4..self.p].try_into().unwrap())
        } else {
            0
        }
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.can(n) {
            self.p += n;
            Some(&self.d[self.p - n..self.p])
        } else {
            None
        }
    }
    fn skip(&mut self, n: usize) {
        self.p += n.min(self.left());
    }
    fn skip_back(&mut self, n: usize) {
        self.p -= n.min(self.p);
    }
    /// `ReadChunk`: the next `n` bytes (fewer at the end), as a reader of their own.
    fn chunk(&mut self, n: usize) -> Rd<'a> {
        let n = n.min(self.left());
        self.p += n;
        Rd::new(&self.d[self.p - n..self.p])
    }
}

/// `ValidateHeader`: "DDMF" and version 1..=10.
pub fn test(data: &[u8]) -> bool {
    data.len() >= 66 && &data[..4] == b"DDMF" && (1..=10).contains(&data[4])
}

fn text(b: &[u8]) -> String {
    // spacePadded: up to the first NUL, trailing spaces trimmed; CP437 shown as Latin-1
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect::<String>().trim_end().to_string()
}

/// `ModCommand::ConvertToVolCommand`
fn to_volcmd(cmd: Cmd, param: u8, force: bool) -> (VolCmd, u8) {
    const PORTA_VOLCMD: [u8; 10] = [0x00, 0x01, 0x04, 0x08, 0x10, 0x20, 0x40, 0x60, 0x80, 0xFF];
    match cmd {
        Cmd::Volume => return (VolCmd::Volume, param.min(64)),
        Cmd::PortaUp | Cmd::PortaDown => {
            if force || (param & 3 == 0 && param < 0xE0) {
                return (if cmd == Cmd::PortaUp { VolCmd::PortaUp } else { VolCmd::PortaDown }, param / 4);
            }
        }
        Cmd::TonePorta => {
            if param >= 0xF0 {
                return (VolCmd::TonePorta, 9);
            }
            for (n, &v) in PORTA_VOLCMD.iter().enumerate() {
                if if force { param <= v } else { param == v } {
                    return (VolCmd::TonePorta, n as u8);
                }
            }
        }
        Cmd::Vibrato => {
            if force {
                return (VolCmd::VibratoDepth, (param & 0x0F).min(9));
            } else if !((param & 0x0F) > 9 || (param & 0xF0) != 0) {
                return (VolCmd::VibratoDepth, param & 0x0F);
            }
        }
        Cmd::Panning8 => return (VolCmd::Panning, if param == 255 { 64 } else { param / 4 }),
        Cmd::VolumeSlide => {
            if param != 0 {
                if param & 0x0F == 0 {
                    return (VolCmd::VolSlideUp, param >> 4);
                } else if param & 0xF0 == 0 {
                    return (VolCmd::VolSlideDown, param);
                } else if param & 0x0F == 0x0F {
                    return (VolCmd::FineVolUp, param >> 4);
                } else if param & 0xF0 == 0xF0 {
                    return (VolCmd::FineVolDown, param & 0x0F);
                }
            }
        }
        Cmd::S3mCmdEx => match param & 0xF0 {
            0x80 => return (VolCmd::Panning, ((param & 0x0F) << 2) + 2),
            0x90 if param >= 0x9E && force => return (VolCmd::PlayControl, param - 0x9E + 2),
            _ => {}
        },
        _ => {}
    }
    (VolCmd::None, 0)
}

/// `ModCommand::FillInTwoCommands` on a fresh cell: (volume column, effect column).
fn fill_two(mut e1: Cmd, mut p1: u8, mut e2: Cmd, mut p2: u8) -> ((VolCmd, u8), (Cmd, u8)) {
    if e1 == e2
        && matches!(e1, Cmd::Arpeggio | Cmd::Panning8 | Cmd::Offset | Cmd::PositionJump | Cmd::Volume | Cmd::Speed | Cmd::Tempo | Cmd::Finetune)
    {
        e2 = Cmd::None;
    }
    for n in 0..4 {
        let v = to_volcmd(e1, p1, n > 1);
        if e1 == Cmd::None || v.0 != VolCmd::None {
            return (v, (e2, p2));
        }
        std::mem::swap(&mut e1, &mut e2);
        std::mem::swap(&mut p1, &mut p2);
    }
    if e1.weight() > e2.weight() {
        std::mem::swap(&mut e1, &mut e2);
        std::mem::swap(&mut p1, &mut p2);
    }
    if e2 == Cmd::Offset && p2 == 0 {
        return ((VolCmd::Offset, 0), (e1, p1));
    }
    ((VolCmd::None, 0), (e2, p2))
}

#[derive(Clone, Copy, PartialEq)]
enum Retry {
    Ignore,
    NextRow,
    PreviousRow,
}

/// `EffectWriter`
struct Writer {
    row: usize,
    /// None: any channel
    channel: Option<usize>,
    retry_mode: Retry,
    is_vol: bool,
    cmd: Cmd,
    param: u8,
    volcmd: VolCmd,
    vol: u8,
    retry: bool,
    allow_multiple: bool,
}

impl Writer {
    fn new(cmd: Cmd, param: u8, row: usize) -> Writer {
        Writer { row, channel: None, retry_mode: Retry::Ignore, is_vol: false, cmd, param, volcmd: VolCmd::None, vol: 0, retry: true, allow_multiple: false }
    }
}

/// `CPattern::WriteEffect` (its recursion unrolled into a loop)
fn write_effect(p: &mut Pattern, ch: usize, w: &mut Writer) -> bool {
    loop {
        if p.cells.is_empty() || w.row >= p.rows || w.channel.is_some_and(|c| c >= ch) {
            return false;
        }
        let (lo, hi) = match w.channel {
            Some(c) => (c, c),
            None => (0, ch - 1),
        };
        let base = w.row * ch;
        if !w.allow_multiple {
            for m in &p.cells[base + lo..=base + hi] {
                if !w.is_vol && m.cmd == w.cmd {
                    return true;
                }
                if w.is_vol && m.volcmd == w.volcmd {
                    return true;
                }
            }
        }
        for m in &mut p.cells[base + lo..=base + hi] {
            if !w.is_vol && m.cmd == Cmd::None {
                m.cmd = w.cmd;
                m.param = w.param;
                return true;
            }
            if w.is_vol && m.volcmd == VolCmd::None {
                m.volcmd = w.volcmd;
                m.vol = w.vol;
                return true;
            }
        }
        if w.retry {
            if !w.is_vol {
                for m in &mut p.cells[base + lo..=base + hi] {
                    match m.cmd {
                        Cmd::Volume => {
                            m.volcmd = VolCmd::Volume;
                            m.vol = m.param;
                            m.cmd = w.cmd;
                            m.param = w.param;
                            return true;
                        }
                        Cmd::Panning8 => {
                            m.volcmd = VolCmd::Panning;
                            m.cmd = w.cmd;
                            m.vol = ((m.param as u32 + 2) / 4) as u8;
                            m.param = w.param;
                            return true;
                        }
                        _ => {}
                    }
                }
            }
            if w.is_vol {
                let conv = match w.volcmd {
                    VolCmd::Panning => Some((Cmd::Panning8, (w.vol as u32 * 4).min(255) as u8)),
                    VolCmd::Volume => Some((Cmd::Volume, w.vol)),
                    _ => None,
                };
                if let Some((c, v)) = conv {
                    w.cmd = c;
                    w.param = v;
                    w.retry = false;
                }
            } else {
                let (vc, v) = to_volcmd(w.cmd, w.param, true);
                if vc != VolCmd::None {
                    w.volcmd = vc;
                    w.vol = v;
                    w.retry = false;
                }
            }
            if !w.retry {
                w.is_vol = !w.is_vol;
                continue;
            }
        }
        if w.retry_mode == Retry::NextRow && w.row + 1 < p.rows {
            w.row += 1;
            w.retry = true;
        } else if w.retry_mode == Retry::PreviousRow && w.row > 0 {
            w.row -= 1;
            w.retry = true;
        } else {
            return false;
        }
    }
}

/// `DMFPatternSettings::ChannelState`
#[derive(Clone, Copy)]
struct ChanState {
    note_buffer: u8,
    last_note: u8,
    vibrato_type: u8,
    tremolo_type: u8,
    high_offset: u8,
    play_dir: bool,
}

/// `DMFPatternSettings`
struct Settings {
    chn: Vec<ChanState>,
    real_bpm: bool,
    beat: u8,
    tempo_ticks: u8,
    tempo_bpm: u8,
    internal_ticks: u8,
    /// the effect-memory search steps left
    steps: u64,
}

fn porta2mpt(val: u8, ticks: u8, has_fine: bool) -> u8 {
    if val == 0 {
        0
    } else if (val <= 0x0F && has_fine) || ticks < 2 {
        val | 0xF0
    } else {
        ((val as u32 / (ticks as u32 - 1)) as u8).max(1)
    }
}

fn slide2mpt(val: u8, ticks: u8, up: bool) -> u8 {
    let mut val = (val / 4).max(1);
    let fine = val < 0x0F || ticks < 2;
    if !fine {
        val = (((val as u32 + ticks as u32 - 2) / (ticks as u32 - 1)) as u8).max(1);
    }
    if up {
        (if fine { 0x0F } else { 0x00 }) | ((val as u32) << 4) as u8
    } else {
        (if fine { 0xF0 } else { 0x00 }) | (val & 0x0F)
    }
}

fn tremor2mpt(val: u8, ticks: u8) -> u8 {
    let on = ((val >> 4) as u32 * ticks as u32 / 15).clamp(1, 15) as u8;
    let off = ((val & 0x0F) as u32 * ticks as u32 / 15).clamp(1, 15) as u8;
    (on << 4) | off
}

fn delay2mpt(val: u8, ticks: u8) -> u8 {
    (val as u32 * ticks as u32 / 255).min(15) as u8
}

fn vibrato2mpt(val: u8, ticks: u8) -> u8 {
    let period = ((val >> 4) as u32).max(1) * ticks as u32;
    let m = (128 / period).clamp(1, 15) as u8;
    (m << 4) | (val & 0x0F).max(1)
}

/// `ApplyEffectMemory`: a parameter equal to the last one of the same effect on this channel
/// becomes 0 (continue).
fn apply_memory(cells: &[Cell], at: usize, row: usize, ch: usize, effect: Cmd, param: &mut u8, steps: &mut u64) {
    if effect == Cmd::None || *param == 0 {
        return;
    }
    let tone = matches!(effect, Cmd::PortaUp | Cmd::PortaDown | Cmd::TonePorta);
    let vslide = effect == Cmd::VolumeSlide;
    let mut i = at;
    let mut row = row;
    while row > 0 {
        if *steps == 0 {
            return;
        }
        *steps -= 1;
        i -= ch;
        row -= 1;
        let m = &cells[i];
        let mut same = effect == m.cmd;
        if tone && matches!(m.cmd, Cmd::PortaUp | Cmd::PortaDown | Cmd::TonePorta) {
            if m.param < 0xE0 {
                same = true;
            } else {
                return;
            }
        } else if vslide && m.cmd == Cmd::VolumeSlide {
            same = true;
        }
        if tone && matches!(m.volcmd, VolCmd::PortaUp | VolCmd::PortaDown | VolCmd::TonePorta) && m.vol != 0 {
            return;
        } else if vslide && matches!(m.volcmd, VolCmd::FineVolUp | VolCmd::FineVolDown | VolCmd::VolSlideUp | VolCmd::VolSlideDown) && m.vol != 0 {
            return;
        }
        if same {
            if *param != m.param && m.param != 0 {
                return;
            } else if *param == m.param {
                *param = 0;
                return;
            }
        }
    }
}

/// `ConvertDMFPattern`: one pattern of `chunk` (header and data) as the order entry plays it.
fn convert_pattern(chunk: Rd, version: u8, s: &mut Settings, ch: usize) -> Pattern {
    let mut f = chunk;
    f.p = 0;
    let (tracks, mut beat, rows) = if version < 3 {
        let t = f.u8();
        f.skip(2);
        let r = f.u16();
        f.u32();
        (t, 0u8, r)
    } else if f.can(8) {
        let t = f.u8();
        let b = f.u8();
        let r = f.u16();
        f.u32();
        (t, b, r)
    } else {
        f.p = f.d.len();
        (0, 0, 0)
    };
    if version < 6 {
        beat = 0;
    }
    let rows = (rows as usize).clamp(1, MAX_PATTERN_ROWS);
    let mut p = Pattern { rows, cells: vec![Cell::default(); rows * ch] };
    let nch = (ch - 1).min(tracks as usize);
    for c in nch + 1..ch {
        p.cells[c].note = NOTE_NOTECUT;
    }
    s.beat = beat >> 4;
    let mut tempo_change = s.real_bpm;
    let mut counter = vec![0u8; nch + 1];
    for row in 0..rows {
        if !f.can(1) {
            break;
        }
        let mut write_delay = 0u8;
        if counter[0] == 0 {
            let mut info = f.u8();
            if info & 0x80 != 0 {
                counter[0] = f.u8();
            }
            info &= 0x3F;
            let data = if info != 0 { f.u8() } else { 0 };
            match info {
                1 => {
                    s.real_bpm = false;
                    s.tempo_ticks = data.max(1);
                    s.tempo_bpm = 0;
                    tempo_change = true;
                }
                2 => {
                    if data != 0 {
                        s.real_bpm = true;
                        s.tempo_bpm = data;
                        if s.beat != 0 {
                            s.tempo_ticks = (data as u32 * s.beat as u32 * 15) as u8;
                        }
                        tempo_change = true;
                    }
                }
                3 => {
                    s.beat = data >> 4;
                    if s.beat != 0 {
                        tempo_change = s.real_bpm;
                    } else {
                        s.real_bpm = false;
                    }
                }
                4 => write_delay = data,
                6 | 7 => {
                    if data > 0 {
                        let t = if s.real_bpm { &mut s.tempo_bpm } else { &mut s.tempo_ticks };
                        if info == 6 {
                            *t = if (*t as u32) < 256 - data as u32 { *t + data } else { 255 };
                        } else {
                            *t = if (*t as u32) > 1 + data as u32 { *t - data } else { 1 };
                        }
                        tempo_change = true;
                    }
                }
                _ => {}
            }
        } else {
            counter[0] -= 1;
        }

        let (mut speed, mut tempo) = (0i32, 0i32);
        if tempo_change {
            if !s.real_bpm || s.beat != 0 {
                let tickspeed = if s.real_bpm { (s.tempo_bpm as i32 * s.beat as i32 * 2).max(1) } else { (s.tempo_ticks as i32 + 1) * 30 };
                speed = 255;
                while speed >= 1 {
                    tempo = tickspeed * speed / 48;
                    if (32..=255).contains(&tempo) {
                        break;
                    }
                    speed -= 1;
                }
                tempo = tempo.clamp(32, 255);
                s.internal_ticks = speed.max(1) as u8;
            } else {
                tempo_change = false;
            }
        }

        for chn in 1..=nch {
            let at = row * ch + chn;
            if counter[chn] != 0 {
                counter[chn] -= 1;
                continue;
            }
            let info = f.u8();
            if info & 0x80 != 0 {
                counter[chn] = f.u8();
            }
            let st = &mut s.chn[chn];
            let mut m = p.cells[at];
            let mut slide_note = true;
            if info & 0x40 != 0 {
                m.instr = f.u8();
                if m.instr != 0 {
                    slide_note = false;
                }
            }
            if info & 0x20 != 0 {
                m.note = f.u8();
                if (1..=108).contains(&m.note) {
                    m.note = (m.note as u32 + 24).min(NOTE_MAX as u32) as u8;
                    st.last_note = m.note;
                } else if (129..=236).contains(&m.note) {
                    st.note_buffer = ((m.note & 0x7F) as u32 + 24).min(NOTE_MAX as u32) as u8;
                    m.note = NOTE_NONE;
                } else if m.note == 255 {
                    m.note = NOTE_NOTECUT;
                }
            }
            if m.note == NOTE_NONE && m.instr > 0 {
                m.note = st.last_note;
                m.instr = 0;
            }
            if m.is_note() {
                st.play_dir = false;
            }
            let (mut e1, mut e2, mut e3) = (Cmd::None, Cmd::None, Cmd::None);
            let (mut p1, mut p2, mut p3) = (0u8, 0u8, 0u8);
            let (mut mem2, mut mem3) = (false, false);
            if info & 0x10 != 0 {
                m.volcmd = VolCmd::Volume;
                m.vol = ((f.u8() as u32 + 2) / 4) as u8;
            }
            let ticks = s.internal_ticks;
            if info & 0x08 != 0 {
                let command = f.u8();
                p1 = f.u8();
                match command {
                    1 => m.note = NOTE_NOTECUT,
                    2 => m.note = NOTE_KEYOFF,
                    3 => {
                        m.note = st.last_note;
                        st.play_dir = false;
                    }
                    4 => {
                        p1 = delay2mpt(p1, ticks);
                        if p1 != 0 {
                            e1 = Cmd::S3mCmdEx;
                            p1 |= 0xD0;
                        }
                        if m.note == NOTE_NONE {
                            m.note = st.last_note;
                            st.play_dir = false;
                        }
                    }
                    5 => {
                        p1 = delay2mpt(p1, ticks).max(1);
                        e1 = Cmd::Retrig;
                        st.play_dir = false;
                    }
                    6..=9 => {
                        if row > 0 && command != st.high_offset {
                            p.cells[at] = m;
                            let mut w = Writer::new(Cmd::S3mCmdEx, 0xA0 | (command - 6), row - 1);
                            w.channel = Some(chn);
                            w.retry_mode = Retry::PreviousRow;
                            if write_effect(&mut p, ch, &mut w) {
                                st.high_offset = command;
                            }
                            m = p.cells[at];
                        }
                        e1 = Cmd::Offset;
                        if m.note == NOTE_NONE {
                            m.note = st.last_note;
                        }
                        st.play_dir = false;
                    }
                    10 => {
                        e1 = Cmd::S3mCmdEx;
                        p1 = if !st.play_dir { 0x9F } else { 0x9E };
                        st.play_dir = !st.play_dir;
                    }
                    _ => {}
                }
            }
            if info & 0x04 != 0 {
                let command = f.u8();
                p2 = f.u8();
                match command {
                    1 => {
                        let x = p2 as i8 as i32 * 8;
                        let (quot, rem) = (x / 128, x % 128);
                        if m.is_note() {
                            m.note = (m.note as i32 + quot).clamp(1, NOTE_MAX as i32) as u8;
                        }
                        e2 = Cmd::Finetune;
                        p2 = (rem as u8) ^ 0x80;
                    }
                    2 => {
                        p2 = delay2mpt(p2, ticks);
                        if p2 != 0 {
                            e2 = Cmd::S3mCmdEx;
                            p2 |= 0xD0;
                        }
                        mem2 = true;
                    }
                    3 => {
                        e2 = Cmd::Arpeggio;
                        mem2 = true;
                    }
                    4 | 5 => {
                        p2 = porta2mpt(p2, ticks, true);
                        e2 = if command == 4 { Cmd::PortaUp } else { Cmd::PortaDown };
                        mem2 = true;
                    }
                    6 => {
                        if m.note == NOTE_NONE {
                            m.note = st.note_buffer;
                        }
                        p2 = porta2mpt(p2, ticks, false);
                        e2 = Cmd::TonePorta;
                        mem2 = true;
                    }
                    7 => {
                        m.note = (p2 as i32 + 25).clamp(1, NOTE_MAX as i32) as u8;
                        e2 = Cmd::TonePorta;
                        p2 = 0xFF;
                        mem2 = true;
                    }
                    8..=10 => {
                        if row > 0 && command != st.vibrato_type {
                            p.cells[at] = m;
                            let mut w = Writer::new(Cmd::S3mCmdEx, 0x30 | (command - 8), row - 1);
                            w.channel = Some(chn);
                            w.retry_mode = Retry::PreviousRow;
                            if write_effect(&mut p, ch, &mut w) {
                                st.vibrato_type = command;
                            }
                            m = p.cells[at];
                        }
                        e2 = Cmd::Vibrato;
                        p2 = vibrato2mpt(p2, ticks);
                        mem2 = true;
                    }
                    11 => {
                        p2 = tremor2mpt(p2, ticks);
                        e2 = Cmd::Tremor;
                        mem2 = true;
                    }
                    12 => {
                        p2 = delay2mpt(p2, ticks);
                        if p2 != 0 {
                            e2 = Cmd::S3mCmdEx;
                            p2 |= 0xC0;
                        } else {
                            m.note = NOTE_NOTECUT;
                        }
                        mem2 = true;
                    }
                    _ => {}
                }
            }
            if info & 0x02 != 0 {
                let command = f.u8();
                p3 = f.u8();
                match command {
                    1 | 2 => {
                        p3 = slide2mpt(p3, ticks, command == 1);
                        e3 = Cmd::VolumeSlide;
                        mem3 = true;
                    }
                    3 => {
                        p3 = tremor2mpt(p3, ticks);
                        e3 = Cmd::Tremor;
                        mem3 = true;
                    }
                    4..=6 => {
                        if row > 0 && command != st.tremolo_type {
                            p.cells[at] = m;
                            let mut w = Writer::new(Cmd::S3mCmdEx, 0x40 | (command - 4), row - 1);
                            w.channel = Some(chn);
                            w.retry_mode = Retry::PreviousRow;
                            if write_effect(&mut p, ch, &mut w) {
                                st.tremolo_type = command;
                            }
                            m = p.cells[at];
                        }
                        e3 = Cmd::Tremolo;
                        p3 = vibrato2mpt(p3, ticks);
                        mem3 = true;
                    }
                    7 => e3 = Cmd::Panning8,
                    8 | 9 => {
                        p3 = slide2mpt(p3, ticks, command == 8);
                        e3 = Cmd::PanningSlide;
                        mem3 = true;
                    }
                    10 => {
                        e3 = Cmd::Panbrello;
                        p3 = vibrato2mpt(p3, ticks);
                        mem3 = true;
                    }
                    _ => {}
                }
            }
            if mem2 {
                apply_memory(&p.cells, at, row, ch, e2, &mut p2, &mut s.steps);
            }
            if mem3 {
                apply_memory(&p.cells, at, row, ch, e3, &mut p3, &mut s.steps);
            }
            if slide_note && m.is_note() {
                if e2 == Cmd::None {
                    e2 = Cmd::TonePorta;
                    p2 = 0xFF;
                } else if e3 == Cmd::None && e2 != Cmd::TonePorta {
                    e3 = Cmd::TonePorta;
                    p3 = 0xFF;
                }
            }
            if m.volcmd == VolCmd::Volume {
                if e2 == Cmd::None {
                    e2 = Cmd::Volume;
                    p2 = m.vol;
                    m.volcmd = VolCmd::None;
                } else if e3 == Cmd::None {
                    e3 = Cmd::Volume;
                    p3 = m.vol;
                    m.volcmd = VolCmd::None;
                }
            }
            let (v, c) = fill_two(e2, p2, e3, p3);
            if m.volcmd == VolCmd::None && v.0 != VolCmd::None {
                (m.volcmd, m.vol) = v;
            }
            if e1 != Cmd::None {
                let (v, c) = fill_two(c.0, c.1, e1, p1);
                if m.volcmd == VolCmd::None && v.0 != VolCmd::None {
                    (m.volcmd, m.vol) = v;
                }
                (m.cmd, m.param) = c;
            } else if c.0 != Cmd::None {
                (m.cmd, m.param) = c;
            }
            p.cells[at] = m;
        }

        if tempo_change {
            tempo_change = false;
            let mut w = Writer::new(Cmd::Tempo, tempo as u8, row);
            w.channel = Some(0);
            w.retry_mode = Retry::NextRow;
            write_effect(&mut p, ch, &mut w);
            let mut w = Writer::new(Cmd::Speed, speed as u8, row);
            w.retry_mode = Retry::NextRow;
            write_effect(&mut p, ch, &mut w);
        }
        if write_delay & 0xF0 != 0 {
            let mut w = Writer::new(Cmd::S3mCmdEx, 0xE0 | (write_delay >> 4), row);
            w.allow_multiple = true;
            write_effect(&mut p, ch, &mut w);
        }
        if write_delay & 0x0F != 0 {
            let param = ((write_delay & 0x0F) as u32 * s.internal_ticks as u32 / 15) as u8;
            let mut w = Writer::new(Cmd::S3mCmdEx, 0x60 | param.clamp(1, 15), row);
            w.allow_multiple = true;
            write_effect(&mut p, ch, &mut w);
        }
    }
    p
}

/// DMF Huffman compression (`DMFUnpack`): fills `out`, stops at the end of the data.
fn dmf_unpack(src: &[u8], out: &mut [u8]) {
    struct Bits<'a> {
        d: &'a [u8],
        p: usize,
        buf: u32,
        n: u32,
    }
    impl Bits<'_> {
        fn read(&mut self, k: u32) -> Option<u32> {
            while self.n < k {
                let b = *self.d.get(self.p)?;
                self.p += 1;
                self.buf |= (b as u32) << self.n;
                self.n += 8;
            }
            let v = self.buf & ((1 << k) - 1);
            self.buf >>= k;
            self.n -= k;
            Some(v)
        }
    }
    #[derive(Clone, Copy, Default)]
    struct Node {
        left: i16,
        right: i16,
        value: u8,
    }
    struct Tree<'a> {
        bits: Bits<'a>,
        last: usize,
        count: usize,
        nodes: [Node; 256],
    }
    // `DMFNewNode`, with its recursion on an explicit stack: each frame is the node whose
    // children are still to be read, and which of them comes next
    fn build(t: &mut Tree) -> Option<()> {
        enum Step {
            Enter,
            Right(usize, bool),
        }
        let mut stack: Vec<Step> = vec![Step::Enter];
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter => {
                    let act = t.count;
                    if act > 255 {
                        continue;
                    }
                    t.nodes[act].value = t.bits.read(7)? as u8;
                    let is_left = t.bits.read(1)? != 0;
                    let is_right = t.bits.read(1)? != 0;
                    let act = t.last;
                    if act > 255 {
                        continue;
                    }
                    t.count += 1;
                    t.last = t.count;
                    stack.push(Step::Right(act, is_right));
                    if is_left {
                        t.nodes[act].left = t.last as i16;
                        stack.push(Step::Enter);
                    } else {
                        t.nodes[act].left = -1;
                    }
                }
                Step::Right(act, is_right) => {
                    t.last = t.count;
                    if is_right {
                        t.nodes[act].right = t.last as i16;
                        stack.push(Step::Enter);
                    } else {
                        t.nodes[act].right = -1;
                    }
                }
            }
        }
        Some(())
    }
    let mut t = Tree { bits: Bits { d: src, p: 0, buf: 0, n: 0 }, last: 0, count: 0, nodes: [Node::default(); 256] };
    if build(&mut t).is_none() {
        return;
    }
    if t.nodes[0].left < 0 || t.nodes[0].right < 0 {
        return;
    }
    let (mut value, mut delta) = (0u8, 0u8);
    for o in out.iter_mut() {
        let mut act = 0usize;
        let Some(sign) = t.bits.read(1) else { return };
        loop {
            let Some(b) = t.bits.read(1) else { return };
            let next = if b != 0 { t.nodes[act].right } else { t.nodes[act].left };
            // a -1 child: OpenMPT reads nodes[-1], which is outside the table; here it is
            // treated as the end of the tree walk (the value stays)
            if next < 0 {
                break;
            }
            act = next as usize;
            if act > 255 {
                break;
            }
            delta = t.nodes[act].value;
            if !(t.nodes[act].left >= 0 && t.nodes[act].right >= 0) {
                break;
            }
        }
        if sign != 0 {
            delta ^= 0xFF;
        }
        value = value.wrapping_add(delta);
        *o = value;
    }
}

/// Loads a DMF module; Err for what OpenMPT's loader refuses.
pub fn load(data: &[u8]) -> Result<Module, ()> {
    if !test(data) {
        return Err(());
    }
    let version = data[4];
    let mut f = Rd::new(data);
    f.p = 66;
    let mut chunks: Vec<([u8; 4], Rd)> = Vec::new();
    while f.can(8) {
        let id: [u8; 4] = f.bytes(4).unwrap().try_into().unwrap();
        let mut len = f.u32() as usize;
        let mut skip = 0;
        if version == 3 && &id == b"SEQU" {
            skip = 2;
        } else if version == 4 && &id == b"SEQU" {
            skip = 4;
        } else if version < 8 && &id == b"SMPD" {
            len = f.left();
        }
        let c = f.chunk(len);
        chunks.push((id, c));
        f.skip(skip);
    }
    let get = |id: &[u8; 4]| chunks.iter().find(|(i, _)| i == id).map(|(_, c)| *c);

    let mut patt = get(b"PATT").ok_or(())?;
    let num_patterns = patt.u16() as usize;
    let num_tracks = patt.u8();
    let head = if version < 3 { 9 } else { 8 };
    let mut pat_chunks = Vec::with_capacity(num_patterns.min(patt.left() / 8 + 1));
    for _ in 0..num_patterns {
        patt.skip(head - 4);
        let len = patt.u32() as usize;
        if !patt.can(len) {
            return Err(());
        }
        patt.skip_back(head);
        pat_chunks.push(patt.chunk(head + len));
    }
    let channels = num_tracks.clamp(1, 32) as usize + 1;
    let title = text(&data[13..43]);
    let artist = text(&data[43..63]);

    let mut seq = get(b"SEQU").unwrap_or(Rd::new(&[]));
    let mut loop_start = 0usize;
    let mut loop_end = usize::MAX;
    if version >= 3 {
        loop_start = seq.u16() as usize;
    }
    if version >= 4 {
        loop_end = seq.u16() as usize;
    }
    if version == 4 && loop_end == 0 {
        loop_end = usize::MAX;
    }
    let n = (seq.left() / 2).min(65536);
    let mut order: Vec<u16> = (0..n).map(|_| seq.u16()).collect();
    let last = order.len().wrapping_sub(1) & 0xFFFF;
    loop_start = loop_start.min(last);
    loop_end = loop_end.min(last);

    let mut s = Settings {
        chn: vec![ChanState { note_buffer: NOTE_NONE, last_note: NOTE_NONE, vibrato_type: 8, tremolo_type: 4, high_offset: 6, play_dir: false }; channels],
        real_bpm: false,
        beat: 0,
        tempo_ticks: 32,
        tempo_bpm: 120,
        internal_ticks: 6,
        steps: MEMORY_STEPS,
    };
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut cells = 0usize;
    for o in order.iter_mut() {
        if (*o as usize) < pat_chunks.len() {
            // `Patterns.InsertAny`: the first free index; none left past MAX_PATTERNS (and here
            // past the cell budget)
            let rows = {
                let mut c = pat_chunks[*o as usize];
                if version < 3 {
                    c.skip(3);
                } else {
                    c.skip(2);
                }
                (c.u16() as usize).clamp(1, MAX_PATTERN_ROWS)
            };
            if patterns.len() >= MAX_PATTERNS || cells + rows * channels > MAX_CELLS {
                *o = ORDER_END;
                continue;
            }
            let p = convert_pattern(pat_chunks[*o as usize], version, &mut s, channels);
            cells += p.cells.len();
            *o = patterns.len() as u16;
            patterns.push(p);
        }
    }
    // the sequence loop: a jump back to its start on the last row of its end
    let valid = |o: usize| order.get(o).is_some_and(|&p| (p as usize) < patterns.len());
    if valid(loop_end) && (loop_start > 0 || loop_end < last) {
        let p = &mut patterns[order[loop_end] as usize];
        let mut w = Writer::new(Cmd::PositionJump, loop_start as u8, p.rows - 1);
        w.retry_mode = Retry::PreviousRow;
        write_effect(p, channels, &mut w);
    }

    // samples
    let mut smpd = get(b"SMPD").unwrap_or(Rd::new(&[]));
    let mut smpi = get(b"SMPI").unwrap_or(Rd::new(&[]));
    let num_samples = smpi.u8() as usize;
    let mut samples = vec![Sample::empty()];
    let mut frames = 0u64;
    for _ in 0..num_samples {
        let name_len = if version < 2 { 30 } else { smpi.u8() as usize };
        smpi.skip(name_len);
        let mut h = Sample::empty();
        let (len, ls, le, c3, vol, flags) = if smpi.can(16) {
            (smpi.u32(), smpi.u32(), smpi.u32(), smpi.u16(), smpi.u8(), smpi.u8())
        } else {
            smpi.p = smpi.d.len();
            (0, 0, 0, 0, 0, 0)
        };
        h.len = len;
        h.sus_start = ls;
        h.sus_end = le;
        h.c5 = c3 as u32;
        if vol != 0 {
            h.volume = vol as i32 + 1;
        } else {
            h.volume = 256;
        }
        h.no_default_volume = vol == 0;
        h.sustain = flags & 1 != 0 && h.sus_end > h.sus_start;
        let is16 = flags & 2 != 0;
        if is16 {
            h.len /= 2;
            h.sus_start /= 2;
            h.sus_end /= 2;
        }
        if version >= 8 {
            smpi.skip(8);
        }
        smpi.skip(if version > 1 { 6 } else { 2 });
        let n = smpd.u32() as usize;
        let sd = smpd.chunk(n);
        h.len = h.len.min(MAX_SAMPLE_LENGTH);
        let src = sd.d;
        let compressed = flags & 0x0C == 0x04;
        let bps = if is16 { 2 } else { 1 };
        if compressed {
            h.len = (h.len as u64).min(src.len() as u64 * 8) as u32;
        } else if h.len > 0x40000 {
            h.len = (h.len as u64).min((src.len() as u64 + bps - 1) / bps) as u32;
        }
        // the frame budget: past it a sample keeps only the frames its data has
        if frames + h.len as u64 > MAX_SAMPLE_FRAMES {
            let have = if compressed { (src.len() as u64) * 8 } else { src.len() as u64 / bps };
            h.len = (h.len as u64).min(have).min(MAX_SAMPLE_FRAMES.saturating_sub(frames)) as u32;
        }
        let readable = if compressed { src.len() > 4 } else { !src.is_empty() };
        if h.len > 0 && readable {
            frames += h.len as u64;
            let mut bytes = vec![0u8; h.len as usize * bps as usize];
            if compressed {
                dmf_unpack(src, &mut bytes);
            } else {
                let k = bytes.len().min(src.len());
                bytes[..k].copy_from_slice(&src[..k]);
            }
            h.data = if is16 {
                Data::I16(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect())
            } else {
                Data::I8(bytes.into_iter().map(|b| b as i8).collect())
            };
        }
        // `SanitizeLoops`
        h.sus_end = h.sus_end.min(h.len);
        if h.sus_start >= h.sus_end {
            h.sus_start = 0;
            h.sus_end = 0;
            h.sustain = false;
        }
        samples.push(h);
    }

    Ok(Module {
        version,
        tracker: if version == 10 { "X-Tracker 32" } else { "X-Tracker" },
        title,
        artist,
        channels,
        patterns,
        order,
        samples,
    })
}

/// `dmf_unpack`, for the tests.
#[cfg(test)]
pub fn tests_unpack(src: &[u8], out: &mut [u8]) {
    dmf_unpack(src, out);
}

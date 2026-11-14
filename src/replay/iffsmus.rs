//! IFF-SMUS (Sonix and Deluxe Music scores), ported from NostalgicPlayer's
//! IFF-SMUS player (Copyright (c) 2023 Thomas Neumann, MIT licence: see
//! LICENSE-NostalgicPlayer).
//!
//! Checked against the IFF-SMUS Eagleplayer ("IFF-SMUS/Sonix player module
//! V2.2", adapted by Delirium & DEFECT; the DeliPlayer of the same name is
//! tried first, so run it with DELIPLAYERS pointing elsewhere). A score names
//! its instruments, and the original loads them from the `Instruments:` volume
//! (the module's `Instruments` folder, as UADE maps it), or else from the
//! current directory: an instrument file (`NAME.instr`) is a synthesis sound, a
//! sampled sound with its sample file (`NAME.ss`), or an IFF 8SVX sample. This
//! port loads them the same way, through the same allocations in the same order
//! (file handles too), so that its sample data and waveform buffers lie in guest
//! memory where the original's do, and keeps the original's variables there in
//! its layout: the player's, the score's, the instruments' as read from their
//! files, the tracks' events as the original converts them.
//!
//! Where the original's register writes differ from what NostalgicPlayer asks of
//! its mixer, this follows the original:
//! * every tick turns DMA off for the voices that start a sample (DMACON is
//!   written even with no voice) and on for the voices that start a note
//!   (DMACON $8000 with none), and the host's ENPP_DMAMask waits for the DMA;
//! * a sampled or IFF note first mutes its voice (DMA off and period 2), writes
//!   length and location, and on the next tick its loop, or 4 words of zeros for
//!   none; a synthesis voice writes location, length, period and volume every
//!   tick, its waveform built anew in one of two 128-byte buffers;
//! * a voice that changes instrument type while it sounds is muted at once;
//! * only 4 tracks and 64 instrument registers, the SNX chunk's four track
//!   switches whatever the number of tracks, an instrument event whose register
//!   has no instrument clears the track's instrument, and a restart clears them;
//! * the original's word arithmetic: products and sums cut to 16 bits where its
//!   code keeps a word (vibrato, LFO, envelope, volume and filter terms), the
//!   envelope rate's shift as a long shift (0 from 32 on), the octave search and
//!   the LFO end as signed and carry tests, the waveform mixes rounding down,
//!   the second phase half stepping by its own length, the filter tables'
//!   wrapping differences;
//! * the transpose shifted with its sign (NostalgicPlayer divides it unsigned);
//! * InitPlayer's timer for the first two ticks (the tempo's, which the first
//!   sets through dtg_SetTimer, is loaded at the next underflow);
//! * the original's time: it writes Paula only through the host's calls
//!   (ENPP_DMAMask, ENPP_Poke*), and each lands at the clock its code's 68000
//!   cycles since the last call reach (`COSTS`), so every write falls on the
//!   reference's colour clock.
//!
//! The `Instruments:` volume's names are found in any case (an engine change:
//! the Amiga's file systems ignore case, and many songs name their instruments
//! in another case than their files').

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;

pub const OPEN: Open = Open { name: "IFF-SMUS", detect, new };

include!("iffsmus_tables.rs");

// The player's variables (the original's a6, $4C8 bytes).
/// Current volume (word, 0-255).
const G_VOL: u32 = 0x000;
/// The tempo index * 2.
const G_TEMPO: u32 = 0x002;
const G_TUNE: u32 = 0x004;
const G_SCORE: u32 = 0x006;
/// Repeats left (word, -1 for ever).
const G_REPEAT: u32 = 0x00a;
const G_START: u32 = 0x00c;
const G_TIME: u32 = 0x010;
const G_END: u32 = 0x014;
/// The memory to keep free (long).
const G_RESERVE: u32 = 0x028;
const G_FLAGS: u32 = 0x030;
const G_COUNT: u32 = 0x032;
const G_CUR_TEMPO: u32 = 0x034;
/// The tempo as a fraction of $8000.
const G_CALC: u32 = 0x036;
const G_SPEED: u32 = 0x038;
/// The four tracks' switches (longs: 0 off, 1 on, else on at half volume).
const G_ENABLED: u32 = 0x03a;
/// 64 instrument registers: the instrument (long) and its users (word).
const G_INSTRS: u32 = 0x04a;
const G_LAST_VOICE: u32 = 0x1ca;
const G_FADE_VOL: u32 = 0x1cc;
const G_FADE_TARGET: u32 = 0x1ce;
const G_FADE_STEP: u32 = 0x1d0;
const G_VOICES: u32 = 0x1d2;
/// Per track (longs): its instrument register, the note's held and released
/// rows, its start position, its current position.
const G_TRK_INSTR: u32 = 0x22a;
const G_HOLD: u32 = 0x23a;
const G_RELEASE: u32 = 0x24a;
const G_START_POS: u32 = 0x25a;
const G_POS: u32 = 0x27a;
/// The 4 * 2 128-byte synthesis waveforms, then 8 bytes of zeros (long).
const G_WAVES: u32 = 0x29a;
const G_HEAD: u32 = 0x32a;
const G_NAME: u32 = 0x30a;
const G_TRACKS: u32 = 0x35a;
const G_PLAYING: u32 = 0x360;
const G_SCORES: u32 = 0x368;
const G_SYNTH: u32 = 0x3e8;
const G_SAMPLED: u32 = 0x450;
const G_IFF: u32 = 0x4a0;
const G_SIZE: u32 = 0x4c8;

// A voice ($16 bytes).
/// 0, or 1 start, 2 release, 3 mute.
const V_CMD: u32 = 0x00;
/// 0 silent, 1 playing, 2 released.
const V_STATUS: u32 = 0x01;
const V_NOTE: u32 = 0x03;
const V_INSTR: u32 = 0x04;
const V_VOLUME: u32 = 0x08;
/// A note's start: -1 (synthesis) or 2 (samples), counted down by the play.
const V_START: u32 = 0x0a;
const V_LOC: u32 = 0x0c;
const V_LEN: u32 = 0x10;
const V_PER: u32 = 0x12;
const V_VOL: u32 = 0x14;
const V_SIZE: u32 = 0x16;

// The score ($84 bytes).
const S_TRANSPOSE: u32 = 0x04;
const S_TUNE: u32 = 0x06;
const S_DEFAULT_INSTR: u32 = 0x0c;
/// Track volumes, the word at $12 + 4 * track.
const S_TRK_VOL: u32 = 0x12;
const S_ENABLED: u32 = 0x20;
const S_TRACKS: u32 = 0x30;
const S_MAP: u32 = 0x40;
const S_NEXT: u32 = 0x80;
const S_SIZE: u32 = 0x84;

// An instrument: its type, its name, then its file from offset $20.
const TYPE_SYNTH: u32 = 1;
const TYPE_SAMPLED: u32 = 2;
const TYPE_FORM: u32 = 3;
const I_NAME: u32 = 0x04;

// Synthesis ($1DA bytes).
const Y_OSC: u32 = 0x024;
const Y_LFO: u32 = 0x0a4;
const Y_AMP_VOL: u32 = 0x1ac;
const Y_AMP_ENV: u32 = 0x1ae;
const Y_AMP_LFO: u32 = 0x1b0;
const Y_PORT: u32 = 0x1b2;
const Y_FREQ_LFO: u32 = 0x1b4;
const Y_FILTER: u32 = 0x1b6;
const Y_FILTER_ENV: u32 = 0x1b8;
const Y_FILTER_LFO: u32 = 0x1ba;
const Y_LFO_SPEED: u32 = 0x1bc;
const Y_LFO_MODE: u32 = 0x1be;
const Y_LFO_DELAY: u32 = 0x1c0;
const Y_PHASE_SPEED: u32 = 0x1c2;
const Y_PHASE_DEPTH: u32 = 0x1c4;
const Y_LEVELS: u32 = 0x1c6;
const Y_RATES: u32 = 0x1ce;
/// The 64 filtered waveforms ($2000 bytes, then their users).
const Y_TABLES: u32 = 0x1d6;
const Y_SIZE: u32 = 0x1da;
const Y_FILE: u32 = 0x1d6;

// A synthesis voice ($1A bytes).
const SV_NOTE: u32 = 0x00;
const SV_PORT_COUNT: u32 = 0x02;
const SV_PORT_STEP: u32 = 0x04;
const SV_BUFFER: u32 = 0x06;
const SV_OCTAVE: u32 = 0x08;
const SV_ENV_STEP: u32 = 0x0a;
const SV_ENV: u32 = 0x0c;
const SV_LFO_POS: u32 = 0x10;
const SV_LFO_WAIT: u32 = 0x12;
const SV_LFO: u32 = 0x14;
const SV_PHASE: u32 = 0x16;
const SV_PHASE_DIR: u32 = 0x18;

// Sampled sound ($60 bytes) and its sample file.
const P_SAMPLE_NAME: u32 = 0x24;
const P_SAMPLE: u32 = 0x44;
const P_VOLUME: u32 = 0x48;
const P_LEVELS: u32 = 0x4a;
const P_RATES: u32 = 0x52;
const P_VIB_DEPTH: u32 = 0x5a;
const P_VIB_SPEED: u32 = 0x5c;
const P_VIB_DELAY: u32 = 0x5e;
const P_SIZE: u32 = 0x60;
const F_LOOP: u32 = 0x02;
const F_FIRST: u32 = 0x04;
const F_LAST: u32 = 0x05;
const F_USERS: u32 = 0x1e;
const F_DATA: u32 = 0x3e;

// A sampled sound voice ($14 bytes).
const PV_LOOP: u32 = 0x00;
const PV_LOOP_LEN: u32 = 0x04;
const PV_NOTE: u32 = 0x06;
const PV_ENV_STEP: u32 = 0x08;
const PV_ENV: u32 = 0x0a;
const PV_VIB_POS: u32 = 0x0e;
const PV_VIB_WAIT: u32 = 0x10;
const PV_VIB: u32 = 0x12;

// IFF 8SVX ($3E bytes): VHDR at $24, the octaves to skip, the BODY.
const X_ONE_SHOT: u32 = 0x24;
const X_REPEAT: u32 = 0x28;
const X_CYCLE: u32 = 0x2c;
const X_OCTAVES: u32 = 0x32;
const X_VOLUME: u32 = 0x34;
const X_SKIP: u32 = 0x38;
const X_BODY: u32 = 0x3a;
const X_SIZE: u32 = 0x3e;

// An IFF voice (10 bytes).
const XV_LOOP: u32 = 0x00;
const XV_LOOP_LEN: u32 = 0x04;
const XV_NOTE: u32 = 0x06;
const XV_ON: u32 = 0x08;

/// InitPlayer's timer (11932: about 60 Hz).
const TIMER: u16 = 0x2e9c;

fn detect(d: &[u8]) -> bool {
    d.len() >= 12 && &d[..4] == b"FORM" && &d[8..12] == b"SMUS"
}

/// divu.w: the quotient in the low word and the remainder in the high, or the
/// dividend unchanged on overflow.
fn divu(n: u32, d: u16) -> u32 {
    if d == 0 {
        return n;
    }
    let q = n / d as u32;
    if q > 0xffff {
        n
    } else {
        (n % d as u32) << 16 | q
    }
}

/// divs.w, as `divu`.
fn divs(n: i32, d: i16) -> u32 {
    if d == 0 {
        return n as u32;
    }
    let q = n.wrapping_div(d as i32);
    if !(-0x8000..=0x7fff).contains(&q) {
        n as u32
    } else {
        (n.wrapping_rem(d as i32) as u32) << 16 | (q as u32 & 0xffff)
    }
}

/// muls.w, then asr.w of the product's low word.
fn muls_asr(a: u16, b: u16, s: u32) -> u16 {
    (((a as i16 as i32).wrapping_mul(b as i16 as i32) as u16 as i16) >> s) as u16
}

/// lsr.w and lsr.l by a register: the count is taken modulo 64.
fn lsr_w(v: u16, n: u16) -> u16 {
    let n = n & 63;
    if n >= 16 { 0 } else { v >> n }
}
fn lsr_l(v: u32, n: u32) -> u32 {
    let n = n & 63;
    if n >= 32 { 0 } else { v >> n }
}
fn asl_l(v: u32, n: u32) -> u32 {
    let n = n & 63;
    if n >= 32 { 0 } else { v << n }
}
fn asr_l(v: i32, n: u32) -> i32 {
    let n = n & 63;
    v >> n.min(31)
}

/// 68000 cycles of the original's code, by the part of it the port runs (the names `c` takes; a
/// name not listed costs 0). A host call (a write, dtg_SetTimer, ENPP_SongEnd, ENPP_Amplifier) lands
/// at the clock the cycles counted since the last one reach, halved and rounded down (the engine's
/// static cycle table: a path's time depends only on its branches), and the count starts again
/// there; ENPP_DMAMask's wait for the DMA ends a tick's writes. Integers fitted (exactly, the sum
/// of absolute values kept small) to every gap between two host calls in the reference's logs of
/// the 17 refcmp rows and of copies with an IFF instrument out of its octaves (177 451 gaps, all
/// within the rounding but one, in row 737). Where several names always run together the split
/// between them is arbitrary, and a few are negative (a path that skips part of a shared one).
/// No reference plays an IFF sample (the original leaves the copies' 8SVX instruments silent), so
/// its writes take a sampled sound's costs.
const COSTS: &[(&str, i64)] = &[
    ("AMP", 130),
    ("DMA_OFF", 584),
    ("DMA_ON", 242),
    ("ENV_DOWN", 10),
    ("ENV_LEVEL", 6),
    ("ENV_NEG", 6),
    ("ENV_NEXT", 30),
    ("EV_END", -70),
    ("EV_ZERO", -91),
    ("FM_REP", 162),
    ("FORM_LEN", 249),
    ("FORM_LOC", 175),
    ("FORM_PER", 207),
    ("FORM_RET", 84),
    ("FORM_START", 736),
    ("FORM_VOL", 178),
    ("HOLD", -25),
    ("INSTR_EV", 166),
    ("LFO_CARRY", 21),
    ("LFO_C_NEG", 9),
    ("LFO_STEP", 204),
    ("LFO_STOP", -16),
    ("LFO_WAIT", 35),
    ("MIX", 70),
    ("MIX2", 70),
    ("MIX_SETUP", 208),
    ("MIX_WRAP", 10),
    ("MUTE", 520),
    ("MUTEV_OLD_SYNTH", 2),
    ("MUTE_PER", 248),
    ("NOTE", 710),
    ("NOTE_OTHER_TYPE", 559),
    ("NOTE_SOUNDING", 55),
    ("OCTAVE", 39),
    ("PHASE", 48),
    ("PHASE_SEG", 296),
    ("PH_BORROW", 4),
    ("PH_OV", 37),
    ("PLAIN", 33),
    ("PLAY_ON", 8),
    ("PLAY_SKIP", 58),
    ("RELEASE", 205),
    ("RELEASE_ON", 12),
    ("REL_DONE", 33),
    ("RESTART_TRACK", 155),
    ("ROW", -552),
    ("ROW_END_DONE", 408),
    ("SAMPLED", 1304),
    ("SEQ_TEMPO", 342),
    ("SETTIMER", 666),
    ("SMP_LEN", 249),
    ("SMP_LOC", 175),
    ("SMP_PER", 207),
    ("SMP_RET", 84),
    ("SMP_VOL", 178),
    ("SM_LOOP", 162),
    ("SM_ST0", 24),
    ("SONG_END", 831),
    ("SP_ENTRY", 217),
    ("SP_LOOP", 78),
    ("SP_POST_FIRST", 13),
    ("SP_PRE_LOC_FIRST", 3),
    ("SYNTH", 1943),
    ("SYN_LEN", 198),
    ("SYN_LOC", 386),
    ("SYN_PER", 182),
    ("SYN_RET", 100),
    ("SYN_VOL", 174),
    ("SY_LFO_MODE", 228),
    ("SY_PHASE0", 16),
    ("SY_PORT", 453),
    ("SY_ST0", 49),
    ("TRACK_READ", 6),
    ("TRACK_WAIT", 132),
    ("VIB_NEG", 8),
    ("VIB_STEP", 106),
    ("VOICE_CMD0", 129),
    ("VOICE_CMD1", 610),
    ("VOICE_CMD2", 182),
    ("VOICE_OFF", 195),
    ("WORD", 108),
];

/// The period of InitPlayer's timer: the reference's first tick falls this long after the song
/// start, and its second as long after the first (the tempo's timer, set in the first tick, is
/// loaded at the next underflow).
const FIRST_PERIOD: u64 = (TIMER as u64 + 1) * 5;

pub struct IffSmus {
    g: u32,
    timer: u16,
    songend: bool,
    /// The clock of the last host call (or the tick's start), and the 68000 cycles spent since.
    seg: u64,
    cyc: i64,
    /// The first tick's time, until the engine has taken it.
    first_at: Option<u64>,
    /// The second tick's time, once the first has run (the tempo it set).
    second_at: std::cell::Cell<Option<u64>>,
    ticked: bool,
}

/// The original's loader: its allocations (AllocMem with a size long in front,
/// dos file handles) in its order, and its reads of the files.
struct Loader<'a> {
    m: &'a mut Machine,
    g: u32,
    allocs: Vec<u32>,
}

/// An open file: its contents and position, and the handle's memory.
struct File {
    data: Vec<u8>,
    pos: usize,
    fh: u32,
    /// The original's reads from it.
    reads: i64,
}

impl File {
    fn read(&mut self, n: usize) -> Option<&[u8]> {
        self.reads += 1;
        let s = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    fn long(&mut self) -> Option<u32> {
        self.read(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn word(&mut self) -> Option<u16> {
        self.read(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn byte(&mut self) -> Option<u8> {
        self.read(1).map(|b| b[0])
    }
}

/// Compares two names as the original does (ASCII letters in either case).
fn same_name(a: &[u8], b: &[u8]) -> bool {
    let up = |c: u8| if c.is_ascii_lowercase() { c & 0xdf } else { c };
    for (i, &c) in a.iter().enumerate() {
        let d = b.get(i).copied().unwrap_or(0);
        if up(c) != up(d) {
            return false;
        }
        if c == 0 {
            return true;
        }
    }
    b.get(a.len()).copied().unwrap_or(0) == 0
}

impl Loader<'_> {
    /// AllocMem through the original's wrapper: a size long in front, and
    /// nothing when less than its reserve would stay free.
    fn alloc(&mut self, size: u32) -> u32 {
        let reserve = self.m.bus.r32(self.g + G_RESERVE);
        if self.g != 0 && (self.m.heap.largest() as i64) < (size + 4) as i64 + reserve as i64 {
            return 0;
        }
        let a = self.m.alloc(size + 4);
        if a == 0 {
            return 0;
        }
        self.allocs.push(a);
        self.m.bus.w32(a, size + 4);
        a + 4
    }

    fn free(&mut self, p: u32) {
        if p != 0 {
            self.m.heap.free(p - 4);
            self.allocs.retain(|&a| a != p - 4);
        }
    }

    /// dos Open: the file's name as an Amiga path, the handle's 44 bytes.
    fn open(&mut self, name: &str) -> Option<File> {
        let p = self.m.os.host_path(name, None);
        let data = self.m.os.read_file(&p).ok()?;
        let fh = self.m.alloc(44);
        self.allocs.push(fh);
        log::debug!("IFF-SMUS: {name} -> {} ({} bytes)", p.display(), data.len());
        Some(File { data, pos: 0, fh, reads: 0 })
    }

    fn close(&mut self, f: File) {
        self.m.heap.free(f.fh);
        self.allocs.retain(|&a| a != f.fh);
    }

    /// The original's path: the directory (with a '/' unless it ends in ':' or
    /// '/'), the name and the suffix.
    fn path(dir: Option<&str>, name: &[u8], suffix: &str) -> String {
        let mut s = String::new();
        if let Some(d) = dir {
            s.push_str(d);
            if !d.ends_with(':') && !d.ends_with('/') {
                s.push('/');
            }
        }
        s.extend(name.iter().take_while(|&&c| c != 0).map(|&c| c as char));
        s.push_str(suffix);
        s
    }

    fn copy_in(&mut self, a: u32, d: &[u8]) {
        self.m.bus.write_bytes(a, d);
    }

    /// The score ($1095E): its chunks, the instruments it names, its tracks.
    fn score(&mut self, path: &str, dir: &str) -> Option<u32> {
        let g = self.g;
        let fl = self.m.bus.r8(g + G_FLAGS);
        self.m.bus.w8(g + G_FLAGS, fl & 0x3f);
        self.m.bus.w16(g + G_TRACKS, 0);
        let mut f = self.open(path)?;
        let s = self.alloc(S_SIZE);
        if s == 0 {
            self.close(f);
            return None;
        }
        let r = self.chunks(&mut f, s, dir);
        self.close(f);
        r?;
        // the list of scores
        let mut at = g + G_SCORES;
        while self.m.bus.r32(at) != 0 {
            at = self.m.bus.r32(at) + S_NEXT;
        }
        self.m.bus.w32(at, s);
        Some(s)
    }

    fn chunks(&mut self, f: &mut File, s: u32, dir: &str) -> Option<()> {
        let g = self.g;
        for k in 0..4 {
            self.m.bus.w16(s + S_TRK_VOL + 4 * k, 0xff);
        }
        if f.long()? != u32::from_be_bytes(*b"FORM") {
            return None;
        }
        let mut left = f.long()?.wrapping_sub(4);
        if f.long()? != u32::from_be_bytes(*b"SMUS") {
            return None;
        }
        while left != 0 {
            let id = f.long()?;
            let mut n = f.long()?.wrapping_add(1) & !1;
            left = left.wrapping_sub(n.wrapping_add(8));
            match &id.to_be_bytes() {
                b"SHDR" => {
                    let t = f.word()?;
                    let mut i = 0u16;
                    if t >= 0xe11 {
                        let q = divu(0xe10_0000, t) as u16;
                        while q < TEMPO[i as usize / 2] {
                            i += 2;
                            if i >= 0x100 {
                                i -= 1;
                                break;
                            }
                        }
                    }
                    self.m.bus.w16(s + 2, i);
                    let mut v = f.byte()? as u16;
                    if v < 0x80 {
                        v <<= 1;
                    }
                    self.m.bus.w16(s, v);
                    let c = f.byte()?.min(4);
                    for k in 0..c as u32 {
                        self.m.bus.w32(s + S_ENABLED + 4 * k, 1);
                    }
                    n = n.wrapping_sub(4);
                }
                b"INS1" => {
                    let reg = f.byte()?;
                    n = n.wrapping_sub(1);
                    let slot = s + S_MAP + reg as u32;
                    if reg < 0x40 && self.m.bus.r8(slot) == 0 {
                        let ty = f.byte()?;
                        n = n.wrapping_sub(1);
                        if ty == 0 {
                            f.word()?;
                            n = n.wrapping_sub(2);
                            let mut name = Vec::new();
                            for _ in 0..25 {
                                name.push(f.byte()?);
                                n = n.wrapping_sub(1);
                                if n == 0 {
                                    break;
                                }
                            }
                            name.push(0);
                            self.copy_in(g + G_NAME, &name);
                            let e = self.instrument(&name, dir);
                            if e == 0 {
                                let fl = self.m.bus.r8(g + G_FLAGS);
                                self.m.bus.w8(g + G_FLAGS, fl | 0x80);
                                return None;
                            }
                            self.m.bus.w8(slot, ((e - (g + G_INSTRS)) / 6 + 1) as u8);
                        }
                    }
                }
                b"TRAK" => {
                    let k = self.m.bus.r16(g + G_TRACKS);
                    if (k as i16) < 4 {
                        self.m.bus.w16(g + G_TRACKS, k + 1);
                        let t = self.alloc(n + 2);
                        self.m.bus.w32(s + S_TRACKS + 4 * k as u32, t);
                        if t == 0 {
                            return None;
                        }
                        let d = f.read(n as usize)?.to_vec();
                        self.copy_in(t, &d);
                        self.m.bus.w16(t + n, 0xffff);
                        n = 0;
                        self.convert(s + 4 * k as u32, s, t);
                    }
                }
                b if id >= u32::from_be_bytes(*b"SNX1") && id < u32::from_be_bytes(*b"SNX:") => {
                    let _ = b;
                    let tr = f.word()?;
                    self.m.bus.w16(s + S_TRANSPOSE, tr);
                    let tu = f.word()?;
                    self.m.bus.w16(s + S_TUNE, tu);
                    f.long()?;
                    for k in 0..4 {
                        let e = f.long()?;
                        self.m.bus.w32(s + S_ENABLED + 4 * k, e);
                    }
                    n = n.wrapping_sub(0x18);
                }
                _ => {}
            }
            while (n as i32) > 0 {
                f.byte()?;
                n -= 1;
            }
        }
        Some(())
    }

    /// A track's events as the original keeps them: notes and rests with their
    /// length, instrument changes, and 0 for the rest; the time signature and
    /// the track's volume are taken out.
    fn convert(&mut self, trk: u32, s: u32, t: u32) {
        let b = &mut self.m.bus;
        let mut p = t;
        loop {
            let w = b.r16(p);
            if w == 0xffff {
                break;
            }
            let hi = w & 0xff00;
            let v = if hi == 0x8100 {
                w
            } else if hi > 0x8100 {
                if hi == 0x8200 {
                    b.w16(s + 8, ((w >> 3) & 0x1f) + 1);
                    b.w16(s + 10, 1 << (w & 7));
                } else if hi == 0x8400 {
                    b.w16(trk + S_TRK_VOL, (w & 0x7f) << 1);
                }
                0
            } else {
                let d = DURATION[(w & 15) as usize];
                if d < 0 { 0 } else { hi | d as u16 }
            };
            b.w16(p, v);
            if hi < 0x8100 {
            }
            p += 2;
        }
    }

    /// An instrument register ($107FE): one already loaded under this name, or
    /// the instrument file's.
    fn instrument(&mut self, name: &[u8], dir: &str) -> u32 {
        let g = self.g;
        let fl = self.m.bus.r8(g + G_FLAGS);
        self.m.bus.w8(g + G_FLAGS, fl & !0x40);
        let mut free = 0;
        for k in 0..64 {
            let e = g + G_INSTRS + 6 * k;
            let ins = self.m.bus.r32(e);
            if ins == 0 {
                if free == 0 {
                    free = e;
                }
                continue;
            }
            let other: Vec<u8> = (0..32).map(|i| self.m.bus.r8(ins + I_NAME + i)).collect();
            if same_name(&other, name) {
                let u = self.m.bus.r16(e + 4);
                self.m.bus.w16(e + 4, u.wrapping_add(1));
                return e;
            }
        }
        if free == 0 {
            return 0;
        }
        let f = self.open(&Self::path(Some(dir), name, ".instr")).or_else(|| self.open(&Self::path(None, name, ".instr")));
        let Some(mut f) = f else {
            let fl = self.m.bus.r8(g + G_FLAGS);
            self.m.bus.w8(g + G_FLAGS, fl | 0x40);
            return 0;
        };
        let Some(head) = f.read(32).map(|h| h.to_vec()) else {
            self.close(f);
            return 0;
        };
        self.copy_in(g + G_HEAD, &head);
        let ty = if head[0] == 0 || same_name(b"Synthesis\0", &head) {
            TYPE_SYNTH
        } else if same_name(b"SampledSound\0", &head) {
            TYPE_SAMPLED
        } else if same_name(b"FORM\0", &head) {
            TYPE_FORM
        } else {
            self.close(f);
            return 0;
        };
        let (ins, f) = match ty {
            TYPE_SYNTH => self.synth(f),
            TYPE_SAMPLED => (self.sampled(f, dir), None),
            _ => self.form(f),
        };
        if ins != 0 {
            self.m.bus.w32(ins, ty);
            let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            self.copy_in(ins + I_NAME, &name[..n]);
            self.m.bus.w8(ins + I_NAME + n as u32, 0);
            self.m.bus.w32(free, ins);
            let u = self.m.bus.r16(free + 4);
            self.m.bus.w16(free + 4, u.wrapping_add(1));
        }
        if let Some(f) = f {
            self.close(f);
        }
        if ins == 0 { 0 } else { free }
    }

    /// A synthesis instrument ($113E2) and its filtered waveforms ($11838).
    fn synth(&mut self, mut f: File) -> (u32, Option<File>) {
        let ins = self.alloc(Y_SIZE);
        if ins == 0 {
            return (0, Some(f));
        }
        let Some(d) = f.read(Y_FILE as usize).map(|d| d.to_vec()) else {
            self.free(ins);
            return (0, Some(f));
        };
        self.copy_in(ins, &d);
        self.tables(ins);
        if self.m.bus.r32(ins + Y_TABLES) == 0 {
            self.free(ins);
            return (0, Some(f));
        }
        (ins, Some(f))
    }

    fn tables(&mut self, ins: u32) {
        let g = self.g;
        // an instrument with the same oscillator shares its tables
        for k in 0..64 {
            let o = self.m.bus.r32(g + G_INSTRS + 6 * k);
            if o == 0 || o == ins || self.m.bus.r32(o) != TYPE_SYNTH || self.m.bus.r32(o + Y_TABLES) == 0 {
                continue;
            }
            if (0..128).all(|i| self.m.bus.r8(o + Y_OSC + i) == self.m.bus.r8(ins + Y_OSC + i)) {
                let t = self.m.bus.r32(o + Y_TABLES);
                self.m.bus.w32(ins + Y_TABLES, t);
                let u = self.m.bus.r16(t + 0x2000);
                self.m.bus.w16(t + 0x2000, u.wrapping_add(1));
                return;
            }
        }
        let t = self.alloc(0x2002);
        if t == 0 {
            return;
        }
        self.m.bus.w32(ins + Y_TABLES, t);
        self.m.bus.w16(t + 0x2000, 1);
        let b = &mut self.m.bus;
        let osc = |b: &crate::bus::Bus, i: u32| b.r8(ins + Y_OSC + i) as i8 as i16;
        let mut d3: i16 = 0;
        let mut d4: i16 = osc(b, 127) << 7;
        let mut out = t;
        for k in 0..64 {
            let c = FILTER[k];
            let d2 = (((0x8000 - c as u32) * 0xe666) >> 16) as i16;
            let d1 = (c >> 1) as i16;
            for i in 0..128 {
                let d6 = (osc(b, i) << 7).wrapping_sub(d4);
                let p = (d6 as i32 * d1 as i32) << 2;
                d3 = d3.wrapping_add((p >> 16) as i16);
                d4 = d4.wrapping_add(d3);
                b.w8(out, (d4 as u16 >> 7) as u8);
                out += 1;
                d3 = (((d2 as i32 * d3 as i32) << 1) >> 16) as i16;
            }
        }
    }

    /// A sampled sound ($119D6) and its sample file, shared by name.
    fn sampled(&mut self, mut f: File, dir: &str) -> u32 {
        let g = self.g;
        let ins = self.alloc(P_SIZE);
        if ins == 0 {
            self.close(f);
            return 0;
        }
        let d = f.read(P_SIZE as usize).map(|d| d.to_vec());
        self.m.bus.w32(ins + P_SAMPLE, 0);
        let Some(d) = d else {
            self.free(ins);
            self.close(f);
            return 0;
        };
        self.copy_in(ins, &d);
        self.m.bus.w32(ins + P_SAMPLE, 0);
        self.close(f);
        let name: Vec<u8> = (0..32).map(|i| self.m.bus.r8(ins + P_SAMPLE_NAME + i)).collect();
        for k in 0..64 {
            let o = self.m.bus.r32(g + G_INSTRS + 6 * k);
            if o == 0 || self.m.bus.r32(o) != TYPE_SAMPLED {
                continue;
            }
            let other: Vec<u8> = (0..32).map(|i| self.m.bus.r8(o + P_SAMPLE_NAME + i)).collect();
            if same_name(&name, &other) {
                let smp = self.m.bus.r32(o + P_SAMPLE);
                self.m.bus.w32(ins + P_SAMPLE, smp);
                let u = self.m.bus.r16(smp + F_USERS);
                self.m.bus.w16(smp + F_USERS, u.wrapping_add(1));
                return ins;
            }
        }
        let smp = self.load_file(&Self::path(Some(dir), &name, ".ss"));
        let smp = if smp != 0 { smp } else { self.load_file(&Self::path(None, &name, ".ss")) };
        if smp == 0 {
            let fl = self.m.bus.r8(g + G_FLAGS);
            self.m.bus.w8(g + G_FLAGS, fl | 0x40);
            self.free(ins);
            return 0;
        }
        self.m.bus.w32(ins + P_SAMPLE, smp);
        self.m.bus.w16(smp + F_USERS, 1);
        ins
    }

    /// A whole file in memory ($121AC).
    fn load_file(&mut self, path: &str) -> u32 {
        let Some(f) = self.open(path) else { return 0 };
        let n = f.data.len() as u32;
        let a = self.alloc(n);
        if a != 0 {
            let d = f.data.clone();
            self.copy_in(a, &d);
        }
        self.close(f);
        a
    }

    /// An IFF 8SVX sample ($11D52), read on from the 32 bytes read already.
    fn form(&mut self, f: File) -> (u32, Option<File>) {
        let ins = self.alloc(X_SIZE);
        if ins == 0 {
            return (0, Some(f));
        }
        // the 32 bytes read count as read: the file goes on from its start
        let mut s = File { data: f.data.clone(), pos: 0, fh: 0, reads: 0 };
        let r = self.form_chunks(&mut s, ins);
        if r.is_none() || self.m.bus.r32(ins + X_BODY) == 0 {
            let b = self.m.bus.r32(ins + X_BODY);
            self.free(b);
            self.free(ins);
            return (0, Some(f));
        }
        let b = &mut self.m.bus;
        let (mut one, mut rep, mut cyc) = (b.r32(ins + X_ONE_SHOT), b.r32(ins + X_REPEAT), b.r32(ins + X_CYCLE));
        let oct = b.r8(ins + X_OCTAVES);
        let mut k = 0u16;
        loop {
            one >>= 1;
            rep >>= 1;
            cyc >>= 1;
            k = k.wrapping_add(1);
            if (one | rep) & 1 != 0 || cyc == 1 || (k as u8).wrapping_add(oct) as i8 >= 8 {
                break;
            }
        }
        b.w16(ins + X_SKIP, k);
        (ins, Some(f))
    }

    fn form_chunks(&mut self, f: &mut File, ins: u32) -> Option<()> {
        if f.long()? != u32::from_be_bytes(*b"FORM") {
            return None;
        }
        let mut left = f.long()?;
        if f.long()? != u32::from_be_bytes(*b"8SVX") {
            return None;
        }
        left = left.wrapping_sub(4);
        while (left as i32) > 0 {
            let id = f.long()?;
            let mut n = f.long()?;
            left = left.wrapping_sub(8);
            match &id.to_be_bytes() {
                b"VHDR" => {
                    let d = f.read(20)?.to_vec();
                    self.copy_in(ins + X_ONE_SHOT, &d);
                    if d[15] != 0 {
                        return None;
                    }
                    n = n.wrapping_sub(20);
                    left = left.wrapping_sub(20);
                }
                b"BODY" => {
                    if self.m.bus.r32(ins + X_BODY) != 0 || self.m.bus.r8(ins + X_OCTAVES) == 0 {
                        return None;
                    }
                    let sz = n.wrapping_add(1) & !1;
                    let a = self.alloc(sz);
                    self.m.bus.w32(ins + X_BODY, a);
                    if a == 0 {
                        return None;
                    }
                    let d = f.read(sz as usize)?.to_vec();
                    self.copy_in(a, &d);
                    left = left.wrapping_sub(sz);
                    continue;
                }
                _ => {}
            }
            while (n as i32) > 0 {
                f.word()?;
                n = n.wrapping_sub(2);
                left = left.wrapping_sub(2);
            }
        }
        Some(())
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    // ExtLoad: the player's variables ($12030), then the score
    let path = m.bus.read_cstr(m.deli.dir_buf) + &m.bus.read_cstr(m.deli.file_buf);
    let mut l = Loader { m, g: 0, allocs: Vec::new() };
    let g = l.alloc(G_SIZE);
    if g == 0 {
        return None;
    }
    l.g = g;
    let b = &mut l.m.bus;
    b.w32(g + G_RESERVE, 0x1000);
    let w = l.alloc(0x408);
    if w == 0 {
        l.free(g);
        return None;
    }
    let b = &mut l.m.bus;
    b.w32(g + G_WAVES, w);
    b.w16(g + G_LAST_VOICE, 0x3f);
    b.w16(g + G_CALC, 0x8000);
    b.w16(g + G_VOL, 0xff);
    b.w16(g + G_TEMPO, 0x80);
    b.w16(g + G_TUNE, 0x80);
    b.w16(g + G_VOL, 0xffff);
    let Some(score) = l.score(&path, "Instruments:") else {
        for a in std::mem::take(&mut l.allocs) {
            l.m.heap.free(a);
        }
        return None;
    };
    // DeliTracker sets dtg_Timer to its default before InitSound, over
    // InitPlayer's: the first tick sets the tempo's
    let mut s = IffSmus { g, timer: crate::deli::DEFAULT_TIMER, songend: false, seg: 0, cyc: 0, first_at: None, second_at: Default::default(), ticked: false };
    // InitSound: the score from its start, for ever (the start-up's time does not matter: the
    // song's ticks and Paula's DMA count from the song start, and the only wait on the beam,
    // ENPP_DMAMask's, ends the tick's writes)
    let mut hw = Hw { bus: &mut l.m.bus };
    s.start(&mut hw, score);
    // the song starts at the end of the start-up, InitPlayer's timer with it
    s.first_at = Some(hw.bus.clock + FIRST_PERIOD);
    Some(Box::new(s))
}

impl Replayer for IffSmus {
    fn format(&self) -> &'static str {
        "IFF-SMUS"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsong_tick_at(&mut self) -> Option<u64> {
        self.first_at.take()
    }

    fn next_tick_at(&self) -> Option<u64> {
        self.second_at.take()
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.first_at.is_none() && !self.ticked {
            self.ticked = true;
            self.second_at.set(Some(hw.bus.clock + FIRST_PERIOD));
        }
        self.rebase(hw);
        self.c("TICK");
        self.fade(hw);
        self.sequence(hw);
        self.voices(hw);
    }
}

impl IffSmus {
    fn voice(&self, ch: usize) -> u32 {
        self.g + G_VOICES + V_SIZE * ch as u32
    }

    /// The original's code for `i` runs once.
    fn c(&mut self, i: &'static str) {
        self.cn(i, 1);
    }

    /// The original's code for `i` runs `n` times.
    fn cn(&mut self, i: &'static str, n: i64) {
        if let Ok(k) = COSTS.binary_search_by(|e| e.0.cmp(i)) {
            self.cyc += COSTS[k].1 * n;
        }
    }

    /// The clock the code has reached: Paula plays up to it.
    fn now(&mut self, hw: &mut Hw) {
        let t = self.seg + (self.cyc.max(0) / 2) as u64;
        if t > hw.bus.clock {
            hw.bus.clock = t;
        }
    }

    /// A host call (a write, `own` its kind): at the clock the cycles since the last reach.
    fn host(&mut self, hw: &mut Hw, own: &'static str) {
        self.c(own);
        self.now(hw);
        self.rebase(hw);
    }

    fn rebase(&mut self, hw: &Hw) {
        self.seg = hw.bus.clock;
        self.cyc = 0;
    }

    /// Lets the time of the code run so far pass, and Paula play up to then
    /// from memory as it is, before the code writes memory Paula reads.
    fn spend(&mut self, hw: &mut Hw) {
        self.now(hw);
        let b = &mut *hw.bus;
        b.paula.sync(b.clock, &b.ram);
    }

    /// The score's start ($10CF6 with start 0, no end, repeat for ever).
    fn start(&mut self, hw: &mut Hw, score: u32) {
        let g = self.g;
        self.stop(hw);
        let b = &mut hw.bus;
        b.w32(g + G_START, 0);
        b.w32(g + G_END, 0xffff_ffff);
        for k in 0..4 {
            let e = b.r32(score + S_ENABLED + 4 * k);
            b.w32(g + G_ENABLED + 4 * k, e);
        }
        // the tracks' start positions ($10D8A, from time 0)
        for k in 0..4 {
            let p = g + G_START_POS + 4 * k;
            b.w32(p, 0);
            b.w8(p + 0x10, 0);
            b.w8(p + 0x11, 0);
            let t = b.r32(score + S_TRACKS + 4 * k);
            if t != 0 {
                b.w32(p, t);
            }
        }
        self.restart(hw, score);
        let b = &mut hw.bus;
        b.w16(g + G_COUNT, 0);
        b.w32(g + G_SCORE, score);
        b.w16(g + G_VOL, 0);
        b.w16(g + G_TEMPO, b.r16(score + 2));
        b.w16(g + G_TUNE, b.r16(score + S_TUNE));
        // $11230: fade from 0 to the score's volume in one step
        let target = b.r16(score);
        b.w16(g + G_FADE_STEP, 0);
        let cur = b.r16(g + G_VOL) << 8;
        b.w16(g + G_FADE_VOL, cur);
        let t = target << 8;
        let (mut d, borrow) = cur.overflowing_sub(t);
        if borrow {
            d = d.wrapping_neg();
        }
        let step = (divu(d as u32, 1) as u16).max(1);
        b.w16(g + G_FADE_TARGET, t);
        b.w16(g + G_FADE_STEP, step);
        b.w16(g + G_REPEAT, 0xffff);
    }

    /// $10D56: releases the tracks' voices if playing.
    fn stop(&mut self, hw: &mut Hw) {
        let g = self.g;
        let b = &mut hw.bus;
        b.w32(g + G_PLAYING, 0);
        let f = b.r8(g + G_FLAGS);
        b.w8(g + G_FLAGS, f & !1);
        self.release_all(hw);
    }

    /// $10D60.
    fn release_all(&mut self, hw: &mut Hw) {
        let g = self.g;
        if hw.bus.r16(g + G_REPEAT) != 0 {
            hw.bus.w16(g + G_REPEAT, 0);
            for t in 0..4 {
                if hw.bus.r32(g + G_ENABLED + 4 * t) != 0 {
                    self.release(hw, t as usize);
                }
            }
        }
    }

    /// The tracks back at their start ($10DF6).
    fn restart(&mut self, hw: &mut Hw, score: u32) {
        let g = self.g;
        for k in 0..8 {
            let v = hw.bus.r32(g + G_START_POS + 4 * k);
            hw.bus.w32(g + G_POS + 4 * k, v);
        }
        let st = hw.bus.r32(g + G_START);
        hw.bus.w32(g + G_TIME, st);
        for t in 0..4u32 {
            let a4 = g + 4 * t;
            if hw.bus.r32(a4 + G_ENABLED) != 0 {
                self.release(hw, t as usize);
            }
            self.c("RESTART_TRACK");
            let b = &mut hw.bus;
            b.w32(a4 + G_HOLD, 0);
            b.w32(a4 + G_RELEASE, b.r8(a4 + G_POS + 0x11) as u32 + 1);
            let mut d = b.r8(a4 + G_POS + 0x10) as u32;
            if d != 0 {
                self.c("RESTART_MAP");
                let b = &mut hw.bus;
                // the original reads the register map 4 bytes on per track
                d = b.r8(score + 4 * t + S_MAP + d - 1) as u32;
                if d != 0 {
                    d = g + G_INSTRS + 6 * (d - 1);
                }
            }
            hw.bus.w32(a4 + G_TRK_INSTR, d);
        }
    }

    /// $111B0: a playing voice released.
    fn release(&mut self, hw: &mut Hw, ch: usize) {
        self.cn("RELEASE", 1);
        let v = self.voice(ch);
        hw.bus.w8(v + V_CMD, 0);
        if hw.bus.r8(v + V_STATUS) == 1 {
            self.c("RELEASE_ON");
            hw.bus.w8(v + V_CMD, 2);
        }
    }

    /// The volume's fade ($1126A).
    fn fade(&mut self, hw: &mut Hw) {
        let g = self.g;
        let b = &mut hw.bus;
        let step = b.r16(g + G_FADE_STEP);
        if step == 0 {
            self.c("FADE_NONE");
            return;
        }
        self.cn("FADE", 1);
        let b = &mut hw.bus;
        let p = step as u32 * b.r16(g + G_CALC) as u32;
        if p & 0x8000_0000 == 0 {
            let d0 = (p << 1 >> 16) as u16;
            let mut d3 = d0;
            let d1 = b.r16(g + G_FADE_VOL);
            let (mut d2, borrow) = b.r16(g + G_FADE_TARGET).overflowing_sub(d1);
            if borrow {
                self.c("FADE_DOWN");
                d3 = d3.wrapping_neg();
                d2 = d2.wrapping_neg();
            }
            if d0 < d2 {
                self.c("FADE_STEP");
                let v = d1.wrapping_add(d3);
                b.w16(g + G_FADE_VOL, v);
                b.w16(g + G_VOL, v >> 8);
                return;
            }
        }
        self.c("FADE_DONE");
        let b = &mut hw.bus;
        let t = b.r16(g + G_FADE_TARGET);
        if t == 0 {
            self.c("FADE_ZERO");
            let b = &mut hw.bus;
            let f = b.r8(g + G_FLAGS);
            b.w8(g + G_FLAGS, f & !1);
            if f & 1 != 0 {
                self.release_all(hw);
            }
        }
        hw.bus.w16(g + G_FADE_STEP, 0);
        hw.bus.w16(g + G_VOL, t >> 8);
    }

    /// The tracks' events ($10EBC): the tempo, then a row when the speed's
    /// count is done.
    fn sequence(&mut self, hw: &mut Hw) {
        let g = self.g;
        let c = hw.bus.r16(g + G_COUNT);
        if c != 0 {
            hw.bus.w16(g + G_COUNT, c - 1);
            if c - 1 != 0 {
                self.c("SEQ_COUNT");
                return self.sequence_end(hw);
            }
            self.c("SEQ_COUNT0");
        }
        self.c("SEQ_TEMPO");
        let b = &mut hw.bus;
        let t = b.r16(g + G_TEMPO) >> 1;
        if t != b.r16(g + G_CUR_TEMPO) {
            self.c("SEQ_NEW_TEMPO");
            let b = &mut hw.bus;
            b.w16(g + G_CUR_TEMPO, t);
            let tempo = TEMPO[t as usize & 0x7f];
            let speed = tempo >> 12;
            b.w16(g + G_SPEED, speed);
            let calc = divu((tempo as u32) << 15, speed << 12) as u16;
            b.w16(g + G_CALC, calc);
            self.timer = ((calc as u32 * TIMER as u32) >> 15) as u16;
            // dtg_SetTimer
            self.host(hw, "SETTIMER");
        }
        let b = &mut hw.bus;
        if b.r16(g + G_REPEAT) == 0 {
            self.c("SEQ_STOPPED");
            return self.sequence_end(hw);
        }
        b.w16(g + G_COUNT, b.r16(g + G_SPEED));
        self.cn("ROW", 1);
        let b = &mut hw.bus;
        let score = b.r32(g + G_SCORE);
        let mut done = 0u16;
        loop {
            for t in 0..4u32 {
                self.track(hw, score, t, &mut done);
            }
            let b = &mut hw.bus;
            let time = b.r32(g + G_TIME);
            b.w32(g + G_TIME, time.wrapping_add(1));
            let end = b.r32(g + G_END);
            let ended = if (end as i32) >= 0 { time == end } else { done == 4 };
            if !ended {
                self.c(if (end as i32) >= 0 { "ROW_END_TIME" } else { "ROW_END_DONE" });
                break;
            }
            self.c("SONG_END");
            self.songend = true;
            self.host(hw, "SONGEND");
            let b = &mut hw.bus;
            let r = b.r16(g + G_REPEAT);
            if (r as i16) >= 0 {
                self.c("REPEAT_COUNT");
                let b = &mut hw.bus;
                b.w16(g + G_REPEAT, r.wrapping_sub(1));
                if r.wrapping_sub(1) == 0 {
                    break;
                }
            }
            self.c("RESTART");
            self.restart(hw, score);
            done += 1;
        }
        self.sequence_end(hw);
    }

    fn sequence_end(&mut self, hw: &mut Hw) {
        if hw.bus.r16(self.g + G_REPEAT) == 0 {
            self.c("SEQ_END_STOP");
            hw.bus.w32(self.g + G_PLAYING, 0);
        }
    }

    /// One track's row: its note's hold and release, then its next events.
    fn track(&mut self, hw: &mut Hw, score: u32, t: u32, done: &mut u16) {
        let a5 = self.g + 4 * t;
        self.cn("TRACK_WAIT", 1);
        let b = &mut hw.bus;
        let hold = b.r32(a5 + G_HOLD);
        if hold != 0 {
            b.w32(a5 + G_HOLD, hold - 1);
            if hold - 1 == 0 {
                self.c("HOLD_DONE");
                if hw.bus.r32(a5 + G_ENABLED) != 0 {
                    self.release(hw, t as usize);
                }
            } else {
                self.c("HOLD");
            }
            return;
        }
        let rel = b.r32(a5 + G_RELEASE);
        if rel != 0 {
            b.w32(a5 + G_RELEASE, rel - 1);
            if rel - 1 != 0 {
                self.c("REL");
                return;
            }
            self.c("REL_DONE");
        }
        self.cn("TRACK_READ", 1);
        let b = &mut hw.bus;
        let mut p = b.r32(a5 + G_POS);
        if p == 0 {
            self.c("TRACK_NONE");
            *done += 1;
            return;
        }
        loop {
            self.cn("WORD", 1);
            let b = &mut hw.bus;
            let w = b.r16(p);
            p += 2;
            if w == 0 {
                self.c("EV_ZERO");
                continue;
            }
            if w == 0xffff {
                self.c("EV_END");
                *done += 1;
                return;
            }
            b.w32(a5 + G_POS, p);
            let mut len = w & 0xff;
            let ev = (w >> 8) as u8;
            if ev & 0x80 == 0 {
                // a note
                self.c("EV_NOTE");
                let b = &mut hw.bus;
                let mut e = b.r32(a5 + G_TRK_INSTR);
                if e == 0 {
                    self.c("EV_DEFAULT");
                    e = hw.bus.r32(score + S_DEFAULT_INSTR);
                }
                let b = &mut hw.bus;
                if b.r32(a5 + G_ENABLED) != 0 && e != 0 {
                    let note = ((b.r16(score + S_TRANSPOSE) as i16 >> 4) as u16).wrapping_sub(8).wrapping_add(ev as u16);
                    let mut vol = b.r16(score + 4 * t + S_TRK_VOL);
                    if b.r16(a5 + G_ENABLED + 2) != 1 {
                        self.c("NOTE_HALF");
                        vol >>= 1;
                    }
                    self.cn("NOTE", 1);
                    self.note_on(hw, t as usize, e, note, vol);
                    let h = ((len as u32 * 0xc000) >> 16) as u16;
                    hw.bus.w16(a5 + G_HOLD + 2, h);
                    len -= h;
                }
                hw.bus.w16(a5 + G_RELEASE + 2, len);
                return;
            }
            if ev == 0x80 {
                self.c("EV_REST");
                b.w16(a5 + G_RELEASE + 2, len);
                return;
            }
            if ev == 0x81 {
                self.cn("INSTR_EV", 1);
                let b = &mut hw.bus;
                let r = b.r8(score + S_MAP + len as u32) as u32;
                let e = if r == 0 { 0 } else { self.g + G_INSTRS + 6 * ((r - 1) & 0xff) };
                b.w32(a5 + G_TRK_INSTR, e);
                if r == 0 {
                    self.c("INSTR_NONE");
                }
            } else {
                self.c("EV_OTHER");
            }
        }
    }

    /// A note on a voice ($11108): a voice that sounds an instrument of another
    /// type is muted first.
    fn note_on(&mut self, hw: &mut Hw, ch: usize, e: u32, note: u16, vol: u16) {
        let ins = hw.bus.r32(e);
        if ins == 0 {
            self.c("NOTE_NO_INSTR");
            return;
        }
        let v = self.voice(ch);
        if hw.bus.r8(v + V_STATUS) != 0 {
            self.c("NOTE_SOUNDING");
            let old = hw.bus.r32(v + V_INSTR);
            if hw.bus.r32(ins) != hw.bus.r32(old) {
                self.c("NOTE_OTHER_TYPE");
                self.c(match hw.bus.r32(old) {
                    TYPE_SYNTH => "MUTEV_OLD_SYNTH",
                    TYPE_SAMPLED => "MUTEV_OLD_SAMPLED",
                    _ => "MUTEV_OLD_FORM",
                });
                self.mute_voice(hw, ch);
            }
        }
        let b = &mut hw.bus;
        b.w32(v + V_INSTR, ins);
        b.w8(v + V_NOTE, note as u8);
        b.w16(v + V_VOLUME, vol & 0xff);
        b.w8(v + V_CMD, 1);
    }

    /// $111EA: a sounding voice muted by its instrument.
    fn mute_voice(&mut self, hw: &mut Hw, ch: usize) {
        let v = self.voice(ch);
        hw.bus.w8(v + V_CMD, 0);
        if hw.bus.r8(v + V_STATUS) != 0 {
            self.c("MUTEV_ON");
            hw.bus.w8(v + V_CMD, 3);
            if hw.bus.r32(v + V_INSTR) != 0 {
                self.mute(hw, ch);
            }
            hw.bus.w8(v + V_STATUS, 0);
            hw.bus.w8(v + V_CMD, 0);
        }
    }

    /// $110E2: the voice's DMA off, and period 2.
    fn mute(&mut self, hw: &mut Hw, ch: usize) {
        self.host(hw, "MUTE");
        hw.dmacon(1 << ch);
        self.host(hw, "MUTE_PER");
        hw.per(ch, 2);
    }

    /// The voices ($1103A): each instrument's setup, DMA off for the voices that
    /// start a sample, each sounding voice's writes, DMA on for the new notes.
    fn voices(&mut self, hw: &mut Hw) {
        let mut off = 0u16;
        for ch in 0..4 {
            let v = self.voice(ch);
            let mut r_off = false;
            if hw.bus.r8(v + V_CMD) == 0 && hw.bus.r8(v + V_STATUS) == 0 {
                self.cn("VOICE_IDLE", 1);
            } else {
                let ins = hw.bus.r32(v + V_INSTR);
                if ins == 0 {
                    self.c("VOICE_NO_INSTR");
                    hw.bus.w8(v + V_STATUS, 0);
                    hw.bus.w8(v + V_CMD, 0);
                    continue;
                }
                let start = hw.bus.r8(v + V_CMD) == 1;
                let r = match hw.bus.r32(ins) {
                    TYPE_SYNTH => {
                        self.cn("SYNTH", 1);
                        self.cn("SYNTH_START", start as i64);
                        self.synth_setup(hw, ch, ins)
                    }
                    TYPE_SAMPLED => {
                        self.cn("SAMPLED", 1);
                        self.cn("SAMPLED_START", start as i64);
                        self.sampled_setup(hw, ch, ins)
                    }
                    _ => {
                        self.cn("FORM", 1);
                        self.cn("FORM_START", start as i64);
                        self.form_setup(hw, ch, ins)
                    }
                };
                if r {
                    off |= 1 << ch;
                }
                r_off = r;
            }
            let b = &mut hw.bus;
            match b.r8(v + V_CMD) {
                1 => {
                    b.w8(v + V_STATUS, 1);
                    self.c("VOICE_CMD1");
                }
                2 => {
                    b.w8(v + V_STATUS, 2);
                    self.c("VOICE_CMD2");
                }
                0 => self.c("VOICE_CMD0"),
                _ => self.c("VOICE_CMDX"),
            }
            if r_off {
                self.c("VOICE_OFF");
            }
            hw.bus.w8(v + V_CMD, 0);
        }
        self.host(hw, "DMA_OFF");
        hw.dmacon(off);
        let mut on = 0x8000u16;
        for ch in 0..4 {
            let v = self.voice(ch);
            if hw.bus.r8(v + V_STATUS) == 0 {
                self.cn("PLAY_SKIP", 1);
                continue;
            }
            let ins = hw.bus.r32(v + V_INSTR);
            let r = match hw.bus.r32(ins) {
                TYPE_SYNTH => self.synth_play(hw, ch),
                _ => self.sample_play(hw, ch, if hw.bus.r32(ins) == TYPE_SAMPLED { G_SAMPLED } else { G_IFF }),
            };
            if r {
                self.c("PLAY_ON");
                on |= 1 << ch;
            }
        }
        self.host(hw, "DMA_ON");
        hw.dmacon(on);
        // ENPP_DMAMask waits for the DMA as dtg_WaitAudioDMA does: 10 lines on
        let now = hw.bus.clock;
        hw.wait((now / LINE + 10) * LINE - now);
        self.rebase(hw);
        // then ENPP_Amplifier (no write)
        self.host(hw, "AMP");
    }

    /// The envelope's step toward its level at a rate ($115CA, $11C76).
    fn envelope(&mut self, hw: &mut Hw, levels: u32, rates: u32, step: u32, env: u32) {
        let calc = hw.bus.r16(self.g + G_CALC) as u32;
        let b = &mut hw.bus;
        let i = b.r16(step) as i16 as i32 as u32;
        let level = (b.r16(levels.wrapping_add(i)) as u32) << 16;
        let mut d2 = b.r32(env);
        let rate = b.r16(rates.wrapping_add(i));
        let sh = ((rate >> 5) ^ 7) as u32;
        let d3 = lsr_l((((rate & 0x1f) + 0x21) as u32 * calc) << 3, sh);
        if (level as i32).wrapping_sub(d2 as i32) < 0 {
            self.c("ENV_NEG");
        }
        let d0 = (level as i32).wrapping_sub(d2 as i32).wrapping_abs();
        if d0 <= d3 as i32 {
            self.c("ENV_LEVEL");
            d2 = level;
            let s = b.r16(step);
            if (s as i16) < 4 {
                self.c("ENV_NEXT");
                b.w16(step, s + 2);
            }
        } else if (d2 as i32) < (level as i32) {
            self.c("ENV_UP");
            d2 = d2.wrapping_add(d3);
        } else {
            self.c("ENV_DOWN");
            d2 = d2.wrapping_sub(d3);
        }
        b.w32(env, d2);
    }

    /// A synthesis voice's tick ($11476): the note's start, the LFO, envelope
    /// and portamento, its period and volume, and its waveform for this tick,
    /// filtered and phased, in the voice's other buffer.
    fn synth_setup(&mut self, hw: &mut Hw, ch: usize, a3: u32) -> bool {
        let g = self.g;
        let v = self.voice(ch);
        let a2 = g + G_SYNTH + 0x1a * ch as u32;
        let calc = hw.bus.r16(g + G_CALC);
        let b = &mut hw.bus;
        match b.r8(v + V_CMD) {
            0 => self.c("SY_CMD0"),
            1 => {
                let n = b.r8(v + V_NOTE) as u16;
                if !(0x24..0x6c).contains(&n) {
                    self.c("SY_RANGE");
                    b.w8(v + V_CMD, 0);
                    if b.r8(v + V_STATUS) == 0 {
                        self.c("SY_RANGE_OFF");
                        return false;
                    }
                } else {
                    self.c("SY_START");
                    let n = n - 0x24;
                    let st = b.r8(v + V_STATUS);
                    if st == 0 {
                        self.c("SY_ST0");
                        b.w32(a2 + SV_ENV, 0);
                    }
                    if st != 1 {
                        self.c("SY_ST_NOT1");
                        b.w16(a2 + SV_ENV_STEP, 0);
                    }
                    let (oct, rem) = (n / 12, n % 12);
                    let mut note = ((0xd5c8 * NOTE[rem as usize] as u32) >> (oct + 0x11)) as u16;
                    let old = b.r16(a2 + SV_NOTE);
                    if old == 0 {
                        self.c("SY_PORT0");
                        b.w16(a2 + SV_PORT_COUNT, 0);
                    } else {
                        self.c("SY_PORT");
                        let diff = note.wrapping_sub(old) as i16 as i32;
                        let q = divu((b.r16(a3 + Y_PORT) as u32) << 15, calc) as u16;
                        let count = (q >> 3).wrapping_add(1);
                        b.w16(a2 + SV_PORT_COUNT, count);
                        let speed = divs(diff, count as i16) as u16;
                        b.w16(a2 + SV_PORT_STEP, speed);
                        note = note.wrapping_sub((speed as u32 * count as u32) as u16);
                    }
                    b.w16(a2 + SV_NOTE, note);
                    b.w16(a2 + SV_PHASE_DIR, 1);
                    if b.r16(a3 + Y_PHASE_SPEED) == 0 {
                        self.c("SY_PHASE0");
                        b.w16(a2 + SV_PHASE, 0);
                    }
                    b.w16(a2 + SV_LFO_WAIT, 0);
                    if b.r16(a3 + Y_LFO_MODE) != 0 {
                        self.c("SY_LFO_MODE");
                        b.w16(a2 + SV_LFO_POS, 0);
                        let w = (divu((b.r16(a3 + Y_LFO_DELAY) as u32) << 15, calc) as u16) >> 2;
                        b.w16(a2 + SV_LFO_WAIT, w);
                        b.w16(a2 + SV_LFO, b.r8(a3 + Y_LFO) as i8 as i16 as u16);
                    }
                    b.w16(v + V_START, 0xffff);
                }
            }
            2 => {
                self.c("SY_REL");
                b.w16(a2 + SV_ENV_STEP, 6);
            }
            3 => {
                self.c("SY_MUTE");
                self.mute(hw, ch);
                return false;
            }
            _ => self.c("SY_CMDX"),
        }
        // the LFO
        let b = &mut hw.bus;
        let wait = b.r16(a2 + SV_LFO_WAIT) as i16;
        if wait > 0 {
            self.c("LFO_WAIT");
            b.w16(a2 + SV_LFO_WAIT, (wait - 1) as u16);
        } else if wait == 0 {
            self.c("LFO_STEP");
            let inc = ((b.r16(a3 + Y_LFO_SPEED) as u32 * calc as u32) << 6 >> 16) as u16;
            let (pos, carry) = b.r16(a2 + SV_LFO_POS).overflowing_add(inc);
            let mode = b.r16(a3 + Y_LFO_MODE) as i16;
            self.c(match (carry, mode.signum()) {
                (false, 1) => "LFO_NC_POS",
                (false, 0) => "LFO_NC_ZERO",
                (false, _) => "LFO_NC_NEG",
                (true, 1) => "LFO_C_POS",
                (true, 0) => "LFO_C_ZERO",
                (true, _) => "LFO_C_NEG",
            });
            if carry && (b.r16(a3 + Y_LFO_MODE) as i16) > 0 {
                self.c("LFO_STOP");
                b.w16(a2 + SV_LFO_WAIT, 0xffff);
            } else {
                if carry {
                    self.c("LFO_CARRY");
                }
                b.w16(a2 + SV_LFO_POS, pos);
                b.w16(a2 + SV_LFO, b.r8(a3 + Y_LFO + (pos >> 8) as u32) as i8 as i16 as u16);
            }
        } else {
            self.c("LFO_OFF");
        }
        self.envelope(hw, a3 + Y_LEVELS, a3 + Y_RATES, a2 + SV_ENV_STEP, a2 + SV_ENV);
        // the portamento and the octave
        let b = &mut hw.bus;
        let mut d0 = b.r16(a2 + SV_NOTE);
        let mut oct: i16 = 5;
        let pc = b.r16(a2 + SV_PORT_COUNT);
        if pc != 0 {
            self.c("PORT");
            b.w16(a2 + SV_PORT_COUNT, pc - 1);
            d0 = d0.wrapping_add(b.r16(a2 + SV_PORT_STEP));
            b.w16(a2 + SV_NOTE, d0);
        }
        while (d0 as i16) > 0x1ac {
            d0 >>= 1;
            oct -= 1;
            self.c("OCTAVE");
        }
        let oct = oct as u16;
        b.w16(a2 + SV_OCTAVE, oct);
        let len = lsr_w(0x40, oct);
        b.w16(v + V_LEN, len);
        let lfo = b.r16(a2 + SV_LFO);
        let tune = b.r16(g + G_TUNE).wrapping_sub(0x80);
        let d1 = muls_asr(lfo, b.r16(a3 + Y_FREQ_LFO), 7).wrapping_sub(tune).wrapping_add(0x1000);
        b.w16(v + V_PER, ((d0 as u32 * d1 as u32) >> 12) as u16);
        // the volume
        let env = (b.r32(a2 + SV_ENV) >> 16) as u16;
        let mut vol = b.r16(a3 + Y_AMP_VOL).wrapping_add(muls_asr(lfo.wrapping_neg(), b.r16(a3 + Y_AMP_LFO), 8));
        if b.r16(a3 + Y_AMP_ENV) != 0 {
            self.c("AMP_ENV");
            vol = (vol as u32 * env as u32) as u16 >> 8;
        } else if b.r16(a2 + SV_ENV_STEP) == 6 {
            self.c("AMP_REL0");
            vol = 0;
        }
        let vol = (vol & 0xff) + 1;
        let vol = ((vol as u32 * b.r16(g + G_VOL) as u32) as u16 >> 8) + 1;
        let vol = ((vol as u32 * b.r16(v + V_VOLUME) as u32) as u16 >> 8) + 1;
        b.w16(v + V_VOL, vol >> 2);
        // the filter picks one of the 64 waveforms
        let f = (env as u32 * b.r16(a3 + Y_FILTER_ENV) as u32) as u16 >> 8;
        let f = (b.r16(a3 + Y_FILTER) ^ 0xff).wrapping_sub(f).wrapping_add(muls_asr(lfo, b.r16(a3 + Y_FILTER_LFO), 8));
        let mut off = ((f & 0xff) >> 2) << 7;
        let mut src = b.r32(a3 + Y_TABLES);
        if src == 0 {
            self.c("SY_OSC");
            src = a3 + Y_OSC;
            off = 0;
        }
        let mut src = src + off as u32;
        // the waveform is written when the original writes it, as Paula may
        // still play this buffer
        self.spend(hw);
        let b = &mut hw.bus;
        let half = b.r16(a2 + SV_BUFFER) ^ 0x80;
        b.w16(a2 + SV_BUFFER, half);
        let mut dst = b.r32(g + G_WAVES) + half as u32 + 0x100 * ch as u32;
        b.w32(v + V_LOC, dst);
        let step = (1u32 << (oct & 31)) as u16 as i16 as i32 as u32;
        let ps = b.r16(a3 + Y_PHASE_SPEED);
        let pd = b.r16(a3 + Y_PHASE_DEPTH);
        let mut bytes = 0u32;
        if ps == 0 {
            // every 2^octave-th byte
            let mut n = lsr_w(0x80, oct);
            loop {
                b.w8(dst, b.r8(src));
                dst += 1;
                src = src.wrapping_add(step);
                bytes += 1;
                n = n.wrapping_sub(1);
                if n == 0 {
                    break;
                }
            }
            self.cn("PLAIN", bytes as i64);
            self.c("PLAIN_SETUP");
        } else if pd == 0 {
            // mixed with itself shifted by the phase
            let mut d2 = len.wrapping_mul(2).wrapping_sub(1);
            let ph = (((ps as u32 * calc as u32) >> 13) as u16).wrapping_add(b.r16(a2 + SV_PHASE));
            b.w16(a2 + SV_PHASE, ph);
            let sh = ph >> 9;
            let mut a5 = src.wrapping_add(sh as i16 as i32 as u32);
            let mut d4 = lsr_w(sh, oct);
            d2 = d2.wrapping_sub(d4);
            let mix = |b: &crate::bus::Bus, x: u32, y: u32| ((b.r8(x) as i8 as i16 + b.r8(y) as i8 as i16) >> 1) as u8;
            loop {
                let m = mix(b, src, a5);
                b.w8(dst, m);
                dst += 1;
                src = src.wrapping_add(step);
                a5 = a5.wrapping_add(step);
                bytes += 1;
                d2 = d2.wrapping_sub(1);
                if d2 == 0xffff {
                    break;
                }
            }
            a5 = a5.wrapping_sub(0x80);
            d4 = d4.wrapping_sub(1);
            self.c("MIX_SETUP");
            self.cn("MIX", bytes as i64);
            bytes = 0;
            if (d4 as i16) >= 0 {
                self.c("MIX_WRAP");
                loop {
                    let m = mix(b, src, a5);
                    b.w8(dst, m);
                    dst += 1;
                    src = src.wrapping_add(step);
                    a5 = a5.wrapping_add(step);
                    bytes += 1;
                    d4 = d4.wrapping_sub(1);
                    if d4 == 0xffff {
                        break;
                    }
                }
            }
            self.cn("MIX2", bytes as i64);
        } else {
            // the waveform's halves squeezed and stretched by the phase
            let dir = b.r16(a2 + SV_PHASE_DIR);
            let d0 = ((ps as u32 * calc as u32) >> 11) as u16;
            let d0 = (d0 as i16 as i32).wrapping_mul(dir as i16 as i32) as u16;
            let ph = b.r16(a2 + SV_PHASE);
            let (mut s, ov) = (d0 as i16).overflowing_add(ph as i16);
            self.c("PH_SETUP");
            if ov {
                self.c("PH_OV");
                if s as u16 == 0x8000 {
                    self.c("PH_8000");
                    s = (s as u16).wrapping_add(dir) as i16;
                }
                b.w16(a2 + SV_PHASE_DIR, dir.wrapping_neg());
                s = s.wrapping_neg();
            }
            b.w16(a2 + SV_PHASE, s as u16);
            let d = asr_l((s as i32).wrapping_mul(pd as i16 as i32), 0x11 + oct as u32) as u16;
            let l1 = len.wrapping_add(d);
            let l2 = len.wrapping_sub(d);
            for (start, l) in [(0u16, l1), (0x40, l2)] {
                if l == 0 {
                    self.c("PH_SEG0");
                    continue;
                }
                self.c("PHASE_SEG");
                let qr = divu(0x40, l);
                let (q, r) = (qr as u16, (qr >> 16) as u16);
                let (mut i, mut acc, mut n) = (start, 0u16, l);
                loop {
                    b.w8(dst, b.r8(src.wrapping_add(i as i16 as i32 as u32)));
                    dst += 1;
                    bytes += 1;
                    let (a, borrow) = acc.overflowing_sub(r);
                    acc = a;
                    let mut x = borrow as u16;
                    if borrow {
                        self.c("PH_BORROW");
                        let (a, c) = acc.overflowing_add(l);
                        acc = a;
                        x = c as u16;
                    }
                    i = i.wrapping_add(q).wrapping_add(x);
                    n = n.wrapping_sub(1);
                    if n == 0 {
                        break;
                    }
                }
            }
            self.cn("PHASE", bytes as i64);
        }
        false
    }

    /// $11444: location, length, period and volume; DMA on for a new note.
    fn synth_play(&mut self, hw: &mut Hw, ch: usize) -> bool {
        let v = self.voice(ch);
        self.host(hw, "SYN_LOC");
        hw.loc(ch, hw.bus.r32(v + V_LOC));
        self.host(hw, "SYN_LEN");
        hw.len(ch, hw.bus.r16(v + V_LEN));
        self.host(hw, "SYN_PER");
        hw.per(ch, hw.bus.r16(v + V_PER));
        self.host(hw, "SYN_VOL");
        hw.vol(ch, hw.bus.r16(v + V_VOL));
        let s = hw.bus.r16(v + V_START);
        hw.bus.w16(v + V_START, 0);
        self.cn("SYN_RET", 1);
        s != 0
    }

    /// A sampled sound's tick ($11B18): the note's start (the octave's part of
    /// the sample, its loop, the vibrato's delay, the voice muted), the vibrato
    /// and envelope, period and volume.
    fn sampled_setup(&mut self, hw: &mut Hw, ch: usize, a3: u32) -> bool {
        let g = self.g;
        let v = self.voice(ch);
        let a2 = g + G_SAMPLED + 0x14 * ch as u32;
        let calc = hw.bus.r16(g + G_CALC);
        let b = &mut hw.bus;
        match b.r8(v + V_CMD) {
            0 => self.c("SM_CMD0"),
            1 => {
                let a5 = b.r32(a3 + P_SAMPLE);
                let n = b.r8(v + V_NOTE) as u16;
                let (oct, rem) = (n / 12, n % 12);
                let d2 = 10u16.wrapping_sub(oct);
                if (d2 as i8) > (b.r8(a5 + F_LAST) as i8) || (d2 as i8) < (b.r8(a5 + F_FIRST) as i8) {
                    self.c(if (d2 as i8) > (b.r8(a5 + F_LAST) as i8) { "SM_RANGE_HI" } else { "SM_RANGE_LO" });
                    b.w8(v + V_CMD, 0);
                    if b.r8(v + V_STATUS) == 0 {
                        self.c("SM_RANGE_OFF");
                        return b.r16(v + V_START) == 2;
                    }
                } else {
                    self.c("SM_START");
                    let st = b.r8(v + V_STATUS);
                    if st == 0 {
                        self.c("SM_ST0");
                        b.w32(a2 + PV_ENV, 0);
                    }
                    if st != 1 {
                        self.c("SM_ST_NOT1");
                        b.w16(a2 + PV_ENV_STEP, 0);
                    }
                    b.w16(a2 + PV_NOTE, ((0xd5c8 * NOTE[rem as usize] as u32) >> 15) as u16);
                    let d4 = asl_l(1, d2 as u32);
                    let d0 = d4.wrapping_sub(asl_l(1, b.r8(a5 + F_FIRST) as u32));
                    let len1 = b.r16(a5) as u32;
                    let a0 = a5.wrapping_add(F_DATA).wrapping_add((d0 & 0xffff) * len1);
                    b.w32(v + V_LOC, a0);
                    b.w16(v + V_LEN, ((len1 * (d4 & 0xffff)) >> 1) as u16);
                    let (mut la, mut ll) = (b.r32(g + G_WAVES) + 0x400, 4u16);
                    let lp = b.r16(a5 + F_LOOP);
                    if lp as u32 != len1 {
                        self.c("SM_LOOP");
                        la = a0.wrapping_add(lp as u32 * (d4 & 0xffff));
                        ll = (((len1 as u16).wrapping_sub(lp) as u32 * (d4 & 0xffff)) >> 1) as u16;
                    }
                    b.w32(a2 + PV_LOOP, la);
                    b.w16(a2 + PV_LOOP_LEN, ll);
                    b.w16(a2 + PV_VIB_POS, 0);
                    let w = (divu((b.r16(a3 + P_VIB_DELAY) as u32) << 15, calc) as u16) >> 1;
                    b.w16(a2 + PV_VIB_WAIT, w);
                    b.w16(v + V_START, 2);
                    self.mute(hw, ch);
                }
            }
            2 => {
                self.c("SM_REL");
                b.w16(a2 + PV_ENV_STEP, 6);
            }
            3 => {
                self.c("SM_MUTE");
                self.mute(hw, ch);
                return hw.bus.r16(v + V_START) == 2;
            }
            _ => self.c("SM_CMDX"),
        }
        // the vibrato
        let b = &mut hw.bus;
        let w = b.r16(a2 + PV_VIB_WAIT);
        if w != 0 {
            self.c("VIB_WAIT");
            b.w16(a2 + PV_VIB_WAIT, w - 1);
        } else {
            self.c("VIB_STEP");
            let inc = (((b.r16(a3 + P_VIB_SPEED) as u32 * calc as u32) << 7 >> 16) as u16).wrapping_add(0x40);
            b.w16(a2 + PV_VIB_POS, b.r16(a2 + PV_VIB_POS).wrapping_add(inc));
        }
        let mut d0 = (b.r16(a2 + PV_VIB_POS) >> 7) + 0x80;
        if d0 & 0x100 != 0 {
            self.c("VIB_NEG");
            d0 ^= 0xff;
        }
        d0 ^= 0x80;
        let vib = (d0 as u8 as i8 as i16).wrapping_neg() as u16;
        b.w16(a2 + PV_VIB, vib);
        self.envelope(hw, a3 + P_LEVELS, a3 + P_RATES, a2 + PV_ENV_STEP, a2 + PV_ENV);
        let b = &mut hw.bus;
        let tune = b.r16(g + G_TUNE).wrapping_sub(0x80);
        let d1 = muls_asr(vib, b.r16(a3 + P_VIB_DEPTH), 7).wrapping_sub(tune).wrapping_add(0x1000);
        b.w16(v + V_PER, ((b.r16(a2 + PV_NOTE) as u32 * d1 as u32) >> 19) as u16);
        let vol = ((b.r16(g + G_VOL).wrapping_add(1) as u32 * b.r16(v + V_VOLUME) as u32) as u16 >> 8) + 1;
        let vol = (vol as u32 * b.r16(a3 + P_VOLUME) as u32) as u16 >> 8;
        let vol = (vol as u32 * (b.r32(a2 + PV_ENV) >> 16)) as u16 >> 10;
        b.w16(v + V_VOL, vol);
        b.r16(v + V_START) == 2
    }

    /// An IFF sample's tick ($11F0A): the note's start (the octave's part of the
    /// sample, its loop, the voice muted), the period and volume.
    fn form_setup(&mut self, hw: &mut Hw, ch: usize, a3: u32) -> bool {
        let g = self.g;
        let v = self.voice(ch);
        let a2 = g + G_IFF + 10 * ch as u32;
        let b = &mut hw.bus;
        match b.r8(v + V_CMD) {
            0 => self.c("FM_CMD0"),
            1 => {
                let a5 = b.r32(a3 + X_BODY);
                let n = b.r8(v + V_NOTE) as u16;
                let (oct, rem) = (n / 12, n % 12);
                b.w16(a2 + XV_NOTE, ((0xd5c8 * NOTE[rem as usize] as u32) >> 15) as u16);
                let d2 = 10u16.wrapping_sub(oct).wrapping_sub(b.r16(a3 + X_SKIP));
                if (d2 as i16) < 0 || (d2 as i8) >= (b.r8(a3 + X_OCTAVES) as i8) {
                    self.c(if (d2 as i16) < 0 { "FM_RANGE_LO" } else { "FM_RANGE_HI" });
                    b.w8(v + V_CMD, 0);
                    if b.r8(v + V_STATUS) == 0 {
                        self.c("FM_RANGE_OFF");
                        return b.r16(v + V_START) == 2;
                    }
                } else {
                    self.c("FM_START");
                    let (one, rep) = (b.r32(a3 + X_ONE_SHOT), b.r32(a3 + X_REPEAT));
                    let all = one.wrapping_add(rep);
                    let a0 = a5.wrapping_add(asl_l(all, d2 as u32).wrapping_sub(all));
                    let (one, rep) = (asl_l(one, d2 as u32) as u16, asl_l(rep, d2 as u32) as u16);
                    b.w32(v + V_LOC, a0);
                    b.w16(v + V_LEN, if one != 0 { one } else { rep } >> 1);
                    if one == 0 {
                        self.c("FM_NO_ONE");
                    }
                    if rep != 0 {
                        self.c("FM_REP");
                    }
                    let (la, ll) = if rep != 0 {
                        (a0.wrapping_add(one as i16 as i32 as u32), rep)
                    } else {
                        (b.r32(g + G_WAVES) + 0x400, 8)
                    };
                    b.w32(a2 + XV_LOOP, la);
                    b.w16(a2 + XV_LOOP_LEN, ll >> 1);
                    b.w16(v + V_START, 2);
                    self.mute(hw, ch);
                    hw.bus.w16(a2 + XV_ON, 1);
                }
            }
            2 => {
                self.c("FM_REL");
                b.w16(a2 + XV_ON, 0);
            }
            3 => {
                self.c("FM_MUTE");
                self.mute(hw, ch);
                return hw.bus.r16(v + V_START) == 2;
            }
            _ => self.c("FM_CMDX"),
        }
        let b = &mut hw.bus;
        let tune = b.r16(g + G_TUNE);
        b.w16(v + V_PER, ((0x1080u16.wrapping_sub(tune) as u32 * b.r16(a2 + XV_NOTE) as u32) >> 19) as u16);
        let vol = ((b.r16(g + G_VOL).wrapping_add(1) as u32 * b.r16(v + V_VOLUME) as u32) as u16 >> 8) + 1;
        let vol = (vol as u32 * ((b.r32(a3 + X_VOLUME) >> 1) & 0xffff)) >> 17;
        b.w16(v + V_VOL, ((vol & 0xffff) * b.r16(a2 + XV_ON) as u32) as u16);
        b.r16(v + V_START) == 2
    }

    /// A sample's writes ($11AB6, $11EC2): on a note's tick its length and
    /// location, on the next its loop's, then period and volume.
    fn sample_play(&mut self, hw: &mut Hw, ch: usize, kind: u32) -> bool {
        let v = self.voice(ch);
        let form = kind == G_IFF;
        self.c("SP_ENTRY");
        let s = hw.bus.r16(v + V_START);
        let mut on = false;
        if s != 0 {
            hw.bus.w16(v + V_START, s - 1);
            let (len, loc) = if s - 1 != 0 {
                self.c("SP_FIRST");
                on = true;
                (hw.bus.r16(v + V_LEN), hw.bus.r32(v + V_LOC))
            } else {
                self.c("SP_LOOP");
                let l = self.g + kind + if kind == G_SAMPLED { 0x14 } else { 10 } * ch as u32;
                (hw.bus.r16(l + 4), hw.bus.r32(l))
            };
            self.host(hw, if form { "FORM_LEN" } else { "SMP_LEN" });
            hw.len(ch, len);
            self.c(if on { "SP_PRE_LOC_FIRST" } else { "SP_PRE_LOC_LOOP" });
            self.host(hw, if form { "FORM_LOC" } else { "SMP_LOC" });
            hw.loc(ch, loc);
            self.c(if on { "SP_POST_FIRST" } else { "SP_POST_LOOP" });
        }
        self.host(hw, if form { "FORM_PER" } else { "SMP_PER" });
        hw.per(ch, hw.bus.r16(v + V_PER));
        self.host(hw, if form { "FORM_VOL" } else { "SMP_VOL" });
        hw.vol(ch, hw.bus.r16(v + V_VOL));
        self.c(if form { "FORM_RET" } else { "SMP_RET" });
        on
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use std::path::{Path, PathBuf};

    fn chunk(d: &mut Vec<u8>, id: &[u8], body: &[u8]) {
        d.extend(id);
        d.extend((body.len() as u32).to_be_bytes());
        d.extend(body);
        if body.len() & 1 != 0 {
            d.push(0);
        }
    }

    /// A score of one track: instrument register 0 ("Test"), a half note 60,
    /// a rest; the instrument a sampled sound whose sample file has one octave
    /// of 4 words a period, no loop.
    fn files(dir: &Path) -> PathBuf {
        let ins = dir.join("Instruments");
        std::fs::create_dir_all(&ins).unwrap();
        let mut body = Vec::new();
        // tempo $E10 (index 0), volume $7F, 1 track
        chunk(&mut body, b"SHDR", &[0x0e, 0x10, 0x7f, 1]);
        chunk(&mut body, b"INS1", b"\x00\x00\x00\x00Test");
        // instrument 0, note 60 for a half (duration 1), a rest as long
        chunk(&mut body, b"TRAK", &[0x81, 0x00, 0x3c, 0x01, 0x80, 0x01]);
        let mut score = b"FORM".to_vec();
        score.extend((body.len() as u32 + 4).to_be_bytes());
        score.extend(b"SMUS");
        score.extend(body);
        let path = dir.join("test.smus");
        std::fs::write(&path, &score).unwrap();
        // the instrument file: its type, then from $20 the sampled sound's
        // fields at their offsets less $20 (the sample's name at $24, the
        // volume at $48, the envelope's levels at $4A and rates at $52)
        let mut instr = vec![0u8; 0x80];
        instr[..12].copy_from_slice(b"SampledSound");
        instr[0x44..0x48].copy_from_slice(b"wave");
        instr[0x68..0x6a].copy_from_slice(&0x00ffu16.to_be_bytes());
        for k in 0..4 {
            instr[0x6a + 2 * k..0x6c + 2 * k].copy_from_slice(&0x00ffu16.to_be_bytes());
            instr[0x72 + 2 * k..0x74 + 2 * k].copy_from_slice(&0x00ffu16.to_be_bytes());
        }
        // in another case, which the Amiga's file systems ignore
        std::fs::write(ins.join("TEST.instr"), &instr).unwrap();
        // the sample: 4 words an octave period and its loop the same (none),
        // octaves 6 to 6, the data from $3E
        let mut ss = vec![0u8; 0x3e + 256];
        ss[1] = 4;
        ss[3] = 4;
        ss[4] = 6;
        ss[5] = 6;
        for i in 0..256 {
            ss[0x3e + i] = (i * 16) as u8;
        }
        std::fs::write(ins.join("wave.ss"), &ss).unwrap();
        path
    }

    #[test]
    fn plays_a_sampled_sound_on_paula() {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join("iffsmus-test");
        let path = files(&dir);
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(&path, std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "IFF-SMUS");
        let mut out = vec![0f32; 2 * 48000];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // the note mutes its voice, turns its DMA off with the others' (none),
        // then on; the next tick turns none off or on
        assert_eq!(&reg(0xDFF096)[..5], &[0x0001, 0x0001, 0x8001, 0x0000, 0x8000]);
        // period 2 to mute, then the note's: 60 less 8 is octave 4's 5th note
        let per = (((0xd5c8 * super::NOTE[4] as u32) >> 15) * 0x1080) >> 19;
        assert_eq!(&reg(0xDFF0A6)[..2], &[2, per as u16]);
        // the octave's 4 words 64 times, then the empty loop's 4 words
        assert_eq!(&reg(0xDFF0A4)[..2], &[128, 4]);
        // the envelope rises to its level
        assert!(reg(0xDFF0A8).iter().any(|&v| v != 0));
        // the other voices stay silent
        assert!(reg(0xDFF0B8).is_empty());
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

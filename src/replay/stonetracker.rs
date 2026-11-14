//! StoneTracker (SPM song, SPS samples) modules: a clean-room replayer.
//!
//! Written from observation only, not from any player's code: the "StoneTracker"
//! Eagleplayer (2.06, "StoneTracker player module 1.6 (29 Dec 95)") was run as a black
//! box on the two modules of the archive and on synthetic modules built once the layout
//! was understood, and its NoteStruct (it is a `_note` player: it hands DeliTracker's
//! NotePlayer genie eight voices) read every tick. `docs/specs/stonetracker/SPEC.md`
//! holds the format and every measurement this follows. No byte of the player is used:
//! the period table is a formula that gives the player's 1152 measured periods.
//!
//! The song (`SPM\x02`) has an info block, songs (each its speed in BPM, 8 channels and
//! a pattern list a channel) and patterns, each one channel's rows packed in two-byte
//! units: a note (1-36) with an instrument, or an effect with its parameter, the top bit
//! ending the row, and `FF nn` skipping nn+1 rows. The samples are in a second file
//! (`SPS\x02`): `SPS.<name>` beside `SPM.<name>`.
//!
//! Under DeliTracker the voices go to the 8Voices-NotePlayer genie, which this plays as it
//! does, measured as a black box from its Paula writes and DMA fetches (SPEC section 9):
//! a double buffer of 192 bytes a Paula channel, refilled from that channel's audio
//! interrupt. Voices 4-7 take the centre position (which the genie leaves silent) or their
//! sides depending on a byte the original reads from outside the module (in the sample
//! file, see `sides`): with them out each of voices 0-3 has its channel at its own period
//! (mode A), else two voices share a channel, resampled to the faster one's period and
//! summed in 8 bits, wrapping (mode B). The handlers and the player's tick hold each other
//! back by the colour clocks the reference takes (`H_*`, `T_*`). Where this differs from
//! the original under DeliTracker:
//! - the reference's handlers and ticks take a few hundred colour clocks more at some
//!   effects, and its Paula fetches fall up to a raster line apart from the port's, so a
//!   change can land a buffer apart;
//! - a sample bank of one sample plays (the original reads none of its data);
//! - a position jump to or past the end of the list ends the song (the original reads past it);
//! - a note when no instrument was ever given plays nothing (the original triggers an empty
//!   sample at a stray period).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "StoneTracker", detect, new };

/// The NoteStruct change bits the original sets, kept to apply a tick's changes in one go.
const SMP: u8 = 0x02;
const REP: u8 = 0x04;
const FREQ: u8 = 0x08;
const VOL: u8 = 0x10;
const TRIG: u8 = 0x20;

const PER_MIN: u16 = 113;
const PER_MAX: u16 = 961;
const VOICES: usize = 8;
/// More rows a pattern than any module has (64): a bound for malformed headers.
const MAX_ROWS: usize = 256;
/// Effects kept a row and channel (a real row has one or two).
const MAX_FX: usize = 16;

/// The 8Voices-NotePlayer genie's output, measured as a black box (SPEC section 9): a double buffer of
/// `GBUF` bytes a Paula channel, refilled from that channel's audio interrupt.
const GBUF: usize = 192;
/// Paula's clock the host converts the player's periods to frequencies with (and the genie back).
const CLOCK: u32 = 3_546_895;
/// Mode B (voices 4-7 on the sides): the voices of each Paula channel (two a channel, the left
/// voices 0, 3, 4, 7 on channels 0, 0, 3, 3, the right ones 1, 2, 5, 6 on 1, 1, 2, 2).
const PAIRS: [[usize; 2]; 4] = [[0, 3], [1, 2], [5, 6], [4, 7]];
/// The genie's handler, in colour clocks (from the reference's write logs): from the interrupt to
/// its first write (PER) when nothing holds it back; from PER to VOL, and to LC (mode A, mode B with
/// sound, mode B silent, mode B with its voices playing at volume 0). The acknowledge (INTREQ) that
/// ends it comes after a time that depends on the work (`Genie::fill`).
const H_FIRST: u64 = 100;
const H_VOL: u64 = 10;
const H_LC: [u64; 4] = [352, 347, 174, 246];
/// After an acknowledge, the next handler's PER, and a held-back tick's start.
const H_NEXT: u64 = 68;
const H_TICK: u64 = 82;
/// The tick (the player's CIA-B interrupt, which the audio handlers hold back and which holds them
/// back): from its start to the NotePlayer call (when the genie takes the changes) and to its
/// end, after which the next handler's PER comes 86 later. Indexed [mode A, mode B] and by
/// [other tick, tick that reads a row, first tick]: the reference's most frequent times (write logs
/// of both archive modules, 20 s; ticks with effects take up to a few hundred clocks more).
const T_NOTE: [[u64; 3]; 2] = [[1417, 1788, 5349], [1689, 2151, 5448]];
const T_END: [[u64; 3]; 2] = [[2238, 2609, 7098], [2874, 3336, 7993]];
const T_AFTER: u64 = 86;
/// The genie's set-up (before the first tick): from its first write to the channels' writes and
/// to DMA on; mode B's comes 652 later. The first tick starts 593 after DMA on.
const S_CHAN: u64 = 2834;
const S_DMA: u64 = 3369;
const S_MODE_B: u64 = 652;
const S_TICK: u64 = 593;

/// ProTracker's vibrato sine, the first half.
const SINE: [u8; 32] =
    [0, 24, 49, 74, 97, 120, 141, 161, 180, 197, 212, 224, 235, 244, 250, 253, 255, 253, 250, 244, 235, 224, 212, 197, 180, 161, 141, 120, 97, 74, 49, 24];

/// The period of `note` (1-36, past 36 allowed for the arpeggio) at finetune `fine`
/// (-16..15, eighths of a semitone): 856 * 2^(-k/96), k = 8 (note - 1) + fine, not below 113.
/// Equal to all 1152 periods measured on the original.
fn period(fine: i32, note: i32) -> u16 {
    let k = 8 * (note - 1) + fine;
    let p = (856.0 * (-(k as f64) / 96.0).exp2()).round();
    (p as i64).clamp(PER_MIN as i64, 65535) as u16
}

/// The first note (1-36) of the finetune's row whose period is not above `per`.
fn note_of(fine: i32, per: u16) -> i32 {
    (1..=36).find(|&n| period(fine, n) <= per).unwrap_or(36)
}

/// The tone portamento target of 0x57 (`s` 1) / 0x58 (-1): the finetune row's first period below
/// `per`, or its last above it (`per` itself when there is none).
fn semitone(fine: i32, per: u16, s: i32) -> u16 {
    let n = if s > 0 { (1..=36).find(|&n| period(fine, n) < per) } else { (1..=36).rev().find(|&n| period(fine, n) > per) };
    n.map_or(per, |n| period(fine, n))
}

#[derive(Clone, Default)]
struct Row {
    note: u8,
    ins: u8,
    fx: Vec<(u8, u8)>,
}

#[derive(Clone, Copy, Default)]
struct Ins {
    start: u32,
    len: u32,
    rep: u32,
    rlen: u32,
    vol: u16,
    fine: i32,
}

struct Song {
    bpm: u8,
    chans: usize,
    npos: usize,
    /// Voices 4-7 on the sides (else centred, which the genie does not play): see `sides`.
    sides: bool,
    /// `lists[ch][pos]`: the pattern.
    lists: Vec<Vec<u16>>,
}

#[derive(Clone, Default)]
struct Ch {
    ins: u8,
    fine: i32,
    per: u16,
    vol: u16,
    out_per: u16,
    out_vol: u16,
    smp: (u32, u32),
    rep: (u32, u32),
    chg: u8,
    /// The change bits of the last tick (for the trace).
    last_chg: u8,
    fx: Vec<(u8, u8)>,
    porta_speed: u16,
    porta_target: Option<u16>,
    gliss: bool,
    vib: (u8, u8, u8, u8),
    trem: (u8, u8, u8, u8),
    loop_row: usize,
    loop_count: u8,
    /// A delayed note: its tick, the change bits it sets then and its shift in semitones.
    delay: Option<(u8, u8, i32)>,
    /// The volume at the row's start (the fade 0x4a).
    fade_base: u16,
}

pub struct StoneTracker {
    title: String,
    songs: Vec<Song>,
    rows: usize,
    patterns: Vec<Vec<(u16, Row)>>,
    ins: Vec<Ins>,
    data: u32,
    sub: usize,
    ch: Vec<Ch>,
    pos: usize,
    row: usize,
    t: u8,
    speed: u8,
    bpm: u8,
    jump: Option<usize>,
    brk: Option<usize>,
    pdelay: u8,
    in_pdelay: bool,
    first: bool,
    stopped: bool,
    songend: bool,
    /// The genie's state.
    g: Genie,
}

/// One NoteStruct channel as the genie holds it.
#[derive(Clone, Copy)]
struct GVoice {
    on: bool,
    /// The part playing (address, bytes) and the repeat that follows it.
    cur: (u32, u32),
    rep: (u32, u32),
    pos: u32,
    frac: u32,
    /// The genie's Paula period of the voice's frequency, and its volume.
    per: u32,
    vol: u32,
}

impl Default for GVoice {
    fn default() -> Self {
        GVoice { on: false, cur: (0, 0), rep: (0, 0), pos: 0, frac: 0, per: genie_period(256), vol: 0 }
    }
}

/// The 8Voices-NotePlayer genie's Paula side.
struct Genie {
    /// Mode B: voices 4-7 on the sides, two voices a channel; else mode A, voices 0-3 alone.
    pairs: bool,
    /// The zero buffer of the set-up, then two buffers a channel.
    buf: u32,
    half: [usize; 4],
    /// The PER and VOL of the buffer filled last, written when Paula starts it.
    pending: [(u16, u16); 4],
    v: [GVoice; VOICES],
    /// The earliest PER of the next handler, and the earliest start of the next tick.
    free: u64,
    tick_free: u64,
    /// When the last handler (or the tick, as far as it ran here) returned.
    end: u64,
    /// The last tick's start (the next is due a period later).
    last_tick: Option<u64>,
    set_up: bool,
    /// Which channels' handlers ran since the set-up.
    filled: [bool; 4],
}

/// The genie's Paula period for a NoteStruct period: the host gives it the frequency
/// `round(CLOCK / per)`, and the genie divides the clock by that.
fn genie_period(per: u16) -> u32 {
    let f = (CLOCK as f64 / per.max(1) as f64).round().max(1.0) as u32;
    CLOCK / f
}

/// The block offsets after the header: the info block, the songs, the patterns.
fn offsets(d: &[u8]) -> Option<Vec<usize>> {
    let nsongs = be16(d, 0x24) as usize;
    let npat = be16(d, 0x26) as usize + 1;
    let n = 1 + nsongs + npat;
    let head = 0x2c + 4 * n;
    if nsongs == 0 || d.len() < head {
        return None;
    }
    let o: Vec<usize> = (0..n).map(|i| be32(d, 0x2c + 4 * i) as usize).collect();
    o.iter().all(|&x| x >= head && x <= d.len()).then_some(o)
}

fn detect(d: &[u8]) -> bool {
    if !d.starts_with(b"SPM\x02") {
        return false;
    }
    let rows = be16(d, 0x2a) as usize;
    if rows == 0 || rows > MAX_ROWS {
        return false;
    }
    let Some(o) = offsets(d) else { return false };
    let nsongs = be16(d, 0x24) as usize;
    // every song block whole, with 1 to 8 channels
    o[1..=nsongs].iter().all(|&s| {
        let (ch, npos) = (d.get(s + 0x20).copied().unwrap_or(0) as usize, be16(d, s + 0x22) as usize);
        (1..=VOICES).contains(&ch) && npos > 0 && s + 0x24 + 2 * ch * npos <= d.len()
    })
}

/// The sample file's names: `SPS.<rest>` for `SPM.<rest>`, else `SPS.<name>` (the
/// original's two rules), and `<stem>.sps` for `<stem>.spm`.
fn companion(file: &str) -> Vec<String> {
    let mut v = Vec::new();
    if file.len() > 4 && file.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("spm.")) {
        v.push(format!("SPS.{}", &file[4..]));
    } else {
        v.push(format!("SPS.{file}"));
    }
    if file.len() > 4 && file.get(file.len() - 4..).is_some_and(|s| s.eq_ignore_ascii_case(".spm")) {
        v.push(format!("{}.sps", &file[..file.len() - 4]));
    }
    v
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let file = file.rsplit(['/', ':']).next().unwrap_or("").to_string();
    let sps = companion(&file).iter().find_map(|n| crate::deli::read_companion(m, n).1.ok());
    let Some(sps) = sps else {
        log::warn!("StoneTracker: no sample file ({:?})", companion(&file));
        return None;
    };
    build(d, &sps, m).map(|p| Box::new(p) as Box<dyn Replayer>)
}

/// One pattern's rows from its units in `d[start..end]`, those that hold anything.
fn decode(d: &[u8], start: usize, end: usize, rows: usize) -> Vec<(u16, Row)> {
    let mut out = Vec::new();
    let (mut r, mut cur, mut pending) = (0usize, Row::default(), false);
    let mut p = start;
    while p + 1 < end && r < rows {
        let (b0, b1) = (d[p], d[p + 1]);
        p += 2;
        if b0 == 0xff {
            if pending {
                out.push((r as u16, std::mem::take(&mut cur)));
                pending = false;
            }
            r += b1 as usize + 1;
            continue;
        }
        let k = b0 & 0x7f;
        if k <= 0x24 {
            (cur.note, cur.ins) = (k, b1);
        } else if cur.fx.len() < MAX_FX {
            cur.fx.push((k, b1));
        }
        pending = true;
        if b0 & 0x80 != 0 {
            out.push((r as u16, std::mem::take(&mut cur)));
            pending = false;
            r += 1;
        }
    }
    if pending && r < rows {
        out.push((r as u16, cur));
    }
    out
}

/// Whether the original gives voices 4-7 the sides: it reads a byte outside the module on its first
/// tick and takes the sides when it is negative. The byte is at the same address whatever the
/// module (the song block's file offset `song` added to a constant), in the sample file that was
/// loaded after the module: at its file offset `song + 14086`, less the module's length
/// rounded up to 8 over 1024 (the sample file's memory starts that much later). Outside the
/// sample file the byte is not known; the centre is assumed.
fn sides(len: usize, song: usize, sps: &[u8]) -> bool {
    let shift = (len.next_multiple_of(8)).saturating_sub(1024);
    (song + 14086).checked_sub(shift).and_then(|f| sps.get(f)).is_some_and(|&b| b & 0x80 != 0)
}

fn build(d: &[u8], sps: &[u8], m: &mut Machine) -> Option<StoneTracker> {
    let o = offsets(d)?;
    let nsongs = be16(d, 0x24) as usize;
    let rows = be16(d, 0x2a) as usize;
    let mut sorted = o.clone();
    sorted.push(d.len());
    sorted.sort_unstable();
    let end_of = |s: usize| sorted.iter().copied().find(|&x| x > s).unwrap_or(d.len());

    let mut songs = Vec::new();
    for &s in &o[1..=nsongs] {
        let chans = d[s + 0x20] as usize;
        let npos = be16(d, s + 0x22) as usize;
        let lists = (0..chans).map(|c| (0..npos).map(|p| be16(d, s + 0x24 + 2 * (c * npos + p))).collect()).collect();
        songs.push(Song { bpm: d[s + 0x1f], chans, npos, sides: sides(d.len(), s, sps), lists });
    }
    let patterns = o[1 + nsongs..].iter().map(|&s| decode(d, s, end_of(s), rows)).collect();

    if !sps.starts_with(b"SPS\x02") {
        log::warn!("StoneTracker: the sample file is not SPS");
        return None;
    }
    let n = be16(sps, 4) as usize;
    let table = 6 + 32 * n;
    let bytes = sps.get(table..).unwrap_or(&[]);
    let size = bytes.len() as u32;
    let data = m.alloc(size.max(4));
    if data == 0 {
        return None;
    }
    m.bus.write_bytes(data, bytes);
    let mut ins = Vec::new();
    for i in 0..n {
        let e = 6 + 32 * i;
        if e + 32 > sps.len() {
            break;
        }
        let start = be32(sps, e + 8).min(size);
        let len = be32(sps, e + 12).min(size - start);
        let rep = be32(sps, e + 16).min(size);
        let rlen = be32(sps, e + 20).min(size - rep);
        let vol = be16(sps, e + 24).min(64);
        let fine = (be16(sps, e + 26) as i16).clamp(-16, 15) as i32;
        ins.push(Ins { start, len, rep, rlen, vol, fine });
    }

    let name = &d[4..0x24];
    let title = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())]).trim().to_string();
    // the genie's buffers: voices 4-7 centred (silent) give mode A, else mode B
    let gbuf = m.alloc((GBUF * 9) as u32);
    if gbuf == 0 {
        return None;
    }
    let g = Genie {
        pairs: songs.first().is_some_and(|s| s.sides),
        buf: gbuf,
        half: [0; 4],
        pending: [(128, 0); 4],
        v: [GVoice::default(); VOICES],
        free: 0,
        tick_free: 0,
        end: 0,
        last_tick: None,
        set_up: false,
        filled: [false; 4],
    };
    let mut p = StoneTracker {
        title,
        songs,
        rows,
        patterns,
        ins,
        data,
        sub: 0,
        ch: Vec::new(),
        pos: 0,
        row: 0,
        t: 0,
        speed: 6,
        bpm: 125,
        jump: None,
        brk: None,
        pdelay: 0,
        in_pdelay: false,
        first: true,
        stopped: false,
        songend: false,
        g,
    };
    p.init(0);
    Some(p)
}

impl StoneTracker {
    fn init(&mut self, sub: usize) {
        self.sub = sub;
        self.ch = vec![Ch { per: 256, out_per: 256, ..Ch::default() }; VOICES];
        (self.pos, self.row, self.t, self.speed) = (0, 0, 0, 6);
        self.bpm = self.songs[sub].bpm;
        (self.jump, self.brk, self.pdelay, self.in_pdelay) = (None, None, 0, false);
        self.first = true;
        self.stopped = self.bpm == 0;
        self.songend = self.stopped;
        self.g.v = [GVoice::default(); VOICES];
    }

    fn row_at(&self, c: usize) -> Option<&Row> {
        let s = &self.songs[self.sub];
        let p = *s.lists.get(c)?.get(self.pos)? as usize;
        let pat = self.patterns.get(p)?;
        pat.binary_search_by_key(&(self.row as u16), |e| e.0).ok().map(|i| &pat[i].1)
    }

    fn set_ins_sample(&mut self, c: usize, i: usize) {
        let s = self.ins[i];
        let ch = &mut self.ch[c];
        ch.smp = (s.start, s.len);
        ch.rep = (s.rep, s.rlen);
    }

    /// Tick 0 of a row: the note and the effects that act at once.
    fn read_row(&mut self) {
        let chans = self.songs[self.sub].chans.min(VOICES);
        for c in 0..chans {
            let row = self.row_at(c).cloned().unwrap_or_default();
            self.ch[c].fx = row.fx.clone();
            self.ch[c].delay = None;
            let has = |k: u8| row.fx.iter().find(|e| e.0 == k).map(|e| e.1);
            let porta = row.fx.iter().any(|e| e.0 == 0x28 || e.0 == 0x2a);
            // 0x57 / 0x58: a tone portamento to a semitone above / below the note
            let semi_porta = row.fx.iter().find(|e| e.0 == 0x57 || e.0 == 0x58).map(|e| if e.0 == 0x57 { 1 } else { -1 });
            // 0x55 / 0x56: a (delayed) retrigger a semitone above / below
            let semi_trig = row.fx.iter().find(|e| e.0 == 0x55 || e.0 == 0x56).map(|e| (if e.0 == 0x55 { 1 } else { -1 }, e.1));
            let ins = row.ins as usize;
            let (note, ins) = if ins > self.ins.len() { (0, 0) } else { (row.note as i32, ins) };
            let fine_set = has(0x3a).filter(|&p| p <= 15).map(|p| p as i32);
            let in_row = |p: u8| p != 0 && p < self.speed;
            let mut delay = has(0x42).filter(|&p| in_row(p)).map(|p| (p, TRIG | SMP | VOL, 0));
            if let Some((s, p)) = semi_trig {
                delay = if in_row(p) { Some((p, TRIG | SMP | FREQ | VOL, s)) } else { None };
            }
            let cut0 = has(0x41) == Some(0);
            let legato = has(0x33);
            if ins != 0 {
                self.ch[c].ins = ins as u8;
                self.ch[c].fine = self.ins[ins - 1].fine;
                self.ch[c].vol = self.ins[ins - 1].vol;
            }
            if let Some(f) = fine_set {
                self.ch[c].fine = f;
            }
            let cur = self.ch[c].ins as usize;
            let fine = self.ch[c].fine;
            let with_ins = if ins != 0 { REP | VOL } else { 0 };
            if ins != 0 {
                self.ch[c].rep = (self.ins[ins - 1].rep, self.ins[ins - 1].rlen);
            }
            if note != 0 && cur != 0 {
                let target = period(fine, note);
                if porta || semi_porta.is_some() {
                    let s = semi_porta.unwrap_or(0);
                    self.ch[c].porta_target = Some(if s == 0 { target } else { semitone(fine, target, s) });
                    self.ch[c].chg |= with_ins | if ins != 0 || s != 0 { FREQ } else { 0 };
                } else if let Some(p) = legato {
                    // no retrigger: the new period only with parameter 0
                    if p == 0 {
                        self.ch[c].per = target;
                    }
                    self.ch[c].chg |= with_ins | FREQ;
                } else if cut0 {
                    self.ch[c].per = target;
                    self.set_ins_sample(c, cur - 1);
                    self.ch[c].vol = 0;
                    self.ch[c].chg |= REP | FREQ | VOL;
                } else {
                    self.ch[c].per = target;
                    self.set_ins_sample(c, cur - 1);
                    if let Some(p) = has(0x2e) {
                        let off = p as u32 * 256;
                        let ch = &mut self.ch[c];
                        if off >= ch.smp.1 {
                            ch.smp.1 = 2;
                        } else {
                            ch.smp = (ch.smp.0 + off, ch.smp.1 - off);
                        }
                    }
                    if let Some(d) = delay {
                        self.ch[c].delay = Some(d);
                        self.ch[c].out_vol = 0;
                        self.ch[c].chg |= with_ins | FREQ | VOL;
                    } else {
                        self.ch[c].chg |= TRIG | SMP | FREQ | with_ins;
                    }
                }
                self.ch[c].out_per = self.ch[c].per;
            } else {
                if ins != 0 && !porta {
                    self.ch[c].chg |= REP | VOL;
                }
                if let Some(s) = semi_porta {
                    let p = self.ch[c].per;
                    self.ch[c].porta_target = Some(semitone(fine, p, s));
                    self.ch[c].chg |= FREQ;
                }
                if let (Some((s, _)), true) = (semi_trig, cur != 0) {
                    if let Some(d) = delay {
                        self.ch[c].delay = Some(d);
                        self.ch[c].out_vol = 0;
                        self.ch[c].chg |= VOL;
                    } else {
                        let ch = &mut self.ch[c];
                        ch.per = period(fine, note_of(fine, ch.per) + s);
                        ch.out_per = ch.per;
                        ch.chg |= TRIG | SMP | FREQ | VOL;
                    }
                }
                if porta && note == 0 {
                    // a row without a note slides on its first tick too
                    let p = row.fx.iter().find(|e| e.0 == 0x28 || e.0 == 0x2a).unwrap();
                    if p.0 == 0x28 && p.1 != 0 {
                        self.ch[c].porta_speed = p.1 as u16;
                    }
                    Self::porta(&mut self.ch[c]);
                    if p.0 == 0x2a {
                        Self::vol_slide(&mut self.ch[c], p.1);
                    }
                }
            }
            if self.ch[c].delay.is_none() {
                self.ch[c].out_vol = self.ch[c].vol;
            }
            for (k, p) in row.fx.iter().copied() {
                self.fx_first(c, k, p);
            }
        }
    }

    /// An effect on its row's first tick.
    fn fx_first(&mut self, c: usize, k: u8, p: u8) {
        let rows = self.rows;
        let ch = &mut self.ch[c];
        match k {
            0x28 if p != 0 => ch.porta_speed = p as u16,
            0x29 => {
                if p >> 4 != 0 {
                    ch.vib.0 = p >> 4;
                }
                if p & 15 != 0 {
                    ch.vib.1 = p & 15;
                }
            }
            0x2c => {
                if p >> 4 != 0 {
                    ch.trem.0 = p >> 4;
                }
                if p & 15 != 0 {
                    ch.trem.1 = p & 15;
                }
            }
            0x30 => self.jump = Some(p as usize),
            0x31 => {
                ch.vol = (p as u16).min(64);
                ch.out_vol = ch.vol;
                ch.chg |= VOL;
            }
            0x32 => self.brk = Some(if (p as usize) < rows { p as usize } else { 0 }),
            0x34 => match p {
                0 => {
                    self.stopped = true;
                    self.songend = true;
                }
                1..0x20 => self.speed = p,
                _ => self.bpm = p,
            },
            0x36 => {
                ch.per = ch.per.saturating_sub(p as u16).max(PER_MIN);
                ch.out_per = ch.per;
                ch.chg |= FREQ;
            }
            0x37 => {
                ch.per = ch.per.saturating_add(p as u16).min(PER_MAX);
                ch.out_per = ch.per;
                ch.chg |= FREQ;
            }
            0x38 => ch.gliss = p != 0,
            0x39 => ch.vib.3 = p,
            0x3b => {
                if p == 0 {
                    ch.loop_row = self.row;
                } else {
                    if ch.loop_count == 0 {
                        ch.loop_count = p;
                    } else {
                        ch.loop_count -= 1;
                    }
                    if ch.loop_count != 0 {
                        self.brk = Some(ch.loop_row);
                        self.jump = Some(self.pos);
                    }
                }
            }
            0x3c => ch.trem.3 = p,
            0x3f => {
                ch.vol = (ch.vol + p as u16).min(64);
                ch.out_vol = ch.vol;
                ch.chg |= VOL;
            }
            0x40 => {
                ch.vol = ch.vol.saturating_sub(p as u16);
                ch.out_vol = ch.vol;
                ch.chg |= VOL;
            }
            0x41 if p == 0 => {
                ch.vol = 0;
                ch.out_vol = 0;
                ch.chg |= VOL;
            }
            0x43 if !self.in_pdelay => self.pdelay = p,
            0x48 if p <= 64 => {
                ch.vol = (ch.vol * p as u16 + 32) >> 6;
                ch.out_vol = ch.vol;
                ch.chg |= VOL;
            }
            0x49 | 0x4b => ch.chg |= VOL,
            0x4a => {
                ch.fade_base = ch.vol;
                Self::fade(ch, p, 0);
            }
            0x4c => {
                ch.vol -= ((p as u32 * ch.vol as u32 + 32) >> 6).min(ch.vol as u32) as u16;
                ch.out_vol = ch.vol;
                ch.chg |= VOL;
            }
            0x57 | 0x58 if p != 0 => ch.porta_speed = p as u16,
            0x4f => self.songend = true,
            _ => {}
        }
    }

    fn vol_slide(ch: &mut Ch, p: u8) {
        if p >> 4 != 0 {
            ch.vol = (ch.vol + (p >> 4) as u16).min(64);
        } else if p & 15 != 0 {
            ch.vol = ch.vol.saturating_sub((p & 15) as u16);
        } else {
            return;
        }
        ch.out_vol = ch.vol;
        ch.chg |= VOL;
    }

    fn wave(w: u8, pos: u8) -> i32 {
        match w {
            0 => {
                let v = SINE[(pos & 31) as usize] as i32;
                if pos >= 32 { -v } else { v }
            }
            1 => {
                if pos < 32 {
                    16 * pos as i32
                } else {
                    16 * (pos as i32 - 64)
                }
            }
            2 => {
                if pos < 32 {
                    255
                } else {
                    -256
                }
            }
            _ => 0,
        }
    }

    /// The fade 0x4a on tick `t`: the row's first volume less (t + 1) p / 64 of it.
    fn fade(ch: &mut Ch, p: u8, t: u8) {
        let v = ch.fade_base as u32;
        ch.vol = v.saturating_sub(((t as u32 + 1) * p as u32 * v + 31) >> 6) as u16;
        ch.out_vol = ch.vol;
        ch.chg |= VOL;
    }

    fn porta(ch: &mut Ch) {
        let Some(t) = ch.porta_target else { return };
        let s = ch.porta_speed;
        if ch.per == t {
            return;
        }
        if ch.per < t {
            ch.per = ch.per.saturating_add(s).min(t);
        } else if ch.per > t {
            ch.per = ch.per.saturating_sub(s).max(t);
        }
        ch.out_per = if ch.gliss { period(ch.fine, note_of(ch.fine, ch.per)) } else { ch.per };
        ch.chg |= FREQ;
    }

    fn vibrato(ch: &mut Ch) {
        let (speed, depth, pos, w) = ch.vib;
        let d = (Self::wave(w, pos) * depth as i32) >> 7;
        ch.out_per = (ch.per as i32 + d).clamp(0, 65535) as u16;
        ch.vib.2 = (pos + speed) & 63;
        ch.chg |= FREQ;
    }

    /// An effect on the row's other ticks.
    fn fx_tick(&mut self, c: usize, k: u8, p: u8, t: u8) {
        let ch = &mut self.ch[c];
        match k {
            0x25 => {
                ch.out_per = match t % 3 {
                    0 => ch.per,
                    x => {
                        let add = if x == 1 { p >> 4 } else { p & 15 } as i32;
                        period(ch.fine, note_of(ch.fine, ch.per) + add)
                    }
                };
                ch.chg |= FREQ;
            }
            0x26 => {
                ch.per = ch.per.wrapping_sub(p as u16);
                if ch.per < PER_MIN {
                    ch.per = PER_MIN;
                }
                ch.out_per = ch.per;
                ch.chg |= FREQ;
            }
            0x27 => {
                ch.per = ch.per.saturating_add(p as u16).min(PER_MAX);
                ch.out_per = ch.per;
                ch.chg |= FREQ;
            }
            0x28 => Self::porta(ch),
            0x29 => Self::vibrato(ch),
            0x2a => {
                Self::porta(ch);
                Self::vol_slide(ch, p);
            }
            0x2b => {
                Self::vibrato(ch);
                Self::vol_slide(ch, p);
            }
            0x2c => {
                let (speed, depth, pos, w) = ch.trem;
                let d = (Self::wave(w, pos) * depth as i32) >> 6;
                ch.out_vol = (ch.vol as i32 + d).clamp(0, 64) as u16;
                ch.trem.2 = (pos + speed) & 63;
                ch.chg |= VOL;
            }
            0x2f => Self::vol_slide(ch, p),
            0x3e if p != 0 && t % p == 0 => ch.chg |= TRIG | SMP,
            0x41 if p == t => {
                ch.vol = 0;
                ch.out_vol = 0;
                ch.chg |= VOL;
            }
            0x42 | 0x55 | 0x56 => match ch.delay {
                Some((d, flags, s)) if d == t => {
                    ch.delay = None;
                    if s != 0 {
                        ch.per = period(ch.fine, note_of(ch.fine, ch.per) + s);
                        ch.out_per = ch.per;
                    }
                    ch.out_vol = ch.vol;
                    ch.chg |= flags;
                }
                Some(_) => ch.chg |= VOL,
                None => {}
            },
            0x4a => Self::fade(ch, p, t),
            0x57 | 0x58 => Self::porta(ch),
            0x49 => ch.chg |= VOL,
            _ => {}
        }
    }

    fn next_row(&mut self) {
        if self.pdelay > 0 {
            self.pdelay -= 1;
            self.in_pdelay = true;
            return;
        }
        self.in_pdelay = false;
        let npos = self.songs[self.sub].npos;
        if self.jump.is_some() || self.brk.is_some() {
            let pos = self.jump.take().unwrap_or(self.pos + 1);
            self.row = self.brk.take().unwrap_or(0);
            self.pos = pos;
        } else {
            self.row += 1;
            if self.row >= self.rows {
                self.row = 0;
                self.pos += 1;
            }
        }
        if self.pos >= npos {
            self.pos = 0;
            self.row = 0;
            self.songend = true;
        }
    }

    /// The tick's changes, as the genie takes them from the NoteStruct (Paula semantics on bytes:
    /// starts rounded down and lengths up to whole words; a repeat of 0 bytes ends the voice).
    fn apply(&mut self) {
        let word = |a: u32, n: u32| (a & !1, (n + 1) & !1);
        for v in 0..VOICES {
            let ch = &mut self.ch[v];
            let chg = std::mem::take(&mut ch.chg);
            ch.last_chg = chg;
            let g = &mut self.g.v[v];
            if chg & TRIG != 0 {
                g.cur = word(self.data + ch.smp.0, ch.smp.1);
                (g.pos, g.frac) = (0, 0);
                g.on = g.cur.1 != 0;
            }
            if chg & (TRIG | REP) != 0 {
                g.rep = word(self.data + ch.rep.0, ch.rep.1);
            }
            if chg & FREQ != 0 {
                g.per = genie_period(ch.out_per);
            }
            if chg & VOL != 0 {
                g.vol = ch.out_vol as u32;
            }
        }
    }

    /// The genie's first writes (its set-up, before the first tick): every channel on the zero
    /// buffer, its audio interrupts on, at the reference's colour clocks from the first write
    /// (channel `c`'s LOC, LEN, PER and VOL at S_CHAN + 47c and 8 apart, DMA on at S_DMA, both
    /// S_MODE_B later in mode B).
    fn set_up(&mut self, hw: &mut Hw) {
        let zero = self.g.buf;
        hw.bus.ram[zero as usize..zero as usize + GBUF * 9].fill(0);
        let t0 = hw.bus.clock;
        let shift = if self.g.pairs { S_MODE_B } else { 0 };
        let at = |hw: &mut Hw, t: u64| {
            let now = hw.bus.clock;
            hw.wait((t0 + t).saturating_sub(now));
        };
        for (i, (r, v)) in [(0xdff096, 0x000f), (0xdff09e, 0x00ff), (0xdff09c, 0x0780), (0xdff09a, 0x0780)].into_iter().enumerate() {
            at(hw, 8 * i as u64);
            hw.bus.write_word(r, v);
        }
        for c in 0..4 {
            let b = S_CHAN + shift + 47 * c as u64;
            at(hw, b);
            hw.loc(c, zero);
            at(hw, b + 8);
            hw.len(c, (GBUF / 2) as u16);
            at(hw, b + 16);
            hw.per(c, 128);
            at(hw, b + 24);
            hw.vol(c, 0);
        }
        for (i, (r, v)) in [(0xdff096, 0x800f), (0xdff09e, 0x00ff), (0xdff09c, 0x0780), (0xdff09a, 0x8780)].into_iter().enumerate() {
            at(hw, S_DMA + shift + 8 * i as u64);
            hw.bus.write_word(r, v);
        }
    }

    /// Fills Paula channel `c`'s next buffer: its PER, VOL, which of the handler cases of `H_LC` it
    /// was and the colour clocks from the handler's PER to its acknowledge. Those are the reference's
    /// most frequent durations (write logs of both archive modules, 20 s): mode A 5824 for a voice
    /// that is off, 4645 at volume 64 and about 5549 + 0.894 per period unit otherwise (5662 at 127,
    /// 5702 at 170, 5778 at 254, 5854 at 340, 6090 at 604); mode B 3296 silent, 5822 for voices at
    /// the channel's period, 6291 when one is resampled, 4743 for voices playing at volume 0.
    fn fill(&mut self, c: usize, out: &mut [u8; GBUF], ram: &[u8]) -> (u16, u16, usize, u64) {
        let g = &mut self.g;
        let (members, n, per, vmax, case): ([usize; 2], usize, u32, u32, usize) = if g.pairs {
            let m = PAIRS[c];
            let per = g.v[m[0]].per.min(g.v[m[1]].per);
            let vmax = g.v[m[0]].vol.max(g.v[m[1]].vol);
            let case = match (vmax, g.v[m[0]].on || g.v[m[1]].on) {
                (0, false) => 2,
                (0, true) => 3,
                _ => 1,
            };
            (m, 2, per, vmax, case)
        } else {
            ([c, c], 1, g.v[c].per, 64, 0)
        };
        let ack = match case {
            0 if !g.v[c].on => 5824,
            0 if g.v[c].vol >= 64 => 4645,
            0 => 5549 + per as u64 * 894 / 1000,
            1 if members.iter().any(|&m| g.v[m].on && g.v[m].vol > 0 && g.v[m].per != per) => 6291,
            1 => 5822,
            2 => 3296,
            _ => 4743,
        } - if g.filled[c] {
            0
        } else {
            // a channel's first handler after the set-up is shorter (mode A 142, 191 for a voice
            // that is off; mode B 147, 96 silent)
            g.filled[c] = true;
            match case {
                0 if !g.v[c].on => 191,
                0 => 142,
                1 => 147,
                2 => 96,
                _ => 0,
            }
        };
        let mut acc = [0i32; GBUF];
        if case != 2 {
            for &vi in &members[..n] {
                let v = &mut g.v[vi];
                // voices at volume 0 play on, unscaled (heard at VOL 0)
                let factor = if vmax == 0 { 64 } else { (v.vol * 64 / vmax) as i32 };
                let step = if v.per == per { 0x10000 } else { (per << 16) / v.per.max(1) + 1 };
                for a in acc.iter_mut() {
                    if !v.on {
                        break;
                    }
                    if step != 0x10000 {
                        v.frac += step;
                        v.pos += v.frac >> 16;
                        v.frac &= 0xffff;
                    }
                    while v.on && v.pos >= v.cur.1 {
                        if v.rep.1 == 0 {
                            v.on = false;
                        } else {
                            v.pos -= v.cur.1;
                            v.cur = v.rep;
                        }
                    }
                    if !v.on {
                        break;
                    }
                    let s = ram.get((v.cur.0 + v.pos) as usize).copied().unwrap_or(0) as i8 as i32;
                    *a += (s * factor) >> 6;
                    if step == 0x10000 {
                        v.pos += 1;
                    }
                }
            }
        }
        for (o, a) in out.iter_mut().zip(acc) {
            *o = a as u8;
        }
        let vol = if g.pairs { vmax } else { 64 };
        (per.min(0xffff) as u16, vol as u16, case, ack)
    }
}

impl Replayer for StoneTracker {
    fn format(&self) -> &'static str {
        "StoneTracker"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (1, self.songs.len() as u16, self.sub as u16 + 1)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        let n = (n.max(1) as usize - 1).min(self.songs.len() - 1);
        self.init(n);
    }

    /// The player's own CIA-B timer: 1789773 / BPM less 42 E-clock ticks of 5 colour clocks,
    /// 32 BPM the slowest.
    fn tick_clocks(&self) -> u64 {
        (1_789_773 / self.bpm.max(32) as u64 - 42) * 5
    }

    /// The next tick is due a period after this one's start, which a genie handler may have
    /// held back.
    fn next_tick_at(&self) -> Option<u64> {
        self.g.last_tick.map(|t| t + self.tick_clocks())
    }

    /// A genie handler: PER and VOL of the buffer Paula starts, LC of the other, which is filled.
    fn audio_irq_ch(&mut self, c: usize, hw: &mut Hw) -> bool {
        if !self.g.set_up {
            return false;
        }
        // a request raised while this channel's own handler ran is gone: its acknowledge cleared it
        if hw.bus.paula.intreq & (0x80 << c) == 0 {
            return true;
        }
        // an interrupt raised while a handler or the tick ran is taken as soon as that ends
        let now = hw.bus.clock;
        let start = if now <= self.g.end { self.g.free } else { (now + H_FIRST).max(self.g.free) };
        hw.wait(start - now);
        let (per, vol) = self.g.pending[c];
        hw.per(c, per);
        hw.wait(H_VOL);
        hw.vol(c, vol);
        let mut out = [0u8; GBUF];
        let (per, vol, case, ack) = self.fill(c, &mut out, &hw.bus.ram);
        self.g.pending[c] = (per, vol);
        let h = self.g.half[c] ^ 1;
        self.g.half[c] = h;
        let a = self.g.buf + (GBUF * (1 + 2 * c + h)) as u32;
        hw.bus.ram[a as usize..a as usize + GBUF].copy_from_slice(&out);
        hw.wait(H_LC[case] - H_VOL);
        hw.loc(c, a);
        hw.wait(ack - H_LC[case]);
        hw.bus.write_word(0xdff09c, 0x0080 << c);
        let end = hw.bus.clock;
        self.g.end = end;
        self.g.free = end + H_NEXT;
        self.g.tick_free = end + H_TICK;
        true
    }

    fn tick(&mut self, hw: &mut Hw) {
        // the tick starts once a genie handler that holds it back has ended
        let now = hw.bus.clock;
        let start = now.max(self.g.tick_free);
        hw.wait(start - now);
        let first = !self.g.set_up;
        let mode = self.g.pairs as usize;
        if first {
            self.g.set_up = true;
            self.set_up(hw);
            let t = hw.bus.clock;
            let shift = if self.g.pairs { S_MODE_B } else { 0 };
            hw.wait((start + S_DMA + shift + S_TICK).saturating_sub(t));
        }
        let kind = if first { 2 } else if self.t == 0 && !self.in_pdelay { 1 } else { 0 };
        let start = hw.bus.clock;
        self.g.last_tick = Some(start);
        self.g.free = start + T_END[mode][kind] + T_AFTER;
        hw.wait(T_NOTE[mode][kind]);
        self.g.end = hw.bus.clock;
        if self.stopped {
            return;
        }
        if self.first {
            self.first = false;
            for ch in self.ch.iter_mut() {
                ch.chg = FREQ | VOL;
            }
        }
        if self.t == 0 && !self.in_pdelay {
            self.read_row();
        } else {
            let t = self.t;
            for c in 0..VOICES {
                for i in 0..self.ch[c].fx.len() {
                    let (k, p) = self.ch[c].fx[i];
                    self.fx_tick(c, k, p, t);
                }
            }
        }
        self.apply();
        self.t += 1;
        if self.t >= self.speed {
            self.t = 0;
            self.next_row();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods() {
        assert_eq!(period(0, 1), 856);
        assert_eq!(period(0, 13), 428);
        assert_eq!(period(0, 36), 113);
        assert_eq!(period(-8, 1), 907);
        assert_eq!(period(7, 21), 256);
        assert_eq!(period(-16, 1), 961);
    }

    fn be(v: u32, n: usize) -> Vec<u8> {
        v.to_be_bytes()[4 - n..].to_vec()
    }

    /// A song of `pats` (each one channel's units), channel `c` playing `lists[c]`, at `bpm`.
    fn module(pats: &[&[u8]], lists: &[Vec<u16>], bpm: u8) -> Vec<u8> {
        let npos = lists[0].len();
        let mut song = vec![0u8; 30];
        song.extend([0, bpm, 8, 0]);
        song.extend(be(npos as u32, 2));
        for c in 0..8 {
            for p in 0..npos {
                song.extend(be(lists.get(c).map_or(0, |l| l[p]) as u32, 2));
            }
        }
        let mut d = b"SPM\x02".to_vec();
        d.extend(b"TEST".iter().copied().chain(std::iter::repeat(0)).take(32));
        d.extend(be(1, 2));
        d.extend(be(pats.len() as u32 - 1, 2));
        d.extend(be(1, 2));
        d.extend(be(64, 2));
        let blocks: Vec<Vec<u8>> = [vec![0, 0], song].into_iter().chain(pats.iter().map(|p| p.to_vec())).collect();
        let mut at = d.len() + 4 * blocks.len();
        for b in &blocks {
            d.extend(be(at as u32, 4));
            at += b.len();
        }
        for b in &blocks {
            d.extend(b);
        }
        d
    }

    /// Two samples: 1000 bytes, volume 64, no loop; 1000 bytes looped from 1200 for 400,
    /// volume 48, finetune 1.
    fn samples() -> Vec<u8> {
        let mut d = b"SPS\x02".to_vec();
        d.extend(be(2, 2));
        for (i, (rep, rlen, vol, fine)) in [(0u32, 0u32, 64u32, 0u32), (1200, 400, 48, 1)].iter().enumerate() {
            d.extend(b"smp\0\0\0\0\0");
            d.extend(be(1000 * i as u32, 4));
            d.extend(be(1000, 4));
            d.extend(be(*rep, 4));
            d.extend(be(*rlen, 4));
            d.extend(be(*vol, 2));
            d.extend(be(*fine, 2));
            d.extend(be(0, 4));
        }
        d.extend((0..2000).map(|i| i as u8));
        d
    }

    fn player(d: &[u8], s: &[u8]) -> (StoneTracker, Machine) {
        let mut m = Machine::new(m68k::CpuType::M68000);
        let p = build(d, s, &mut m).unwrap();
        (p, m)
    }

    /// Each tick's (change bits, period, volume) of the eight voices.
    fn run(p: &mut StoneTracker, m: &mut Machine, ticks: usize) -> Vec<Vec<(u8, u16, u16)>> {
        (0..ticks)
            .map(|_| {
                p.tick(&mut Hw { bus: &mut m.bus });
                p.ch.iter().map(|c| (c.last_chg, c.out_per, c.out_vol)).collect()
            })
            .collect()
    }

    #[test]
    fn detection() {
        let d = module(&[&[0xff, 0x3f]], &[vec![0]], 125);
        assert!(detect(&d));
        let mut bad = d.clone();
        bad[3] = 1;
        assert!(!detect(&bad));
        assert!(!detect(&d[..0x30]));
        let mut bad = d.clone();
        bad[0x2b] = 0;
        assert!(!detect(&bad), "no rows");
        let mut bad = d.clone();
        let song = be32(&d, 0x30) as usize;
        bad[song + 0x20] = 9;
        assert!(!detect(&bad), "9 channels");
        let mut bad = d.clone();
        bad[0x2c] = 0x7f;
        assert!(!detect(&bad), "offset past the end");
    }

    /// A sample file of `n` zero bytes with `b` at `at`.
    fn flagged(n: usize, at: usize, b: u8) -> Vec<u8> {
        let mut v = vec![0u8; n];
        v[at] = b;
        v
    }

    #[test]
    fn sides_of_voices_4_to_7() {
        // a 144-byte module with its song block at 74 (measured on the original): the byte read is
        // the sample file's at 74 + 14086
        let f = 74 + 14086;
        assert!(sides(144, 74, &flagged(20000, f, 0x80)));
        assert!(sides(144, 74, &flagged(20000, f, 0xff)));
        assert!(!sides(144, 74, &flagged(20000, f, 0x7f)));
        assert!(!sides(144, 74, &flagged(20000, f + 1, 0x80)), "only that byte");
        // a module up to 1024 bytes moves nothing; above, by its length rounded up to 8, less 1024
        assert!(sides(1024, 74, &flagged(20000, f, 0x80)));
        assert!(sides(1025, 74, &flagged(20000, f - 8, 0x80)));
        assert!(sides(1044, 74, &flagged(20000, f - 24, 0x80)));
        assert!(sides(6144, 74, &flagged(20000, f - 5120, 0x80)));
        // the song block's offset moves it one for one
        assert!(sides(144, 78, &flagged(20000, f + 4, 0x80)));
        // outside the file, or before it: the centre
        assert!(!sides(144, 74, &vec![0x80u8; f]));
        assert!(!sides(60000, 74, &vec![0x80u8; 20000]));
    }

    #[test]
    fn centred_voices_are_muted() {
        let d = module(&[&[0xff, 0x3f]], &[vec![0]], 125);
        // the sample file is too short for the byte: voices 4-7 centred, silent
        let (p, _) = player(&d, &samples());
        assert!(!p.g.pairs, "mode A: voices 0-3 alone on their channels, 4-7 silent");
        // with the byte negative: all play
        let song = be32(&d, 0x30) as usize;
        let mut s = samples();
        s.resize(song + 14086 + 1, 0);
        s[song + 14086] = 0x80;
        let (p, _) = player(&d, &s);
        assert!(p.g.pairs, "mode B: two voices a channel");
    }

    #[test]
    fn tick_period() {
        let d = module(&[&[0xff, 0x3f]], &[vec![0]], 200);
        let (p, _) = player(&d, &samples());
        assert_eq!(p.tick_clocks(), 5 * (1_789_773 / 200 - 42));
    }

    #[test]
    fn companion_names() {
        assert_eq!(companion("SPM.antarctica")[0], "SPS.antarctica");
        assert_eq!(companion("spm.x")[0], "SPS.x");
        assert_eq!(companion("song.spm"), vec!["SPS.song.spm".to_string(), "song.sps".to_string()]);
    }

    #[test]
    fn note_volume_slide_and_timer() {
        // note 25 with sample 1, then a volume slide down 4 on the next row
        let pat: &[u8] = &[0x19, 0x01, 0xff, 0x00, 0xaf, 0x04, 0xff, 0x3d];
        let (mut p, mut m) = player(&module(&[&[0xff, 0x3f], pat], &[vec![1]], 125), &samples());
        assert_eq!(p.tick_clocks(), 71380);
        let t = run(&mut p, &mut m, 12);
        assert_eq!(t[0][0], (TRIG | SMP | REP | FREQ | VOL, 214, 64));
        assert_eq!(t[0][1], (FREQ | VOL, 256, 0));
        assert_eq!(t[1][0].0, 0);
        assert_eq!(t[6][0].0, 0, "a slide does nothing on its row's first tick");
        assert_eq!((t[7][0].0, t[7][0].2), (VOL, 60));
        assert_eq!(t[11][0].2, 44);
    }

    #[test]
    fn tone_portamento_stops_at_its_note() {
        // note 1, then note 25 with a portamento of 160 a tick
        let pat: &[u8] = &[0x01, 0x01, 0xff, 0x00, 0x19, 0x00, 0xa8, 0xa0, 0xff, 0x3d];
        let (mut p, mut m) = player(&module(&[&[0xff, 0x3f], pat], &[vec![1]], 125), &samples());
        let t = run(&mut p, &mut m, 18);
        assert_eq!(t[0][0].1, 856);
        assert_eq!(t[6][0].0 & TRIG, 0, "no retrigger");
        assert_eq!(t[7][0].1, 856 - 160);
        let reached = t.iter().position(|r| r[0].1 == 214).unwrap();
        assert!(t[reached + 1..].iter().all(|r| r[0].0 == 0));
    }

    #[test]
    fn song_end_and_slow_bpm() {
        let (mut p, mut m) = player(&module(&[&[0xff, 0x3f]], &[vec![0, 0]], 10), &samples());
        assert_eq!(p.tick_clocks(), (1_789_773 / 32 - 42) * 5);
        run(&mut p, &mut m, 2 * 64 * 6 - 1);
        assert!(!p.songend());
        run(&mut p, &mut m, 1);
        assert!(p.songend());
        assert_eq!(p.subsongs(), (1, 1, 1));
    }

    #[test]
    fn instrument_finetune_and_out_of_range() {
        // sample 2 (finetune 1) on note 25, then instrument 9 (none), which is ignored
        let pat: &[u8] = &[0x19, 0x02, 0xff, 0x00, 0x01, 0x09, 0xff, 0x3d];
        let (mut p, mut m) = player(&module(&[&[0xff, 0x3f], pat], &[vec![1]], 125), &samples());
        let t = run(&mut p, &mut m, 8);
        assert_eq!(t[0][0], (TRIG | SMP | REP | FREQ | VOL, period(1, 25), 48));
        assert_eq!(t[6][0].0, 0);
    }

    #[test]
    fn malformed_input_does_not_panic() {
        let pat: &[u8] = &[0x19, 0x01, 0xa5, 0x47, 0xb0, 0x7f, 0xbb, 0x02, 0xc3, 0x05, 0xff, 0x3d];
        let good = module(&[&[0xff, 0x3f], pat], &[vec![1, 5, 1]], 125);
        let s = samples();
        let mut seed = 0x1234_5678u32;
        for i in 0..3000usize {
            let (mut d, mut sp) = (good.clone(), s.clone());
            for _ in 0..1 + i % 8 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let buf = if seed & 1 == 0 { &mut d } else { &mut sp };
                let o = (seed >> 8) as usize % buf.len();
                buf[o] = (seed >> 3) as u8;
            }
            if i % 7 == 0 {
                d.truncate(d.len() * (i % 5 + 1) / 6);
            }
            if i % 11 == 0 {
                sp.truncate(sp.len() * (i % 5 + 1) / 6);
            }
            if !detect(&d) {
                continue;
            }
            let mut m = Machine::new(m68k::CpuType::M68000);
            if let Some(mut p) = build(&d, &sp, &mut m) {
                for _ in 0..400 {
                    p.tick(&mut Hw { bus: &mut m.bus });
                }
                let _ = p.tick_clocks();
            }
        }
        // a header that asks for 65536 patterns in a short file is refused
        let mut d = good.clone();
        d[0x26] = 0xff;
        d[0x27] = 0xff;
        assert!(!detect(&d));
        // a sample bank that is not one
        assert!(build(&good, b"XXXX", &mut Machine::new(m68k::CpuType::M68000)).is_none());
    }

    /// `STONE_MODULE=SPM.x STONE_TICKS=n cargo test --release stonetracker::tests::trace -- --ignored --nocapture`:
    /// every tick's changes, as the reference's NoteStruct (sample offsets in the SPS data).
    #[test]
    #[ignore]
    fn trace() {
        let (Ok(file), Ok(ticks)) = (std::env::var("STONE_MODULE"), std::env::var("STONE_TICKS")) else { return };
        let ticks: usize = ticks.parse().unwrap();
        let d = std::fs::read(&file).unwrap();
        let path = std::path::Path::new(&file);
        let name = path.file_name().unwrap().to_str().unwrap();
        let sps = companion(name).iter().find_map(|n| std::fs::read(path.with_file_name(n)).ok()).unwrap();
        let mut m = Machine::new(m68k::CpuType::M68000);
        let mut p = build(&d, &sps, &mut m).unwrap();
        let mut clk = 0u64;
        for i in 0..ticks {
            p.tick(&mut Hw { bus: &mut m.bus });
            let mut parts = Vec::new();
            for (c, ch) in p.ch.iter().enumerate() {
                if ch.last_chg != 0 {
                    parts.push(format!(
                        "{c}:{:02x} s{:06x}/{:x} r{:06x}/{:x} p{} v{}",
                        ch.last_chg, ch.smp.0, ch.smp.1, ch.rep.0, ch.rep.1, ch.out_per, ch.out_vol
                    ));
                }
            }
            println!("{i:4} {clk:9} {}", parts.join(" | "));
            clk += p.tick_clocks();
            if p.stopped {
                break;
            }
        }
    }
}

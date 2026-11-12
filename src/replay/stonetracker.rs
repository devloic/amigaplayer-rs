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
//! The voices are the host's amplifier's (`Hw::voice_*`), one a channel, sides left,
//! right, right, left for 0-3 and again for 4-7, mixed as the NotePlayer genie does (8
//! bits, wrapping). Where this differs from the original under DeliTracker:
//! - the genie that plays it there (8Voices-NotePlayer) plays a lone voice on its Paula
//!   channel at the voice's own period and mixes only the voices that share one; this
//!   always mixes, at the 32-voice genie's rate;
//! - the original gives voices 4-7 the centre position (which the genie leaves silent) or
//!   their sides depending on a byte it reads from outside the module; they always play;
//! - a sample bank of one sample plays (the original reads none of its data);
//! - a position jump to or past the end of the list ends the song (the original reads past it);
//! - a note when no instrument was ever given plays nothing (the original triggers an empty
//!   sample at a stray period).

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;

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

/// The genie's mix (as `ptm.rs`): Paula period 245 (14477 Hz), boost 3.9.
const MIX_PERIOD: u16 = 245;
const MIX_BOOST: u32 = 39;

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
    silence: u32,
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
        songs.push(Song { bpm: d[s + 0x1f], chans, npos, lists });
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
    let silence = m.alloc(4);
    if data == 0 || silence == 0 {
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
    let pans: Vec<i16> = (0..VOICES).map(|v| if matches!(v % 4, 0 | 3) { 1 } else { -1 }).collect();
    if !crate::amplifier::start_panned_mix(
        m,
        VOICES,
        crate::eagle::epamb::DIRECT | crate::eagle::epamb::BIT8 | crate::eagle::epamb::CHIP_RAM,
        &pans,
        MIX_PERIOD,
        MIX_BOOST,
    ) {
        return None;
    }
    let mut p = StoneTracker {
        title,
        songs,
        rows,
        patterns,
        ins,
        data,
        silence,
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

    fn apply(&mut self, hw: &mut Hw) {
        for v in 0..VOICES {
            let ch = &mut self.ch[v];
            let chg = std::mem::take(&mut ch.chg);
            ch.last_chg = chg;
            if chg == 0 {
                continue;
            }
            if chg & TRIG != 0 {
                hw.voice_dma(1 << v, false);
                if ch.smp.1 == 0 {
                    hw.voice_loc_byte(v, self.silence);
                    hw.voice_len(v, 1);
                } else {
                    hw.voice_loc_byte(v, self.data + ch.smp.0);
                    hw.voice_len_bytes(v, ch.smp.1);
                }
            }
            if chg & FREQ != 0 {
                hw.voice_per(v, ch.out_per);
            }
            if chg & VOL != 0 {
                hw.voice_vol(v, ch.out_vol);
            }
            if chg & TRIG != 0 {
                hw.voice_dma(1 << v, true);
            }
            if chg & (TRIG | REP) != 0 {
                if ch.rep.1 == 0 {
                    hw.voice_loc_byte(v, self.silence);
                    hw.voice_len(v, 1);
                } else {
                    hw.voice_loc_byte(v, self.data + ch.rep.0);
                    hw.voice_len_bytes(v, ch.rep.1);
                }
            }
        }
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

    fn tick(&mut self, hw: &mut Hw) {
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
        self.apply(hw);
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

//! Activision Pro (Martin Walker's replayer), ported from NostalgicPlayer's
//! Activision Pro player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! An Activision Pro module is a game's own replayer with its music, and each
//! game's replayer is a little different. The original player, the Activision
//! Pro Eagleplayer, finds the replayer's init and play routines by their code,
//! copies the module to memory of its own and runs those routines: the music
//! writes Paula directly and waits with its own delay loops. This port does the
//! same with the module in guest memory: its variables are where the module
//! keeps them (many are not set by the init routine, and keep the values the
//! module file holds), and the replayer's tables are read through the code's
//! pc-relative references, as NostalgicPlayer reads them.
//!
//! The replayers differ in where they keep each variable and in their code.
//! Where NostalgicPlayer tells the variants apart by a few bytes, this port
//! reads the code with templates (`Tm`): each is the replayer's code to the
//! instruction, with its variables' offsets captured (and checked the same
//! wherever the code uses them again), so that a replayer is played only when
//! its code is the code this port follows. The init routine's stores are read
//! as they are (`Iop`).
//!
//! Where the original's register writes differ from what NostalgicPlayer asks
//! of its mixer, this follows the original:
//! * nothing is range-checked: notes and arpeggios index the period table as a
//!   byte, and a stop delay's arpeggio reads past the instrument's 4 entries;
//! * the volume is the replayer's 16-bit `mulu`/`divu` arithmetic (an overflow
//!   leaving the product), the envelope's clamp is the replayer's byte test;
//! * a voice's note writes DMACON, period $7e, the sample and its length; the
//!   loop, the period and the volume follow after the replayer's delay loops;
//! * the song never ends: the Eagleplayer reports no song end.
//!
//! One deviation: a vibrato of depth 0 in the dividing replayers is a `divu`
//! by zero, which crashes the original (later subsongs of the Thunderhawk
//! modules do); the port takes the step undivided and plays on.
//!
//! The variants read from the code: the track's form (a flags byte, a speed
//! byte with the track volume, or commands), where the sample's length and
//! loop are, the vibrato's division or halving, which of the five effects are
//! there, the envelope, the speed variation (a table or an accumulator), the
//! sound effect hooks, the transposes, the position list's loop, the volume
//! (master, track, the game's; gated by a speed count), and the later
//! replayers (Reach for the Skies, AV8B, Legends of Valour, Overlord; the last
//! three reached through a jump table) that write the volume, the period and
//! the loop after their delay.
//!
//! Timing: the writes are spaced as the original's, measured on its write log
//! (`C_*`): the time between voices, a note's, the envelope's, the delay loops
//! after them and the final loop's writes.

use super::{Hw, Open, Replayer};
use crate::machine::Machine;
use m68k::AddressBus;
use std::collections::HashMap;

pub const OPEN: Open = Open { name: "ActivisionPro", detect, new };

/// The module, reading zeros outside it.
#[derive(Clone, Copy)]
struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn b(&self, o: usize) -> u8 {
        self.0.get(o).copied().unwrap_or(0)
    }
    fn w(&self, o: usize) -> u16 {
        (self.b(o) as u16) << 8 | self.b(o + 1) as u16
    }
    fn sw(&self, o: usize) -> i64 {
        self.w(o) as i16 as i64
    }
    /// Whether the words at `o` match `pat` (`ANY` matches any word).
    fn at(&self, o: usize, pat: &[u32]) -> bool {
        pat.iter().enumerate().all(|(i, &p)| p == ANY || self.w(o + 2 * i) as u32 == p)
    }
    fn find(&self, pat: &[u32]) -> Option<usize> {
        (0..self.0.len().saturating_sub(2 * pat.len() - 1)).step_by(2).find(|&o| self.at(o, pat))
    }
}

const ANY: u32 = 0x1_0000;

/// The Eagleplayer's check: the replayer's init and play routines, and the
/// first subsong.
#[derive(Clone, Copy, Debug)]
struct Ep {
    init: usize,
    play: usize,
    min: u16,
    /// The routines are reached through a jump table: the init routine only
    /// asks, the play routine starts the subsong.
    jt: bool,
}

const INIT1: &[u32] = &[0x48e7, 0xfcfe, 0xe941, 0x7000];
const PLAY1: &[u32] = &[0x48e7, 0xfcfe, 0x41fa, ANY, 0x43fa, ANY, 0x47fa, ANY, 0x4bfa, ANY, 0x2c7c, 0x00df, 0xf0a0];
const INIT2: &[u32] = &[0x48e7, 0x00c0, 0x41fa, ANY, 0x43fa, ANY, 0x0280, 0x0000, 0x00ff, 0xe540];

fn ep(d: &[u8]) -> Option<Ep> {
    let r = Rd(d);
    if let Some(init) = r.find(INIT1) {
        let play = r.find(PLAY1)?;
        let min = if r.w(0x28) == 0x4e75 { 2 } else { 0 };
        return Some(Ep { init, play, min, jt: false });
    }
    if r.find(INIT2).is_some() {
        // the other replayer (no module of it known): not ported
        return None;
    }
    if d.len() > 0x24 && (0..0x24).step_by(4).all(|k| r.w(k) == 0x6000) {
        let init = (0x16 + r.sw(0x16)) as usize;
        if r.w(init) != 0x2f08 || r.w(init + 2) != 0x41fa {
            return None;
        }
        let play = (0x1a + r.sw(0x1a)) as usize;
        return Some(Ep { init, play, min: 2, jt: true });
    }
    None
}

/// Code templates: words in hex (`??` for any byte), `@NAME` a word or
/// `XX@NAME` a low byte captured as the variable's offset (signed), `%NAME`
/// the target of a pc-relative word, `=NAME` the address here, `*` any word.
/// A name captured again must be the same.
struct Tm<'a> {
    r: Rd<'a>,
    p: usize,
    v: HashMap<&'static str, i64>,
}

impl<'a> Tm<'a> {
    fn set(&mut self, n: &'static str, x: i64) -> bool {
        *self.v.entry(n).or_insert(x) == x
    }

    fn tok(&mut self, t: &'static str) -> bool {
        if let Some(n) = t.strip_prefix('=') {
            return self.set(n, self.p as i64);
        }
        if t == "+T" || t == "+R" {
            // the transpose added to the note, if the replayer has one (at
            // the row's period, and there perhaps stored in the note)
            let (w, e) = (self.r.w(self.p), self.r.w(self.p + 2));
            if !matches!(w, 0xda31 | 0xd831) || e >> 8 != 0 {
                return true;
            }
            self.p += 4;
            if t == "+R" {
                self.set("ROWT", 1);
                if self.r.w(self.p) == 0xda29 {
                    // and a global transpose
                    let g = self.r.sw(self.p + 2);
                    self.p += 4;
                    self.set("GTRANSP", g);
                }
                if self.r.w(self.p) == 0x1385 && self.v.get("NOTE") == Some(&(self.r.w(self.p + 2) as u8 as i8 as i64)) {
                    self.p += 4;
                    self.set("NSTORE", 1);
                }
            } else {
                self.set("FXT", 1);
            }
            return self.set("TRANSP", e as u8 as i8 as i64);
        }
        let pos = self.p;
        let w = self.r.w(pos);
        self.p += 2;
        if pos + 2 > self.r.0.len() {
            return false;
        }
        if let Some(n) = t.strip_prefix('%') {
            return self.set(n, pos as i64 + self.r.sw(pos));
        }
        if let Some(n) = t.strip_prefix('@') {
            return self.set(n, w as i16 as i64);
        }
        if t == "*" {
            return true;
        }
        let byte = |s: &str, b: u16| s == "??" || u16::from_str_radix(s, 16) == Ok(b);
        if t.contains('|') {
            // either word (the assemblers' encodings of the same instruction)
            return t.split('|').any(|a| byte(&a[..2], w >> 8) && byte(&a[2..], w & 0xff));
        }
        let (hi, lo) = t.split_at(2);
        if !byte(hi, w >> 8) {
            return false;
        }
        if let Some(n) = lo.strip_prefix('@') {
            return self.set(n, w as u8 as i8 as i64);
        }
        byte(lo, w & 0xff)
    }

    /// Matches `t` here and moves past it; if it does not match, nothing.
    fn m(&mut self, t: &'static str) -> bool {
        let (p, v) = (self.p, self.v.clone());
        let mut bad = None;
        for x in t.split_whitespace() {
            let at = self.p;
            if !self.tok(x) {
                bad = Some((x, at));
                break;
            }
        }
        let Some((x, at)) = bad else { return true };
        if std::env::var_os("DELI_AVP_DEBUG").is_some() {
            eprintln!("activisionpro: no match at {p:#x} ({x} at {at:#x}): {}", &t[..t.len().min(40)]);
        }
        self.p = p;
        self.v = v;
        false
    }

    fn need(&mut self, t: &'static str) -> Option<()> {
        self.m(t).then_some(())
    }
}

/// A store of the init routine (in its voice loop, or after it).
#[derive(Clone, Copy, Debug)]
enum Iop {
    /// A byte or word at G+offset (+ the voice, or twice it).
    Clr { o: i64, word: bool, idx: u8 },
    Imm { o: i64, word: bool, idx: u8, v: u16 },
    Copy { from: i64, o: i64, word: bool, idx: u8 },
    Imm32 { o: i64, v: u32 },
    NextPos,
    Vol(usize),
    Adk(u16),
    Led,
}

// what the code is
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Track {
    /// A speed byte with flags, then an instrument or a portamento, the note.
    #[default]
    Flags,
    /// Instrument or portamento, and the track volume, a speed, the note.
    Speed,
    /// Commands (each a byte $80 on and its value), a speed, the note.
    Cmds,
}

#[derive(Clone, Debug, Default)]
struct Lay {
    v: HashMap<&'static str, i64>,
    min: u16,
    cmds: Vec<(u8, Vec<Act>, Next)>,
    /// Reached through a jump table: the subsong is asked for (at REQ), and
    /// started by the play routine.
    jt: bool,
    /// The voice loop ends with the effects; the volume, the period, the
    /// loop and the stop delay come after the delay (else the stop delay is
    /// the voice loop's, and the volume comes last).
    late: bool,
    /// A row clears the voice's mute; its note comes before its speed.
    mute: bool,
    nf_clear: bool,
    /// A second speed count, up from the row (its gate: below the maximum,
    /// else the first above it); the volume × the game's / 64; a global
    /// transpose.
    spd2: bool,
    gate_lo: bool,
    gvol: bool,
    note_first: bool,
    fields: Vec<(u16, Dst, bool)>,
    /// The note starts at an offset in the sample (else at its start).
    sofs: bool,
    /// The period written less a finetune.
    fine: bool,
    /// The speed variation is an accumulator's carry (else a table).
    sv2: bool,
    /// A track's first row sets the track volume to $40.
    tvreset: bool,
    /// The note's period is looked up after the instrument (else before).
    period_after: bool,
    /// The instrument check masks the byte (else tests its sign).
    chk_and: bool,
    /// The transpose is added to the note: in the effects, at the row (and
    /// there stored in the note).
    transp: bool,
    row_transp: bool,
    note_store: bool,
    /// The volume is 0 unless the speed count is above a maximum.
    gate: bool,
    /// The position list's loop is counted down from its count byte (else
    /// with a flag).
    np_count: bool,
    /// A new track sets the maximum speed to $ff.
    np_maxff: bool,
    init_v: Vec<Iop>,
    init_g: Vec<Iop>,
    track: Track,
    /// The instrument's sample number is kept per voice (else in a global).
    sample_v: bool,
    /// The sample's length and loop are in a table (else before the sample).
    sepinfo: bool,
    /// Vibrato then portamento each tick (else portamento or vibrato); the
    /// vibrato's step halved by a count (else divided).
    both: bool,
    vib_halve: bool,
    fx: [bool; 6],
    env: bool,
    /// The envelope's offset is a byte (else a word).
    env_byte: bool,
    /// A new row clears the vibrato's word (else its count byte).
    row_vibw: bool,
    /// The final volume: × the track volume × master / $1000 (else × master /
    /// 64).
    track_vol: bool,
    /// The sound effect hooks.
    s1: bool,
    s2: bool,
    s3: bool,
}

impl Lay {
    fn r(&self, n: &str) -> i64 {
        self.v[n]
    }
}

fn init_ops(t: &mut Tm, voice: bool) -> Option<Vec<Iop>> {
    let mut ops = Vec::new();
    loop {
        let r = t.r;
        let p = t.p;
        let w = r.w(p);
        let x = |k: usize| r.w(p + 2 * k);
        let ix = |e: u16| -> Option<(i64, u8)> {
            match e >> 8 {
                0x00 => Some((e as u8 as i8 as i64, 1)),
                0x10 => Some((e as u8 as i8 as i64, 2)),
                _ => None,
            }
        };
        let (op, len) = match w {
            0x51c8 if voice => {
                t.p += 4;
                return Some(ops);
            }
            0x4cdf | 0x41fa if !voice => return Some(ops),
            0x4231 | 0x4271 => {
                let (o, idx) = ix(x(1))?;
                (Iop::Clr { o, word: w == 0x4271, idx }, 2)
            }
            0x4229 | 0x4269 => (Iop::Clr { o: r.sw(p + 2), word: w == 0x4269, idx: 0 }, 2),
            0x13bc | 0x33bc => {
                let (o, idx) = ix(x(2))?;
                (Iop::Imm { o, word: w == 0x33bc, idx, v: x(1) }, 3)
            }
            0x137c | 0x337c => (Iop::Imm { o: r.sw(p + 4), word: w == 0x337c, idx: 0, v: x(1) }, 3),
            0x237c => (Iop::Imm32 { o: r.sw(p + 6), v: (x(1) as u32) << 16 | x(2) as u32 }, 4),
            0x13b1 | 0x33b1 => {
                let (from, i1) = ix(x(1))?;
                let (o, idx) = ix(x(2))?;
                if i1 != idx {
                    return None;
                }
                (Iop::Copy { from, o, word: w == 0x33b1, idx }, 3)
            }
            0x1369 | 0x3369 => (Iop::Copy { from: r.sw(p + 2), o: r.sw(p + 4), word: w == 0x3369, idx: 0 }, 3),
            0x3200 if voice && x(1) == 0xe341 => {
                t.p += 4;
                continue;
            }
            0x6100 if voice => {
                if !t.set("NEXTPOS", p as i64 + 2 + r.sw(p + 2)) {
                    return None;
                }
                (Iop::NextPos, 2)
            }
            0x4279 if !voice && x(1) == 0x00df && matches!(x(2), 0xf0a8 | 0xf0b8 | 0xf0c8 | 0xf0d8) => {
                (Iop::Vol(((x(2) - 0xf0a8) / 16) as usize), 3)
            }
            0x33fc if !voice && x(2) == 0x00df && x(3) == 0xf09e => (Iop::Adk(x(1)), 4),
            0x08f9 if !voice && x(1) == 1 && x(2) == 0x00bf && x(3) == 0xe001 => (Iop::Led, 4),
            _ => {
                if std::env::var_os("DELI_AVP_DEBUG").is_some() {
                    eprintln!("activisionpro: init op {w:04x} at {p:#x}");
                }
                return None;
            }
        };
        ops.push(op);
        t.p += 2 * len;
    }
}

// The play routine's pieces, as the replayers have them.
const PLAY: &str = "48e7 fcfe 41fa %H 43fa %G 47fa %PER 4bfa %INS 2c7c 00df f0a0";
const JT_PLAY: &str = "48e7 fcfe 43fa %G 7200 1229 @REQ 137c 00ff @REQ 0c01 00ff 6700 %JPLAY 4a01 6a?? \
    0c01 0080 6700 %HALT 0201 007f 1341 @FADE 6000 %JPLAY e941 7000 41fa %SUBS";
const PLAY_A: &str = "4a29 @PLAYING 6a00 %EXIT";
const PLAY_E: &str = "4a29 @PLAYING 6b?? =EXIT 6100 %DELAY 4cdf 7f3f 4e75";
const FADE: &str = "4a29 @FADE 6b?? 5329 @FADECNT 6a?? 1369 @FADE @FADECNT 5369 @MVOL 67??";
const SV2: &str = "7a00 1a29 @SVACC da29 @SVINC 64?? 4229 @SVCNT 60?? 137c 00ff @SVCNT 1345 @SVACC";
const VOICE: &str = "7003 3200 3400 3600 e341 e543 e942";
const S1: &str = "45fa %SFX 4a32 00@SFXK 67?? 13bc 00ff 00@DIS";
const ROW0: &str = "4a29 @SVCNT 6600 %NOROW";
const SPD2: &str = "5231 00@SPD2";
const ROW: &str = "5331 00@SPD 6600 %NOROW 7a00 0c31 00fe 00@TRACK 65?? 66?? \
    =HALT 4229 @PLAYING 4279 00df f0a8 4279 00df f0b8 4279 00df f0c8 4279 00df f0d8 33fc 000f 00df f096 \
    6000 %EXIT 4231 00@TPOS 6100 %NEXTPOS 7a00 1a31 00@TRACK da45 49fa %TOFFS 3a34 5000 49fa %TRACKS \
    d9c5 7800 1831 00@TPOS";
const TVRESET: &str = "66?? 13bc 0040 10@TVOLB";
const ROW_SPEED: &str = "4229 @TMP 4231 00@PT 4231 00@STOP 4a34 4000 6a?? 1374 4000 @TMP 5204 4a34 4000 6b?? \
    13b4 4000 10@TVOLB 60?? 13b4 4000 00@PT 5204 5431 00@TPOS 13b4 4000 00@SPD 5204 13b4 4000 00@NF \
    13b4 4000 00@NOTE 5204 5431 00@TPOS 0231 007f 00@NOTE";
const ROW_FLAGS: &str = "137c 00ff @TMP 13b4 4000 00@TBYTE 13b4 4000 00@SPD 0231 003f 00@SPD 4231 00@PT \
    4231 00@STOP 0831 0006 00@TBYTE 67?? 11bc 0001 00@ENVWAIT 11bc 0001 00@ENVCNT 11bc 0009 00@ENVPOS \
    4a31 00@TBYTE 6a?? 5204 5231 00@TPOS 4a34 4000 6b?? 1374 4000 @TMP 60?? 13b4 4000 00@PT 5204 5231 00@TPOS \
    13b4 4000 00@NF 13b4 4000 00@NOTE 0231 007f 00@NOTE";
const CMDS: &str = "137c 00ff @TMP 4231 00@PT 4231 00@STOP";
const CMDS_MUTE: &str = "4230 00@MUTE";
const CMDS_NFCLR: &str = "4231 00@NF";
const CMDS_LOOP: &str = "=CMDLOOP 1a34 4000 6a?? 5204";
const CMDS_LOOP_W: &str = "=CMDLOOP 1a34 4000 6a00 * 5204";
const CMDS_END: &str = "=CMDEND 5204 5431 00@TPOS 60??";
const CMDS_END_W: &str = "=CMDEND 5204 5431 00@TPOS 6000 *";
const CMDS_NOTE3: &str = "13b4 4000 00@NOTE 5204 =CSPD 13b4 4000 00@SPD 5204 5431 00@TPOS";
const CMDS_NOTE4: &str = "13b4 4000 00@NOTE 5204 =CSPD 13b4 4000 00@SPD 4231 00@SPD2 5204 5431 00@TPOS";
const CMDS_NOTE1: &str = "13b4 4000 00@SPD 5204 13b4 4000 00@NF 13b4 4000 00@NOTE 5204 5431 00@TPOS \
    0231 007f 00@NOTE";
const CMDS_NOTE2: &str = "13b4 4000 00@SPD 5204 13b4 4000 00@NF 13b4 4000 00@NOTE 0231 007f 00@NOTE \
    5204 5431 00@TPOS";
const ROW_PERIOD: &str = "7a00 1a31 00@NOTE +R da45 31b3 5000 10@PERIOD";
const S2: &str = "4a31 00@DIS 67?? 45fa %SFX 4a32 00@SFXK 66?? 4231 00@NF";
const CHK_AND: &str = "4a29 @TMP 67?? 1a29 @TMP 0205 007f ba31 00@INSTR 67?? 1385 00@INSTR 4231 00@NF";
const CHK_BMI: &str = "4a29 @TMP 6b?? 1a29 @TMP ba31 00@INSTR 67?? 1385 00@INSTR 4231 00@NF";
const LOAD: &str = "4a31 00@NF 6b00 %ROWEND 7a00 1a31 00@INSTR e945 3185 10@INSOFF";
const LOAD_SG: &str = "7a00 1a29 @TMP";
const LOAD_SV: &str = "7a00 1a31 00@SAMPLE";
const LOAD_SD: &str = "1a35 5000 0245 00ff";
const LOAD_S: &str = "e545 45fa %SSTART 2a32 5000 45fa %SDATA d5c5";
const SMP_INLINE: &str = "3192 2004 31aa 0002 2006 31aa 0004 200a 5c8a 218a 2000 7a00 3a30 2006 d5c5 218a 2006";
const SMP_SEP_G: &str = "218a 2000 7a00 1a29 @TMP cafc 0006 45fa %SINFO";
const SMP_SEP_V: &str = "218a 2000 7a00 1a31 00@SAMPLE cafc 0006 45fa %SINFO";
const SMP_SEP: &str = "31b2 5000 2004 31b2 5002 2006 31b2 5004 200a 7a00 3a30 2006 dab0 2000 2185 2006";
const S3: &str = "45fa %SFX 4a32 00@SFXK 66?? 4231 00@DIS";
const DMA: &str = "33f0 200e 00df f096 3dbc @NOTEPER 2006";
const DMA_SMP: &str = "2db0 2000 2000 3db0 2004 2004 =ROWEND";
const DMA_OFS: &str = "7a00 3a29 @TMPW 2470 2000 d5c5 2d8a 2000 3470 2004 94c5 3d8a 2004 =ROWEND";
const ROWEND_FLAGS: &str = "5204 5231 00@TPOS";
const ROWEND: &str = "1a34 4000 ba3c|0c05 00ff 66?? 4231 00@TPOS 5331 00@LOOPC 4a31 00@LOOPC 66?? \
    6100 %NEXTPOS 4a31 00@DIS 6b00 %VEND 4231 00@TICK";
const ROWEND_VIBC: &str = "4231 00@VIBC 6000 %FXROW";
const ROWEND_VIBW: &str = "4271 10@VIBW 6000 %FXROW";
const NOROW: &str = "=NOROW 4a31 00@DIS 6b00 %VEND";
const PORTA: &str = "4a31 00@PT 67?? 7a00 1a31 00@PT 0c05|ba3c 00c0 65?? 0245 003f db70 10@PERIOD";
const PORTA_ONE: &str = "6000 %FXCHAIN 0245 003f 9b70 10@PERIOD 6000 %FXCHAIN";
const PORTA_BOTH: &str = "60?? 0245 003f 9b70 10@PERIOD =FXCHAIN";
const VIB_HEAD: &str = "4a31 00@VDEL 67?? 5331 00@VDEL 6000 * 4a31 00@SDELAY 6600 * 3a30 10@INSOFF \
    4a35 500e 6700 *";
const VIB_DIV: &str = "4a71 10@VIBW 6b?? 1a35 500e 0245 00ff 13b0 50@VTCNT 00@VMAX 13b0 50@VTDEP 10@VIBWL \
    4231 00@VCD 4231 00@VCNT 7a00 1a31 00@NOTE +T da45 31b3 5000 10@PERIOD 31b3 5000 10@VSPD 3a33 5002 \
    9b70 10@VSPD 3a30 10@VSPD 8af1 10@VIBW 66?? 7a01 3185 10@VSPD 13bc 00ff 10@VIBW 60??";
const VIB_HALVE: &str = "4a31 00@VIBC 6b?? 1a35 500e 0245 00ff 13b0 50@VTCNT 00@VMAX 13b0 50@VTDEP 00@VIBC \
    4231 00@VCD 4231 00@VCNT 7a00 1a31 00@NOTE +T da45 31b3 5000 10@PERIOD 31b3 5000 10@VSPD 3a33 5002 \
    9b70 10@VSPD 5331 00@VIBC 6b?? 3a30 10@VSPD e24d 3185 10@VSPD 66?? 31bc 0001 10@VSPD 13bc 00ff 00@VIBC 60??";
const VIB_RUN: &str = "3a30 10@VSPD 4a31 00@VDIR 6b?? db70 10@PERIOD 60?? 9b70 10@PERIOD 4a31 00@VCD 6b?? \
    5231 00@VCNT 1a31 00@VCNT ba31 00@VMAX 65?? 13bc 00ff 00@VCD 4631 00@VDIR 60?? 5331 00@VCNT 66?? \
    4231 00@VCD";
const FX1: &str = "0831 0001 00@FX 67?? 3a30 10@INSOFF 0831 0000 00@TICK 66?? 7a00 1a31 00@NOTE +T 60?? \
    1a35 500d 4a05 6a?? 0245 007f da31 00@NOTE +T 0245 00ff da45 31b3 5000 10@PERIOD";
const FX2: &str = "0831 0002 00@FX 67?? 3a30 10@INSOFF 0831 0000 00@TICK 66?? 1835 5000 60?? 7802 \
    1384 00@SAMPLE 7800 1835 5004 d970 10@PERIOD 6a?? 9970 10@PERIOD";
const FX3: &str = "0831 0003 00@FX 67?? 1a31 00@TICK 0245 0003 da70 10@INSOFF 60??";
const FX4: &str = "0831 0004 00@FX 67?? 3a30 10@INSOFF 4a31 00@SDELAY 67?? 1835 5008 60?? 4a31 00@STOP \
    6b?? 1835 5000 1384 00@SAMPLE";
const FX5: &str = "0831 0005 00@FX 67?? 4a31 00@SDELAY 67?? 7a00 1a31 00@TICK da70 10@INSOFF 1a35 5009 \
    0245 00ff 4a05 6a?? 0245 007f 0c05|ba3c 0040 64?? da31 00@NOTE +T 60?? 0245 003f 7800 1831 00@NOTE +T 9845 \
    3a04 60?? 4a31 00@STOP 6b?? 1a31 00@NOTE +T 0245 00ff da45 31b3 5000 10@PERIOD";
const ENV: &str = "4a31 00@NF 6b?? 4a31 00@TICK 66?? 3a30 10@INSOFF 7800 1835 5001 e944";
const ENV_SW: &str = "3184 10@ENVOFF";
const ENV_SB: &str = "1184 00@ENVOFFB";
const ENV2: &str = "45fa %ENVT 11b2 4002 00@ENVCNT 4230 00@ENVPOS 11bc 0001 00@ENVWAIT 4270 200c 7800";
const ENV_LW: &str = "3830 10@ENVOFF";
const ENV_LB: &str = "1830 00@ENVOFFB";
const ENV3: &str = "45fa %ENVT d5c4 7800 1830 00@ENVPOS 4a30 00@ENVWAIT 6b?? 5330 00@ENVWAIT 66?? 3a30 200c da32 4001 \
    ba3c|0c05 0041 65?? 0c05|ba3c 00ff 65?? 7a00 60?? 3a3c 0040 3185 200c 5330 00@ENVCNT 67?? 1a32 4000 \
    1185 00@ENVWAIT 60?? 5630 00@ENVPOS 7800 1830 00@ENVPOS 1a32 4000 0c05|ba3c 00c0 65?? 0205 003f \
    1185 00@ENVPOS 7800 1805 1a32 4000 1185 00@ENVWAIT 0c30 000f 00@ENVPOS 67?? 1a32 4002 1185 00@ENVCNT";
const TAIL: &str = "4a31 00@SDELAY 66?? 13bc 00ff 00@STOP 60?? 5331 00@SDELAY 5231 00@TICK =VEND 51c8 *";
const END: &str = "203c 0000 00ff 51c8 fffe 33fc 820f 00df f096";
const VEND: &str = "=VEND 51c8 *";
const END_E: &str = "6100 %DELAY 33fc 820f 00df f096";
const FINAL_E: &str = "7003 4a31 00@DIS 6b?? 3200 3400 e341 e942 4a30 00@MUTE 6b??";
const GATE_HI: &str = "1a31 00@SPD ba31 00@MAXSPD 62?? 7600 60??";
const GATE_LO: &str = "1a31 00@SPD2 ba31 00@MAXSPD 65?? 7600 60??";
const VOL_E: &str = "3630 200c c6f1 10@TVOL c6e9 @MVOL 86fc 1000";
const VOL_GAME: &str = "c6e9 @GVOL 86fc 0040";
const FINAL_E2: &str = "3d83 2008 3a30 10@PERIOD 9a70 10@FINE 3d85 2006 \
    4a31 00@STOP 67?? 2db0 2006 2000 3db0 200a 2004 4a31 00@SDELAY 66?? 13bc 00ff 00@STOP 60?? 5331 00@SDELAY \
    5231 00@TICK 51c8 * 4cdf 7f3f 4e75";
const END_SV1: &str = "5329 @SVCNT 6a?? 5329 @SVIDX 6a?? 137c 0007 @SVIDX 7a00 1a29 @SVIDX 1371 50@SVTAB @SVCNT \
    7020 51c8 fffe";
const END_SV2: &str = "7030 51c8 fffe";
const FINAL: &str = "7003 4a31 00@DIS 6b?? 3200 3400 e341 e942 2db0 2006 2000 3db0 200a 2004";
const FPER: &str = "3db0 10@PERIOD 2006";
const FPER_FINE: &str = "3a30 10@PERIOD 9a70 10@FINE 3d85 2006";
const GATE: &str = "1a31 00@SPD ba31 00@MAXSPD 62?? 7600 60??";
const VOL_TRACK: &str = "3630 200c c6f1 10@TVOL c6e9 @MVOL 86fc 1000 3d83 2008 51c8 * 4cdf 7f3f 4e75";
const VOL_MASTER: &str = "3630 200c c6e9 @MVOL 86fc 0040 3d83 2008 51c8 * 4cdf 7f3f 4e75";
const NP: &str = "=NEXTPOS 7a00 3a31 10@POSOFF 49fa %PLISTS d9c5 =NP 5231 00@POS 7a00 1a31 00@POS \
    0c34 00fd 5000 65?? 67?? 13b4 5000 00@TRACK 5205 13b4 5000 00@POS 5331 00@POS 4e75 5231 00@POS \
    5205 1374 5000 @FADE 60?? 0834 0006 5000 67??";
const NP_FLAG: &str = "4a31 00@PLEN 6b?? 13bc 00ff 00@PLEN 13b4 5000 00@PLC 0231 003f 00@PLC 5231 00@POS 5205 \
    1385 00@PLS 60?? 5331 00@PLC 66?? 4231 00@PLEN 60?? 13b1 00@PLS 00@POS 1a31 00@POS";
const NP_COUNT: &str = "4a31 00@PLC 66?? 13b4 5000 00@PLC 0231 003f 00@PLC 5231 00@POS 5205 1385 00@PLS 60?? \
    5331 00@PLC 67?? 13b1 00@PLS 00@POS 1a31 00@POS";
const NP_TAIL: &str = "13b4 5000 00@LOOPC 5231 00@POS 5205 13b4 5000 00@TRACK";
const NP_MAXFF: &str = "13bc 00ff 00@MAXSPD";

/// A row command's action: the next track byte stored in a global byte, a
/// voice's byte, a byte of a voice's word, or added to it; a value stored in a
/// voice's byte, or one at a0.
#[derive(Clone, Copy, Debug)]
enum Act {
    G(i64),
    V(i64),
    VW(i64),
    AddVW(i64),
    ImmV(i64, u8),
    ImmX(i64, u8),
}

/// After a command: the next command's test, the next track byte, or the
/// speed (the command is the row's note).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Next {
    Fall,
    End,
    Speed,
}

/// The row commands (Indiana Jones on): each a value, its actions, and where
/// its branch goes (none: on to the next test).
fn commands(t: &mut Tm) -> Option<Vec<(u8, Vec<Act>, Option<i64>)>> {
    let r = t.r;
    let mut c = Vec::new();
    loop {
        let p = t.p;
        if !matches!(r.w(p), 0x0c05 | 0xba3c) || r.w(p + 2) >> 8 != 0 || r.w(p + 4) >> 8 != 0x66 {
            return Some(c);
        }
        let x = r.w(p + 2) as u8;
        let mut q = p + 6;
        let mut acts = Vec::new();
        loop {
            let (w, e, f) = (r.w(q), r.w(q + 2), r.w(q + 4));
            let lo = f as u8 as i8 as i64;
            let (a, n) = match (w, e, f >> 8) {
                (0x1374, 0x4000, _) => (Act::G(r.sw(q + 4)), 6),
                (0x13b4, 0x4000, 0x00) => (Act::V(lo), 6),
                (0x13b4, 0x4000, 0x10) => (Act::VW(lo), 6),
                (0x13bc, _, 0x00) if e >> 8 == 0 => (Act::ImmV(lo, e as u8), 6),
                (0x11bc, _, 0x00) if e >> 8 == 0 => (Act::ImmX(lo, e as u8), 6),
                (0x1a31, _, _)
                    if e >> 8 == 0x10 && f == 0xda34 && r.w(q + 6) == 0x4000 && r.w(q + 8) == 0x1385
                        && r.w(q + 10) == e =>
                {
                    (Act::AddVW(e as u8 as i8 as i64), 12)
                }
                _ => break,
            };
            acts.push(a);
            q += n;
        }
        let w = r.w(q);
        let next = if w == 0x6000 {
            q += 4;
            Some(q as i64 - 2 + r.sw(q - 2))
        } else if w >> 8 == 0x60 {
            q += 2;
            Some(q as i64 + (w as u8 as i8 as i64))
        } else {
            None
        };
        t.p = q;
        c.push((x, acts, next));
    }
}

/// Where the instrument's bytes go: a global, a voice's byte, a voice's word
/// (at a0), the hardware copy.
#[derive(Clone, Copy, Debug)]
enum Dst {
    G(i64),
    V(i64),
    X(i64),
    H(i64),
}

/// The instrument's fields the replayer copies (its offset, where, a word).
fn ins_fields(t: &mut Tm) -> Vec<(u16, Dst, bool)> {
    let r = t.r;
    let mut f = Vec::new();
    loop {
        let p = t.p;
        let (w, e, x) = (r.w(p), r.w(p + 2), r.w(p + 4));
        if e >> 8 != 0x50 {
            return f;
        }
        let lo = x as u8 as i8 as i64;
        let (d, word) = match (w, x >> 8) {
            (0x11b5, 0x20) => (Dst::H(lo), false),
            (0x13b5, 0x00) => (Dst::V(lo), false),
            (0x31b5, 0x10) => (Dst::X(lo), true),
            (0x1375, _) => (Dst::G(r.sw(p + 4)), false),
            (0x3375, _) => (Dst::G(r.sw(p + 4)), true),
            _ => return f,
        };
        f.push((e & 0xff, d, word));
        t.p += 6;
    }
}

fn layout(d: &[u8]) -> Option<Lay> {
    let e = ep(d)?;
    let mut l = Lay { min: e.min, jt: e.jt, ..Lay::default() };
    let mut t = Tm { r: Rd(d), p: e.init, v: HashMap::new() };
    if e.jt {
        // the init routine asks for the subsong, the play routine starts it
        t.need("2f08 41fa %G 1140 @REQ")?;
        t.p = e.play;
        t.need(JT_PLAY)?;
    } else {
        t.need("48e7 fcfe e941 7000 41fa %SUBS 43fa %G")?;
    }
    t.need("23b0 1000 00@POSOFF 5801 5800 0c00|b03c 00@INITN 66f0 7003")?;
    l.init_v = init_ops(&mut t, true)?;
    l.init_g = init_ops(&mut t, false)?;
    if e.jt {
        t.need("=JPLAY 41fa %H 47fa %PER 4bfa %INS 2c7c 00df f0a0")?;
    } else {
        t.p = e.play;
        t.need(PLAY)?;
    }
    l.late = !t.m(PLAY_A);
    if l.late {
        t.need(PLAY_E)?;
    }
    t.need(FADE)?;
    l.sv2 = t.m(SV2);
    t.need(VOICE)?;
    l.s1 = t.m(S1);
    t.need(ROW0)?;
    l.spd2 = t.m(SPD2);
    t.need(ROW)?;
    l.tvreset = t.m(TVRESET);
    l.track = if t.m(ROW_SPEED) {
        Track::Speed
    } else if t.m(ROW_FLAGS) {
        Track::Flags
    } else {
        t.need(CMDS)?;
        l.mute = t.m(CMDS_MUTE);
        l.nf_clear = t.m(CMDS_NFCLR);
        // (a short branch's pattern would match a long one's first word)
        if !t.m(CMDS_LOOP_W) {
            t.need(CMDS_LOOP)?;
        }
        let cmds = commands(&mut t)?;
        if !t.m(CMDS_END_W) {
            t.need(CMDS_END)?;
        }
        l.note_first = !t.m(CMDS_NOTE1) && !t.m(CMDS_NOTE2);
        if l.note_first && !t.m(CMDS_NOTE3) {
            t.need(CMDS_NOTE4)?;
        }
        let (end, spd) = (t.v["CMDEND"], t.v.get("CSPD").copied());
        for (x, acts, next) in cmds {
            let next = match next {
                None => Next::Fall,
                Some(a) if a == end => Next::End,
                Some(a) if Some(a) == spd => Next::Speed,
                _ => return None,
            };
            l.cmds.push((x, acts, next));
        }
        Track::Cmds
    };
    l.period_after = !t.m(ROW_PERIOD);
    l.s2 = t.m(S2);
    l.chk_and = t.m(CHK_AND);
    if !l.chk_and {
        t.need(CHK_BMI)?;
    }
    t.need(LOAD)?;
    l.fields = ins_fields(&mut t);
    if !t.m(LOAD_SG) {
        l.sample_v = true;
        if !t.m(LOAD_SV) {
            t.need(LOAD_SD)?;
        }
    }
    t.need(LOAD_S)?;
    if !t.m(SMP_INLINE) {
        t.need(if l.sample_v { SMP_SEP_V } else { SMP_SEP_G })?;
        t.need(SMP_SEP)?;
        l.sepinfo = true;
    }
    l.s3 = t.m(S3);
    t.need(DMA)?;
    l.sofs = !t.m(DMA_SMP);
    if l.sofs {
        t.need(DMA_OFS)?;
    }
    if l.period_after {
        t.need(ROW_PERIOD)?;
    }
    if l.track == Track::Flags {
        t.need(ROWEND_FLAGS)?;
    }
    t.need(ROWEND)?;
    l.row_vibw = t.m(ROWEND_VIBW);
    if !l.row_vibw {
        t.need(ROWEND_VIBC)?;
    }
    t.need(NOROW)?;
    if t.m(PORTA) {
        t.need(PORTA_ONE)?;
        t.need(VIB_HEAD)?;
        t.need(VIB_DIV)?;
        t.need(VIB_RUN)?;
    } else {
        l.both = true;
        t.need(VIB_HEAD)?;
        l.vib_halve = t.m(VIB_HALVE);
        if !l.vib_halve {
            t.need(VIB_DIV)?;
        }
        t.need(VIB_RUN)?;
        t.need(PORTA)?;
        t.need(PORTA_BOTH)?;
    }
    if !l.both {
        t.need("=FXCHAIN")?;
    }
    let mut row_seen = false;
    for (k, f) in [(1, FX1), (2, FX2), (3, FX3), (4, FX4), (5, FX5)] {
        row_seen |= k >= 4 && t.v.get("FXROW") == Some(&(t.p as i64));
        l.fx[k] = t.m(f);
    }
    if !row_seen {
        return None;
    }
    l.env = !l.late && t.m(ENV);
    if l.env {
        l.env_byte = !t.m(ENV_SW);
        if l.env_byte {
            t.need(ENV_SB)?;
        }
        t.need(ENV2)?;
        t.need(if l.env_byte { ENV_LB } else { ENV_LW })?;
        t.need(ENV3)?;
    }
    if l.late {
        t.need(VEND)?;
        t.need(END_E)?;
        t.need(FINAL_E)?;
        l.gate_lo = !t.m(GATE_HI);
        if l.gate_lo {
            t.need(GATE_LO)?;
        }
        t.need(VOL_E)?;
        l.gvol = t.m(VOL_GAME);
        t.need(FINAL_E2)?;
        l.fine = true;
        l.gate = true;
        l.track_vol = true;
    } else {
        t.need(TAIL)?;
        t.need(END)?;
        t.need(if l.sv2 { END_SV2 } else { END_SV1 })?;
        t.need(FINAL)?;
        l.fine = !t.m(FPER);
        if l.fine {
            t.need(FPER_FINE)?;
        }
        l.gate = t.m(GATE);
        l.track_vol = t.m(VOL_TRACK);
        if !l.track_vol {
            t.need(VOL_MASTER)?;
        }
    }
    t.p = *t.v.get("NEXTPOS")? as usize;
    t.need(NP)?;
    l.np_count = !t.m(NP_FLAG);
    if l.np_count {
        t.need(NP_COUNT)?;
    }
    t.need(NP_TAIL)?;
    l.np_maxff = t.m(NP_MAXFF);
    t.need("4e75")?;
    l.transp = t.v.contains_key("FXT");
    l.row_transp = t.v.contains_key("ROWT");
    l.note_store = t.v.contains_key("NSTORE");
    l.v = t.v;
    Some(l)
}

fn detect(d: &[u8]) -> bool {
    layout(d).is_some()
}

// Timing, in colour clocks, measured on the original's write log.
/// From one voice to the next, and a voice's new note up to its DMA off.
const C_VOICE: u64 = 300;
/// A voice's envelope each tick, and its start on a new note.
const C_ENV: u64 = 36;
const C_ENV_NEW: u64 = 264;
const C_NOTE: u64 = 330;
/// The note's writes: DMA off to the period, to the sample, to its length.
const C_PER: u64 = 9;
const C_LOC: u64 = 16;
const C_LEN: u64 = 12;
/// From the last voice to the DMA on (the delay loop), from there to the first
/// voice's loop (with the speed variation's step, and the second delay loop).
const C_DELAY: u64 = 1290;
const C_FINAL: u64 = 224;
const C_SPEEDVAR: u64 = 32;
/// The accumulator's replayers: no step, a longer second delay loop.
const C_SV2: u64 = 40;
/// The final loop's writes: the loop's start, its length, the period, the
/// volume; and on to the next voice.
const C_FLEN: u64 = 12;
const C_FPER: u64 = 12;
const C_FVOL: u64 = 141;
/// The same with the master volume only (no multiply by the track volume).
const C_FVOL_M: u64 = 109;
const C_FNEXT: u64 = 47;
/// The later replayers: the voices to the DMA on (their delay routine), on to
/// the first volume; the volume to the period, to the loop, and on to the
/// next voice's volume (from the loop's length, else from the period).
/// The volume's multiplies (not when the gate or the mute gives 0) come
/// before its write; a voice is shorter than in the earlier replayers, its
/// new note longer.
const C_DELAY_LATE: u64 = 1050;
const C_FINAL_LATE: u64 = 55;
const C_LPER: u64 = 21;
const C_LLOC: u64 = 27;
const C_LNEXT_LOOP: u64 = 92;
const C_LNEXT: u64 = 103;
const C_LVOL: u64 = 240;
const C_VOICE_LATE: u64 = 150;
const C_NOTE_LATE: u64 = 616;

pub struct ActivisionPro {
    l: Lay,
    /// The module's copy in guest memory.
    mb: u32,
    sub: u16,
    /// Clocks since the last write.
    pend: u64,
    /// A track ended with its end byte (the Eagleplayer reports the song's end there: the engine's
    /// empty-subsong skip then goes on to the next subsong, as it does after the original).
    ended: bool,
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    if std::env::var_os("DELI_AVP_DEBUG").is_some() {
        let mut k = l.clone();
        k.v.clear();
        k.cmds.clear();
        eprintln!("activisionpro: {k:?}");
    }
    // the Eagleplayer's copy: module size + $2000, the module at +$1000
    let blk = m.alloc(d.len() as u32 + 0x2000);
    let mb = blk + 0x1000;
    m.bus.write_bytes(mb, d);
    let mut a = ActivisionPro { sub: l.min, l, mb, pend: 0, ended: false };
    let mut hw = Hw { bus: &mut m.bus };
    a.init(a.sub, &mut hw);
    Some(Box::new(a))
}

impl Replayer for ActivisionPro {
    fn format(&self) -> &'static str {
        "ActivisionPro"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn songend(&self) -> bool {
        self.ended
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (self.l.min, 32, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.sub = n;
        self.init(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.pend = 0;
        self.play(hw);
    }
}

impl ActivisionPro {
    fn a(&self, n: &str) -> u32 {
        self.mb.wrapping_add(self.l.r(n) as u32)
    }
    fn o(&self, n: &str) -> u32 {
        self.l.r(n) as u32
    }
    fn g(&self) -> u32 {
        self.a("G")
    }
    fn h(&self) -> u32 {
        self.a("H")
    }
    // globals at a1, voices' bytes at a1+v and words at a1+2v; the hardware
    // copies at a0+16v and their words at a0+2v, bytes at a0+v
    fn gb(&self, hw: &Hw, n: &str) -> u8 {
        hw.bus.r8(self.g().wrapping_add(self.o(n)))
    }
    fn sgb(&self, hw: &mut Hw, n: &str, x: u8) {
        hw.bus.w8(self.g().wrapping_add(self.o(n)), x)
    }
    fn gw(&self, hw: &Hw, n: &str) -> u16 {
        hw.bus.r16(self.g().wrapping_add(self.o(n)))
    }
    fn sgw(&self, hw: &mut Hw, n: &str, x: u16) {
        hw.bus.w16(self.g().wrapping_add(self.o(n)), x)
    }
    fn vb(&self, hw: &Hw, v: usize, n: &str) -> u8 {
        hw.bus.r8(self.g().wrapping_add(self.o(n)) + v as u32)
    }
    fn svb(&self, hw: &mut Hw, v: usize, n: &str, x: u8) {
        hw.bus.w8(self.g().wrapping_add(self.o(n)) + v as u32, x)
    }
    fn vw(&self, hw: &Hw, v: usize, n: &str) -> u16 {
        hw.bus.r16(self.g().wrapping_add(self.o(n)) + 2 * v as u32)
    }
    fn svw(&self, hw: &mut Hw, v: usize, n: &str, x: u16) {
        hw.bus.w16(self.g().wrapping_add(self.o(n)) + 2 * v as u32, x)
    }
    fn xb(&self, hw: &Hw, v: usize, n: &str) -> u8 {
        hw.bus.r8(self.h().wrapping_add(self.o(n)) + v as u32)
    }
    fn sxb(&self, hw: &mut Hw, v: usize, n: &str, x: u8) {
        hw.bus.w8(self.h().wrapping_add(self.o(n)) + v as u32, x)
    }
    fn xw(&self, hw: &Hw, v: usize, n: &str) -> u16 {
        hw.bus.r16(self.h().wrapping_add(self.o(n)) + 2 * v as u32)
    }
    fn sxw(&self, hw: &mut Hw, v: usize, n: &str, x: u16) {
        hw.bus.w16(self.h().wrapping_add(self.o(n)) + 2 * v as u32, x)
    }
    fn hreg(&self, v: usize, o: u32) -> u32 {
        self.h() + 16 * v as u32 + o
    }
    /// An instrument's byte (the instrument's offset plus `o`, a word index).
    fn ins(&self, hw: &Hw, o: u16) -> u8 {
        hw.bus.r8(self.a("INS").wrapping_add(o as i16 as i32 as u32))
    }
    /// The period for a note (as a byte).
    fn period(&self, hw: &Hw, note: u16) -> u16 {
        hw.bus.r16(self.a("PER") + 2 * (note & 0xff) as u32)
    }

    fn write(&mut self, hw: &mut Hw, gap: u64, a: u32, x: u16) {
        hw.wait(self.pend + gap);
        self.pend = 0;
        hw.bus.write_word(a, x);
    }

    /// The Eagleplayer's InitSound: the replayer's init routine.
    fn init(&mut self, n: u16, hw: &mut Hw) {
        self.ended = false;
        if self.l.jt {
            self.sgb(hw, "REQ", n as u8);
            return;
        }
        self.init_sub(n, hw);
    }

    /// The replayer's init: the subsong's position lists, each voice's start.
    fn init_sub(&mut self, n: u16, hw: &mut Hw) {
        self.ended = false;
        let d1 = n.wrapping_shl(4);
        let subs = self.a("SUBS");
        for k in 0..(self.l.r("INITN") as u32 / 4) {
            let x = hw.bus.r32(subs.wrapping_add((d1.wrapping_add(4 * k as u16)) as i16 as i32 as u32));
            hw.bus.w32(self.g().wrapping_add(self.o("POSOFF")) + 4 * k, x);
        }
        for v in (0..4).rev() {
            for op in self.l.init_v.clone() {
                self.iop(hw, op, v);
            }
        }
        for op in self.l.init_g.clone() {
            self.iop(hw, op, 0);
        }
    }

    fn iop(&mut self, hw: &mut Hw, op: Iop, v: usize) {
        let g = self.g();
        let at = |o: i64, idx: u8| g.wrapping_add(o as u32).wrapping_add(idx as u32 * v as u32);
        match op {
            Iop::Clr { o, word, idx } => {
                if word {
                    hw.bus.w16(at(o, idx), 0)
                } else {
                    hw.bus.w8(at(o, idx), 0)
                }
            }
            Iop::Imm { o, word, idx, v: x } => {
                if word {
                    hw.bus.w16(at(o, idx), x)
                } else {
                    hw.bus.w8(at(o, idx), x as u8)
                }
            }
            Iop::Copy { from, o, word, idx } => {
                if word {
                    let x = hw.bus.r16(at(from, idx));
                    hw.bus.w16(at(o, idx), x)
                } else {
                    let x = hw.bus.r8(at(from, idx));
                    hw.bus.w8(at(o, idx), x)
                }
            }
            Iop::Imm32 { o, v: x } => hw.bus.w32(at(o, 0), x),
            Iop::NextPos => self.nextpos(hw, v),
            Iop::Vol(ch) => self.write(hw, 10, 0xdff0a8 + 16 * ch as u32, 0),
            Iop::Adk(x) => self.write(hw, 10, 0xdff09e, x),
            Iop::Led => {
                hw.wait(self.pend + 12);
                self.pend = 0;
                // the Eagleplayer's ENPP_ModuleChange patch replaces the module's bset.b
                // #1,$bfe001 with a call that leaves the LED bit alone (the original's
                // LED stays on): only the time it takes is kept
            }
        }
    }

    /// The next entry of the voice's position list.
    fn nextpos(&mut self, hw: &mut Hw, v: usize) {
        let a4 = self.a("PLISTS").wrapping_add(self.vw(hw, v, "POSOFF") as u32);
        loop {
            let pos = self.vb(hw, v, "POS").wrapping_add(1);
            self.svb(hw, v, "POS", pos);
            let mut d5 = pos;
            let b = hw.bus.r8(a4 + d5 as u32);
            if b >= 0xfd {
                if b == 0xfd {
                    self.svb(hw, v, "POS", pos.wrapping_add(1));
                    d5 = d5.wrapping_add(1);
                    let f = hw.bus.r8(a4 + d5 as u32);
                    self.sgb(hw, "FADE", f);
                    continue;
                }
                self.svb(hw, v, "TRACK", b);
                d5 = d5.wrapping_add(1);
                let p = hw.bus.r8(a4 + d5 as u32);
                self.svb(hw, v, "POS", p.wrapping_sub(1));
                return;
            }
            if b & 0x40 != 0 && self.l.np_count {
                let c = self.vb(hw, v, "PLC");
                if c == 0 {
                    self.svb(hw, v, "PLC", b & 0x3f);
                    let p = self.vb(hw, v, "POS").wrapping_add(1);
                    self.svb(hw, v, "POS", p);
                    d5 = d5.wrapping_add(1);
                    self.svb(hw, v, "PLS", d5);
                } else {
                    self.svb(hw, v, "PLC", c - 1);
                    if c == 1 {
                        continue;
                    }
                    let s = self.vb(hw, v, "PLS");
                    self.svb(hw, v, "POS", s);
                    d5 = s;
                }
            } else if b & 0x40 != 0 {
                if (self.vb(hw, v, "PLEN") as i8) >= 0 {
                    self.svb(hw, v, "PLEN", 0xff);
                    self.svb(hw, v, "PLC", b & 0x3f);
                    let p = self.vb(hw, v, "POS").wrapping_add(1);
                    self.svb(hw, v, "POS", p);
                    d5 = d5.wrapping_add(1);
                    self.svb(hw, v, "PLS", d5);
                } else {
                    let c = self.vb(hw, v, "PLC").wrapping_sub(1);
                    self.svb(hw, v, "PLC", c);
                    if c == 0 {
                        self.svb(hw, v, "PLEN", 0);
                        continue;
                    }
                    let s = self.vb(hw, v, "PLS");
                    self.svb(hw, v, "POS", s);
                    d5 = s;
                }
            }
            let c = hw.bus.r8(a4 + d5 as u32);
            self.svb(hw, v, "LOOPC", c);
            let p = self.vb(hw, v, "POS").wrapping_add(1);
            self.svb(hw, v, "POS", p);
            d5 = d5.wrapping_add(1);
            let t = hw.bus.r8(a4 + d5 as u32);
            self.svb(hw, v, "TRACK", t);
            if self.l.np_maxff {
                self.svb(hw, v, "MAXSPD", 0xff);
            }
            return;
        }
    }

    fn sfx(&self, hw: &Hw, v: usize) -> u8 {
        hw.bus.r8(self.a("SFX").wrapping_add(self.o("SFXK")) + v as u32)
    }

    /// Stops: the voices' volumes 0, their DMA off.
    fn stop(&mut self, hw: &mut Hw) {
        self.sgb(hw, "PLAYING", 0);
        for ch in 0..4 {
            self.write(hw, 10, 0xdff0a8 + 16 * ch, 0);
        }
        self.write(hw, 10, 0xdff096, 0xf);
    }

    fn play(&mut self, hw: &mut Hw) {
        if self.l.jt {
            let d1 = self.gb(hw, "REQ");
            self.sgb(hw, "REQ", 0xff);
            if d1 == 0x80 {
                self.stop(hw);
                return;
            } else if d1 & 0x80 != 0 && d1 != 0xff {
                self.sgb(hw, "FADE", d1 & 0x7f);
            } else if d1 & 0x80 == 0 {
                self.init_sub(d1 as u16, hw);
            }
        }
        if (self.gb(hw, "PLAYING") as i8) >= 0 {
            return;
        }
        if (self.gb(hw, "FADE") as i8) >= 0 {
            let c = self.gb(hw, "FADECNT").wrapping_sub(1);
            self.sgb(hw, "FADECNT", c);
            if (c as i8) < 0 {
                let f = self.gb(hw, "FADE");
                self.sgb(hw, "FADECNT", f);
                let m = self.gw(hw, "MVOL").wrapping_sub(1);
                self.sgw(hw, "MVOL", m);
                if m == 0 {
                    self.stop(hw);
                    return;
                }
            }
        }
        if self.l.sv2 {
            let (acc, carry) = self.gb(hw, "SVACC").overflowing_add(self.gb(hw, "SVINC"));
            self.sgb(hw, "SVCNT", if carry { 0 } else { 0xff });
            self.sgb(hw, "SVACC", acc);
        }
        for v in (0..4).rev() {
            self.pend += if self.l.late { C_VOICE_LATE } else { C_VOICE };
            if self.l.s1 && self.sfx(hw, v) != 0 {
                self.svb(hw, v, "DIS", 0xff);
            }
            if self.l.spd2 && self.gb(hw, "SVCNT") == 0 {
                let s = self.vb(hw, v, "SPD2").wrapping_add(1);
                self.svb(hw, v, "SPD2", s);
            }
            let row = self.gb(hw, "SVCNT") == 0 && {
                let s = self.vb(hw, v, "SPD").wrapping_sub(1);
                self.svb(hw, v, "SPD", s);
                s == 0
            };
            if row {
                if !self.row(hw, v) {
                    return;
                }
            } else {
                if (self.vb(hw, v, "DIS") as i8) < 0 {
                    continue;
                }
                self.norow(hw, v);
                self.fx(hw, v, false);
            }
        }
        if self.l.late {
            self.final_late(hw);
            return;
        }
        // the delay loop, all voices on
        self.write(hw, C_DELAY, 0xdff096, 0x820f);
        let mut c = C_FINAL;
        let sv = if self.l.sv2 { 0 } else { self.gb(hw, "SVCNT").wrapping_sub(1) };
        if !self.l.sv2 {
            self.sgb(hw, "SVCNT", sv);
        } else {
            c += C_SV2;
        }
        if !self.l.sv2 && (sv as i8) < 0 {
            c += C_SPEEDVAR;
            let mut i = self.gb(hw, "SVIDX").wrapping_sub(1);
            if (i as i8) < 0 {
                i = 7;
            }
            self.sgb(hw, "SVIDX", i);
            let x = hw.bus.r8(self.g().wrapping_add(self.o("SVTAB")) + i as u32);
            self.sgb(hw, "SVCNT", x);
        }
        self.pend += c;
        for v in (0..4).rev() {
            if (self.vb(hw, v, "DIS") as i8) < 0 {
                continue;
            }
            let r = 0xdff0a0 + 16 * v as u32;
            let lp = hw.bus.r32(self.hreg(v, 6));
            self.write(hw, 0, r, (lp >> 16) as u16);
            hw.bus.write_word(r + 2, lp as u16);
            let len = hw.bus.r16(self.hreg(v, 0xa));
            self.write(hw, C_FLEN, r + 4, len);
            let mut per = self.xw(hw, v, "PERIOD");
            if self.l.fine {
                per = per.wrapping_sub(self.xw(hw, v, "FINE"));
            }
            self.write(hw, C_FPER, r + 6, per);
            let d3 = self.volume(hw, v);
            self.write(hw, if self.l.track_vol { C_FVOL } else { C_FVOL_M }, r + 8, d3);
            self.pend += C_FNEXT;
        }
    }

    /// Whether the gate (if any) lets the volume through.
    fn gated(&self, hw: &Hw, v: usize) -> bool {
        if !self.l.gate {
            return true;
        }
        let max = self.vb(hw, v, "MAXSPD");
        !if self.l.gate_lo { self.vb(hw, v, "SPD2") >= max } else { self.vb(hw, v, "SPD") <= max }
    }

    /// The volume written: the voice's × the track's × the master volume.
    fn volume(&self, hw: &Hw, v: usize) -> u16 {
        if !self.gated(hw, v) {
            return 0;
        }
        let vol = hw.bus.r16(self.hreg(v, 0xc)) as u32;
        let mvol = self.gw(hw, "MVOL") as u32;
        let (p, div) = if self.l.track_vol {
            let tv = self.vw(hw, v, "TVOL") as u32;
            (((vol * tv) & 0xffff) * mvol, 0x1000)
        } else {
            (vol * mvol, 0x40)
        };
        let q = p / div;
        let d3 = if q > 0xffff { p } else { (p % div) << 16 | q };
        if !self.l.gvol {
            return d3 as u16;
        }
        let p = (d3 & 0xffff) * self.gw(hw, "GVOL") as u32;
        let q = p / 0x40;
        (if q > 0xffff { p } else { q }) as u16
    }

    /// The later replayers' end: the delay, all voices on, then each voice's
    /// volume, period, loop (once its stop delay has run out) and its stop
    /// delay.
    fn final_late(&mut self, hw: &mut Hw) {
        self.write(hw, C_DELAY_LATE, 0xdff096, 0x820f);
        self.pend += C_FINAL_LATE;
        for v in (0..4).rev() {
            if (self.vb(hw, v, "DIS") as i8) < 0 {
                continue;
            }
            let r = 0xdff0a0 + 16 * v as u32;
            let vol = if (self.xb(hw, v, "MUTE") as i8) < 0 { None } else { self.gated(hw, v).then(|| self.volume(hw, v)) };
            self.write(hw, if vol.is_some() { C_LVOL } else { 0 }, r + 8, vol.unwrap_or(0));
            let per = self.xw(hw, v, "PERIOD").wrapping_sub(self.xw(hw, v, "FINE"));
            self.write(hw, C_LPER, r + 6, per);
            if self.vb(hw, v, "STOP") != 0 {
                let lp = hw.bus.r32(self.hreg(v, 6));
                self.write(hw, C_LLOC, r, (lp >> 16) as u16);
                hw.bus.write_word(r + 2, lp as u16);
                let len = hw.bus.r16(self.hreg(v, 0xa));
                self.write(hw, C_FLEN, r + 4, len);
                self.pend += C_LNEXT_LOOP;
            } else {
                self.pend += C_LNEXT;
            }
            self.stop_delay(hw, v);
        }
    }

    /// A voice's new row; false when the song stops.
    fn row(&mut self, hw: &mut Hw, v: usize) -> bool {
        let t = self.vb(hw, v, "TRACK");
        if t >= 0xfe {
            if t == 0xfe {
                self.ended = true;
                self.stop(hw);
                return false;
            }
            self.svb(hw, v, "TPOS", 0);
            self.nextpos(hw, v);
        }
        let tr = self.vb(hw, v, "TRACK") as u32;
        let off = hw.bus.r16(self.a("TOFFS") + 2 * tr) as u32;
        let a4 = self.a("TRACKS").wrapping_add(off);
        let rd = |hw: &Hw, d4: u8| hw.bus.r8(a4 + d4 as u32);
        let mut d4 = self.vb(hw, v, "TPOS");
        if std::env::var_os("DELI_AVP_DEBUG").is_some() {
            let b: Vec<u8> = (0..6).map(|k| rd(hw, d4.wrapping_add(k))).collect();
            eprintln!(
                "row v{v} track {tr:02x} tpos {d4:02x} a4 {:x} pos {:02x} posoff {:04x} bytes {b:02x?}",
                a4 - self.mb,
                self.vb(hw, v, "POS"),
                self.vw(hw, v, "POSOFF")
            );
        }
        let tpos_add = |s: &Self, hw: &mut Hw, k: u8| {
            let p = s.vb(hw, v, "TPOS").wrapping_add(k);
            s.svb(hw, v, "TPOS", p);
        };
        if self.l.tvreset && d4 == 0 {
            hw.bus.w8(self.g().wrapping_add(self.o("TVOLB")) + 2 * v as u32, 0x40);
        }
        match self.l.track {
            Track::Cmds => {
                self.sgb(hw, "TMP", 0xff);
                self.svb(hw, v, "PT", 0);
                self.svb(hw, v, "STOP", 0);
                if self.l.mute {
                    self.sxb(hw, v, "MUTE", 0);
                }
                if self.l.nf_clear {
                    self.svb(hw, v, "NF", 0);
                }
                let mut speed = false;
                'row: loop {
                    let b = rd(hw, d4);
                    if b & 0x80 == 0 {
                        break;
                    }
                    d4 = d4.wrapping_add(1);
                    let x = rd(hw, d4);
                    for (c, acts, next) in &self.l.cmds {
                        if b != *c {
                            continue;
                        }
                        let g = self.g();
                        for &a in acts {
                            match a {
                                Act::G(o) => hw.bus.w8(g.wrapping_add(o as u32), x),
                                Act::V(o) => hw.bus.w8(g.wrapping_add(o as u32) + v as u32, x),
                                Act::VW(o) => hw.bus.w8(g.wrapping_add(o as u32) + 2 * v as u32, x),
                                Act::AddVW(o) => {
                                    let a = g.wrapping_add(o as u32) + 2 * v as u32;
                                    let y = hw.bus.r8(a).wrapping_add(x);
                                    hw.bus.w8(a, y);
                                }
                                Act::ImmV(o, i) => hw.bus.w8(g.wrapping_add(o as u32) + v as u32, i),
                                Act::ImmX(o, i) => hw.bus.w8(self.h().wrapping_add(o as u32) + v as u32, i),
                            }
                        }
                        match next {
                            Next::Fall => continue,
                            Next::End => break,
                            Next::Speed => {
                                speed = true;
                                break 'row;
                            }
                        }
                    }
                    d4 = d4.wrapping_add(1);
                    tpos_add(self, hw, 2);
                }
                if self.l.note_first {
                    if !speed {
                        let n = rd(hw, d4);
                        self.svb(hw, v, "NOTE", n);
                        d4 = d4.wrapping_add(1);
                    }
                    let s = rd(hw, d4);
                    self.svb(hw, v, "SPD", s);
                    if self.l.spd2 {
                        self.svb(hw, v, "SPD2", 0);
                    }
                } else {
                    let s = rd(hw, d4);
                    self.svb(hw, v, "SPD", s);
                    d4 = d4.wrapping_add(1);
                    let n = rd(hw, d4);
                    self.svb(hw, v, "NF", n);
                    self.svb(hw, v, "NOTE", n & 0x7f);
                }
                d4 = d4.wrapping_add(1);
                tpos_add(self, hw, 2);
            }
            Track::Speed => {
                self.sgb(hw, "TMP", 0);
                self.svb(hw, v, "PT", 0);
                self.svb(hw, v, "STOP", 0);
                let b = rd(hw, d4);
                if b & 0x80 != 0 {
                    self.sgb(hw, "TMP", b);
                    d4 = d4.wrapping_add(1);
                    let b2 = rd(hw, d4);
                    if b2 & 0x80 == 0 {
                        hw.bus.w8(self.g().wrapping_add(self.o("TVOLB")) + 2 * v as u32, b2);
                    } else {
                        self.svb(hw, v, "PT", b2);
                    }
                    d4 = d4.wrapping_add(1);
                    tpos_add(self, hw, 2);
                }
                let s = rd(hw, d4);
                self.svb(hw, v, "SPD", s);
                d4 = d4.wrapping_add(1);
                let n = rd(hw, d4);
                self.svb(hw, v, "NF", n);
                self.svb(hw, v, "NOTE", n & 0x7f);
                d4 = d4.wrapping_add(1);
                tpos_add(self, hw, 2);
            }
            Track::Flags => {
                self.sgb(hw, "TMP", 0xff);
                let b = rd(hw, d4);
                self.svb(hw, v, "TBYTE", b);
                self.svb(hw, v, "SPD", b & 0x3f);
                self.svb(hw, v, "PT", 0);
                self.svb(hw, v, "STOP", 0);
                if b & 0x40 != 0 {
                    self.sxb(hw, v, "ENVWAIT", 1);
                    self.sxb(hw, v, "ENVCNT", 1);
                    self.sxb(hw, v, "ENVPOS", 9);
                }
                if b & 0x80 != 0 {
                    d4 = d4.wrapping_add(1);
                    tpos_add(self, hw, 1);
                    let b2 = rd(hw, d4);
                    if b2 & 0x80 == 0 {
                        self.sgb(hw, "TMP", b2);
                    } else {
                        self.svb(hw, v, "PT", b2);
                    }
                }
                d4 = d4.wrapping_add(1);
                tpos_add(self, hw, 1);
                let n = rd(hw, d4);
                self.svb(hw, v, "NF", n);
                self.svb(hw, v, "NOTE", n & 0x7f);
            }
        }
        if !self.l.period_after {
            self.note_period(hw, v);
        }
        if self.l.s2 && self.vb(hw, v, "DIS") != 0 && self.sfx(hw, v) == 0 {
            self.svb(hw, v, "NF", 0);
        }
        let tmp = self.gb(hw, "TMP");
        let change = if self.l.chk_and {
            (tmp != 0).then_some(tmp & 0x7f)
        } else {
            ((tmp as i8) >= 0).then_some(tmp)
        };
        if let Some(i) = change {
            if i != self.vb(hw, v, "INSTR") {
                self.svb(hw, v, "INSTR", i);
                self.svb(hw, v, "NF", 0);
            }
        }
        if (self.vb(hw, v, "NF") as i8) >= 0 {
            self.instrument(hw, v);
        }
        if self.l.period_after {
            self.note_period(hw, v);
        }
        if self.l.track == Track::Flags {
            d4 = d4.wrapping_add(1);
            tpos_add(self, hw, 1);
        }
        if rd(hw, d4) == 0xff {
            self.svb(hw, v, "TPOS", 0);
            let c = self.vb(hw, v, "LOOPC").wrapping_sub(1);
            self.svb(hw, v, "LOOPC", c);
            if c == 0 {
                self.nextpos(hw, v);
            }
        }
        if (self.vb(hw, v, "DIS") as i8) < 0 {
            return true;
        }
        self.svb(hw, v, "TICK", 0);
        if self.l.row_vibw {
            self.svw(hw, v, "VIBW", 0);
        } else {
            self.svb(hw, v, "VIBC", 0);
        }
        self.fx(hw, v, true);
        true
    }

    /// The note, transposed.
    fn note(&self, hw: &Hw, v: usize) -> u8 {
        let n = self.vb(hw, v, "NOTE");
        if self.l.transp { n.wrapping_add(self.vb(hw, v, "TRANSP")) } else { n }
    }

    fn note_period(&self, hw: &mut Hw, v: usize) {
        let mut n = self.vb(hw, v, "NOTE");
        if self.l.row_transp {
            n = n.wrapping_add(self.vb(hw, v, "TRANSP"));
            if self.l.v.contains_key("GTRANSP") {
                n = n.wrapping_add(self.gb(hw, "GTRANSP"));
            }
            if self.l.note_store {
                self.svb(hw, v, "NOTE", n);
            }
        }
        let p = self.period(hw, n as u16);
        self.sxw(hw, v, "PERIOD", p);
    }

    /// The new instrument (or the same, struck again), and its note's writes.
    fn instrument(&mut self, hw: &mut Hw, v: usize) {
        let d5 = (self.vb(hw, v, "INSTR") as u16) << 4;
        self.sxw(hw, v, "INSOFF", d5);
        for &(k, dst, word) in &self.l.fields {
            let a = match dst {
                Dst::G(o) => self.g().wrapping_add(o as u32),
                Dst::V(o) => self.g().wrapping_add(o as u32) + v as u32,
                Dst::X(o) => self.h().wrapping_add(o as u32) + 2 * v as u32,
                Dst::H(o) => self.hreg(v, o as u32),
            };
            let src = self.a("INS").wrapping_add((d5 + k) as u32);
            if word {
                let x = hw.bus.r16(src);
                hw.bus.w16(a, x);
            } else {
                let x = hw.bus.r8(src);
                hw.bus.w8(a, x);
            }
        }
        let s = self.ins(hw, d5) as u32;
        let off = hw.bus.r32(self.a("SSTART") + 4 * s);
        let mut a2 = self.a("SDATA").wrapping_add(off);
        if self.l.sepinfo {
            hw.bus.w32(self.hreg(v, 0), a2);
            let si = self.a("SINFO") + 6 * s;
            for (k, o) in [(0, 4), (2, 6), (4, 0xa)] {
                let x = hw.bus.r16(si + k);
                hw.bus.w16(self.hreg(v, o), x);
            }
        } else {
            for (k, o) in [(0, 4), (2, 6), (4, 0xa)] {
                let x = hw.bus.r16(a2 + k);
                hw.bus.w16(self.hreg(v, o), x);
            }
            a2 += 6;
            hw.bus.w32(self.hreg(v, 0), a2);
        }
        let lp = (hw.bus.r16(self.hreg(v, 6)) as u32).wrapping_add(hw.bus.r32(self.hreg(v, 0)));
        hw.bus.w32(self.hreg(v, 6), lp);
        if self.l.s3 {
            if self.sfx(hw, v) != 0 {
                return;
            }
            self.svb(hw, v, "DIS", 0);
        }
        let r = 0xdff0a0 + 16 * v as u32;
        let mask = hw.bus.r16(self.hreg(v, 0xe));
        self.write(hw, if self.l.late { C_NOTE_LATE } else { C_NOTE }, 0xdff096, mask);
        self.write(hw, C_PER, r + 6, self.l.r("NOTEPER") as u16);
        let ofs = if self.l.sofs { self.gw(hw, "TMPW") } else { 0 };
        let p = hw.bus.r32(self.hreg(v, 0)).wrapping_add(ofs as u32);
        self.write(hw, C_LOC, r, (p >> 16) as u16);
        hw.bus.write_word(r + 2, p as u16);
        let len = hw.bus.r16(self.hreg(v, 4)).wrapping_sub(ofs);
        self.write(hw, C_LEN, r + 4, len);
    }

    fn add_period(&self, hw: &mut Hw, v: usize, x: u16, add: bool) -> u16 {
        let p = self.xw(hw, v, "PERIOD");
        let p = if add { p.wrapping_add(x) } else { p.wrapping_sub(x) };
        self.sxw(hw, v, "PERIOD", p);
        p
    }

    fn porta(&self, hw: &mut Hw, v: usize) -> bool {
        let pt = self.vb(hw, v, "PT") as u16;
        if pt == 0 {
            return false;
        }
        self.add_period(hw, v, pt & 0x3f, pt >= 0xc0);
        true
    }

    /// A tick without a new row: portamento, vibrato.
    fn norow(&mut self, hw: &mut Hw, v: usize) {
        if !self.l.both && self.porta(hw, v) {
            return;
        }
        self.vibrato(hw, v);
        if self.l.both {
            self.porta(hw, v);
        }
    }

    fn vibrato(&mut self, hw: &mut Hw, v: usize) {
        let d = self.vb(hw, v, "VDEL");
        if d != 0 {
            self.svb(hw, v, "VDEL", d - 1);
            return;
        }
        if self.vb(hw, v, "SDELAY") != 0 {
            return;
        }
        let io = self.xw(hw, v, "INSOFF");
        let vn = self.ins(hw, io.wrapping_add(14));
        if vn == 0 {
            return;
        }
        let running = if self.l.vib_halve {
            (self.vb(hw, v, "VIBC") as i8) < 0
        } else {
            (self.vw(hw, v, "VIBW") as i16) < 0
        };
        if !running {
            let h = self.h();
            let cnt = hw.bus.r8(h.wrapping_add(self.o("VTCNT")) + vn as u32);
            self.svb(hw, v, "VMAX", cnt);
            let dep = hw.bus.r8(h.wrapping_add(self.o("VTDEP")) + vn as u32);
            if self.l.vib_halve {
                self.svb(hw, v, "VIBC", dep);
            } else {
                hw.bus.w8(self.g().wrapping_add(self.o("VIBWL")) + 2 * v as u32, dep);
            }
            self.svb(hw, v, "VCD", 0);
            self.svb(hw, v, "VCNT", 0);
            let n = self.note(hw, v) as u16;
            let p = self.period(hw, n);
            self.sxw(hw, v, "PERIOD", p);
            let mut s = p.wrapping_sub(self.period(hw, n + 1));
            self.sxw(hw, v, "VSPD", s);
            if self.l.vib_halve {
                loop {
                    let c = self.vb(hw, v, "VIBC").wrapping_sub(1);
                    self.svb(hw, v, "VIBC", c);
                    if (c as i8) < 0 {
                        break;
                    }
                    s >>= 1;
                    self.sxw(hw, v, "VSPD", s);
                    if s == 0 {
                        self.sxw(hw, v, "VSPD", 1);
                        self.svb(hw, v, "VIBC", 0xff);
                        break;
                    }
                }
            } else {
                let div = self.vw(hw, v, "VIBW") as u32;
                // (a depth of 0 is a division by zero in the original)
                let q = if div == 0 { s } else { (s as u32 / div) as u16 };
                self.sxw(hw, v, "VSPD", if q == 0 { 1 } else { q });
                hw.bus.w8(self.g().wrapping_add(self.o("VIBW")) + 2 * v as u32, 0xff);
            }
            return;
        }
        let s = self.xw(hw, v, "VSPD");
        self.add_period(hw, v, s, (self.vb(hw, v, "VDIR") as i8) >= 0);
        if (self.vb(hw, v, "VCD") as i8) >= 0 {
            let c = self.vb(hw, v, "VCNT").wrapping_add(1);
            self.svb(hw, v, "VCNT", c);
            if c >= self.vb(hw, v, "VMAX") {
                self.svb(hw, v, "VCD", 0xff);
                let d = !self.vb(hw, v, "VDIR");
                self.svb(hw, v, "VDIR", d);
            }
        } else {
            let c = self.vb(hw, v, "VCNT").wrapping_sub(1);
            self.svb(hw, v, "VCNT", c);
            if c == 0 {
                self.svb(hw, v, "VCD", 0);
            }
        }
    }

    /// The instrument's effects (from a new row, the sample switch and the
    /// arpeggio only), the envelope, the stop delay.
    fn fx(&mut self, hw: &mut Hw, v: usize, row: bool) {
        let fx = self.vb(hw, v, "FX");
        let io = self.xw(hw, v, "INSOFF");
        let tick = self.vb(hw, v, "TICK");
        let note = self.note(hw, v);
        let mut arp = None;
        let mut per = None;
        if !row {
            if self.l.fx[1] && fx & 2 != 0 {
                let n = if tick & 1 == 0 {
                    note as u16
                } else {
                    let b = self.ins(hw, io.wrapping_add(13));
                    if b & 0x80 != 0 { ((b & 0x7f).wrapping_add(note)) as u16 } else { b as u16 }
                };
                let p = self.period(hw, n);
                self.sxw(hw, v, "PERIOD", p);
            }
            if self.l.fx[2] && fx & 4 != 0 {
                let s = if tick & 1 == 0 { self.ins(hw, io) } else { 2 };
                self.svb(hw, v, "SAMPLE", s);
                let d4 = self.ins(hw, io.wrapping_add(4)) as u16;
                let p = self.add_period(hw, v, d4, true);
                if (p as i16) < 0 {
                    self.add_period(hw, v, d4, false);
                }
            }
            if self.l.fx[3] && fx & 8 != 0 {
                arp = Some(((tick & 3) as u16).wrapping_add(io));
            }
        }
        if arp.is_none() {
            if self.l.fx[4] && fx & 0x10 != 0 {
                let s = if self.vb(hw, v, "SDELAY") != 0 {
                    Some(self.ins(hw, io.wrapping_add(8)))
                } else if (self.vb(hw, v, "STOP") as i8) >= 0 {
                    Some(self.ins(hw, io))
                } else {
                    None
                };
                if let Some(s) = s {
                    self.svb(hw, v, "SAMPLE", s);
                }
            }
            if self.l.fx[5] && fx & 0x20 != 0 {
                if self.vb(hw, v, "SDELAY") != 0 {
                    arp = Some((tick as u16).wrapping_add(io));
                } else if (self.vb(hw, v, "STOP") as i8) >= 0 {
                    per = Some(note as u16);
                }
            }
        }
        if let Some(d5) = arp {
            let b = self.ins(hw, d5.wrapping_add(9));
            per = Some(if b & 0x80 == 0 {
                b as u16
            } else {
                let c = b & 0x7f;
                if c < 0x40 { c.wrapping_add(note) as u16 } else { (note as u16).wrapping_sub((c & 0x3f) as u16) & 0xff }
            });
        }
        if let Some(n) = per {
            let p = self.period(hw, n);
            self.sxw(hw, v, "PERIOD", p);
        }
        if self.l.env {
            self.pend += C_ENV;
            self.envelope(hw, v);
        }
        if !self.l.late {
            self.stop_delay(hw, v);
        }
    }

    fn stop_delay(&self, hw: &mut Hw, v: usize) {
        let s = self.vb(hw, v, "SDELAY");
        if s == 0 {
            self.svb(hw, v, "STOP", 0xff);
        } else {
            self.svb(hw, v, "SDELAY", s - 1);
        }
        let t = self.vb(hw, v, "TICK").wrapping_add(1);
        self.svb(hw, v, "TICK", t);
    }

    fn envelope(&mut self, hw: &mut Hw, v: usize) {
        let envt = self.a("ENVT");
        if (self.vb(hw, v, "NF") as i8) >= 0 && self.vb(hw, v, "TICK") == 0 {
            self.pend += C_ENV_NEW;
            let io = self.xw(hw, v, "INSOFF");
            let e = (self.ins(hw, io.wrapping_add(1)) as u16) << 4;
            if self.l.env_byte {
                self.sxb(hw, v, "ENVOFFB", e as u8);
            } else {
                self.sxw(hw, v, "ENVOFF", e);
            }
            let c = hw.bus.r8(envt + e as u32 + 2);
            self.sxb(hw, v, "ENVCNT", c);
            self.sxb(hw, v, "ENVPOS", 0);
            self.sxb(hw, v, "ENVWAIT", 1);
            hw.bus.w16(self.hreg(v, 0xc), 0);
        }
        let eo = if self.l.env_byte { self.xb(hw, v, "ENVOFFB") as u16 } else { self.xw(hw, v, "ENVOFF") };
        let a2 = envt + eo as u32;
        let pos = self.xb(hw, v, "ENVPOS");
        let w = self.xb(hw, v, "ENVWAIT");
        if (w as i8) < 0 {
            return;
        }
        self.sxb(hw, v, "ENVWAIT", w.wrapping_sub(1));
        if w != 1 {
            return;
        }
        let vol = hw.bus.r16(self.hreg(v, 0xc));
        let b = (vol as u8).wrapping_add(hw.bus.r8(a2 + pos as u32 + 1));
        let vol = if b < 0x41 {
            vol & 0xff00 | b as u16
        } else if b < 0xff {
            0x40
        } else {
            0
        };
        hw.bus.w16(self.hreg(v, 0xc), vol);
        let c = self.xb(hw, v, "ENVCNT").wrapping_sub(1);
        self.sxb(hw, v, "ENVCNT", c);
        if c != 0 {
            let x = hw.bus.r8(a2 + pos as u32);
            self.sxb(hw, v, "ENVWAIT", x);
            return;
        }
        let mut p = self.xb(hw, v, "ENVPOS").wrapping_add(3);
        self.sxb(hw, v, "ENVPOS", p);
        let mut x = hw.bus.r8(a2 + p as u32);
        if x >= 0xc0 {
            p = x & 0x3f;
            self.sxb(hw, v, "ENVPOS", p);
            x = hw.bus.r8(a2 + p as u32);
        }
        self.sxb(hw, v, "ENVWAIT", x);
        if self.xb(hw, v, "ENVPOS") == 0xf {
            return;
        }
        let c = hw.bus.r8(a2 + p as u32 + 2);
        self.sxb(hw, v, "ENVCNT", c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::path::Path;

    /// Writes replayer code from the port's own templates: `??` bytes as 2,
    /// `*` as 0, the first of `a|b`; each variable gets an offset of its own
    /// (voice bytes 4 apart, words 8, tables 16; none in the hardware copies
    /// at 0..$40), `%NAME` a pc-relative word to a label (`=NAME`, here or in
    /// the data).
    #[derive(Default)]
    struct Gen {
        d: Vec<u8>,
        labels: HashMap<String, usize>,
        fix: Vec<(usize, String)>,
        offs: HashMap<String, i64>,
        next: i64,
        gnext: i64,
    }

    impl Gen {
        fn new() -> Self {
            Gen { next: -128, gnext: 0x100, ..Default::default() }
        }
        fn reserve(&mut self, n: &str, size: i64) -> i64 {
            if let Some(&o) = self.offs.get(n) {
                return o;
            }
            if self.next < 0 && self.next + size > 0 {
                self.next = 0x40;
            }
            let o = self.next;
            self.next += size;
            assert!(self.next <= 0x80, "out of byte offsets at {n}");
            self.offs.insert(n.into(), o);
            o
        }
        fn word(&mut self, n: &str) -> i64 {
            match n {
                "NOTEPER" => 0x7e,
                _ => {
                    if let Some(&o) = self.offs.get(n) {
                        return o;
                    }
                    let o = self.gnext;
                    self.gnext += 2;
                    self.offs.insert(n.into(), o);
                    o
                }
            }
        }
        fn t(&mut self, t: &str) -> &mut Self {
            for tok in t.split_whitespace() {
                if let Some(n) = tok.strip_prefix('=') {
                    self.labels.insert(n.into(), self.d.len());
                    continue;
                }
                if tok == "+T" || tok == "+R" {
                    continue;
                }
                let w = if let Some(n) = tok.strip_prefix('%') {
                    self.fix.push((self.d.len(), n.into()));
                    0
                } else if let Some(n) = tok.strip_prefix('@') {
                    self.word(n) as u16
                } else if tok == "*" {
                    0
                } else {
                    let tok = tok.split('|').next().unwrap();
                    let (hi, lo) = tok.split_at(2);
                    let hi = u8::from_str_radix(hi, 16).unwrap();
                    let lo = if let Some(n) = lo.strip_prefix('@') {
                        if n == "INITN" {
                            0x10
                        } else {
                            let size = match hi {
                                0x00 => 4,
                                0x10 => 8,
                                _ => 16,
                            };
                            self.reserve(n, size) as u8
                        }
                    } else if lo == "??" {
                        2
                    } else {
                        u8::from_str_radix(lo, 16).unwrap()
                    };
                    (hi as u16) << 8 | lo as u16
                };
                self.d.extend(w.to_be_bytes());
            }
            self
        }
        fn label(&mut self, n: &str) -> &mut Self {
            self.labels.insert(n.into(), self.d.len());
            self
        }
        fn bytes(&mut self, b: &[u8]) -> &mut Self {
            self.d.extend(b);
            self
        }
        fn words(&mut self, ws: &[u16]) -> &mut Self {
            for w in ws {
                self.d.extend(w.to_be_bytes());
            }
            self
        }
        fn done(mut self) -> (Vec<u8>, HashMap<String, usize>) {
            for (at, n) in std::mem::take(&mut self.fix) {
                let t = self.labels[&n] as i64 - at as i64;
                self.d[at..at + 2].copy_from_slice(&(t as i16 as u16).to_be_bytes());
            }
            (self.d, self.labels)
        }
    }

    /// One of the earlier replayers (speed and track volume in the track, the
    /// sample's header before it, master volume only): one subsong whose four
    /// voices play the same one-row track, instrument 1 (sample 0, 16 words,
    /// volume 64) at note $18, again and again.
    fn module() -> Vec<u8> {
        let mut g = Gen::new();
        g.reserve("POSOFF", 16);
        g.bytes(&[0; 0x40]);
        // init: the subsong's position lists, each voice's start
        g.t("48e7 fcfe e941 7000 41fa %SUBS 43fa %G")
            .t("23b0 1000 00@POSOFF 5801 5800 0c00 00@INITN 66f0 7003")
            .t("3200 e341 4231 00@TPOS 13bc 0001 00@SPD 13bc 00ff 00@POS 6100 %NEXTPOS 51c8 fff0")
            .t("137c 00ff @PLAYING 137c 00ff @FADE 337c 0040 @MVOL 4229 @SVCNT")
            .t("4279 00df f0a8 4279 00df f0b8 4279 00df f0c8 4279 00df f0d8 33fc 00ff 00df f09e")
            .t("4cdf 3f7f 4e75");
        // play
        g.t(PLAY).t(PLAY_A).t(FADE).t(VOICE).t(ROW0).t(ROW).t(ROW_SPEED).t(ROW_PERIOD).t(CHK_AND).t(LOAD);
        // the instrument's volume (byte 2) to the hardware copy's volume
        g.words(&[0x11b5, 0x5002, 0x200d]);
        g.t(LOAD_SG).t(LOAD_S).t(SMP_INLINE).t(DMA).t(DMA_SMP).t(ROWEND).t(ROWEND_VIBC).t(NOROW);
        g.t(PORTA).t(PORTA_ONE).t(VIB_HEAD).t(VIB_DIV).t(VIB_RUN).label("FXCHAIN").t(FX3).label("FXROW");
        g.t(TAIL).t(END).t(END_SV1).t(FINAL).t(FPER).t(VOL_MASTER);
        g.t(NP).t(NP_FLAG).t(NP_TAIL).t("4e75").label("EXIT").t("4e75");
        // data: the variables (G, H: hardware copies, their DMA bits)
        g.bytes(&[0; 0x80]).label("G").bytes(&[0; 0x180]);
        g.bytes(&[0; 0x80]).label("H");
        for v in 0..4 {
            g.words(&[0, 0, 0, 0, 0, 0, 0, 1 << v]);
        }
        g.bytes(&[0; 0x40]);
        // periods: 856 down by 8 a note
        g.label("PER");
        g.words(&(0..64).map(|n| 856 - 8 * n).collect::<Vec<_>>());
        // instruments 0 and 1: sample 0, volume 64
        g.label("INS").bytes(&[0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        g.bytes(&[0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        // subsong 0: every voice's list at offset 0
        g.label("SUBS").bytes(&[0; 16]);
        // the list: once track 0, then back to its start
        g.label("PLISTS").bytes(&[1, 0, 0xff, 0]);
        g.label("TOFFS").words(&[0]);
        // the track: instrument 1 and track volume $40, speed 4, note $18; end
        g.label("TRACKS").bytes(&[0x81, 0x40, 4, 0x18, 0xff, 0]);
        g.label("SSTART").words(&[0, 0]);
        // the sample: 16 words, the loop from its start, 16 words
        g.label("SDATA").words(&[16, 0, 16]);
        g.bytes(&[0x40; 16]).bytes(&[0xc0; 16]);
        g.done().0
    }

    #[test]
    fn plays_a_note_on_paula() {
        let d = module();
        let l = layout(&d).expect("the port reads its templates back");
        assert_eq!((l.track, l.late, l.sepinfo, l.track_vol, l.min), (Track::Speed, false, false, false, 0));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("test.avp"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "ActivisionPro");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // each voice's note (3 to 0): its DMA off, then all on after the delay
        assert_eq!(&reg(0xDFF096)[..5], &[8, 4, 2, 1, 0x820f]);
        // the note: period $7e, the sample (16 words); then the loop, the
        // note's period ($18: 856 - 8 * 24), the volume (64 * 64 / 64)
        assert_eq!(&reg(0xDFF0A6)[..2], &[0x7e, 856 - 8 * 24]);
        assert_eq!(&reg(0xDFF0A4)[..2], &[16, 16]);
        assert_eq!(&reg(0xDFF0A8)[..2], &[0, 64]);
        let loc = reg(0xDFF0A2);
        assert_eq!(loc[0], loc[1]);
        // the next row 4 ticks later: DMA off again
        assert_eq!(reg(0xDFF096).iter().filter(|&&x| x == 1).count() >= 2, true);
        // the delay loop between the last note and the DMA on
        let at = |v: u16| log.iter().position(|w| w.0 == 0xDFF096 && w.1 == v).unwrap();
        assert!(log[at(0x820f)].2 - log[at(1)].2 >= C_DELAY);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

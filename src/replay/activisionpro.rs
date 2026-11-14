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
    Adk,
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
            0x33fc if !voice && x(2) == 0x00df && x(3) == 0xf09e => (Iop::Adk, 4),
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

/// The code paths of the replayer's tick (as the Eagleplayer runs it), each a whole number of
/// 68000 cycles in a build of the replayer (`Build`).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum K {
    Entry,
    JtNone,
    JtStop,
    JtFade,
    JtInit,
    InitVoice,
    InitOp,
    NotPlaying,
    FadeOff,
    FadeCount,
    FadeReload,
    FadeStop,
    Sv2Carry,
    Sv2NoCarry,
    Voice,
    S1Off,
    S1On,
    Spd2,
    SvcntNz,
    SpdNz,
    RowGo,
    DisSkip,
    NoRow,
    TrkFE,
    TrkFF,
    Trk,
    TvReset,
    TvNo,
    CmdsHead,
    CmdByte,
    CmdT0,
    CmdT1,
    CmdT2,
    CmdT3,
    CmdT4,
    CmdT5,
    CmdT6,
    CmdT7,
    CmdT8,
    CmdT9,
    CmdT10,
    CmdT11,
    CmdH0,
    CmdH1,
    CmdH2,
    CmdH3,
    CmdH4,
    CmdH5,
    CmdH6,
    CmdH7,
    CmdH8,
    CmdH9,
    CmdH10,
    CmdH11,
    ActG,
    ActV,
    ActVW,
    ActAddVW,
    ActImmV,
    ActImmX,
    InitVol,
    LStart,
    VDivQ0,
    StopVol,
    StopDma,
    CmdFall,
    CmdEnd,
    CmdSpeed,
    CmdNone,
    NoteFirstSpd,
    NoteFirst,
    NoteLast,
    SpeedHead,
    SpeedCmd,
    SpeedTvol,
    SpeedPt,
    SpeedTail,
    FlagsHead,
    Flags40,
    Flags80,
    FlagsTmp,
    FlagsPt,
    FlagsTail,
    Period,
    S2Dis0,
    S2Sfx,
    S2Clr,
    ChkNone,
    ChkSame,
    ChkNew,
    NfNeg,
    Instr,
    S3Sfx,
    S3Clr,
    NoteOff,
    NotePer,
    NoteLoc,
    NoteLen,
    PeriodAfter,
    FlagsEnd,
    RowEndNo,
    RowEndFF,
    LoopcNz,
    LoopcZ,
    RowDis,
    RowTail,
    NpHead,
    NpIter,
    NpFd,
    NpJump,
    NpCnt0,
    NpCntDec,
    NpCntEnd,
    NpFlagStart,
    NpFlagDec,
    NpFlagEnd,
    NpNo40,
    NpTail,
    PortaYes,
    PortaNo,
    PortaBothYes,
    PortaBothNo,
    VDel,
    VSdelay,
    VZero,
    VStartDiv,
    VStartHalve,
    VHalveIter,
    VHalveZero,
    VHalveEnd,
    VRunAdd,
    VRunSub,
    VCntUp,
    VCntMax,
    VCntDown,
    VCntZero,
    VCntDownNz,
    FxRow,
    FxTick,
    Fx1No,
    Fx1Even,
    Fx1Abs,
    Fx1Rel,
    Fx2No,
    Fx2Even,
    Fx2Odd,
    Fx2Neg,
    Fx3No,
    Fx3Yes,
    Fx4No,
    Fx4Sd,
    Fx4Stop,
    Fx4None,
    Fx5No,
    Fx5Sd,
    Fx5Stop,
    Fx5None,
    ArpAbs,
    ArpUp,
    ArpDown,
    Per,
    Env,
    EnvNew,
    EnvNfNeg,
    EnvTickNz,
    EnvWaitNeg,
    EnvWaitDec,
    EnvStep,
    EnvVol,
    Env40,
    Env0,
    EnvCntNz,
    EnvNext,
    EnvLoop,
    EnvPosF,
    EnvCnt,
    SdZero,
    SdDec,
    VoiceEnd,
    DmaOn,
    SvDec,
    SvStep,
    SvWrap,
    Sv2Fin,
    FDis,
    FLoc,
    FLen,
    FPer,
    GateShut,
    GateOpen,
    Vol,
    GVol,
    FVol,
    FNext,
    LMute,
    LGateShut,
    LVol,
    LPer,
    LLoop,
    LNoLoop,
    LLen,
    N,
}

const NK: usize = K::N as usize;

/// A row command's test, and the command taken (by its place in the list).
const CMD_T: [K; 12] = [K::CmdT0, K::CmdT1, K::CmdT2, K::CmdT3, K::CmdT4, K::CmdT5, K::CmdT6, K::CmdT7, K::CmdT8, K::CmdT9, K::CmdT10, K::CmdT11];
const CMD_H: [K; 12] = [K::CmdH0, K::CmdH1, K::CmdH2, K::CmdH3, K::CmdH4, K::CmdH5, K::CmdH6, K::CmdH7, K::CmdH8, K::CmdH9, K::CmdH10, K::CmdH11];

/// A build of the replayer: which of its writes the Eagleplayer turns into its ENPP_Poke calls
/// (a call's write comes at the previous call's clock plus half the cycles since, rounded
/// down, and the next counts from it; the code's own writes count from the last call), and
/// the cycles of its paths.
struct Build {
    /// The note's DMA off, period, sample and length; the final loop's volume; all of the
    /// later replayers' final loop.
    note_host: bool,
    vol_host: bool,
    final_host: bool,
    /// InitSound's cycles before its first write, besides the position lists' paths and the
    /// volume write's call (the voices' and the globals' clears are the same in every module
    /// of a build).
    init: u16,
    c: [u16; NK],
}

pub struct ActivisionPro {
    l: Lay,
    /// The module's copy in guest memory.
    mb: u32,
    sub: u16,
    /// The clock the cycles count from (the tick's start, the last ENPP call), and the cycles
    /// since.
    base: u64,
    cyc: u64,
    b: &'static Build,
    trace: Trace,
    tickno: u64,
    /// In InitSound: the voices' and globals' clears count in `Build::init`.
    isnd: bool,
    /// A track ended with its end byte (the Eagleplayer reports the song's end there: the engine's
    /// empty-subsong skip then goes on to the next subsong, as it does after the original).
    ended: bool,
}

/// AVP_TRACE=file: each write's clock and the paths since the previous one (for fitting the
/// paths' cycles to the original's log).
struct Trace {
    out: Option<std::io::BufWriter<std::fs::File>>,
    pend: Vec<K>,
}

impl Trace {
    fn open() -> Self {
        let out = std::env::var_os("AVP_TRACE").and_then(|p| std::fs::File::create(p).ok()).map(std::io::BufWriter::new);
        Trace { out, pend: Vec::new() }
    }
    fn line(&mut self, s: std::fmt::Arguments) {
        if let Some(o) = &mut self.out {
            use std::io::Write;
            let _ = write!(o, "{s}");
            for e in self.pend.drain(..) {
                let _ = write!(o, " {:?}", e);
            }
            let _ = writeln!(o);
        }
    }
}

/// The build's signature: what the templates read of its code, without the variables' offsets.
fn signature(l: &Lay) -> String {
    let fields: Vec<String> = l
        .fields
        .iter()
        .map(|(_, d, w)| format!("{}{}", match d { Dst::G(_) => 'g', Dst::V(_) => 'v', Dst::X(_) => 'x', Dst::H(_) => 'h' }, *w as u8))
        .collect();
    let cmds: Vec<String> = l.cmds.iter().map(|(_, a, n)| format!("{}{:?}", a.len(), n)).collect();
    let b = |x: bool| x as u8;
    format!(
        "{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{} {:?} {:?} {} {}",
        b(l.jt), b(l.late), b(l.mute), b(l.nf_clear), b(l.spd2), b(l.gate_lo), b(l.gvol), b(l.note_first),
        b(l.sofs), b(l.fine), b(l.sv2), b(l.tvreset), b(l.period_after), b(l.chk_and), b(l.transp),
        b(l.row_transp), b(l.note_store), b(l.gate), b(l.np_count), b(l.np_maxff), b(l.sample_v), b(l.sepinfo),
        b(l.both), b(l.vib_halve), b(l.env), b(l.env_byte), b(l.row_vibw), b(l.track_vol), b(l.s1), b(l.s2),
        b(l.s3), b(l.v.contains_key("GTRANSP")), b(l.v.contains_key("FXT")), l.track, l.fx, fields.join(","), cmds.join(",")
    )
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x100_0000_01b3))
}

fn build(l: &Lay) -> &'static Build {
    let h = fnv(&signature(l));
    BUILDS.iter().find(|(k, _)| *k == h).map(|(_, b)| b).unwrap_or(&BUILDS[0].1)
}

/// The cycles of each path (enum K's order), fitted to the reference's write clocks; a path
/// not taken in the fitted files has a related build's.
#[rustfmt::skip]
const C_E4A8DC2F9BC5F1B8: [u16; NK] = [0, 22, 0, 0, 0, 0, 33, 0, 393, 25, 45, 0, 126, 0, 478, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 438, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 0, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 0, 148, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 994, 129, 190, 180, 212, 0, 0, 0, 59, 0, 0, 0, 0, 0, 168, 0, 0, 115, 88, 0, 133, 88, 0, 0, 60, 20, 0, 0, 0, 0, 0, 36, 466, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 816, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 69, 0, 0, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 218, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_570D6E097F80C0E4: [u16; NK] = [0, 22, 0, 0, 0, 0, 4080, 0, 86, 25, 45, 0, 126, 0, 0, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 461, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 0, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 0, 148, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 994, 0, 190, 180, 212, 0, 0, 0, 59, 0, 0, 0, 0, 0, 0, 0, 0, 115, 88, 0, 133, 88, 0, 0, 228, 20, 0, 0, 0, 0, 0, 36, 466, 0, 219, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 62, 0, 50, 0, 0, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 7, 509, 353, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 218, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_FA5122DE7B3A347D: [u16; NK] = [0, 29, 0, 0, 0, 0, 4080, 0, 342, 25, 45, 0, 126, 0, 360, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 127, 0, 0, 0, 35, 17, 38, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 4, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 0, 148, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 493, 0, 18, 60, 36, 0, 0, 0, 59, 0, 0, 0, 382, 0, 0, 0, 0, 115, 88, 0, 133, 88, 0, 0, 228, 20, 0, 0, 0, 0, 0, 36, 466, 0, 219, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 62, 0, 50, 0, 0, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 7, 26, 0, 473, 63, 16, 464, 0, 239, 24, 42, 0, 0, 467, 0, 0, 0, 0, 0, 385, 206, 203, 15, 212];
#[rustfmt::skip]
const C_D23E3AA417E2EFD5: [u16; NK] = [0, 22, 0, 0, 0, 0, 33, 0, 342, 25, 45, 0, 126, 0, 0, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 0, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 138, 10, 0, 0, 0, 54, 69, 43, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 994, 1096, 190, 180, 212, 0, 0, 0, 59, 0, 0, 0, 465, 0, 168, 0, 0, 115, 88, 0, 133, 88, 0, 0, 60, 20, 0, 0, 0, 0, 0, 36, 466, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 0, 0, 23, 0, 27, 0, 0, 0, 0, 0, 98, 44, 0, 0, 0, 69, 424, 51, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 218, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_C5A4C94CD81F5AC4: [u16; NK] = [0, 22, 0, 0, 0, 0, 33, 0, 0, 25, 45, 0, 126, 0, 326, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 267, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 4, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 138, 10, 0, 0, 0, 54, 69, 43, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 747, 794, 190, 180, 212, 0, 0, 0, 59, 0, 0, 0, 107, 0, 168, 0, 0, 115, 88, 0, 133, 88, 0, 0, 60, 20, 0, 0, 0, 0, 0, 36, 466, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 0, 0, 23, 0, 27, 0, 0, 0, 0, 0, 98, 44, 0, 0, 0, 69, 0, 393, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 282, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_612E1C1AEF80288D: [u16; NK] = [265, 22, 0, 0, 0, 0, 4080, 0, 0, 25, 45, 0, 126, 0, 357, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 25, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 0, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 138, 10, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 843, 0, 190, 180, 212, 0, 0, 0, 32, 27, 0, 0, 372, 111, 126, 0, 0, 115, 88, 0, 133, 88, 0, 0, 18, 20, 0, 0, 0, 0, 0, 36, 470, 0, 219, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 62, 0, 50, 0, 0, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 7, 0, 128, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 282, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_6ECD0C303FA7FD5D: [u16; NK] = [0, 22, 0, 0, 0, 0, 4080, 0, 393, 25, 45, 0, 126, 0, 0, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 200, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 4, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 0, 148, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 747, 698, 190, 180, 212, 0, 0, 0, 32, 27, 0, 0, 174, 69, 168, 0, 0, 115, 88, 0, 133, 88, 0, 0, 18, 20, 0, 0, 0, 0, 0, 36, 466, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 816, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 69, 326, 0, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 282, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_F147A85DC8A757D4: [u16; NK] = [0, 22, 0, 0, 0, 0, 4080, 0, 393, 25, 45, 0, 126, 0, 0, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 254, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 0, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 0, 148, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 747, 0, 190, 180, 212, 0, 0, 0, 59, 0, 0, 0, 143, 0, 168, 0, 0, 115, 88, 0, 133, 88, 0, 0, 60, 20, 0, 0, 0, 0, 0, 36, 470, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 816, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 69, 357, 0, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 282, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_553F66D07BCE3739: [u16; NK] = [325, 22, 0, 0, 0, 0, 4080, 0, 17, 25, 45, 0, 126, 0, 43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 931, 0, 6, 0, 0, 151, 0, 0, 0, 35, 17, 17, 0, 0, 43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 4, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 0, 148, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 8, 0, 32, 0, 0, 0, 0, 0, 190, 180, 212, 0, 0, 0, 59, 0, 0, 0, 181, 0, 210, 0, 0, 115, 88, 0, 133, 88, 0, 0, 18, 20, 0, 0, 0, 0, 0, 0, 466, 0, 219, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 62, 0, 50, 0, 0, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 7, 379, 0, 473, 63, 16, 464, 0, 239, 24, 24, 0, 0, 467, 0, 0, 0, 0, 0, 385, 206, 203, 15, 212];
#[rustfmt::skip]
const C_85DDB921B3AF0EAE: [u16; NK] = [0, 22, 0, 0, 0, 0, 4080, 0, 0, 25, 45, 0, 126, 0, 219, 0, 0, 0, 0, 29, 0, 0, 0, 0, 0, 1072, 0, 6, 0, 0, 127, 0, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 4, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 138, 10, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 747, 0, 190, 180, 212, 0, 0, 0, 59, 0, 0, 0, 0, 0, 168, 0, 0, 115, 88, 0, 133, 88, 0, 0, 60, 20, 0, 0, 0, 0, 0, 36, 466, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 816, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 0, 69, 107, 393, 473, 63, 16, 464, 0, 95, 24, 24, 37, 0, 282, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_71CE06C5380C8392: [u16; NK] = [0, 22, 0, 0, 0, 0, 33, 0, 776, 25, 45, 0, 126, 0, 286, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 143, 0, 0, 0, 35, 17, 17, 0, 0, 43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 4, 170, 176, 0, 0, 0, 0, 0, 0, 0, 0, 138, 10, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 493, 0, 18, 60, 36, 0, 0, 0, 59, 0, 0, 0, 775, 0, 0, 0, 0, 115, 88, 0, 133, 88, 0, 0, 228, 20, 0, 0, 0, 0, 0, 36, 466, 0, 219, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 62, 0, 50, 0, 0, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 151, 7, 0, 1390, 473, 63, 16, 464, 0, 239, 24, 42, 0, 0, 254, 0, 0, 0, 0, 0, 385, 206, 203, 15, 212];
#[rustfmt::skip]
const C_0C43C57B71E9FCD5: [u16; NK] = [84, 22, 0, 0, 0, 0, 33, 0, 0, 25, 45, 0, 126, 0, 218, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 378, 0, 6, 0, 0, 40, 16, 0, 0, 0, 36, 17, 0, 0, 43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 0, 4, 170, 176, 0, 105, 0, 0, 0, 0, 0, 0, 138, 10, 0, 0, 0, 54, 0, 112, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 493, 431, 18, 60, 36, 0, 0, 0, 59, 0, 0, 0, 0, 0, 140, 0, 0, 115, 88, 0, 133, 88, 0, 0, 88, 20, 0, 0, 0, 0, 0, 36, 466, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 816, 0, 23, 0, 27, 0, 151, 0, 0, 0, 98, 44, 0, 0, 151, 69, 68, 2111, 473, 63, 16, 464, 0, 239, 24, 42, 37, 0, 291, 0, 0, 0, 0, 0, 348, 206, 203, 15, 212];
#[rustfmt::skip]
const C_8BF757FF01470696: [u16; NK] = [73, 29, 0, 0, 0, 0, 31, 0, 247, 25, 45, 0, 198, 0, 304, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 40, 16, 0, 0, 13, 0, 38, 0, 0, 43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 170, 17, 4, 170, 176, 0, 105, 0, 0, 0, 0, 0, 0, 138, 10, 0, 0, 0, 54, 69, 43, 0, 0, 0, 0, 0, 0, 0, 0, 32, 0, 0, 0, 493, 444, 18, 60, 36, 0, 0, 0, 59, 0, 0, 0, 395, 0, 140, 0, 0, 115, 88, 0, 133, 88, 0, 0, 106, 20, 0, 0, 0, 0, 0, 36, 466, 294, 72, 19, 0, 174, 164, 28, 74, 0, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 0, 0, 0, 96, 0, 0, 0, 23, 0, 27, 0, 0, 0, 0, 0, 98, 44, 0, 0, 0, 69, 0, 1796, 473, 63, 16, 464, 0, 239, 24, 42, 37, 0, 481, 0, 0, 0, 0, 0, 183, 42, 55, 23, 24];

/// The builds known, by their signature's hash.
static BUILDS: [(u64, Build); 13] = [
    (0xe4a8dc2f9bc5f1b8, Build { note_host: true, vol_host: false, final_host: false, init: 1129, c: C_E4A8DC2F9BC5F1B8 }),
    (0x570d6e097f80c0e4, Build { note_host: true, vol_host: false, final_host: false, init: 1129, c: C_570D6E097F80C0E4 }),
    (0xfa5122de7b3a347d, Build { note_host: false, vol_host: true, final_host: false, init: 1253, c: C_FA5122DE7B3A347D }),
    (0xd23e3aa417e2efd5, Build { note_host: true, vol_host: false, final_host: false, init: 1129, c: C_D23E3AA417E2EFD5 }),
    (0xc5a4c94cd81f5ac4, Build { note_host: true, vol_host: false, final_host: false, init: 1129, c: C_C5A4C94CD81F5AC4 }),
    (0x612e1c1aef80288d, Build { note_host: true, vol_host: false, final_host: false, init: 1021, c: C_612E1C1AEF80288D }),
    (0x6ecd0c303fa7fd5d, Build { note_host: true, vol_host: false, final_host: false, init: 1025, c: C_6ECD0C303FA7FD5D }),
    (0xf147a85dc8a757d4, Build { note_host: true, vol_host: false, final_host: false, init: 1133, c: C_F147A85DC8A757D4 }),
    (0x553f66d07bce3739, Build { note_host: true, vol_host: true, final_host: false, init: 1253, c: C_553F66D07BCE3739 }),
    (0x85ddb921b3af0eae, Build { note_host: true, vol_host: false, final_host: false, init: 1181, c: C_85DDB921B3AF0EAE }),
    (0x71ce06c5380c8392, Build { note_host: false, vol_host: false, final_host: true, init: 1253, c: C_71CE06C5380C8392 }),
    (0x0c43c57b71e9fcd5, Build { note_host: false, vol_host: false, final_host: true, init: 0, c: C_0C43C57B71E9FCD5 }),
    (0x8bf757ff01470696, Build { note_host: false, vol_host: false, final_host: false, init: 0, c: C_8BF757FF01470696 }),
];

/// The Eagleplayer's check of a module, in colour clocks (measured on modified copies): a scan
/// of the words before the play routine, with time for each 48e7 word and 48e7 fcfe pair
/// besides the init routine's, and for an rts at $28; a module the scan fails on (its routines
/// reached through a jump table) is scanned whole.
fn check_time(d: &[u8]) -> u64 {
    let r = Rd(d);
    let rts = 2 * (r.w(0x28) == 0x4e75) as u64;
    match ep(d) {
        Some(e) if !e.jt => {
            let ws: Vec<u16> = (0..e.play).step_by(2).map(|o| r.w(o)).collect();
            let c1 = ws.iter().filter(|&&w| w == 0x48e7).count() as u64;
            let c2 = ws.windows(2).filter(|p| p[0] == 0x48e7 && p[1] == 0xfcfe).count() as u64;
            843 + 101 * (e.play as u64 / 2) + 30 * (c1.saturating_sub(1) + c2.saturating_sub(1)) + rts
        }
        _ => 101 * d.len() as u64 + 521 + rts,
    }
}

fn new(d: &[u8], _base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let l = layout(d)?;
    if std::env::var_os("DELI_AVP_DEBUG").is_some() {
        let mut k = l.clone();
        k.v.clear();
        k.cmds.clear();
        eprintln!("activisionpro: {k:?}");
        eprintln!("activisionpro: build {:016x} {}", fnv(&signature(&l)), signature(&l));
    }
    // the Eagleplayer's copy: module size + $2000, the module at +$1000
    let blk = m.alloc(d.len() as u32 + 0x2000);
    let mb = blk + 0x1000;
    m.bus.write_bytes(mb, d);
    let b = build(&l);
    // the Eagleplayer's check, then InitPlayer (the check again and its own work), then the
    // calls before InitSound
    let x = check_time(d);
    m.bus.clock += 2 * x + 59827 + 4522 + 20;
    let mut a = ActivisionPro { sub: l.min, l, mb, base: m.bus.clock, cyc: 0, b, trace: Trace::open(), tickno: 0, isnd: false, ended: false };
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
        // the timer goes on: a tick due during InitSound comes when it ends
        self.init(n, hw);
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.tickno += 1;
        self.trace.line(format_args!("E"));
        self.base = hw.bus.clock;
        self.cyc = 0;
        self.trace.line(format_args!("T {} {}", self.tickno, self.base));
        self.k(K::Entry);
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

    /// The original's code takes path `e`.
    fn k(&mut self, e: K) {
        if !(self.isnd && matches!(e, K::InitVoice | K::InitOp)) {
            self.cyc += self.b.c[e as usize] as u64;
        }
        if self.trace.out.is_some() {
            self.trace.pend.push(e);
        }
    }

    /// A write (two words for a sample's address) at the clock the cycles since the base give;
    /// an ENPP call's starts the next count (DMA on: after its wait, to the start of the 10th
    /// raster line after the write's).
    fn wr(&mut self, hw: &mut Hw, host: bool, a: u32, x: u32) {
        let t = self.base + self.cyc / 2;
        if hw.bus.clock < t {
            hw.bus.clock = t;
        }
        self.trace.line(format_args!("W {} {:06x} {:x} {} {}", self.tickno, a, x, host as u8, hw.bus.clock));
        if a & 0xf == 0 {
            hw.bus.write_word(a, (x >> 16) as u16);
            hw.bus.write_word(a + 2, x as u16);
        } else {
            hw.bus.write_word(a, x as u16);
        }
        if host {
            self.base = hw.bus.clock;
            self.cyc = 0;
            if a == 0xdff096 && x & 0x8000 != 0 {
                use crate::paula::LINE;
                self.base = (hw.bus.clock / LINE + 10) * LINE;
                hw.bus.clock = self.base;
            }
        }
    }

    /// The Eagleplayer's InitSound: the replayer's init routine.
    /// It ends 164 colour clocks after its last volume write (a jump table's: 109 after it
    /// starts).
    fn init(&mut self, n: u16, hw: &mut Hw) {
        self.ended = false;
        self.base = hw.bus.clock;
        if self.l.jt {
            self.sgb(hw, "REQ", n as u8);
            hw.bus.clock += 109;
            return;
        }
        self.cyc = self.b.init as u64;
        self.isnd = true;
        self.init_sub(n, hw);
        self.isnd = false;
        hw.bus.clock = hw.bus.clock.max(self.base + (self.cyc + 328) / 2);
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
            self.k(K::InitVoice);
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
            Iop::NextPos => {}
            Iop::Vol(_) => self.k(K::InitVol),
            _ => self.k(K::InitOp),
        }
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
            Iop::Vol(ch) => self.wr(hw, true, 0xdff0a8 + 16 * ch as u32, 0),
            // the reference never shows the module's ADKCON write (0x00ff, clearing the
            // modulation bits that are clear anyway): only the time it takes is kept
            Iop::Adk => {}
            // the Eagleplayer's ENPP_ModuleChange patch replaces the module's bset.b #1,$bfe001
            // with a call that leaves the LED bit alone (the original's LED stays on): only
            // the time it takes is kept
            Iop::Led => {}
        }
    }

    /// The next entry of the voice's position list.
    fn nextpos(&mut self, hw: &mut Hw, v: usize) {
        let a4 = self.a("PLISTS").wrapping_add(self.vw(hw, v, "POSOFF") as u32);
        self.k(K::NpHead);
        loop {
            self.k(K::NpIter);
            let pos = self.vb(hw, v, "POS").wrapping_add(1);
            self.svb(hw, v, "POS", pos);
            let mut d5 = pos;
            let b = hw.bus.r8(a4 + d5 as u32);
            if b >= 0xfd {
                if b == 0xfd {
                    self.k(K::NpFd);
                    self.svb(hw, v, "POS", pos.wrapping_add(1));
                    d5 = d5.wrapping_add(1);
                    let f = hw.bus.r8(a4 + d5 as u32);
                    self.sgb(hw, "FADE", f);
                    continue;
                }
                self.k(K::NpJump);
                self.svb(hw, v, "TRACK", b);
                d5 = d5.wrapping_add(1);
                let p = hw.bus.r8(a4 + d5 as u32);
                self.svb(hw, v, "POS", p.wrapping_sub(1));
                return;
            }
            if b & 0x40 != 0 && self.l.np_count {
                let c = self.vb(hw, v, "PLC");
                if c == 0 {
                    self.k(K::NpCnt0);
                    self.svb(hw, v, "PLC", b & 0x3f);
                    let p = self.vb(hw, v, "POS").wrapping_add(1);
                    self.svb(hw, v, "POS", p);
                    d5 = d5.wrapping_add(1);
                    self.svb(hw, v, "PLS", d5);
                } else {
                    self.svb(hw, v, "PLC", c - 1);
                    if c == 1 {
                        self.k(K::NpCntEnd);
                        continue;
                    }
                    self.k(K::NpCntDec);
                    let s = self.vb(hw, v, "PLS");
                    self.svb(hw, v, "POS", s);
                    d5 = s;
                }
            } else if b & 0x40 != 0 {
                if (self.vb(hw, v, "PLEN") as i8) >= 0 {
                    self.k(K::NpFlagStart);
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
                        self.k(K::NpFlagEnd);
                        self.svb(hw, v, "PLEN", 0);
                        continue;
                    }
                    self.k(K::NpFlagDec);
                    let s = self.vb(hw, v, "PLS");
                    self.svb(hw, v, "POS", s);
                    d5 = s;
                }
            } else {
                self.k(K::NpNo40);
            }
            self.k(K::NpTail);
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
            self.k(K::StopVol);
            self.wr(hw, true, 0xdff0a8 + 16 * ch, 0);
        }
        self.k(K::StopDma);
        self.wr(hw, true, 0xdff096, 0xf);
    }

    fn play(&mut self, hw: &mut Hw) {
        if self.l.jt {
            let d1 = self.gb(hw, "REQ");
            self.sgb(hw, "REQ", 0xff);
            if d1 == 0x80 {
                self.k(K::JtStop);
                self.stop(hw);
                return;
            } else if d1 & 0x80 != 0 && d1 != 0xff {
                self.k(K::JtFade);
                self.sgb(hw, "FADE", d1 & 0x7f);
            } else if d1 & 0x80 == 0 {
                self.k(K::JtInit);
                self.init_sub(d1 as u16, hw);
            } else {
                self.k(K::JtNone);
            }
        }
        if (self.gb(hw, "PLAYING") as i8) >= 0 {
            self.k(K::NotPlaying);
            return;
        }
        if (self.gb(hw, "FADE") as i8) >= 0 {
            self.k(K::FadeCount);
            let c = self.gb(hw, "FADECNT").wrapping_sub(1);
            self.sgb(hw, "FADECNT", c);
            if (c as i8) < 0 {
                self.k(K::FadeReload);
                let f = self.gb(hw, "FADE");
                self.sgb(hw, "FADECNT", f);
                let m = self.gw(hw, "MVOL").wrapping_sub(1);
                self.sgw(hw, "MVOL", m);
                if m == 0 {
                    self.k(K::FadeStop);
                    // the Eagleplayer reports the song's end when the fade stops it (R-Type II)
                    self.ended = true;
                    self.stop(hw);
                    return;
                }
            }
        } else {
            self.k(K::FadeOff);
        }
        if self.l.sv2 {
            let (acc, carry) = self.gb(hw, "SVACC").overflowing_add(self.gb(hw, "SVINC"));
            self.k(if carry { K::Sv2Carry } else { K::Sv2NoCarry });
            self.sgb(hw, "SVCNT", if carry { 0 } else { 0xff });
            self.sgb(hw, "SVACC", acc);
        }
        for v in (0..4).rev() {
            self.k(K::Voice);
            if self.l.s1 {
                if self.sfx(hw, v) != 0 {
                    self.k(K::S1On);
                    self.svb(hw, v, "DIS", 0xff);
                } else {
                    self.k(K::S1Off);
                }
            }
            if self.l.spd2 && self.gb(hw, "SVCNT") == 0 {
                self.k(K::Spd2);
                let s = self.vb(hw, v, "SPD2").wrapping_add(1);
                self.svb(hw, v, "SPD2", s);
            }
            let row = if self.gb(hw, "SVCNT") != 0 {
                self.k(K::SvcntNz);
                false
            } else {
                let s = self.vb(hw, v, "SPD").wrapping_sub(1);
                self.svb(hw, v, "SPD", s);
                if s != 0 {
                    self.k(K::SpdNz);
                }
                s == 0
            };
            if row {
                self.k(K::RowGo);
                if !self.row(hw, v) {
                    return;
                }
            } else {
                if (self.vb(hw, v, "DIS") as i8) < 0 {
                    self.k(K::DisSkip);
                } else {
                    self.k(K::NoRow);
                    self.norow(hw, v);
                    self.fx(hw, v, false);
                }
            }
            self.k(K::VoiceEnd);
        }
        if self.l.late {
            self.final_late(hw);
            return;
        }
        // the delay loop, all voices on
        self.k(K::DmaOn);
        self.wr(hw, true, 0xdff096, 0x800f);
        let sv = if self.l.sv2 { 0 } else { self.gb(hw, "SVCNT").wrapping_sub(1) };
        if !self.l.sv2 {
            self.k(K::SvDec);
            self.sgb(hw, "SVCNT", sv);
        } else {
            self.k(K::Sv2Fin);
        }
        if !self.l.sv2 && (sv as i8) < 0 {
            self.k(K::SvStep);
            let mut i = self.gb(hw, "SVIDX").wrapping_sub(1);
            if (i as i8) < 0 {
                self.k(K::SvWrap);
                i = 7;
            }
            self.sgb(hw, "SVIDX", i);
            let x = hw.bus.r8(self.g().wrapping_add(self.o("SVTAB")) + i as u32);
            self.sgb(hw, "SVCNT", x);
        }
        let vh = self.b.vol_host;
        for v in (0..4).rev() {
            if (self.vb(hw, v, "DIS") as i8) < 0 {
                self.k(K::FDis);
                continue;
            }
            let r = 0xdff0a0 + 16 * v as u32;
            let lp = hw.bus.r32(self.hreg(v, 6));
            self.k(K::FLoc);
            self.wr(hw, false, r, lp);
            let len = hw.bus.r16(self.hreg(v, 0xa));
            self.k(K::FLen);
            self.wr(hw, false, r + 4, len as u32);
            let mut per = self.xw(hw, v, "PERIOD");
            if self.l.fine {
                per = per.wrapping_sub(self.xw(hw, v, "FINE"));
            }
            self.k(K::FPer);
            self.wr(hw, false, r + 6, per as u32);
            let d3 = self.volume(hw, v);
            self.k(K::FVol);
            self.wr(hw, vh, r + 8, d3 as u32);
            self.k(K::FNext);
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
    fn volume(&mut self, hw: &Hw, v: usize) -> u16 {
        if self.l.gate {
            if !self.gated(hw, v) {
                self.k(K::GateShut);
                return 0;
            }
            self.k(K::GateOpen);
        }
        self.k(K::Vol);
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
        self.k(K::GVol);
        let p = (d3 & 0xffff) * self.gw(hw, "GVOL") as u32;
        let q = p / 0x40;
        (if q > 0xffff { p } else { q }) as u16
    }

    /// The later replayers' end: the delay, all voices on, then each voice's
    /// volume, period, loop (once its stop delay has run out) and its stop
    /// delay.
    fn final_late(&mut self, hw: &mut Hw) {
        self.k(K::DmaOn);
        self.wr(hw, true, 0xdff096, 0x800f);
        self.k(K::LStart);
        let fh = self.b.final_host;
        for v in (0..4).rev() {
            if (self.vb(hw, v, "DIS") as i8) < 0 {
                self.k(K::FDis);
                continue;
            }
            let r = 0xdff0a0 + 16 * v as u32;
            let vol = if (self.xb(hw, v, "MUTE") as i8) < 0 {
                self.k(K::LMute);
                0
            } else {
                self.volume(hw, v)
            };
            self.k(K::LVol);
            self.wr(hw, fh, r + 8, vol as u32);
            let per = self.xw(hw, v, "PERIOD").wrapping_sub(self.xw(hw, v, "FINE"));
            self.k(K::LPer);
            self.wr(hw, fh, r + 6, per as u32);
            // the Eagleplayer's ENPP_PokePer call leaves the period in d0, the voice's index
            // in the replayer's test of the stop flag that follows: the reference writes the
            // loop exactly when the byte the period indexes is not 0
            let idx = if fh { per as i16 as i32 as u32 } else { v as u32 };
            if hw.bus.r8(self.g().wrapping_add(self.o("STOP")).wrapping_add(idx)) != 0 {
                let lp = hw.bus.r32(self.hreg(v, 6));
                self.k(K::LLoop);
                self.wr(hw, fh, r, lp);
                let len = hw.bus.r16(self.hreg(v, 0xa));
                self.k(K::LLen);
                self.wr(hw, fh, r + 4, len as u32);
            } else {
                self.k(K::LNoLoop);
            }
            self.stop_delay(hw, v);
            self.k(K::FNext);
        }
    }

    /// A voice's new row; false when the song stops.
    fn row(&mut self, hw: &mut Hw, v: usize) -> bool {
        let t = self.vb(hw, v, "TRACK");
        if t >= 0xfe {
            if t == 0xfe {
                self.k(K::TrkFE);
                self.ended = true;
                self.stop(hw);
                return false;
            }
            self.k(K::TrkFF);
            self.svb(hw, v, "TPOS", 0);
            self.nextpos(hw, v);
        }
        self.k(K::Trk);
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
        if self.l.tvreset {
            if d4 == 0 {
                self.k(K::TvReset);
                hw.bus.w8(self.g().wrapping_add(self.o("TVOLB")) + 2 * v as u32, 0x40);
            } else {
                self.k(K::TvNo);
            }
        }
        match self.l.track {
            Track::Cmds => {
                self.k(K::CmdsHead);
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
                        self.k(K::CmdNone);
                        break;
                    }
                    self.k(K::CmdByte);
                    d4 = d4.wrapping_add(1);
                    let x = rd(hw, d4);
                    let cmds = std::mem::take(&mut self.l.cmds);
                    let mut out = None;
                    for (ci, (c, acts, next)) in cmds.iter().enumerate() {
                        self.k(CMD_T[ci.min(11)]);
                        if b != *c {
                            continue;
                        }
                        self.k(CMD_H[ci.min(11)]);
                        let g = self.g();
                        for &a in acts {
                            self.k(match a {
                                Act::G(_) => K::ActG,
                                Act::V(_) => K::ActV,
                                Act::VW(_) => K::ActVW,
                                Act::AddVW(_) => K::ActAddVW,
                                Act::ImmV(..) => K::ActImmV,
                                Act::ImmX(..) => K::ActImmX,
                            });
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
                            Next::Fall => {
                                self.k(K::CmdFall);
                                continue;
                            }
                            Next::End => {
                                out = Some(false);
                                break;
                            }
                            Next::Speed => {
                                out = Some(true);
                                break;
                            }
                        }
                    }
                    self.l.cmds = cmds;
                    if out == Some(true) {
                        self.k(K::CmdSpeed);
                        speed = true;
                        break 'row;
                    }
                    self.k(K::CmdEnd);
                    d4 = d4.wrapping_add(1);
                    tpos_add(self, hw, 2);
                }
                if self.l.note_first {
                    self.k(if speed { K::NoteFirstSpd } else { K::NoteFirst });
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
                    self.k(K::NoteLast);
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
                self.k(K::SpeedHead);
                self.sgb(hw, "TMP", 0);
                self.svb(hw, v, "PT", 0);
                self.svb(hw, v, "STOP", 0);
                let b = rd(hw, d4);
                if b & 0x80 != 0 {
                    self.k(K::SpeedCmd);
                    self.sgb(hw, "TMP", b);
                    d4 = d4.wrapping_add(1);
                    let b2 = rd(hw, d4);
                    if b2 & 0x80 == 0 {
                        self.k(K::SpeedTvol);
                        hw.bus.w8(self.g().wrapping_add(self.o("TVOLB")) + 2 * v as u32, b2);
                    } else {
                        self.k(K::SpeedPt);
                        self.svb(hw, v, "PT", b2);
                    }
                    d4 = d4.wrapping_add(1);
                    tpos_add(self, hw, 2);
                }
                self.k(K::SpeedTail);
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
                self.k(K::FlagsHead);
                self.sgb(hw, "TMP", 0xff);
                let b = rd(hw, d4);
                self.svb(hw, v, "TBYTE", b);
                self.svb(hw, v, "SPD", b & 0x3f);
                self.svb(hw, v, "PT", 0);
                self.svb(hw, v, "STOP", 0);
                if b & 0x40 != 0 {
                    self.k(K::Flags40);
                    self.sxb(hw, v, "ENVWAIT", 1);
                    self.sxb(hw, v, "ENVCNT", 1);
                    self.sxb(hw, v, "ENVPOS", 9);
                }
                if b & 0x80 != 0 {
                    self.k(K::Flags80);
                    d4 = d4.wrapping_add(1);
                    tpos_add(self, hw, 1);
                    let b2 = rd(hw, d4);
                    if b2 & 0x80 == 0 {
                        self.k(K::FlagsTmp);
                        self.sgb(hw, "TMP", b2);
                    } else {
                        self.k(K::FlagsPt);
                        self.svb(hw, v, "PT", b2);
                    }
                }
                self.k(K::FlagsTail);
                d4 = d4.wrapping_add(1);
                tpos_add(self, hw, 1);
                let n = rd(hw, d4);
                self.svb(hw, v, "NF", n);
                self.svb(hw, v, "NOTE", n & 0x7f);
            }
        }
        if !self.l.period_after {
            self.k(K::Period);
            self.note_period(hw, v);
        }
        if self.l.s2 {
            if self.vb(hw, v, "DIS") == 0 {
                self.k(K::S2Dis0);
            } else if self.sfx(hw, v) != 0 {
                self.k(K::S2Sfx);
            } else {
                self.k(K::S2Clr);
                self.svb(hw, v, "NF", 0);
            }
        }
        let tmp = self.gb(hw, "TMP");
        let change = if self.l.chk_and {
            (tmp != 0).then_some(tmp & 0x7f)
        } else {
            ((tmp as i8) >= 0).then_some(tmp)
        };
        match change {
            None => self.k(K::ChkNone),
            Some(i) if i == self.vb(hw, v, "INSTR") => self.k(K::ChkSame),
            Some(i) => {
                self.k(K::ChkNew);
                self.svb(hw, v, "INSTR", i);
                self.svb(hw, v, "NF", 0);
            }
        }
        if (self.vb(hw, v, "NF") as i8) >= 0 {
            self.instrument(hw, v);
        } else {
            self.k(K::NfNeg);
        }
        if self.l.period_after {
            self.k(K::PeriodAfter);
            self.note_period(hw, v);
        }
        if self.l.track == Track::Flags {
            self.k(K::FlagsEnd);
            d4 = d4.wrapping_add(1);
            tpos_add(self, hw, 1);
        }
        if rd(hw, d4) == 0xff {
            self.k(K::RowEndFF);
            self.svb(hw, v, "TPOS", 0);
            let c = self.vb(hw, v, "LOOPC").wrapping_sub(1);
            self.svb(hw, v, "LOOPC", c);
            if c == 0 {
                self.k(K::LoopcZ);
                self.nextpos(hw, v);
            } else {
                self.k(K::LoopcNz);
            }
        } else {
            self.k(K::RowEndNo);
        }
        if (self.vb(hw, v, "DIS") as i8) < 0 {
            self.k(K::RowDis);
            return true;
        }
        self.k(K::RowTail);
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
        self.k(K::Instr);
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
                self.k(K::S3Sfx);
                return;
            }
            self.k(K::S3Clr);
            self.svb(hw, v, "DIS", 0);
        }
        let r = 0xdff0a0 + 16 * v as u32;
        let mask = hw.bus.r16(self.hreg(v, 0xe));
        let nh = self.b.note_host;
        self.k(K::NoteOff);
        self.wr(hw, nh, 0xdff096, mask as u32);
        self.k(K::NotePer);
        self.wr(hw, nh, r + 6, self.l.r("NOTEPER") as u16 as u32);
        let ofs = if self.l.sofs { self.gw(hw, "TMPW") } else { 0 };
        let p = hw.bus.r32(self.hreg(v, 0)).wrapping_add(ofs as u32);
        self.k(K::NoteLoc);
        self.wr(hw, nh, r, p);
        let len = hw.bus.r16(self.hreg(v, 4)).wrapping_sub(ofs);
        self.k(K::NoteLen);
        self.wr(hw, nh, r + 4, len as u32);
    }

    fn add_period(&self, hw: &mut Hw, v: usize, x: u16, add: bool) -> u16 {
        let p = self.xw(hw, v, "PERIOD");
        let p = if add { p.wrapping_add(x) } else { p.wrapping_sub(x) };
        self.sxw(hw, v, "PERIOD", p);
        p
    }

    fn porta(&mut self, hw: &mut Hw, v: usize) -> bool {
        let pt = self.vb(hw, v, "PT") as u16;
        let (yes, no) = if self.l.both { (K::PortaBothYes, K::PortaBothNo) } else { (K::PortaYes, K::PortaNo) };
        if pt == 0 {
            self.k(no);
            return false;
        }
        self.k(yes);
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
            self.k(K::VDel);
            self.svb(hw, v, "VDEL", d - 1);
            return;
        }
        if self.vb(hw, v, "SDELAY") != 0 {
            self.k(K::VSdelay);
            return;
        }
        let io = self.xw(hw, v, "INSOFF");
        let vn = self.ins(hw, io.wrapping_add(14));
        if vn == 0 {
            self.k(K::VZero);
            return;
        }
        let running = if self.l.vib_halve {
            (self.vb(hw, v, "VIBC") as i8) < 0
        } else {
            (self.vw(hw, v, "VIBW") as i16) < 0
        };
        if !running {
            self.k(if self.l.vib_halve { K::VStartHalve } else { K::VStartDiv });
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
                        self.k(K::VHalveEnd);
                        break;
                    }
                    self.k(K::VHalveIter);
                    s >>= 1;
                    self.sxw(hw, v, "VSPD", s);
                    if s == 0 {
                        self.k(K::VHalveZero);
                        self.sxw(hw, v, "VSPD", 1);
                        self.svb(hw, v, "VIBC", 0xff);
                        break;
                    }
                }
            } else {
                let div = self.vw(hw, v, "VIBW") as u32;
                // (a depth of 0 is a division by zero in the original)
                let q = if div == 0 { s } else { (s as u32 / div) as u16 };
                if q == 0 {
                    self.k(K::VDivQ0);
                }
                self.sxw(hw, v, "VSPD", if q == 0 { 1 } else { q });
                hw.bus.w8(self.g().wrapping_add(self.o("VIBW")) + 2 * v as u32, 0xff);
            }
            return;
        }
        let s = self.xw(hw, v, "VSPD");
        let up = (self.vb(hw, v, "VDIR") as i8) >= 0;
        self.k(if up { K::VRunAdd } else { K::VRunSub });
        self.add_period(hw, v, s, up);
        if (self.vb(hw, v, "VCD") as i8) >= 0 {
            let c = self.vb(hw, v, "VCNT").wrapping_add(1);
            self.svb(hw, v, "VCNT", c);
            if c >= self.vb(hw, v, "VMAX") {
                self.k(K::VCntMax);
                self.svb(hw, v, "VCD", 0xff);
                let d = !self.vb(hw, v, "VDIR");
                self.svb(hw, v, "VDIR", d);
            } else {
                self.k(K::VCntUp);
            }
        } else {
            let c = self.vb(hw, v, "VCNT").wrapping_sub(1);
            self.svb(hw, v, "VCNT", c);
            if c == 0 {
                self.k(K::VCntZero);
                self.svb(hw, v, "VCD", 0);
            } else {
                self.k(K::VCntDownNz);
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
        self.k(if row { K::FxRow } else { K::FxTick });
        if !row {
            if self.l.fx[1] {
                if fx & 2 == 0 {
                    self.k(K::Fx1No);
                } else {
                    let n = if tick & 1 == 0 {
                        self.k(K::Fx1Even);
                        note as u16
                    } else {
                        let b = self.ins(hw, io.wrapping_add(13));
                        if b & 0x80 != 0 {
                            self.k(K::Fx1Rel);
                            ((b & 0x7f).wrapping_add(note)) as u16
                        } else {
                            self.k(K::Fx1Abs);
                            b as u16
                        }
                    };
                    let p = self.period(hw, n);
                    self.sxw(hw, v, "PERIOD", p);
                }
            }
            if self.l.fx[2] {
                if fx & 4 == 0 {
                    self.k(K::Fx2No);
                } else {
                    self.k(if tick & 1 == 0 { K::Fx2Even } else { K::Fx2Odd });
                    let s = if tick & 1 == 0 { self.ins(hw, io) } else { 2 };
                    self.svb(hw, v, "SAMPLE", s);
                    let d4 = self.ins(hw, io.wrapping_add(4)) as u16;
                    let p = self.add_period(hw, v, d4, true);
                    if (p as i16) < 0 {
                        self.k(K::Fx2Neg);
                        self.add_period(hw, v, d4, false);
                    }
                }
            }
            if self.l.fx[3] {
                if fx & 8 == 0 {
                    self.k(K::Fx3No);
                } else {
                    self.k(K::Fx3Yes);
                    arp = Some(((tick & 3) as u16).wrapping_add(io));
                }
            }
        }
        if arp.is_none() {
            if self.l.fx[4] {
                if fx & 0x10 == 0 {
                    self.k(K::Fx4No);
                } else {
                    let s = if self.vb(hw, v, "SDELAY") != 0 {
                        self.k(K::Fx4Sd);
                        Some(self.ins(hw, io.wrapping_add(8)))
                    } else if (self.vb(hw, v, "STOP") as i8) >= 0 {
                        self.k(K::Fx4Stop);
                        Some(self.ins(hw, io))
                    } else {
                        self.k(K::Fx4None);
                        None
                    };
                    if let Some(s) = s {
                        self.svb(hw, v, "SAMPLE", s);
                    }
                }
            }
            if self.l.fx[5] {
                if fx & 0x20 == 0 {
                    self.k(K::Fx5No);
                } else if self.vb(hw, v, "SDELAY") != 0 {
                    self.k(K::Fx5Sd);
                    arp = Some((tick as u16).wrapping_add(io));
                } else if (self.vb(hw, v, "STOP") as i8) >= 0 {
                    self.k(K::Fx5Stop);
                    per = Some(note as u16);
                } else {
                    self.k(K::Fx5None);
                }
            }
        }
        if let Some(d5) = arp {
            let b = self.ins(hw, d5.wrapping_add(9));
            per = Some(if b & 0x80 == 0 {
                self.k(K::ArpAbs);
                b as u16
            } else {
                let c = b & 0x7f;
                if c < 0x40 {
                    self.k(K::ArpUp);
                    c.wrapping_add(note) as u16
                } else {
                    self.k(K::ArpDown);
                    (note as u16).wrapping_sub((c & 0x3f) as u16) & 0xff
                }
            });
        }
        if let Some(n) = per {
            self.k(K::Per);
            let p = self.period(hw, n);
            self.sxw(hw, v, "PERIOD", p);
        }
        if self.l.env {
            self.k(K::Env);
            self.envelope(hw, v);
        }
        if !self.l.late {
            self.stop_delay(hw, v);
        }
    }

    fn stop_delay(&mut self, hw: &mut Hw, v: usize) {
        let s = self.vb(hw, v, "SDELAY");
        if s == 0 {
            self.k(K::SdZero);
            self.svb(hw, v, "STOP", 0xff);
        } else {
            self.k(K::SdDec);
            self.svb(hw, v, "SDELAY", s - 1);
        }
        let t = self.vb(hw, v, "TICK").wrapping_add(1);
        self.svb(hw, v, "TICK", t);
    }

    fn envelope(&mut self, hw: &mut Hw, v: usize) {
        let envt = self.a("ENVT");
        let new = if (self.vb(hw, v, "NF") as i8) < 0 {
            self.k(K::EnvNfNeg);
            false
        } else if self.vb(hw, v, "TICK") != 0 {
            self.k(K::EnvTickNz);
            false
        } else {
            true
        };
        if new {
            self.k(K::EnvNew);
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
            self.k(K::EnvWaitNeg);
            return;
        }
        self.sxb(hw, v, "ENVWAIT", w.wrapping_sub(1));
        if w != 1 {
            self.k(K::EnvWaitDec);
            return;
        }
        self.k(K::EnvStep);
        let vol = hw.bus.r16(self.hreg(v, 0xc));
        let b = (vol as u8).wrapping_add(hw.bus.r8(a2 + pos as u32 + 1));
        let vol = if b < 0x41 {
            self.k(K::EnvVol);
            vol & 0xff00 | b as u16
        } else if b < 0xff {
            self.k(K::Env40);
            0x40
        } else {
            self.k(K::Env0);
            0
        };
        hw.bus.w16(self.hreg(v, 0xc), vol);
        let c = self.xb(hw, v, "ENVCNT").wrapping_sub(1);
        self.sxb(hw, v, "ENVCNT", c);
        if c != 0 {
            self.k(K::EnvCntNz);
            let x = hw.bus.r8(a2 + pos as u32);
            self.sxb(hw, v, "ENVWAIT", x);
            return;
        }
        self.k(K::EnvNext);
        let mut p = self.xb(hw, v, "ENVPOS").wrapping_add(3);
        self.sxb(hw, v, "ENVPOS", p);
        let mut x = hw.bus.r8(a2 + p as u32);
        if x >= 0xc0 {
            self.k(K::EnvLoop);
            p = x & 0x3f;
            self.sxb(hw, v, "ENVPOS", p);
            x = hw.bus.r8(a2 + p as u32);
        }
        self.sxb(hw, v, "ENVWAIT", x);
        if self.xb(hw, v, "ENVPOS") == 0xf {
            self.k(K::EnvPosF);
            return;
        }
        self.k(K::EnvCnt);
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
        assert_eq!(&reg(0xDFF096)[..5], &[8, 4, 2, 1, 0x800f]);
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
        assert!(log[at(0x800f)].2 > log[at(1)].2);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

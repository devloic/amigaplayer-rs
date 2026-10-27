//! TFMX (Chris Hülsbeck), ported from NostalgicPlayer's TFMX player
//! (Copyright (c) 2023 Thomas Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! A TFMX song is two files: `mdat.NAME` (the song: tracks, patterns, macros)
//! and `smpl.NAME` (the samples). NostalgicPlayer plays all three kinds (1.5,
//! Professional, 7 voices) with one C engine and a mixer of its own; the
//! originals are three different 68k replayers. This port follows the
//! Eagleplayer TFMX_Pro ("TFMX_Pro player module V1.4"), whose replayer it
//! transliterates: its per-voice, track and global blocks are kept byte for
//! byte in the original's layout (`Blk`), so the original's word and byte
//! overlaps (a word write over two byte fields, a byte test of a word) come out
//! the same.
//!
//! Where NostalgicPlayer differs from the original, the port follows the
//! original:
//! * the macros write Paula themselves: DMA off writes DMACON at once, or (with
//!   a non-zero second byte) at the start of the next tick, with the period 9;
//!   SetBegin, AddBegin and the SID SetBegin write the location (after waiting
//!   9 lines, as the original does), SetLen and AddLen the length, SampleLoop
//!   and OneShot both; DMA on and the periods of a tick go out at its end, after
//!   a 9-line wait, with the DMACON of the tick's DMA ons (written even when 0);
//! * the volume is written every tick (`(volume * master) >> 6`, masked to 7
//!   bits), not mixed in;
//! * the 52 macro commands of the original (NostalgicPlayer knows 34): random
//!   play, the SID synthesis (which writes its waveform into the sample memory,
//!   where the original keeps it), the memory pokes and macro self-modification;
//! * Wait's bit 0 waits for the random player, not a note; a note in a pattern
//!   is $7F..$BF with a wait (NostalgicPlayer's $80..$BF); `$F5` adds the
//!   pattern's transpose to its second byte; RoPt restores the first pattern's
//!   return point, whichever pattern it is in; command 3 of a track step (time
//!   share) is a fade, and steps over 4 stop the song; AddNote's finetune is
//!   the whole second word; no volume clamp at $40;
//! * the subsongs are the original's list (`subsong_list`), and the song ends
//!   where the original calls dtg_SongEnd;
//! * wait on DMA counts the voice's audio interrupts with a handler in guest
//!   memory (`IRQ_CODE`), as the original's does;
//! * the random numbers come from the beam position (`VHPOSR`), as the
//!   original's do, so a random play macro matches only where the writes are
//!   timed to the colour clock.
//!
//! TFMX 1.5 (`V15`) follows the Eagleplayer TFMX_1.5 ("TFMX 1.6 player module
//! V1.91") the same way: an older replayer with the tables at $400/$600/$800,
//! 26 macro commands, additive vibrato and portamento, the period written as a
//! macro or effect sets it, the volume every tick of a running voice, and DMA
//! on once at the end of the tick through ENPP_DMAMask (whose host waits for
//! the DMA after it). Notes past its 64 periods read on into its variables; the
//! port gives the DMA word that follows the table, then 0.
//!
//! Both space their writes as the originals do: each code path costs the
//! colour clocks fitted by least squares to the Eagleplayers' write logs
//! (`COST`, `COST15`), and the originals' waits for the DMA are kept.

use super::{be16, be32, Hw, Open, Replayer};
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

include!("tfmx_tables.rs");

pub const OPEN_PRO: Open = Open { name: "TFMX Pro", detect: detect_pro, new: new_pro };

/// A block of the original's variables, byte for byte.
#[derive(Clone)]
struct Blk<const N: usize>([u8; N]);

impl<const N: usize> Blk<N> {
    fn b(&self, o: usize) -> u8 {
        self.0[o]
    }
    fn w(&self, o: usize) -> u16 {
        u16::from_be_bytes([self.0[o], self.0[o + 1]])
    }
    fn l(&self, o: usize) -> u32 {
        u32::from_be_bytes([self.0[o], self.0[o + 1], self.0[o + 2], self.0[o + 3]])
    }
    fn sb(&mut self, o: usize, v: u8) {
        self.0[o] = v;
    }
    fn sw(&mut self, o: usize, v: u16) {
        self.0[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn sl(&mut self, o: usize, v: u32) {
        self.0[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
}

// The voice block (the original's at $12da6, $92 bytes each).
const V_RUN: usize = 0x00; // macro running
const V_EFX: usize = 0x01; // effects: <0 off, 0 from the next tick
const V_AB_TIME: usize = 0x03; // AddBegin counter
const V_PREV: usize = 0x04; // previous note
const V_NOTE: usize = 0x05;
const V_VEL: usize = 0x08; // word; the velocity is its low byte
const V_FINE: usize = 0x0a; // finetune, a word
const V_MACRO: usize = 0x0c;
const V_STEP: usize = 0x10;
const V_WAIT: usize = 0x12;
const V_DMA_ON: usize = 0x14;
const V_DMA_OFF: usize = 0x16;
const V_VOL: usize = 0x18;
const V_LOOP: usize = 0x1a;
const V_AB_RESET: usize = 0x1b;
const V_ENV: usize = 0x1c; // envelope speed, counter, end volume, rate
const V_VIB_WIDTH: usize = 0x20;
const V_PORTA_RESET: usize = 0x22;
const V_PORTA_TIME: usize = 0x23;
const V_VIB_OFS: usize = 0x24;
const V_VIB_RESET: usize = 0x26;
const V_VIB_TIME: usize = 0x27;
const V_PER: usize = 0x28;
const V_ADDR: usize = 0x2c;
const V_PORTA_RATE: usize = 0x30;
const V_PORTA_PER: usize = 0x32;
const V_LEN: usize = 0x34;
const V_KEY: usize = 0x36; // 1 while the key is down
const V_RND_MASK: usize = 0x37;
const V_RET: usize = 0x38;
const V_SFX: usize = 0x3c;
const V_PRIO: usize = 0x3d;
const V_LOCK: usize = 0x3e;
const V_RET_STEP: usize = 0x40;
const V_SFX_NUM: usize = 0x42;
const V_SID_LAST: usize = 0x43;
const V_INT_ON: usize = 0x44;
const V_INT_OFF: usize = 0x46;
const V_RND_SPEED: usize = 0x48;
const V_RND_MODE: usize = 0x49;
const V_RND_COUNT: usize = 0x4a;
const V_RND: usize = 0x4b;
const V_RND_PTR: usize = 0x4c;
const V_RND_STEP: usize = 0x50;
const V_RND_MACRO: usize = 0x52;
const V_RND_WAIT: usize = 0x53;
const V_AB_DELTA: usize = 0x5c;
const V_SID_BUF: usize = 0x60;
const V_SID_SRC: usize = 0x64;
const V_SID_MASK: usize = 0x68;
const V_SID: usize = 0x6a;
const V_PEND: usize = 0x8a; // a sound effect's note, for the next tick
const V_PER_OUT: usize = 0x8e; // the period, written at the end of the tick
const V_OFF_DONE: usize = 0x90; // 0 after a DMA off left to the next tick
const V_F91: usize = 0x91;
const VOICE_SIZE: usize = 0x92;

/// The voice blocks' initial contents (the original's data).
fn voice_init(ch: usize) -> Blk<VOICE_SIZE> {
    let mut v = Blk([0; VOICE_SIZE]);
    v.sw(V_DMA_ON, 0x8200 | 1 << ch);
    v.sw(V_DMA_OFF, 1 << ch);
    v.sw(V_INT_ON, 0x8000 | 0x80 << ch);
    v.sw(V_INT_OFF, 0x80 << ch);
    v.sl(V_SID_BUF, 4 + 0x100 * ch as u32);
    v.sb(V_OFF_DONE, 0xff);
    v
}

// The global block ($12d1a).
const G_MDAT: usize = 0x00;
const G_SMPL: usize = 0x04;
const G_SYNTH: usize = 0x08;
const G_RESTART: usize = 0x0d;
const G_SONG: usize = 0x10;
const G_FADE_DIR: usize = 0x11;
const G_SEED: usize = 0x12;
const G_CMD: usize = 0x18; // the command at hand, a long
const G_SPEED_CNT: usize = 0x1c;
const G_ON: usize = 0x1e;
const G_CIA: usize = 0x24;
const G_START: usize = 0x2c; // flags; the song to start is the next byte
const G_SFX7: usize = 0x2e;
const G_MASTER: usize = 0x30;
const G_FADE_END: usize = 0x31;
const G_FADE_CNT: usize = 0x32;
const G_FADE_RESET: usize = 0x33;
const G_TRACK_LOOP: usize = 0x36;
const G_STEPS: usize = 0x38;
const G_PATTERNS: usize = 0x3c;
const G_MACROS: usize = 0x40;
const G_SFX_TAB: usize = 0x44;
const G_DMA_ON: usize = 0x48;
const G_DMA_OFF: usize = 0x4a;

// The track block ($12fee): positions, speed, and eight patterns 4 bytes apart.
const P_FIRST: usize = 0x00;
const P_LAST: usize = 0x02;
const P_POS: usize = 0x04;
const P_SPEED: usize = 0x06;
const P_MUTE: usize = 0x08;
const P_PTR: usize = 0x28;
const P_NUM: usize = 0x48;
const P_XPOSE: usize = 0x49;
const P_LOOP: usize = 0x4a;
const P_STEP: usize = 0x68;
const P_WAIT: usize = 0x6a;
const P_RET: usize = 0x88;
const P_RET_STEP: usize = 0xa8;

/// The original's empty pattern (`$F4000000 $F0000000`), where every pattern
/// starts: a guest address is needed, so the port allocates it.
const EMPTY_PATTERN: [u8; 8] = [0xf4, 0, 0, 0, 0xf0, 0, 0, 0];

/// The audio interrupt handler for wait on DMA, one per voice (a1 = the voice's
/// counter word, then its flag byte): clear the request, count down, and when
/// the count goes negative let the macro run again and disable the interrupt.
/// The mask words are patched in.
const IRQ_CODE: [u16; 13] = [
    0x33fc, 0x0080, 0x00df, 0xf09c, // move.w #bit,INTREQ
    0x5351, // subq.w #1,(a1)
    0x6a0c, // bpl.s done
    0x50e9, 0x0002, // st 2(a1)
    0x33fc, 0x0080, 0x00df, 0xf09a, // move.w #bit,INTENA
    0x4e75, // rts
];
const IRQ_SIZE: u32 = 26 + 22 + 4; // code, interrupt structure, counter and flag

/// The time the original takes from its last write, by code path. Its waits for
/// the audio DMA (`dma_wait`) are to a line boundary, and are not here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
enum At {
    Tick,
    OffStart,
    OffPer,
    VoiceStart,
    Pending,
    MacroCmd,
    Effects,
    AddBegin,
    Vibrato,
    Porta,
    Env,
    Random,
    Fade,
    Volume,
    SidStart,
    SidByte,
    Tracks,
    Row,
    PatternCmd,
    Note,
    TrackStep,
    PerOut,
    Count,
}

/// Fitted by least squares to the reference's write times (51 modules, 570000
/// gaps between writes); Pending and Random had no samples and are estimates.
const COST: [u64; At::Count as usize] = [
    300, 0, 25, 119, 160, 163, 0, 93, 124, 178, 36, 150, 34, 228, 191, 43, 166, 76, 172, 464, 456, 17,
];

pub struct Pro {
    v: [Blk<VOICE_SIZE>; 4],
    g: Blk<0x4c>,
    p: Blk<0xc8>,
    /// $13176: the fade lock word, the "a pattern plays" byte ($15) and the cues.
    fade_lock: u16,
    cues: [u16; 4],
    mdat: u32,
    mdat_len: u32,
    smpl: u32,
    empty: u32,
    irq: u32,
    /// The subsongs (song numbers), as InitPlayer lists them.
    songs: Vec<u8>,
    sub: u16,
    /// The default timer InitSound saved ($1027a).
    timer0: u16,
    /// The module's byte $B (bit 1: a 60 Hz song, its timer 5/6 of the default).
    mdat_flag: u8,
    /// A module of sound effects only (no patterns): its subsongs are effects.
    sfx_module: bool,
    songend: bool,
    spent: u64,
    counts: [u32; At::Count as usize],
    trace: Option<std::fs::File>,
}

/// Check2: a TFMX-SONG (or tfmxsong) module that is not 7 voices, with the
/// original's tests for odd headers.
fn detect_pro(d: &[u8]) -> bool {
    let w = |o: usize| be16(d, o);
    let l = |o: usize| be32(d, o);
    if l(0) == 0x74666d78 {
        return l(4) == 0x736f6e67 && w(8) == 0;
    }
    if l(0) != 0x54464d58 || l(4) != 0x2d534f4e || w(8) != 0x4720 {
        return false;
    }
    let mut d1 = l(0x1d0);
    if d1 != 0 {
        if d1 & 0xffff == 0 {
            // little-endian offsets
            return ((d1 >> 16) as u8 as i8) > 1;
        }
    } else {
        if w(0xc) != 0 || w(0x10) == 0x2020 || w(0x10) == 0x303d {
            return false;
        }
        if l(0x10) == 0x28456d70 && l(0x14) == 0x74792920 {
            // "(Empty) ": the editor's text, so the songs of a few known modules
            let e = w(0xe);
            let ok = e == 0xe60
                || (e == 0x860 && w(0x1224) == 0x90c)
                || (e == 0xb20 && w(0x1400) == 0x8c26)
                || (e == 0x920 && w(0xf24) == 0x9305);
            if !ok {
                return false;
            }
        }
        d1 = 0x800;
    }
    if w(0xe) == 0x8b0 && l(0x204) == 0x01f4ff00 {
        return false;
    }
    for s in 0..32 {
        let st = w(0x100 + 2 * s) as u32;
        if st == 0x1ff {
            continue;
        }
        let mut a = (d1 + (st << 4)) as usize;
        while w(a) == 0xeffe {
            if w(a + 2) == 3 && !(d[a + 4..].first() == Some(&0) && d.get(a + 7).is_some_and(|&b| b != 0)) {
                return false;
            }
            a += 16;
        }
    }
    let (first, end) = if l(0x1d4) != 0 {
        (l(l(0x1d8) as usize), l(0x1d4))
    } else {
        let (f, e) = (l(0x600), l(0x7fc));
        if f == 0 || f & 0x8000_0001 != 0 || e == 0 || e & 0x8000_0001 != 0 {
            return false;
        }
        (f, e)
    };
    let (mut a, end) = (first as usize, end as usize);
    while a != end {
        if a >= d.len() || d[a] > 0x3f {
            return false;
        }
        a += 4;
    }
    true
}

/// A divu.w: the quotient, or the dividend's low word when it overflows.
fn divu(n: u32, d: u16) -> u16 {
    match n.checked_div(d as u32) {
        Some(q) if q <= 0xffff => q as u16,
        _ => n as u16,
    }
}

fn new_pro(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_pro(d) {
        return None;
    }
    // ExtLoad: smpl.NAME for mdat.NAME (the prefix in any case), else smpl.FILE
    let file = m.bus.read_cstr(m.deli.file_buf);
    let rest = match file.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("mdat.") => &file[5..],
        _ => &file[..],
    };
    let smpl_data = crate::deli::read_companion(m, &format!("smpl.{rest}")).1.ok()?;
    let smpl = m.alloc((smpl_data.len() as u32).max(4));
    let empty = m.alloc(EMPTY_PATTERN.len() as u32);
    let irq = m.alloc(4 * IRQ_SIZE);
    if smpl == 0 || empty == 0 || irq == 0 {
        return None;
    }
    m.bus.write_bytes(smpl, &smpl_data);
    m.bus.write_bytes(empty, &EMPTY_PATTERN);
    let mut p = Pro {
        v: std::array::from_fn(voice_init),
        g: Blk([0; 0x4c]),
        p: Blk([0; 0xc8]),
        fade_lock: 0,
        cues: [0; 4],
        mdat: base,
        mdat_len: d.len() as u32,
        smpl,
        empty,
        irq,
        songs: Vec::new(),
        sub: 0,
        timer0: crate::deli::DEFAULT_TIMER,
        mdat_flag: d[0xb],
        sfx_module: false,
        songend: false,
        spent: 0,
        counts: [0; At::Count as usize],
        trace: std::env::var_os("TFMX_TRACE").and_then(|p| std::fs::File::create(p).ok()),
    };
    p.g.sw(G_CIA, 0x7fff);
    p.g.sw(G_MASTER, 0x4040);
    p.g.sw(G_TRACK_LOOP, 0xffff);
    p.p.sw(P_SPEED, 6);
    p.init_player(&mut m.bus)?;
    // the audio interrupt handlers (the original's SetIntVector of INTB_AUD0-3)
    for ch in 0..4u32 {
        let a = irq + ch * IRQ_SIZE;
        for (k, &w) in IRQ_CODE.iter().enumerate() {
            m.bus.w16(a + 2 * k as u32, w);
        }
        m.bus.w16(a + 2, 0x80 << ch);
        m.bus.w16(a + 18, 0x80 << ch);
        let is = a + 26;
        m.bus.w32(is + 14, is + 22);
        m.bus.w32(is + 18, a);
        m.os.int_vectors[crate::os::INTB_AUD0 + ch as usize] = is;
    }
    let mut hw = Hw { bus: &mut m.bus };
    p.init_sound(0, &mut hw);
    Some(Box::new(p))
}

impl Pro {
    fn at(&mut self, a: At) {
        self.spent += COST[a as usize];
        self.counts[a as usize] += 1;
    }

    /// Lets the time pass the original took since its last write.
    fn flush(&mut self, hw: &mut Hw, what: &str) {
        hw.wait(self.spent);
        self.spent = 0;
        if let Some(f) = &mut self.trace {
            use std::io::Write;
            let c: Vec<String> =
                self.counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| format!("{i}:{n}")).collect();
            let _ = writeln!(f, "{} {} {}", hw.bus.clock, what, c.join(","));
        }
        self.counts = [0; At::Count as usize];
    }

    fn reg(&mut self, hw: &mut Hw, a: u32, v: u16) {
        self.flush(hw, "w");
        hw.bus.write_word(a, v);
    }

    fn dmacon(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff096, v);
    }

    fn intena(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff09a, v);
    }

    fn intreq(&mut self, hw: &mut Hw, v: u16) {
        self.reg(hw, 0xdff09c, v);
    }

    fn loc(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.flush(hw, "w");
        hw.loc(ch, a);
    }

    fn len(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a4 + 16 * ch as u32, v);
    }

    fn per(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a6 + 16 * ch as u32, v);
    }

    /// $1134c: the volume, through the Eagleplayer's volume (64: unchanged).
    fn vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a8 + 16 * ch as u32, v & 0x7f);
    }

    /// $107ee: waits for the beam to start 9 new lines.
    fn dma_wait(&mut self, hw: &mut Hw) {
        self.flush(hw, "d");
        let now = hw.bus.clock;
        hw.wait((now / LINE + 9) * LINE - now);
    }

    /// $127d0: the random number, from the beam position.
    fn random(&mut self, hw: &mut Hw) {
        self.flush(hw, "r");
        let (line, h) = hw.bus.beam();
        let d7 = ((line as u16 & 0xff) << 8) | h as u16;
        let s = self.g.w(G_SEED) ^ d7;
        self.g.sw(G_SEED, s.wrapping_add(0x4335));
    }

    /// The address of macro `n` (any number: the table is not range checked).
    fn macro_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.g.l(G_MACROS).wrapping_add(n * 4)))
    }

    fn pattern_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.g.l(G_PATTERNS).wrapping_add(n * 4)))
    }

    fn irq_cell(&self, i: usize) -> u32 {
        self.irq + i as u32 * IRQ_SIZE + 26 + 22
    }

    /// The interrupt handler's flag: its count went negative, the macro runs.
    fn irq_flag(&mut self, hw: &mut Hw, i: usize) {
        let a = self.irq_cell(i);
        if hw.bus.r8(a + 2) != 0 {
            hw.bus.w8(a + 2, 0);
            self.v[i].sb(V_RUN, 0xff);
        }
    }

    fn set_dma_count(&mut self, hw: &mut Hw, i: usize, n: u16) {
        let a = self.irq_cell(i);
        hw.bus.w16(a, n);
        hw.bus.w8(a + 2, 0);
    }

    /// InitPlayer: the module's fixes, the subsong list, and the tests the
    /// original fails on (it refuses the module).
    fn init_player(&mut self, bus: &mut crate::bus::Bus) -> Option<()> {
        let a0 = self.mdat;
        let end = a0 + self.mdat_len;
        // a known module's song end
        if bus.r16(a0 + 0xe) == 0xa40 && bus.r16(a0 + 0x112) == 0x26 && bus.r16(a0 + 0x152) == 0 {
            bus.w16(a0 + 0x152, 0x26);
        }
        self.songs = subsong_list(bus, a0);
        let mut sfx_count = None;
        let data_end;
        let mut d4: u32;
        let table;
        let off = bus.r32(a0 + 0x1d0);
        if off == 0 {
            // the end of the last macro: the song data's size
            let mut a3 = a0.wrapping_add(bus.r32(a0 + 0x7fc));
            loop {
                let x = bus.r32(a3);
                a3 = a3.wrapping_add(4);
                if x == 0x0700_0000 {
                    break;
                }
                if (end as i32).wrapping_sub(a3 as i32) <= 0 {
                    return None;
                }
            }
            data_end = a3.wrapping_sub(a0);
            // the patterns that are not 8 bytes long
            let (mut a1, mut n, mut k) = (a0 + 0x400, 0u32, 0x7fu32);
            'p: loop {
                if k == 0 {
                    break;
                }
                k -= 1;
                loop {
                    let d1 = bus.r32(a1);
                    a1 += 4;
                    if bus.r32(a1).wrapping_sub(d1) as u16 == 8 {
                        continue 'p;
                    }
                    n += 1;
                    k = (k & 0xffff_0000) | (k as u16).wrapping_sub(1) as u32;
                    if k as u16 == 0xffff {
                        break 'p;
                    }
                }
            }
            if n == 0 {
                self.sfx_module = true;
            }
            d4 = 0x7f;
            table = a0 + 0x600;
        } else {
            let mut d2 = off;
            if d2 & 0xffff == 0 {
                // little-endian offsets
                for o in [0x1d0, 0x1d4, 0x1d8] {
                    let v = bus.r32(a0 + o);
                    bus.w32(a0 + o, v.swap_bytes());
                }
                d2 = bus.r32(a0 + 0x1d0);
            }
            let d1 = bus.r32(a0 + 0x1d4);
            let mut a3 = a0.wrapping_add(d1);
            if d1 == bus.r32(a0 + 0x1d8) {
                sfx_count = Some(d2.wrapping_sub(0x200) >> 3);
                self.sfx_module = true;
            }
            let first_pattern = bus.r32(a3);
            let mut guard = 0u32;
            loop {
                a3 = a3.wrapping_sub(4);
                guard += 1;
                if bus.r32(a3) == 0x0700_0000 || guard > 0x40_0000 {
                    break;
                }
            }
            let d4a = a3;
            // back to the start of the last macro
            loop {
                if bus.r32(a3) == 0x2020_2020 {
                    a3 = d4a;
                    while (bus.r8(a3) as i8) >= 0 && guard < 0x80_0000 {
                        a3 = a3.wrapping_sub(4);
                        guard += 1;
                    }
                    break;
                }
                a3 = a3.wrapping_sub(4);
                guard += 1;
                if bus.r32(a3) == 0x0700_0000 || guard > 0x80_0000 {
                    break;
                }
            }
            let last = a3.wrapping_add(4).wrapping_sub(a0);
            let d3 = bus.r32(a0 + 0x1d8);
            let mut a1 = a0.wrapping_add(d3);
            d4 = 0;
            loop {
                d4 += 1;
                let d1 = bus.r32(a1);
                a1 = a1.wrapping_add(4);
                if d1 == 0 || (d1 as i32) < 0 {
                    return None;
                }
                if d1 == last {
                    break;
                }
                if (end as i32).wrapping_sub(a1 as i32) <= 0 {
                    return None;
                }
            }
            let a1o = a1.wrapping_sub(a0);
            data_end = a1o;
            if a1o == 0x32e0 && bus.r32(a0 + 0x2f24) == 0x0400_0027 {
                bus.w32(a0 + 0x2f24, 0x0400_0026);
            }
            let _ = first_pattern;
            table = a0.wrapping_add(d3);
        }
        // the macros that play a sample: the subsongs of a sound effects module
        let (mut d3, mut a4) = (0u32, table);
        let mut guard = 0u32;
        'm: loop {
            if d4 == 0 {
                break;
            }
            d4 = d4.wrapping_sub(1);
            loop {
                let d1 = bus.r32(a4);
                a4 = a4.wrapping_add(4);
                if off == 0 && bus.r32(a4).wrapping_sub(d1) as u16 == 8 {
                    continue 'm;
                }
                let mut a1 = a0.wrapping_add(d1);
                loop {
                    guard += 1;
                    if guard > 0x40_0000 {
                        break 'm;
                    }
                    let b = bus.r8(a1);
                    if b == 2 || b == 0x22 {
                        if b == 2 {
                            d3 += 1;
                        }
                        break;
                    }
                    let x = bus.r32(a1);
                    a1 = a1.wrapping_add(4);
                    if x == 0x0700_0000 {
                        continue 'm;
                    }
                }
                d4 = (d4 & 0xffff_0000) | (d4 as u16).wrapping_sub(1) as u32;
                if d4 as u16 == 0xffff {
                    break 'm;
                }
            }
        }
        if off == 0 && self.sfx_module {
            sfx_count = Some(d3);
        }
        if let Some(n) = sfx_count {
            self.songs.resize(n as usize, 0);
        }
        if data_end >= 0x9c40 && bus.r16(a0 + 0x108) == 0x28 {
            bus.w16(a0 + 0x108, 0x27);
        }
        Some(())
    }
}

/// InitPlayer's subsong list: song 0, then each later song unless its start is
/// $1FF, it repeats the next song, or it is an empty song after the first.
fn subsong_list(bus: &crate::bus::Bus, a0: u32) -> Vec<u8> {
    let w = |o: u32| bus.r16(a0 + o);
    let mut songs = vec![0u8];
    let mut zero_seen = false;
    for i in 0..31u32 {
        let a1 = 2 * i;
        let d1 = i as u8 + 1;
        let (d2, d3) = (w(a1 + 0x102), w(a1 + 0x142));
        let last = a1 >= 0x3c;
        if d3 == 0 && zero_seen {
            continue;
        }
        if d2 == 0x1ff {
            continue;
        }
        if last && d3 == w(a1 + 0x140) {
            continue;
        }
        if d3 == w(a1 + 0x144) && d2 == w(a1 + 0x104) {
            // the next song is the same
            if !zero_seen {
                zero_seen = true;
                if d3 == w(a1 + 0x140) {
                    continue;
                }
            }
            if d3 == d2 || !last || d3 == w(a1 + 0x146) {
                continue;
            }
        }
        songs.push(d1);
    }
    songs
}

/// How a macro command goes on.
enum Next {
    /// Read the next step (the command set the step itself).
    Read,
    /// Step on and read the next.
    Step,
    /// $11bf4: step on and end the macro for this tick, unless a DMA off was
    /// left to this tick (then go on reading).
    Pause,
    /// To the effects.
    Effects,
}

impl Pro {
    /// InitSound: the player's init, the subsong's song, and its sound effect
    /// in a sound effects module.
    fn init_sound(&mut self, sub: u16, hw: &mut Hw) {
        self.sub = sub;
        self.songend = false;
        self.g.sw(G_CIA, self.timer());
        // $12c4a
        self.g.sl(G_MASTER, 0x4040_0000);
        self.g.sb(G_FADE_DIR, 0);
        self.g.sl(G_MDAT, self.mdat);
        self.g.sl(G_SMPL, self.smpl);
        hw.bus.w32(self.smpl, 0);
        self.g.sl(G_SYNTH, self.smpl);
        let a = self.mdat;
        if hw.bus.r32(a + 0x1d0) != 0 {
            self.g.sl(G_STEPS, a.wrapping_add(hw.bus.r32(a + 0x1d0)));
            self.g.sl(G_PATTERNS, a.wrapping_add(hw.bus.r32(a + 0x1d4)));
            self.g.sl(G_MACROS, a.wrapping_add(hw.bus.r32(a + 0x1d8)));
            self.g.sl(G_SFX_TAB, a + 0x200);
        } else {
            self.g.sl(G_STEPS, a + 0x800);
            self.g.sl(G_PATTERNS, a + 0x400);
            self.g.sl(G_MACROS, a + 0x600);
        }
        self.p.sw(P_SPEED, 5);
        let song = self.song_of(sub);
        self.start_song(song, hw);
        if self.sfx_module {
            self.play_sfx(sub, hw);
        }
    }

    /// The default timer (14187), as 50/60 of it for a module whose flag says so.
    fn timer(&self) -> u16 {
        // InitSound reads the module's byte $B
        let flag = self.mdat_flag;
        let t = self.timer0;
        if flag & 2 != 0 {
            divu(t as u32 * 50, 60)
        } else {
            t
        }
    }

    /// $11260: the subsong's song, from InitPlayer's list (and the bytes after
    /// it, for a sound effects module with more effects than the list holds).
    fn song_of(&self, sub: u16) -> u8 {
        self.songs.get(sub as usize).copied().unwrap_or(0)
    }

    /// $12a84: all voices off (EndSound).
    fn all_off(&mut self, hw: &mut Hw) {
        self.g.sb(G_ON, 0);
        self.g.sw(G_DMA_ON, 0);
        for v in &mut self.v {
            v.sb(V_RUN, 0);
            v.sb(V_F91, 0);
            v.sl(V_SFX, 0);
            v.sl(V_PEND, 0);
            v.sw(V_SID, 0);
            v.sb(V_RND, 0);
        }
        for ch in 0..4 {
            self.reg(hw, 0xdff0a8 + 16 * ch, 0);
        }
        self.dmacon(hw, 0x000f);
        self.intreq(hw, 0x0780);
        self.intena(hw, 0x0780);
        self.intreq(hw, 0x0780);
    }

    /// $12b22: starts song `song`.
    fn start_song(&mut self, song: u8, hw: &mut Hw) {
        self.g.sb(G_START + 1, song);
        self.all_off(hw);
        self.g.sb(G_ON, 0);
        self.g.sw(G_SFX7, 0);
        let a4 = self.mdat + 2 * (song as u32 & 0x1f);
        self.g.sw(G_CIA, self.timer());
        let first = hw.bus.r16(a4 + 0x100);
        self.p.sw(P_POS, first);
        self.p.sw(P_FIRST, first);
        self.p.sw(P_LAST, hw.bus.r16(a4 + 0x140));
        let mut d2 = hw.bus.r16(a4 + 0x180);
        if d2 > 0xf {
            self.g.sw(G_CIA, divu(0x1b51f8, d2));
            d2 = self.p.w(P_SPEED);
        }
        for k in 0..8 {
            self.p.sl(P_PTR + 4 * k, self.empty);
            self.p.sw(P_NUM + 4 * k, 0xff00);
            self.p.sl(P_STEP + 4 * k, 0);
        }
        self.p.sw(P_SPEED, d2);
        if (song as i8) >= 0 {
            self.track_step(hw);
        }
        self.g.sb(G_RESTART, 0);
        self.g.sw(G_SPEED_CNT, 0);
        self.g.sb(G_TRACK_LOOP, 0xff);
        self.g.sb(G_SONG, song);
        self.g.sb(G_START, 0);
        self.g.sw(G_DMA_ON, 0);
        self.fade_lock = 0;
        // the filter off, no modulation
        hw.bus.ciaa.pra |= 2;
        self.reg(hw, 0xdff09e, 0x00ff);
        self.g.sb(G_ON, 1);
    }

    /// $129c8: starts sound effect `n` of the module's list.
    fn play_sfx(&mut self, n: u16, hw: &mut Hw) {
        let a5 = if hw.bus.r32(self.mdat + 0x1d0) == 0 {
            self.mdat.wrapping_add(hw.bus.r32(self.mdat + 0x5fc))
        } else {
            self.g.l(G_SFX_TAB)
        };
        let d2 = (n << 3) as u32;
        let e = a5.wrapping_add(d2 as u16 as i16 as u32);
        if hw.bus.r8(e) == 0xfb {
            // a pattern on the eighth track
            let d0 = hw.bus.r16(e + 2);
            self.g.sw(G_SFX7, 1);
            self.p.sw(P_NUM + 28, d0);
            let pa = self.pattern_addr(hw, ((d0 & 0xff00) >> 8) as u32);
            self.p.sl(P_PTR + 28, pa);
            self.p.sl(P_STEP + 28, 0);
            self.p.sb(P_LOOP + 28, 0);
            return;
        }
        let mut d3 = hw.bus.r8(e + 2);
        if (self.g.b(G_SONG) as i8) < 0 {
            d3 = hw.bus.r8(e + 4);
        }
        let i = (d3 & 3) as usize;
        let d1 = hw.bus.r8(e + 5) & 0x7f;
        let v = &self.v[i];
        let locked = (v.w(V_LOCK) as i16) >= 0;
        if d1 < v.b(V_PRIO) && locked {
            return;
        }
        if d2 as u8 == v.b(V_SFX_NUM) && locked && hw.bus.r8(e + 5) & 0x80 != 0 {
            return;
        }
        let d0 = (hw.bus.r32(e) & 0xffff_f0ff) | ((d3 as u32 & 0xf) << 8);
        let v = &mut self.v[i];
        v.sl(V_PEND, d0);
        v.sb(V_PRIO, d1);
        v.sw(V_LOCK, hw.bus.r16(e + 6));
        v.sb(V_SFX_NUM, d2 as u8);
    }

    /// $1188c: reads the track step at the current position: commands, then
    /// the eight patterns.
    fn track_step(&mut self, hw: &mut Hw) {
        for _ in 0..0x10000 {
            self.at(At::TrackStep);
            let a0 = self.g.l(G_STEPS).wrapping_add((self.p.w(P_POS) << 4) as i16 as u32);
            let d0 = hw.bus.r16(a0);
            if d0 == 0xeffe {
                let c = hw.bus.r16(a0 + 2).wrapping_mul(4);
                let a0 = a0 + 4;
                match if c < 0x14 { c / 4 } else { 0 } {
                    0 => {
                        self.g.sb(G_ON, 0);
                        self.songend = true;
                        return;
                    }
                    1 => {
                        let c = self.g.w(G_TRACK_LOOP);
                        if c == 0 {
                            self.g.sw(G_TRACK_LOOP, 0xffff);
                            self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                            continue;
                        }
                        if (c as i16) < 0 {
                            let n = hw.bus.r16(a0 + 2);
                            if (n as i16) <= 0 {
                                self.songend = true;
                            }
                            self.g.sw(G_TRACK_LOOP, n.wrapping_sub(1));
                        } else {
                            self.g.sw(G_TRACK_LOOP, c - 1);
                        }
                        self.p.sw(P_POS, hw.bus.r16(a0));
                    }
                    2 => {
                        let s = hw.bus.r16(a0);
                        self.p.sw(P_SPEED, s);
                        self.g.sw(G_SPEED_CNT, s);
                        let t = hw.bus.r16(a0 + 2);
                        if (t as i16) >= 0 && t & 0x1ff != 0 {
                            self.g.sw(G_CIA, divu(0x1b51f8, t & 0x1ff));
                        }
                        self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                    }
                    _ => {
                        self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                        if self.fade_lock == 0 {
                            self.fade_lock = 1;
                            let (b1, b3) = (hw.bus.r8(a0 + 1), hw.bus.r8(a0 + 3));
                            self.fade(b1, b3);
                        }
                    }
                }
                continue;
            }
            for k in 0..8 {
                let w = hw.bus.r16(a0 + 2 * k as u32);
                if k == 7 && self.g.w(G_SFX7) != 0 {
                    break;
                }
                let o = 4 * k;
                self.p.sw(P_NUM + o, w);
                if (w as i16) >= 0 {
                    let pa = self.pattern_addr(hw, (w >> 8) as u32);
                    self.p.sl(P_PTR + o, pa);
                    self.p.sl(P_STEP + o, 0);
                    self.p.sb(P_LOOP + o, 0);
                }
            }
            return;
        }
    }

    /// A fade to `end` every `speed` ticks (at once for 0).
    fn fade(&mut self, speed: u8, end: u8) {
        self.g.sb(G_FADE_END, end);
        self.g.sb(G_FADE_CNT, speed);
        self.g.sb(G_FADE_RESET, speed);
        if speed == 0 {
            self.g.sb(G_MASTER, end);
        } else {
            self.g.sb(G_FADE_DIR, 1);
            let m = self.g.b(G_MASTER);
            if m != end {
                if m > end {
                    self.g.sb(G_FADE_DIR, 0xff);
                }
                return;
            }
        }
        self.g.sb(G_FADE_DIR, 0);
        self.fade_lock = 0;
    }

    /// $11588: the patterns, when the speed counter is up.
    fn tracks(&mut self, hw: &mut Hw) {
        self.at(At::Tracks);
        let c = self.g.w(G_SPEED_CNT).wrapping_sub(1);
        self.g.sw(G_SPEED_CNT, c);
        if (c as i16) >= 0 {
            return;
        }
        self.g.sw(G_SPEED_CNT, self.p.w(P_SPEED));
        for k in 0..8 {
            self.p.sw(P_MUTE + 4 * k, hw.bus.r16(self.mdat + 0x1c0 + 2 * k as u32));
        }
        'again: for _ in 0..256 {
            self.g.sb(G_RESTART, 0);
            for k in 0..8 {
                self.pattern(hw, 4 * k);
                if self.g.b(G_RESTART) != 0 {
                    continue 'again;
                }
            }
            break;
        }
    }

    /// $11616: pattern `o / 4`'s row.
    fn pattern(&mut self, hw: &mut Hw, o: usize) {
        self.at(At::Row);
        let num = self.p.b(P_NUM + o);
        if num >= 0x90 {
            if num == 0xfe {
                self.p.sb(P_NUM + o, 0xff);
                if self.p.w(P_MUTE + o) == 0 {
                    self.channel_off(hw, self.p.b(P_XPOSE + o));
                }
            }
            return;
        }
        let w = self.p.b(P_WAIT + o);
        if w != 0 {
            self.p.sb(P_WAIT + o, w - 1);
            return;
        }
        for _ in 0..0x10000 {
            let step = self.p.w(P_STEP + o);
            let a = self.p.l(P_PTR + o).wrapping_add(step.wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a);
            self.g.sl(G_CMD, cmd);
            let b0 = (cmd >> 24) as u8;
            let step_on = |s: &mut Self| s.p.sw(P_STEP + o, s.p.w(P_STEP + o).wrapping_add(1));
            if b0 < 0xf0 {
                self.at(At::Note);
                if (0x7f..0xc0).contains(&b0) {
                    self.p.sb(P_WAIT + o, cmd as u8);
                    self.g.sb(G_CMD + 3, 0);
                }
                let mut d0 = b0.wrapping_add(self.p.b(P_XPOSE + o));
                if b0 < 0xc0 {
                    d0 &= 0x3f;
                }
                self.g.sb(G_CMD, d0);
                if self.p.w(P_MUTE + o) == 0 {
                    self.note_port(hw, self.g.l(G_CMD));
                }
                step_on(self);
                if (0x7f..0xc0).contains(&b0) {
                    return;
                }
                continue;
            }
            self.at(At::PatternCmd);
            let (b1, b2, b3) = ((cmd >> 16) as u8, (cmd >> 8) as u8, cmd as u8);
            let w1 = cmd as u16;
            match b0 & 0xf {
                0 => {
                    self.p.sb(P_NUM + o, 0xff);
                    if self.p.w(P_POS) == self.p.w(P_LAST) {
                        self.p.sw(P_POS, self.p.w(P_FIRST));
                        self.songend = true;
                    } else {
                        self.p.sw(P_POS, self.p.w(P_POS).wrapping_add(1));
                    }
                    self.track_step(hw);
                    self.g.sb(G_RESTART, 0xff);
                    return;
                }
                1 => {
                    let c = self.p.b(P_LOOP + o);
                    if c != 0 {
                        self.p.sb(P_LOOP + o, c - 1);
                        if c == 1 {
                            step_on(self);
                            continue;
                        }
                    } else {
                        self.p.sb(P_LOOP + o, b1);
                    }
                    self.p.sw(P_STEP + o, w1);
                }
                2 | 8 => {
                    if b0 & 0xf == 8 {
                        self.p.sl(P_RET + o, self.p.l(P_PTR + o));
                        self.p.sw(P_RET_STEP + o, step);
                    }
                    self.p.sb(P_NUM + o, b1);
                    let pa = self.pattern_addr(hw, b1 as u32);
                    self.p.sl(P_PTR + o, pa);
                    self.p.sw(P_STEP + o, w1);
                }
                3 => {
                    self.p.sb(P_WAIT + o, b1);
                    step_on(self);
                    return;
                }
                4 | 14 => {
                    if b0 & 0xf == 14 {
                        self.g.sw(G_SFX7, 0);
                    }
                    self.p.sb(P_NUM + o, 0xff);
                    return;
                }
                5..=7 | 12 => {
                    if b0 & 0xf == 5 {
                        self.g.sb(G_CMD + 1, b1.wrapping_add(self.p.b(P_XPOSE + o)));
                    }
                    if self.p.w(P_MUTE + o) == 0 {
                        self.note_port(hw, self.g.l(G_CMD));
                    }
                    step_on(self);
                }
                9 => {
                    // the first pattern's return point, whichever pattern this is
                    self.p.sl(P_PTR, self.p.l(P_RET));
                    self.p.sw(P_STEP, self.p.w(P_RET_STEP));
                    step_on(self);
                }
                10 => {
                    if self.fade_lock == 0 {
                        self.fade_lock = 1;
                        self.fade(b1, b3);
                    }
                    step_on(self);
                }
                11 => {
                    let d1 = 4 * (b2 & 7) as usize;
                    self.p.sb(P_NUM + d1, b1);
                    self.p.sb(P_XPOSE + d1, b3);
                    let pa = self.pattern_addr(hw, (b1 & 0x7f) as u32);
                    self.p.sl(P_PTR + d1, pa);
                    self.p.sl(P_STEP + d1, 0);
                    self.p.sb(P_LOOP + d1, 0);
                    step_on(self);
                }
                13 => {
                    self.cues[(b1 & 3) as usize] = w1;
                    step_on(self);
                }
                _ => step_on(self),
            }
        }
    }

    /// $12956: a voice off ($FE pattern), unless a sound effect holds it.
    fn channel_off(&mut self, hw: &mut Hw, d0: u8) {
        let i = (d0 & 3) as usize;
        if self.v[i].b(V_SFX) == 0 {
            let (int_off, dma_off) = (self.v[i].w(V_INT_OFF), self.v[i].w(V_DMA_OFF));
            self.intena(hw, int_off);
            self.dmacon(hw, dma_off);
            let v = &mut self.v[i];
            v.sb(V_RUN, 0);
            v.sw(V_SID, 0);
            v.sb(V_RND, 0);
        }
    }

    /// $127ea: a note, or a note command, for the voice its third byte names.
    fn note_port(&mut self, hw: &mut Hw, d0: u32) {
        let saved = self.g.l(G_CMD);
        self.g.sl(G_CMD, d0);
        let [b0, b1, b2, b3] = d0.to_be_bytes();
        let i = (b2 & 3) as usize;
        if b0 == 0xfc {
            self.v[i].sb(V_SFX, b1);
            self.v[i].sw(V_LOCK, b3 as u16);
        } else if self.v[i].b(V_SFX) == 0 {
            if b0 == 0xf7 {
                let v = &mut self.v[i];
                v.sb(V_ENV + 3, b1);
                let t = (b2 >> 4) + 1;
                v.sb(V_ENV + 1, t);
                v.sb(V_ENV, t);
                v.sb(V_ENV + 2, b3);
            } else if b0 == 0xf6 {
                let v = &mut self.v[i];
                let d = b1 & 0xfe;
                v.sb(V_VIB_RESET, d);
                v.sb(V_VIB_TIME, d >> 1);
                v.sb(V_VIB_WIDTH, b3);
                v.sb(V_VIB_WIDTH + 1, 1);
                v.sw(V_VIB_OFS, 0);
            } else if b0 == 0xf5 {
                self.v[i].sb(V_KEY, 0);
            } else if b0 >= 0xbf {
                // portamento to the note
                let v = &mut self.v[i];
                v.sb(V_PORTA_RESET, b1);
                v.sb(V_PORTA_TIME, 1);
                if v.w(V_PORTA_RATE) == 0 {
                    v.sw(V_PORTA_PER, v.w(V_PER));
                }
                v.sw(V_PORTA_RATE, b3 as u16);
                let n = b0 & 0x3f;
                v.sb(V_NOTE, n);
                v.sw(V_PER, PERIODS[n as usize]);
            } else {
                let ma = self.macro_addr(hw, b1 as u32);
                let v = &mut self.v[i];
                v.sw(V_FINE, b3 as i8 as u16);
                v.sb(V_VEL + 1, (b2 >> 4) & 0xf);
                v.sb(V_PREV, v.b(V_NOTE));
                v.sb(V_NOTE, b0);
                if ma != v.l(V_MACRO) {
                    v.sb(V_F91, 0);
                }
                v.sl(V_MACRO, ma);
                v.sw(V_STEP, 0);
                v.sw(V_WAIT, 0);
                v.sb(V_EFX, 0);
                v.sb(V_LOOP, 0);
                v.sb(V_RUN, 0xff);
                self.set_dma_count(hw, i, 0);
                let m = self.v[i].w(V_INT_OFF);
                self.intena(hw, m);
                self.intreq(hw, m);
                self.v[i].sb(V_KEY, 1);
            }
        }
        self.g.sl(G_CMD, saved);
    }

    /// $11ab0: voice `i`'s tick: a sound effect's note, the macro, the effects.
    fn voice(&mut self, hw: &mut Hw, i: usize) {
        self.at(At::VoiceStart);
        self.irq_flag(hw, i);
        let lock = self.v[i].w(V_LOCK);
        if (lock as i16) >= 0 {
            self.v[i].sw(V_LOCK, lock.wrapping_sub(1));
        } else {
            self.v[i].sb(V_SFX, 0);
            self.v[i].sb(V_PRIO, 0);
        }
        let d0 = self.v[i].l(V_PEND);
        if d0 != 0 {
            self.at(At::Pending);
            self.v[i].sl(V_PEND, 0);
            self.v[i].sb(V_SFX, 0);
            self.note_port(hw, d0);
            self.v[i].sb(V_SFX, self.v[i].b(V_PRIO));
        }
        if self.v[i].b(V_RUN) != 0 {
            let w = self.v[i].w(V_WAIT);
            if w != 0 {
                self.v[i].sw(V_WAIT, w - 1);
            } else {
                self.run_macro(hw, i);
            }
        }
        self.effects(hw, i);
    }

    fn step_on(&mut self, i: usize) {
        let s = self.v[i].w(V_STEP).wrapping_add(1);
        self.v[i].sw(V_STEP, s);
    }

    /// $1204c: a note's period (`note` + the command's note, the finetune plus
    /// the command's word), out unless a portamento runs.
    fn note_period(&mut self, i: usize, note: u8) {
        let n = self.g.b(G_CMD + 1).wrapping_add(note) & 0x3f;
        let mut d0 = PERIODS[n as usize] as u32;
        let d1 = self.v[i].w(V_FINE).wrapping_add(self.g.w(G_CMD + 2));
        if d1 != 0 {
            d0 = (d0 * d1.wrapping_add(0x100) as u32) >> 8;
        }
        self.v[i].sw(V_PER, d0 as u16);
        if self.v[i].w(V_PORTA_RATE) == 0 {
            self.v[i].sw(V_PER_OUT, d0 as u16);
        }
    }

    /// $11c52: DMA off's volume: the third byte, or the velocity * 3 + the
    /// fourth, when either is not 0.
    fn off_volume(&mut self, hw: &mut Hw, i: usize) {
        let (b2, b3) = (self.g.b(G_CMD + 2), self.g.b(G_CMD + 3));
        let d0 = if b2 != 0 {
            b3 as u16
        } else if b3 != 0 {
            self.v[i].w(V_VEL).wrapping_mul(3).wrapping_add(self.g.w(G_CMD + 2))
        } else {
            return;
        };
        self.v[i].sb(V_VOL, d0 as u8);
        self.master_volume(hw, i, false);
    }

    /// The volume out: scaled by the master volume when it is under 64 (and, at
    /// the end of a tick, when no sound effect holds the voice).
    fn master_volume(&mut self, hw: &mut Hw, i: usize, lock_test: bool) {
        let d1 = self.g.b(G_MASTER) as u16;
        let mut d0 = self.v[i].b(V_VOL) as u16;
        if (!lock_test || (self.v[i].w(V_LOCK) as i16) < 0) && d1 & 0x40 == 0 {
            d0 = (d0.wrapping_mul(4) as u32 * d1 as u32) as u16 >> 8;
        }
        self.at(At::Volume);
        self.vol(hw, i, d0);
    }

    /// The macro, from its step, until a command ends it for this tick.
    fn run_macro(&mut self, hw: &mut Hw, i: usize) {
        for _ in 0..0x10000 {
            self.at(At::MacroCmd);
            let a0 = self.v[i].l(V_MACRO).wrapping_add(self.v[i].w(V_STEP).wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a0);
            self.g.sl(G_CMD, cmd & 0x00ff_ffff);
            let next = self.macro_cmd(hw, i, (cmd >> 24) as u8, a0);
            match next {
                Next::Read => {}
                Next::Step => self.step_on(i),
                Next::Pause => {
                    self.step_on(i);
                    if self.v[i].b(V_OFF_DONE) != 0 {
                        return;
                    }
                    self.v[i].sb(V_OFF_DONE, 0xff);
                }
                Next::Effects => return,
            }
        }
    }

    fn macro_cmd(&mut self, hw: &mut Hw, i: usize, c: u8, a0: u32) -> Next {
        let cmd = self.g.l(G_CMD);
        let (b1, b2, b3) = ((cmd >> 16) as u8, (cmd >> 8) as u8, cmd as u8);
        let w1 = cmd as u16;
        let ch = i;
        match c {
            0x00 | 0x13 => {
                if c == 0 {
                    let v = &mut self.v[i];
                    v.sb(V_ENV, 0);
                    v.sb(V_VIB_RESET, 0);
                    v.sw(V_PORTA_RATE, 0);
                    v.sb(V_RND, 0);
                    v.sw(V_SID, 0);
                }
                self.step_on(i);
                if b1 != 0 {
                    // left to the start of the next tick
                    let m = self.g.w(G_DMA_OFF) | self.v[i].w(V_DMA_OFF);
                    self.g.sw(G_DMA_OFF, m);
                    self.v[i].sb(V_OFF_DONE, 0);
                    self.off_volume(hw, i);
                    return Next::Effects;
                }
                let m = self.v[i].w(V_DMA_OFF);
                self.dmacon(hw, m);
                self.off_volume(hw, i);
                Next::Read
            }
            0x01 => {
                let m = self.v[i].w(V_INT_OFF);
                self.intena(hw, m);
                self.intreq(hw, m);
                self.v[i].sb(V_EFX, b1);
                self.step_on(i);
                self.dma_wait(hw);
                let m = self.g.w(G_DMA_ON) | self.v[i].w(V_DMA_ON);
                self.g.sw(G_DMA_ON, m);
                Next::Read
            }
            0x02 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = cmd.wrapping_add(self.g.l(G_SMPL));
                self.set_begin(hw, i, a);
                Next::Read
            }
            0x11 => {
                self.v[i].sb(V_AB_TIME, b1);
                self.v[i].sb(V_AB_RESET, b1);
                let d1 = w1 as i16 as i32 as u32;
                self.v[i].sl(V_AB_DELTA, d1);
                let a = self.v[i].l(V_ADDR).wrapping_add(d1);
                if self.v[i].w(V_SID) == 0 {
                    self.set_begin(hw, i, a);
                } else {
                    self.v[i].sl(V_ADDR, a);
                    self.v[i].sl(V_SID_SRC, a);
                    self.step_on(i);
                }
                Next::Read
            }
            0x12 => {
                let d1 = self.v[i].w(V_LEN).wrapping_add(w1);
                self.v[i].sw(V_LEN, d1);
                if self.v[i].w(V_SID) != 0 {
                    self.v[i].sw(V_SID_MASK, d1);
                } else {
                    self.len(hw, ch, d1);
                }
                Next::Step
            }
            0x03 => {
                self.v[i].sw(V_LEN, w1);
                self.len(hw, ch, w1);
                Next::Step
            }
            0x04 => {
                if b1 & 1 != 0 {
                    if self.v[i].b(V_RND_WAIT) != 0 {
                        return Next::Effects;
                    }
                    self.v[i].sb(V_RND_WAIT, 1);
                    return Next::Step;
                }
                self.v[i].sw(V_WAIT, w1);
                Next::Pause
            }
            0x05 | 0x10 => {
                if c == 0x10 && self.v[i].b(V_KEY) == 0 {
                    return Next::Step;
                }
                let l = self.v[i].b(V_LOOP);
                if l != 0 {
                    self.v[i].sb(V_LOOP, l - 1);
                    if l == 1 {
                        return Next::Step;
                    }
                } else {
                    self.v[i].sb(V_LOOP, b1);
                }
                self.v[i].sw(V_STEP, w1);
                Next::Read
            }
            0x06 | 0x15 => {
                if c == 0x15 {
                    self.v[i].sl(V_RET, self.v[i].l(V_MACRO));
                    self.v[i].sw(V_RET_STEP, self.v[i].w(V_STEP));
                }
                let ma = self.macro_addr(hw, (b1 & 0x7f) as u32);
                let v = &mut self.v[i];
                v.sl(V_MACRO, ma);
                v.sw(V_STEP, w1);
                v.sb(V_LOOP, 0);
                v.sb(V_F91, 0);
                Next::Read
            }
            0x07 => {
                self.v[i].sb(V_RUN, 0);
                Next::Effects
            }
            0x08 | 0x09 | 0x1f => {
                let note = match c {
                    0x08 => self.v[i].b(V_NOTE),
                    0x09 => 0,
                    _ => self.v[i].b(V_PREV),
                };
                self.note_period(i, note);
                Next::Pause
            }
            0x0a => {
                let v = &mut self.v[i];
                v.sb(V_RND, 0);
                v.sw(V_SID, 0);
                v.sb(V_AB_TIME, 0);
                v.sb(V_ENV, 0);
                v.sb(V_VIB_RESET, 0);
                v.sw(V_PORTA_RATE, 0);
                Next::Step
            }
            0x0b => {
                let v = &mut self.v[i];
                v.sb(V_PORTA_RESET, b1);
                v.sb(V_PORTA_TIME, 1);
                if v.w(V_PORTA_RATE) == 0 {
                    v.sw(V_PORTA_PER, v.w(V_PER));
                }
                v.sw(V_PORTA_RATE, w1);
                Next::Step
            }
            0x0c => {
                let v = &mut self.v[i];
                v.sb(V_VIB_RESET, b1);
                v.sb(V_VIB_TIME, b1 >> 1);
                v.sb(V_VIB_WIDTH, b3);
                v.sb(V_VIB_WIDTH + 1, 1);
                if v.w(V_PORTA_RATE) != 0 {
                    return Next::Step;
                }
                v.sw(V_PER_OUT, v.w(V_PER));
                v.sw(V_VIB_OFS, 0);
                Next::Step
            }
            0x0d | 0x0e => {
                if b2 == 0xfe {
                    // a note first, the command's second byte from the note
                    self.g.sw(G_CMD + 2, 0);
                    let n = self.v[i].b(V_NOTE);
                    self.note_period(i, n);
                    self.g.sb(G_CMD + 3, b3);
                }
                let v = if c == 0x0d {
                    self.v[i].w(V_VEL).wrapping_mul(3).wrapping_add(self.g.w(G_CMD + 2)) as u8
                } else {
                    self.g.b(G_CMD + 3)
                };
                self.v[i].sb(V_VOL, v);
                Next::Step
            }
            0x0f => {
                let v = &mut self.v[i];
                v.sb(V_ENV, b2);
                v.sb(V_ENV + 3, b1);
                v.sb(V_ENV + 1, b2);
                v.sb(V_ENV + 2, b3);
                Next::Step
            }
            0x14 => {
                if self.v[i].b(V_KEY) == 0 {
                    return Next::Step;
                }
                let l = self.v[i].b(V_LOOP);
                if l != 0 {
                    self.v[i].sb(V_LOOP, l - 1);
                    if l == 1 {
                        return Next::Step;
                    }
                } else {
                    self.v[i].sb(V_LOOP, b3);
                }
                Next::Effects
            }
            0x16 => {
                self.v[i].sl(V_MACRO, self.v[i].l(V_RET));
                self.v[i].sw(V_STEP, self.v[i].w(V_RET_STEP));
                Next::Step
            }
            0x17 => {
                self.v[i].sw(V_PER, w1);
                if self.v[i].w(V_PORTA_RATE) == 0 {
                    self.v[i].sw(V_PER_OUT, w1);
                }
                Next::Step
            }
            0x18 => {
                let a = self.v[i].l(V_ADDR).wrapping_add(cmd);
                self.v[i].sl(V_ADDR, a);
                self.loc(hw, ch, a);
                let l = self.v[i].w(V_LEN).wrapping_sub((cmd as u16) >> 1);
                self.v[i].sw(V_LEN, l);
                self.len(hw, ch, l);
                Next::Step
            }
            0x19 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = self.g.l(G_SMPL);
                self.v[i].sl(V_ADDR, a);
                self.loc(hw, ch, a);
                self.v[i].sw(V_LEN, 1);
                self.len(hw, ch, 1);
                Next::Step
            }
            0x1a => {
                self.set_dma_count(hw, i, w1);
                self.v[i].sb(V_RUN, 0);
                let m = self.v[i].w(V_INT_ON);
                self.intena(hw, m);
                Next::Pause
            }
            0x1b => {
                self.v[i].sb(V_RND_MACRO, b1);
                self.v[i].sw(V_RND_SPEED, w1);
                self.v[i].sw(V_RND_COUNT, 0x101);
                self.random_play(hw, i);
                self.v[i].sb(V_RND_WAIT, 1);
                Next::Step
            }
            0x1c | 0x1d => {
                let x = self.v[i].b(if c == 0x1c { V_NOTE } else { V_VOL });
                if b1 >= x {
                    return Next::Step;
                }
                self.v[i].sw(V_STEP, w1);
                Next::Read
            }
            0x1e => {
                self.v[i].sb(V_RND_MASK, b1);
                Next::Step
            }
            0x20 => {
                self.cues[(b1 & 3) as usize] = w1;
                Next::Step
            }
            0x21 => {
                let d = ((self.v[i].b(V_NOTE) as u32) << 24)
                    | (cmd & 0x00ff_0000)
                    | (((b2 | (self.v[i].b(V_VEL + 1) << 4)) as u32) << 8)
                    | b3 as u32;
                self.g.sl(G_CMD, d);
                self.note_port(hw, d);
                Next::Step
            }
            0x22 => {
                self.v[i].sb(V_AB_TIME, 0);
                let a = cmd.wrapping_add(self.g.l(G_SMPL));
                self.v[i].sl(V_SID_SRC, a);
                self.v[i].sl(V_ADDR, a);
                let buf = self.g.l(G_SMPL).wrapping_add(self.v[i].l(V_SID_BUF));
                self.dma_wait(hw);
                self.loc(hw, ch, buf);
                Next::Step
            }
            0x23 => {
                let n = (cmd >> 16) as u16;
                let l = if n == 0 { 0x100 } else { n } >> 1;
                self.len(hw, ch, l);
                let v = &mut self.v[i];
                v.sw(V_SID, n.wrapping_sub(1) & 0xff);
                v.sw(V_SID_MASK, w1);
                v.sw(V_LEN, w1);
                Next::Step
            }
            0x24 => {
                self.v[i].sl(0x6c, cmd << 8);
                Next::Step
            }
            0x25 => {
                let v = &mut self.v[i];
                v.sw(0x70, (cmd >> 16) as u16);
                v.sw(0x72, (cmd >> 16) as u16);
                v.sw(0x74, w1);
                Next::Step
            }
            0x26 => {
                self.v[i].sl(0x78, cmd);
                Next::Step
            }
            0x27 => {
                let v = &mut self.v[i];
                v.sw(0x84, (cmd >> 16) as u16);
                v.sw(0x86, (cmd >> 16) as u16);
                v.sw(0x82, w1);
                Next::Step
            }
            0x28 => {
                let v = &mut self.v[i];
                v.sb(0x76, b3);
                v.sw(0x80, ((b2 as i8 as i16) << 4) as u16);
                v.sw(0x7c, (cmd >> 16) as u16);
                v.sw(0x7e, (cmd >> 16) as u16);
                Next::Step
            }
            0x29 => {
                self.step_on(i);
                let v = &mut self.v[i];
                v.sw(V_SID, 0);
                if b1 != 0 {
                    for o in [0x6c, 0x78] {
                        v.sl(o, 0);
                    }
                    for o in [0x70, 0x72, 0x74, 0x84, 0x86, 0x82, 0x80, 0x7c, 0x7e] {
                        v.sw(o, 0);
                    }
                    v.sb(0x76, 0);
                    v.sb(0x88, b3);
                }
                Next::Read
            }
            0x2c => {
                let a = (0x80000u32.wrapping_add(w1 as i16 as u32)) & 0x7ffff;
                hw.bus.w8(a, b1);
                Next::Step
            }
            0x2d => {
                let a = (0x80000u32.wrapping_add(w1 as i16 as u32)) & 0x7ffff;
                if hw.bus.r8(a) != b1 {
                    return Next::Step;
                }
                let s = self.v[i].w(V_STEP).wrapping_add(2);
                self.v[i].sw(V_STEP, s);
                Next::Read
            }
            0x2e => {
                self.reg(hw, 0xdff000 + 2 * b1 as u32, w1);
                Next::Step
            }
            0x2f => {
                let a1 = self.macro_addr(hw, (b1 & 0x7f) as u32);
                let x = hw.bus.r32(a0 + 4);
                hw.bus.w32(a1.wrapping_add(w1.wrapping_mul(4) as i16 as u32), x);
                let s = self.v[i].w(V_STEP).wrapping_add(2);
                self.v[i].sw(V_STEP, s);
                Next::Read
            }
            0x30 => {
                if self.v[i].b(V_F91) == 0 {
                    self.v[i].sb(V_F91, 0xff);
                    return Next::Step;
                }
                self.v[i].sw(V_STEP, w1);
                Next::Read
            }
            0x31 => {
                let d = (0xf5 << 24) | cmd;
                self.g.sl(G_CMD, d);
                self.note_port(hw, d);
                Next::Step
            }
            0x32 | 0x33 => {
                let a = a0.wrapping_add(((cmd >> 16) as u16).wrapping_mul(2) as i16 as u32);
                let x = hw.bus.r16(a);
                hw.bus.w16(a, if c == 0x32 { x.wrapping_add(w1) } else { x & w1 });
                Next::Step
            }
            0x2a | 0x2b => Next::Step,
            _ => Next::Pause,
        }
    }

    /// SetBegin's location, out after the original's wait for the DMA.
    fn set_begin(&mut self, hw: &mut Hw, i: usize, a: u32) {
        self.v[i].sl(V_ADDR, a);
        self.dma_wait(hw);
        self.loc(hw, i, a);
        self.step_on(i);
    }
}

impl Pro {
    /// $12384: the voice's effects, then the random player, the fade and the
    /// volume.
    fn effects(&mut self, hw: &mut Hw, i: usize) {
        self.at(At::Effects);
        let e = self.v[i].b(V_EFX);
        if e & 0x80 != 0 {
            return self.fade_volume(hw, i);
        }
        if e == 0 {
            self.v[i].sb(V_EFX, 1);
            return self.fade_volume(hw, i);
        }
        // AddBegin
        if self.v[i].b(V_AB_TIME) != 0 {
            self.at(At::AddBegin);
            let a = self.v[i].l(V_ADDR).wrapping_add(self.v[i].l(V_AB_DELTA));
            self.v[i].sl(V_ADDR, a);
            if self.v[i].w(V_SID) != 0 {
                self.v[i].sl(V_SID_SRC, a);
            } else {
                self.loc(hw, i, a);
            }
            let t = self.v[i].b(V_AB_TIME).wrapping_sub(1);
            self.v[i].sb(V_AB_TIME, t);
            if t == 0 {
                let v = &mut self.v[i];
                v.sb(V_AB_TIME, v.b(V_AB_RESET));
                v.sl(V_AB_DELTA, v.l(V_AB_DELTA).wrapping_neg());
            }
        }
        if self.v[i].w(V_SID) != 0 {
            self.sid(hw, i);
        }
        // vibrato
        if self.v[i].b(V_VIB_RESET) != 0 {
            self.at(At::Vibrato);
            let v = &mut self.v[i];
            let o = v.w(V_VIB_OFS).wrapping_add(v.b(V_VIB_WIDTH) as i8 as u16);
            v.sw(V_VIB_OFS, o);
            let mut d0 = v.w(V_PER) as u32;
            if o != 0 {
                d0 = ((d0 * o.wrapping_add(0x800) as u32) << 5) >> 16;
            }
            if v.w(V_PORTA_RATE) == 0 {
                v.sw(V_PER_OUT, d0 as u16);
            }
            let t = v.b(V_VIB_TIME).wrapping_sub(1);
            v.sb(V_VIB_TIME, t);
            if t == 0 {
                v.sb(V_VIB_TIME, v.b(V_VIB_RESET));
                v.sb(V_VIB_WIDTH, (v.b(V_VIB_WIDTH) ^ 0xff).wrapping_add(1));
            }
        }
        // portamento
        if self.v[i].w(V_PORTA_RATE) != 0 {
            let v = &mut self.v[i];
            let t = v.b(V_PORTA_TIME).wrapping_sub(1);
            v.sb(V_PORTA_TIME, t);
            if t == 0 {
                self.at(At::Porta);
                let v = &mut self.v[i];
                v.sb(V_PORTA_TIME, v.b(V_PORTA_RESET));
                let d1 = v.w(V_PER);
                let mut d0 = v.w(V_PORTA_PER) as u32;
                let reached = if d0 as u16 == d1 {
                    true
                } else if (d0 as u16) < d1 {
                    d0 = (d0 * v.w(V_PORTA_RATE).wrapping_add(0x100) as u32) >> 8;
                    d0 as u16 >= d1
                } else {
                    d0 = (d0 * 0x100u16.wrapping_sub(v.w(V_PORTA_RATE)) as u32) >> 8;
                    d0 as u16 <= d1
                };
                if reached {
                    v.sw(V_PORTA_RATE, 0);
                    d0 = v.w(V_PER) as u32;
                }
                let d0 = d0 as u16 & 0x7ff;
                v.sw(V_PORTA_PER, d0);
                v.sw(V_PER_OUT, d0);
            }
        }
        // envelope
        if self.v[i].b(V_ENV) != 0 {
            self.at(At::Env);
            let v = &mut self.v[i];
            let c = v.b(V_ENV + 1);
            if c != 0 {
                v.sb(V_ENV + 1, c - 1);
            } else {
                v.sb(V_ENV + 1, v.b(V_ENV));
                let end = v.b(V_ENV + 2) as i8;
                let rate = v.b(V_ENV + 3);
                let vol = v.b(V_VOL) as i8;
                let done = if end > vol {
                    let n = (vol as u8).wrapping_add(rate) as i8;
                    v.sb(V_VOL, n as u8);
                    end <= n
                } else {
                    let n = (vol as u8).wrapping_sub(rate) as i8;
                    v.sb(V_VOL, n as u8);
                    n < 0 || end >= n
                };
                if done {
                    v.sb(V_VOL, end as u8);
                    v.sb(V_ENV, 0);
                }
            }
        }
        self.random_play(hw, i);
    }

    /// $123d2: the SID synthesis, into the voice's buffer in the sample memory.
    fn sid(&mut self, hw: &mut Hw, i: usize) {
        self.at(At::SidStart);
        let v = &self.v[i];
        let src = v.l(V_SID_SRC);
        let mut d4 = v.l(0x6c);
        let a3 = v.l(0x78);
        let mut a1 = v.l(V_SID_BUF).wrapping_add(self.g.l(G_SYNTH));
        let mut a2 = a1.wrapping_add(0x100);
        let n = v.w(V_SID) as u32 + 1;
        let mask = v.w(V_SID_MASK);
        let d3 = v.b(0x76);
        let d5 = v.b(0x88);
        let mut d0: u32 = 0;
        let mut d1 = v.b(V_SID_LAST);
        for _ in 0..n {
            self.at(At::SidByte);
            d4 = d4.wrapping_add(a3);
            let s = d0.rotate_left(16);
            let (sum, x) = s.overflowing_add(d4);
            d0 = sum.rotate_left(16);
            d0 = (d0 & 0xffff_0000) | (d0 as u16 & mask) as u32;
            let d2 = hw.bus.r8(src.wrapping_add(d0 as u16 as i16 as u32)) as i8;
            let out = if d3 == 0 {
                d2
            } else {
                let c = d1 as i8;
                if d2 == c {
                    d2
                } else if d2 > c {
                    let r = (c as i16) + (d3 as i8 as i16) + x as i16;
                    if !(-128..=127).contains(&r) {
                        d2
                    } else if d2 > r as i8 {
                        r as i8
                    } else {
                        d2
                    }
                } else {
                    let r = (c as i16) - (d3 as i8 as i16) - x as i16;
                    if !(-128..=127).contains(&r) || d2 >= r as i8 {
                        d2
                    } else {
                        r as i8
                    }
                }
            };
            d1 = out as u8;
            hw.bus.w8(a1, out as u8);
            a1 = a1.wrapping_add(1);
            if d5 != 0 {
                hw.bus.w8(a2, (out as u8).wrapping_neg());
                a2 = a2.wrapping_add(1);
            }
        }
        let v = &mut self.v[i];
        v.sb(V_SID_LAST, d1);
        if d3 != 0 {
            v.sw(0x76, v.w(0x76).wrapping_add(v.w(0x80)));
            let c = v.w(0x7c).wrapping_sub(1);
            v.sw(0x7c, c);
            if c == 0 {
                v.sw(0x7c, v.w(0x7e));
                v.sw(0x80, v.w(0x80).wrapping_neg());
            }
        }
        for (acc, delta, cnt, reset) in [(0x6c, 0x74, 0x70, 0x72), (0x78, 0x82, 0x84, 0x86)] {
            v.sl(acc, v.l(acc).wrapping_add(v.w(delta) as i16 as u32));
            let c = v.w(cnt).wrapping_sub(1);
            v.sw(cnt, c);
            if c == 0 {
                v.sw(cnt, v.w(reset));
                if v.w(reset) != 0 {
                    v.sw(delta, v.w(delta).wrapping_neg());
                }
            }
        }
    }

    /// The period of the random player's note, out unless a portamento runs.
    fn random_period(&mut self, i: usize, d0: u16) {
        self.v[i].sw(V_PER, d0);
        if self.v[i].w(V_PORTA_RATE) == 0 {
            self.v[i].sw(V_PER_OUT, d0);
        }
    }

    /// $126f6: a random position in the random player's macro, then the fade
    /// and the volume (the original reaches them through its tail).
    fn random_pos(&mut self, hw: &mut Hw, i: usize) {
        self.random(hw);
        let d1 = self.g.b(G_SEED + 1) & self.v[i].b(V_RND_MASK);
        self.v[i].sw(V_RND_STEP, d1 as u16);
        self.fade_volume(hw, i);
    }

    /// $125ba: the random player, then the fade and the volume.
    fn random_play(&mut self, hw: &mut Hw, i: usize) {
        let r = self.v[i].b(V_RND);
        if r == 0 {
            return self.fade_volume(hw, i);
        }
        self.at(At::Random);
        if r & 0x80 == 0 {
            let n = (self.v[i].b(V_RND_MACRO) & 0x7f) as u32;
            let a = self.macro_addr(hw, n);
            self.v[i].sl(V_RND_PTR, a);
            self.v[i].sw(V_RND_STEP, 0);
            self.v[i].sb(V_RND, 0xff);
            if self.v[i].b(V_RND_MODE) & 1 != 0 {
                self.random_pos(hw, i);
            }
        }
        let c = self.v[i].b(V_RND_COUNT).wrapping_sub(1);
        self.v[i].sb(V_RND_COUNT, c);
        if c != 0 {
            // $1270c: pass the period and a lower volume on to the next voice
            if self.v[i].b(V_RND_MODE) & 2 != 0 {
                let d0 = (self.v[i].b(V_RND_SPEED) as u16 * 3) >> 3;
                if d0 as u8 == self.v[i].b(V_RND_COUNT) {
                    let per = self.v[i].w(V_PER);
                    let vol = ((self.v[i].b(V_VOL) as u16 * 5) >> 3) as u8;
                    let j = (i + 1) % 4;
                    self.v[j].sb(V_VOL, vol);
                    if per != self.v[j].w(V_PER) {
                        self.v[j].sw(V_PER, per);
                        self.v[j].sw(V_PER_OUT, per);
                        if self.g.b(G_CMD) & 0x80 != 0 {
                            self.v[j].sb(V_RND_WAIT, 0);
                        }
                    }
                }
            }
            return self.fade_volume(hw, i);
        }
        self.v[i].sb(V_RND_COUNT, self.v[i].b(V_RND_SPEED));
        let mut d0;
        loop {
            let s = self.v[i].w(V_RND_STEP);
            d0 = hw.bus.r8(self.v[i].l(V_RND_PTR).wrapping_add(s as i16 as u32));
            self.g.sb(G_CMD, d0);
            if d0 != 0 {
                break;
            }
            if s == 0 {
                return self.fade_volume(hw, i);
            }
            self.v[i].sw(V_RND_STEP, 0);
        }
        let n = d0.wrapping_add(self.v[i].b(V_NOTE)) & 0x3f;
        if n == 0 {
            return self.random_pos(hw, i);
        }
        let mut per = PERIODS[n as usize] as u32;
        let f = self.v[i].w(V_FINE);
        if f != 0 {
            per = (per * f.wrapping_add(0x100) as u32) >> 8;
        }
        let per = per as u16;
        if self.v[i].b(V_RND_MODE) & 1 == 0 {
            self.v[i].sw(V_PER, per);
            if self.v[i].w(V_PORTA_RATE) != 0 {
                return self.fade_volume(hw, i);
            }
            self.v[i].sw(V_PER_OUT, per);
            if self.g.b(G_CMD) & 0x80 != 0 {
                self.v[i].sb(V_RND_WAIT, 0);
            }
            let s = self.v[i].w(V_RND_STEP).wrapping_add(1);
            self.v[i].sw(V_RND_STEP, s);
            return self.fade_volume(hw, i);
        }
        self.random(hw);
        let skip = self.v[i].b(V_RND_MODE) & 4 == 0
            && self.v[i].w(V_RND_STEP) & 3 == 0
            && 0x10 >= self.g.b(G_SEED + 1);
        if !skip {
            if self.g.b(G_CMD) & 0x80 != 0 {
                self.v[i].sb(V_RND_WAIT, 0);
            }
            self.random_period(i, per);
        }
        let s = self.v[i].w(V_RND_STEP).wrapping_add(1);
        self.v[i].sw(V_RND_STEP, s);
        if self.g.b(G_CMD) & 0x40 == 0 {
            return self.fade_volume(hw, i);
        }
        self.random(hw);
        if 6 >= self.g.b(G_SEED) {
            return self.fade_volume(hw, i);
        }
        self.random_pos(hw, i)
    }

    /// $12776: the fade's step, then the voice's volume.
    fn fade_volume(&mut self, hw: &mut Hw, i: usize) {
        self.at(At::Fade);
        let dir = self.g.b(G_FADE_DIR);
        if dir != 0 {
            let c = self.g.b(G_FADE_CNT).wrapping_sub(1);
            self.g.sb(G_FADE_CNT, c);
            if c == 0 {
                self.g.sb(G_FADE_CNT, self.g.b(G_FADE_RESET));
                let m = self.g.b(G_MASTER).wrapping_add(dir);
                self.g.sb(G_MASTER, m);
                if self.g.b(G_FADE_END) == m {
                    self.g.sb(G_FADE_DIR, 0);
                    self.fade_lock = 0;
                }
            }
        }
        self.master_volume(hw, i, true);
    }

    /// $114c8 (with the Eagleplayer's song end test before it).
    fn interrupt(&mut self, hw: &mut Hw) {
        self.flush(hw, "t");
        self.at(At::Tick);
        if !self.sfx_module && (0..8).all(|k| self.p.b(P_NUM + 4 * k) == 0xff) {
            self.songend = true;
        }
        let saved = self.g.l(G_CMD);
        let off = self.g.w(G_DMA_OFF);
        if off != 0 {
            self.at(At::OffStart);
            self.dmacon(hw, off);
            for ch in 0..4 {
                if off & 1 << ch != 0 {
                    self.at(At::OffPer);
                    self.per(hw, ch, 9);
                }
            }
            self.g.sw(G_DMA_OFF, 0);
        }
        if self.g.b(G_ON) != 0 {
            for i in 0..4 {
                self.voice(hw, i);
            }
            if (self.g.b(G_SONG) as i8) >= 0 {
                self.tracks(hw);
            }
            for ch in 0..4 {
                self.at(At::PerOut);
                let p = self.v[ch].w(V_PER_OUT);
                self.per(hw, ch, p);
            }
            self.dma_wait(hw);
            let m = self.g.w(G_DMA_ON);
            self.dmacon(hw, m);
            self.g.sw(G_DMA_ON, 0);
        }
        self.g.sl(G_CMD, saved);
        self.flush(hw, "e");
    }
}

impl Replayer for Pro {
    fn format(&self) -> &'static str {
        "TFMX Pro"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.interrupt(hw);
    }

    fn tick_clocks(&self) -> u64 {
        (self.g.w(G_CIA) as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, (self.songs.len() as u16).saturating_sub(1), self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        self.all_off(hw);
        self.init_sound(n, hw);
    }
}

// ---------------------------------------------------------------------------
// TFMX 1.5: the Eagleplayer TFMX_1.5 ("TFMX 1.6 player module V1.91"), an
// older replayer of its own: fixed tables at $400/$600/$800, 26 macro
// commands, and its Paula writes through the ENPP_Poke functions.

pub const OPEN_15: Open = Open { name: "TFMX 1.5", detect: detect_15, new: new_15 };

/// CHECK5: "TFMX" not followed by "SONG" (the TFHD single file, which it also
/// takes, is not supported here).
fn detect_15(d: &[u8]) -> bool {
    d.len() >= 0x800 && be32(d, 0) == 0x54464d58 && &d[5..9] != b"SONG"
}

// The global block ($11614).
const H_MDAT: usize = 0x00;
const H_SMPL: usize = 0x04;
const H_ON: usize = 0x08;
const H_RESTART: usize = 0x0a;
const H_SONG: usize = 0x0c;
const H_MASTER: usize = 0x18;
const H_FADE_END: usize = 0x19;
const H_FADE_CNT: usize = 0x1a;
const H_FADE_RESET: usize = 0x1b;
const H_FADE_DIR: usize = 0x1c;
const H_CMD: usize = 0x28;
const H_SPEED_CNT: usize = 0x2c;
const H_PLAYING: usize = 0x2e;
const H_START: usize = 0x3c; // the song to start, or negative
const H_HALF: usize = 0x3e; // a song of tempo over 15: every other row
const H_SFX7: usize = 0x40;
const H_NOTE_WAIT: usize = 0x44;

// The voice area ($1165a): the voices' fields interleaved, 4 bytes apart.
const W_STATE: usize = 0x02; // 0 off, >0 a new note, <0 the macro runs
const W_NOTE: usize = 0x11;
const W_MACRO_NUM: usize = 0x13;
const W_VEL: usize = 0x20;
const W_FINE: usize = 0x22; // the finetune is its low byte
const W_MACRO: usize = 0x30;
const W_STEP: usize = 0x40;
const W_WAIT: usize = 0x42;
const W_DMA_ON: usize = 0x50;
const W_DMA_OFF: usize = 0x52;
const W_VOL: usize = 0x60;
const W_LOOP: usize = 0x62;
const W_ENV: usize = 0x70; // speed, counter, end volume, rate
const W_VIB_WIDTH: usize = 0x80;
const W_PORTA_RESET: usize = 0x82;
const W_PORTA_TIME: usize = 0x83;
const W_VIB_OFS: usize = 0x90;
const W_VIB_RESET: usize = 0x92;
const W_VIB_TIME: usize = 0x93;
const W_PER: usize = 0xa0;
const W_ADDR: usize = 0xb0;
const W_PORTA_RATE: usize = 0xc0;
const W_PORTA_PER: usize = 0xc2;
const W_LEN: usize = 0xd0;
const W_KEY: usize = 0xd2;
const W_RET: usize = 0xe0;
const W_RET_STEP: usize = 0xf0;
const W_LOCK: usize = 0x100;

/// The original's empty pattern pointer: it loads the pattern's first long
/// ($F4000000), not its address.
const EMPTY_15: u32 = 0xf400_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(usize)]
enum B {
    Tick,
    Start,
    Tracks,
    Row,
    PatternCmd,
    Note,
    TrackStep,
    Voice,
    Effects,
    Vibrato,
    Porta,
    Env,
    Fade,
    Volume,
    NewNote,
    MacroCmd,
    DmaOn,
    Count,
}

/// Fitted by least squares to the reference's write times (18 modules, 120000
/// gaps); the time from the interrupt to the first row or voice (Tick, with
/// Tracks) cannot be told from the reference's timer phase and is estimated.
const COST15: [u64; B::Count as usize] = [150, 0, 0, 62, 134, 377, 375, 58, 0, 142, 187, 80, 277, 0, 63, 205, 106];

pub struct V15 {
    g: Blk<0x46>,
    w: Blk<0x120>,
    p: Blk<0xc8>,
    fade_lock: u16,
    dma_on: u16,
    mdat: u32,
    smpl: u32,
    subs: u16,
    sub: u16,
    songend: bool,
    spent: u64,
    counts: [u32; B::Count as usize],
    trace: Option<std::fs::File>,
}

fn new_15(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    if !detect_15(d) {
        return None;
    }
    let file = m.bus.read_cstr(m.deli.file_buf);
    let rest = match file.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("mdat.") => &file[5..],
        _ => &file[..],
    };
    let smpl_data = crate::deli::read_companion(m, &format!("smpl.{rest}")).1.ok()?;
    let smpl = m.alloc((smpl_data.len() as u32).max(4));
    if smpl == 0 {
        return None;
    }
    m.bus.write_bytes(smpl, &smpl_data);
    // InitPlayer: the last subsong is before the second song starting at 0
    let (mut n, mut zeros) = (29i32, 2);
    for k in 0..31 {
        if be16(d, 0x100 + 2 * k as usize) == 0 {
            zeros -= 1;
            if zeros == 0 {
                n = k - 1;
                break;
            }
        }
    }
    let mut p = V15 {
        g: Blk([0; 0x46]),
        w: Blk([0; 0x120]),
        p: Blk([0; 0xc8]),
        fade_lock: 0,
        dma_on: 0,
        mdat: base,
        smpl,
        subs: n.max(0) as u16,
        sub: 0,
        songend: false,
        spent: 0,
        counts: [0; B::Count as usize],
        trace: std::env::var_os("TFMX_TRACE").and_then(|p| std::fs::File::create(p).ok()),
    };
    p.g.sb(H_MASTER, 0x40);
    p.g.sb(H_MASTER + 1, 0x40);
    p.g.sw(0x34, 0x78);
    p.g.sw(H_START, 0xffff);
    p.g.sw(H_NOTE_WAIT, 1);
    for ch in 0..4 {
        p.w.sw(4 * ch + W_DMA_ON, 0x8200 | 1 << ch);
        p.w.sw(4 * ch + W_DMA_OFF, 1 << ch);
    }
    p.init_sound(0);
    Some(Box::new(p))
}

impl V15 {
    fn at(&mut self, a: B) {
        self.spent += COST15[a as usize];
        self.counts[a as usize] += 1;
    }

    fn flush(&mut self, hw: &mut Hw, what: &str) {
        hw.wait(self.spent);
        self.spent = 0;
        if let Some(f) = &mut self.trace {
            use std::io::Write;
            let c: Vec<String> =
                self.counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| format!("{i}:{n}")).collect();
            let _ = writeln!(f, "{} {} {}", hw.bus.clock, what, c.join(","));
        }
        self.counts = [0; B::Count as usize];
    }

    fn reg(&mut self, hw: &mut Hw, a: u32, v: u16) {
        self.flush(hw, "w");
        hw.bus.write_word(a, v);
    }

    /// ENPP_DMAMask (the host waits for the DMA after switching voices on).
    fn dma_mask(&mut self, hw: &mut Hw, m: u16) {
        let v = (m & 0x8000) | (m & 0xf);
        self.reg(hw, 0xdff096, v);
        if v & 0x8000 != 0 {
            let now = hw.bus.clock;
            hw.wait((now / LINE + 10) * LINE - now);
        }
    }

    fn poke_adr(&mut self, hw: &mut Hw, ch: usize, a: u32) {
        self.flush(hw, "w");
        hw.loc(ch, a);
    }

    fn poke_len(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a4 + 16 * ch as u32, v);
    }

    fn poke_per(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a6 + 16 * ch as u32, v);
    }

    fn poke_vol(&mut self, hw: &mut Hw, ch: usize, v: u16) {
        self.reg(hw, 0xdff0a8 + 16 * ch as u32, v);
    }

    /// The period table ($118F2), which notes past its 64 entries read on
    /// from: the DMA mask word after it, then (here) zeros.
    fn period(&self, n: usize) -> u16 {
        match n {
            0..=63 => PERIODS[n],
            64 => self.dma_on,
            _ => 0,
        }
    }

    fn macro_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.mdat.wrapping_add(0x600 + 4 * n)))
    }

    fn pattern_addr(&self, hw: &Hw, n: u32) -> u32 {
        self.mdat.wrapping_add(hw.bus.r32(self.mdat.wrapping_add(0x400 + 4 * n)))
    }

    /// InitSound: the subsong is started by the next interrupt, and a fade to
    /// $3F with speed 0 set up (as the original does).
    fn init_sound(&mut self, sub: u16) {
        self.sub = sub;
        self.songend = false;
        self.g.sl(H_MDAT, self.mdat);
        self.g.sl(H_SMPL, self.smpl);
        self.g.sw(H_ON, 1);
        self.g.sw(H_START, sub);
        // $11476
        self.fade_lock = 1;
        self.g.sb(H_FADE_END, 0x3f);
        self.g.sb(H_FADE_CNT, 0);
        self.g.sb(H_FADE_RESET, 0);
        self.g.sb(H_FADE_DIR, 1);
        let m = self.g.b(H_MASTER);
        if m == 0x3f {
            self.g.sb(H_FADE_DIR, 0);
            self.fade_lock = 0;
        } else if m > 0x3f {
            self.g.sb(H_FADE_DIR, 0xff);
        }
    }

    /// $11348: the pending song's start.
    fn start_song(&mut self, hw: &mut Hw) {
        self.at(B::Start);
        // $11588
        self.g.sw(H_PLAYING, 0);
        for ch in 0..4 {
            self.w.sw(4 * ch + W_STATE, 0);
        }
        self.g.sw(H_SFX7, 0);
        self.g.sw(H_NOTE_WAIT, 1);
        hw.bus.w32(self.g.l(H_SMPL), 0);
        let a4 = self.mdat + 2 * (self.g.w(H_START) as u32 & 0x1f);
        self.g.sw(H_HALF, 0);
        let first = hw.bus.r16(a4 + 0x100);
        self.p.sw(4, first);
        self.p.sw(0, first);
        self.p.sw(2, hw.bus.r16(a4 + 0x140));
        self.p.sw(6, hw.bus.r16(a4 + 0x180));
        if self.p.w(6) > 0xf {
            self.g.sw(H_HALF, 1);
            self.p.sw(6, self.p.w(6).wrapping_sub(0x10));
            self.g.sw(H_NOTE_WAIT, 2);
        }
        for k in 0..8 {
            self.p.sl(0x28 + 4 * k, EMPTY_15);
            self.p.sw(0x48 + 4 * k, 0xff00);
            self.p.sl(0x68 + 4 * k, 0);
        }
        if self.p.w(0) != 0x1ff {
            self.track_step(hw);
        }
        self.g.sw(H_RESTART, 0);
        self.g.sw(H_SPEED_CNT, 0);
        self.g.sw(H_SONG, self.g.w(H_START));
        self.g.sw(H_START, 0xffff);
        self.g.sb(H_MASTER, self.g.b(H_FADE_END));
        self.g.sb(H_FADE_DIR, 0);
        self.fade_lock = 0;
        for ch in 0..4 {
            self.w.sw(4 * ch + W_LOCK, 0);
            self.w.sb(4 * ch + W_LOCK + 3, 0);
        }
        self.g.sw(H_PLAYING, 1);
    }

    /// $10AAA: the track step at the current position.
    fn track_step(&mut self, hw: &mut Hw) {
        for _ in 0..0x10000 {
            self.at(B::TrackStep);
            let a0 = self.mdat.wrapping_add(((self.p.w(4) as u32) << 4) & 0xffff).wrapping_add(0x800);
            let d0 = hw.bus.r16(a0);
            if d0 == 0xeffe {
                let c = hw.bus.r16(a0 + 2);
                let a0 = a0 + 4;
                match if c < 5 { c } else { 0 } {
                    0 => {
                        // stops without telling DeliTracker
                        self.g.sw(H_PLAYING, 0);
                        return;
                    }
                    1 => {
                        self.p.sw(4, hw.bus.r16(a0));
                        self.songend = true;
                    }
                    2 => {
                        let s = hw.bus.r16(a0);
                        self.p.sw(6, s);
                        self.g.sw(H_SPEED_CNT, s);
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                    }
                    3 => {
                        self.g.sw(H_HALF, hw.bus.r16(a0));
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                    }
                    _ => {
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                        if self.fade_lock == 0 {
                            let (b1, b3) = (hw.bus.r8(a0 + 1), hw.bus.r8(a0 + 3));
                            self.fade(b1, b3);
                        }
                    }
                }
                continue;
            }
            for k in 0..8 {
                if k == 7 && self.g.w(H_SFX7) != 0 {
                    break;
                }
                let w = hw.bus.r16(a0 + 2 * k as u32);
                let o = 4 * k;
                self.p.sw(0x48 + o, w);
                if (w as i16) >= 0 {
                    let pa = self.pattern_addr(hw, ((w >> 8) & 0x7f) as u32);
                    self.p.sl(0x28 + o, pa);
                    self.p.sl(0x68 + o, 0);
                    self.p.sw(0x4a + o, 0xffff);
                }
            }
            return;
        }
    }

    fn fade(&mut self, speed: u8, end: u8) {
        self.fade_lock = 1;
        self.g.sb(H_FADE_END, end);
        self.g.sb(H_FADE_CNT, speed);
        self.g.sb(H_FADE_RESET, speed);
        if speed == 0 {
            self.g.sb(H_MASTER, end);
        } else {
            self.g.sb(H_FADE_DIR, 1);
            let m = self.g.b(H_MASTER);
            if m != end {
                if m > end {
                    self.g.sb(H_FADE_DIR, 0xff);
                }
                return;
            }
        }
        self.g.sb(H_FADE_DIR, 0);
        self.fade_lock = 0;
    }

    /// $107FC: the patterns, when the speed counter is up.
    fn tracks(&mut self, hw: &mut Hw) {
        self.at(B::Tracks);
        let c = self.g.w(H_SPEED_CNT);
        if c != 0 {
            self.g.sw(H_SPEED_CNT, c - 1);
            return;
        }
        self.g.sw(H_SPEED_CNT, self.p.w(6));
        'again: for _ in 0..256 {
            if self.g.w(H_HALF) != 0 && self.g.w(H_RESTART) != 0 {
                self.g.sw(H_SPEED_CNT, 0);
                self.g.sw(H_RESTART, 0);
                self.g.sw(H_NOTE_WAIT, 1);
                return;
            }
            self.g.sb(H_RESTART, 0);
            for k in 0..8 {
                self.pattern(hw, 4 * k);
                if self.g.b(H_RESTART) != 0 {
                    continue 'again;
                }
            }
            break;
        }
    }

    /// $1088E: pattern `o / 4`'s row.
    fn pattern(&mut self, hw: &mut Hw, o: usize) {
        self.at(B::Row);
        let num = self.p.b(0x48 + o);
        if num >= 0x90 {
            if num == 0xfe {
                self.p.sb(0x48 + o, 0xff);
                if self.p.w(0x08 + o) == 0 {
                    // $11322
                    let i = (self.p.b(0x49 + o) & 3) as usize;
                    let m = self.w.w(4 * i + W_DMA_OFF);
                    self.dma_mask(hw, m);
                    self.w.sw(4 * i + W_STATE, 0);
                }
            }
            return;
        }
        let wt = self.p.w(0x6a + o);
        if wt != 0 {
            self.p.sw(0x6a + o, wt - 1);
            return;
        }
        for _ in 0..0x10000 {
            let step = self.p.w(0x68 + o);
            let a = self.p.l(0x28 + o).wrapping_add(step.wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a);
            self.g.sl(H_CMD, cmd);
            let b0 = (cmd >> 24) as u8;
            let step_on = |s: &mut Self| s.p.sw(0x68 + o, s.p.w(0x68 + o).wrapping_add(1));
            if b0 < 0xf0 {
                self.at(B::Note);
                self.g.sb(H_CMD, b0.wrapping_add(self.p.b(0x49 + o)));
                if self.p.w(0x08 + o) == 0 {
                    self.note_port(hw, self.g.l(H_CMD));
                }
                step_on(self);
                continue;
            }
            self.at(B::PatternCmd);
            let (b1, b3) = ((cmd >> 16) as u8, cmd as u8);
            let w1 = cmd as u16;
            match b0 & 0xf {
                0 => {
                    if self.p.b(0x48 + o) == 0x81 {
                        self.p.sb(0x48 + o, 0xff);
                        return;
                    }
                    if self.p.w(4) == self.p.w(2) {
                        self.p.sw(4, self.p.w(0));
                        self.songend = true;
                    } else {
                        self.p.sw(4, self.p.w(4).wrapping_add(1));
                    }
                    self.track_step(hw);
                    self.g.sw(H_RESTART, 0xffff);
                    return;
                }
                1 => {
                    let c = self.p.b(0x4a + o);
                    if c == 0 {
                        self.p.sb(0x4a + o, 0xff);
                        step_on(self);
                        continue;
                    }
                    if c == 0xff {
                        self.p.sb(0x4a + o, b1.wrapping_sub(1));
                    } else {
                        self.p.sb(0x4a + o, c - 1);
                    }
                    self.p.sw(0x68 + o, w1);
                }
                2 | 8 => {
                    if b0 & 0xf == 8 {
                        self.p.sl(0x88 + o, self.p.l(0x28 + o));
                        self.p.sw(0xa8 + o, step);
                    }
                    let n = b1 & 0x7f;
                    self.p.sb(0x48 + o, n);
                    let pa = self.pattern_addr(hw, n as u32);
                    self.p.sl(0x28 + o, pa);
                    self.p.sw(0x68 + o, w1);
                }
                3 => {
                    self.p.sb(0x6b + o, b1);
                    step_on(self);
                    return;
                }
                4 => {
                    self.p.sb(0x48 + o, 0xff);
                    return;
                }
                14 => {
                    self.g.sw(H_SFX7, 0);
                    self.p.sb(0x48 + o, 0xff);
                    return;
                }
                5..=7 => {
                    if self.p.w(0x08 + o) == 0 {
                        self.note_port(hw, self.g.l(H_CMD));
                    }
                    step_on(self);
                }
                9 => {
                    self.p.sl(0x28, self.p.l(0x88));
                    self.p.sw(0x68, self.p.w(0xa8));
                    step_on(self);
                }
                10 => {
                    if self.fade_lock == 0 {
                        self.fade(b1, b3);
                    }
                    step_on(self);
                }
                11..=13 => {
                    step_on(self);
                    return;
                }
                _ => step_on(self),
            }
        }
    }

    /// $111FE: a note, or a note command, for the voice its third byte names.
    fn note_port(&mut self, hw: &mut Hw, d0: u32) {
        let saved = self.g.l(H_CMD);
        self.g.sl(H_CMD, d0);
        let [b0, b1, b2, b3] = d0.to_be_bytes();
        let v = 4 * (b2 & 3) as usize;
        if self.w.b(v + W_LOCK + 3) == 0 {
            if b0 == 0xf7 {
                self.w.sb(v + W_ENV, 1);
                self.w.sb(v + W_ENV + 3, b1);
                self.w.sb(v + W_ENV + 1, 1);
                self.w.sb(v + W_ENV + 2, b3);
            } else if b0 == 0xf6 {
                let d = b1 & 0xfe;
                self.w.sb(v + W_VIB_RESET, d);
                self.w.sb(v + W_VIB_TIME, d >> 1);
                self.w.sb(v + W_VIB_WIDTH, b3);
                self.w.sb(v + W_VIB_WIDTH + 1, 1);
                self.w.sw(v + W_VIB_OFS, 0);
            } else if b0 == 0xf5 {
                self.w.sb(v + W_KEY, 0);
            } else if b0 >= 0xbf {
                self.w.sb(v + W_PORTA_RESET, b1);
                self.w.sb(v + W_PORTA_TIME, 1);
                self.w.sw(v + W_PORTA_RATE, b3 as u16);
                self.w.sw(v + W_PORTA_PER, self.w.w(v + W_PER));
                let n = (b0 & 0x3f) as usize;
                self.w.sb(v + W_NOTE, n as u8);
                self.w.sw(v + W_PER, self.period(n));
            } else if b0 < 0x40 {
                self.w.sb(v + W_FINE + 1, b3);
                self.w.sb(v + W_VEL + 1, (b2 >> 4) & 0xf);
                self.w.sb(v + W_MACRO_NUM, b1);
                self.w.sb(v + W_NOTE, b0);
                let ma = self.macro_addr(hw, b1 as u32);
                self.w.sl(v + W_MACRO, ma);
                self.w.sw(v, 1);
                self.w.sw(v + W_STATE, 1);
                self.w.sb(v + W_KEY, 1);
            }
        }
        self.g.sl(H_CMD, saved);
    }

    /// $10CF2: voice `ch`'s tick.
    fn voice(&mut self, hw: &mut Hw, ch: usize) {
        self.at(B::Voice);
        let v = 4 * ch;
        let st = self.w.w(v + W_STATE);
        if (st as i16) <= 0 {
            self.at(B::Effects);
            let l = self.w.w(v + W_LOCK);
            if (l as i16) >= 0 {
                self.w.sw(v + W_LOCK, l.wrapping_sub(1));
            } else {
                self.w.sb(v + W_LOCK + 3, 0);
            }
            self.vibrato(hw, ch);
            self.porta(hw, ch);
            self.envelope(ch);
            self.at(B::Fade);
            let dir = self.g.b(H_FADE_DIR);
            if dir != 0 {
                let c = self.g.b(H_FADE_CNT).wrapping_sub(1);
                self.g.sb(H_FADE_CNT, c);
                if c == 0 {
                    self.g.sb(H_FADE_CNT, self.g.b(H_FADE_RESET));
                    let m = self.g.b(H_MASTER).wrapping_add(dir);
                    self.g.sb(H_MASTER, m);
                    if self.g.b(H_FADE_END) == m {
                        self.g.sb(H_FADE_DIR, 0);
                        self.fade_lock = 0;
                    }
                }
            }
            self.at(B::Volume);
            let d1 = self.g.b(H_MASTER) as u16;
            let mut d0 = self.w.b(v + W_VOL) as u16;
            if d1 != 0x40 {
                d0 = ((((d0 << 2) as u32 * d1 as u32) as u16) >> 8) & 0x7f;
            }
            self.poke_vol(hw, ch, d0);
        }
        let st = self.w.w(v + W_STATE);
        if st == 0 {
            return;
        }
        if (st as i16) > 0 {
            self.at(B::NewNote);
            self.w.sw(v + W_STEP, 0);
            self.w.sw(v + W_WAIT, self.g.w(H_NOTE_WAIT));
            self.w.sw(v + W_STATE, 0xffff);
            self.w.sw(v + W_LOOP, 0xffff);
        }
        self.run_macro(hw, ch);
    }

    fn vibrato(&mut self, hw: &mut Hw, ch: usize) {
        let v = 4 * ch;
        if self.w.b(v + W_VIB_RESET) == 0 {
            return;
        }
        self.at(B::Vibrato);
        let o = self.w.w(v + W_VIB_OFS).wrapping_add(self.w.b(v + W_VIB_WIDTH) as i8 as u16);
        self.w.sw(v + W_VIB_OFS, o);
        let d0 = o.wrapping_add(self.w.w(v + W_PER));
        if self.w.w(v + W_PORTA_RATE) == 0 {
            self.poke_per(hw, ch, d0);
        }
        let t = self.w.b(v + W_VIB_TIME).wrapping_sub(1);
        self.w.sb(v + W_VIB_TIME, t);
        if t == 0 {
            self.w.sb(v + W_VIB_TIME, self.w.b(v + W_VIB_RESET));
            self.w.sb(v + W_VIB_WIDTH, (self.w.b(v + W_VIB_WIDTH) ^ 0xff).wrapping_add(1));
        }
    }

    fn porta(&mut self, hw: &mut Hw, ch: usize) {
        let v = 4 * ch;
        let rate = self.w.w(v + W_PORTA_RATE);
        if rate == 0 {
            return;
        }
        let t = self.w.b(v + W_PORTA_TIME).wrapping_sub(1);
        self.w.sb(v + W_PORTA_TIME, t);
        if t != 0 {
            return;
        }
        self.at(B::Porta);
        self.w.sb(v + W_PORTA_TIME, self.w.b(v + W_PORTA_RESET));
        let d1 = self.w.w(v + W_PER);
        let mut d0 = self.w.w(v + W_PORTA_PER);
        if d0 == d1 {
            return;
        }
        let reached = if d0 < d1 {
            d0 = d0.wrapping_add(rate);
            d0 >= d1
        } else {
            d0 = d0.wrapping_sub(rate);
            d0 <= d1
        };
        if reached {
            self.w.sw(v + W_PORTA_RATE, 0);
            d0 = self.w.w(v + W_PER);
        }
        let d0 = d0 & 0x7ff;
        self.w.sw(v + W_PORTA_PER, d0);
        self.poke_per(hw, ch, d0);
    }

    fn envelope(&mut self, ch: usize) {
        let v = 4 * ch;
        if self.w.b(v + W_ENV) == 0 {
            return;
        }
        let c = self.w.b(v + W_ENV + 1);
        if c != 0 {
            self.w.sb(v + W_ENV + 1, c - 1);
            return;
        }
        self.at(B::Env);
        self.w.sb(v + W_ENV + 1, self.w.b(v + W_ENV));
        let end = self.w.b(v + W_ENV + 2) as i8;
        let rate = self.w.b(v + W_ENV + 3);
        let vol = self.w.b(v + W_VOL);
        let done = if end > vol as i8 {
            let n = vol.wrapping_add(rate) as i8;
            self.w.sb(v + W_VOL, n as u8);
            end <= n
        } else {
            let n = vol.wrapping_sub(rate) as i8;
            self.w.sb(v + W_VOL, n as u8);
            n < 0 || end >= n
        };
        if done {
            self.w.sb(v + W_VOL, end as u8);
            self.w.sb(v + W_ENV, 0);
        }
    }

    /// $10D84: the macro, from its step, until a command ends it for this tick.
    fn run_macro(&mut self, hw: &mut Hw, ch: usize) {
        let v = 4 * ch;
        let wt = self.w.w(v + W_WAIT);
        if wt != 0 {
            self.w.sw(v + W_WAIT, wt - 1);
            return;
        }
        let step_on = |s: &mut Self| s.w.sw(v + W_STEP, s.w.w(v + W_STEP).wrapping_add(1));
        for _ in 0..0x10000 {
            self.at(B::MacroCmd);
            let a = self.w.l(v + W_MACRO).wrapping_add(self.w.w(v + W_STEP).wrapping_mul(4) as i16 as u32);
            let cmd = hw.bus.r32(a);
            self.g.sl(H_CMD, cmd);
            let [c, b1, b2, b3] = cmd.to_be_bytes();
            let w1 = cmd as u16;
            match c {
                0x00 | 0x0a | 0x13 => {
                    if c != 0x13 {
                        self.w.sb(v + W_ENV, 0);
                        self.w.sb(v + W_VIB_RESET, 0);
                        self.w.sw(v + W_PORTA_RATE, 0);
                    }
                    if c != 0x0a {
                        let m = self.w.w(v + W_DMA_OFF);
                        self.dma_mask(hw, m);
                    }
                    step_on(self);
                }
                0x01 => {
                    self.dma_on |= self.w.w(v + W_DMA_ON);
                    step_on(self);
                }
                0x02 => {
                    let a = (cmd & 0xff_ffff).wrapping_add(self.g.l(H_SMPL));
                    self.w.sl(v + W_ADDR, a);
                    self.poke_adr(hw, ch, a);
                    step_on(self);
                }
                0x03 => {
                    self.w.sw(v + W_LEN, w1);
                    self.poke_len(hw, ch, w1);
                    step_on(self);
                }
                0x04 => {
                    self.w.sw(v + W_WAIT, w1);
                    step_on(self);
                    return;
                }
                0x05 | 0x10 => {
                    if c == 0x10 && self.w.b(v + W_KEY) == 0 {
                        step_on(self);
                        continue;
                    }
                    let l = self.w.b(v + W_LOOP);
                    if l == 0 {
                        self.w.sb(v + W_LOOP, 0xff);
                        step_on(self);
                        continue;
                    }
                    if l == 0xff {
                        self.w.sb(v + W_LOOP, b1.wrapping_sub(1));
                    } else {
                        self.w.sb(v + W_LOOP, l - 1);
                    }
                    self.w.sw(v + W_STEP, w1);
                }
                0x06 | 0x15 => {
                    if c == 0x15 {
                        self.w.sl(v + W_RET, self.w.l(v + W_MACRO));
                        self.w.sw(v + W_RET_STEP, self.w.w(v + W_STEP));
                    }
                    let ma = self.macro_addr(hw, (b1 & 0x7f) as u32);
                    self.w.sl(v + W_MACRO, ma);
                    self.w.sw(v + W_STEP, w1);
                    // through $10D66: the voice's state and wait again
                    let st = self.w.w(v + W_STATE);
                    if st == 0 {
                        return;
                    }
                    if (st as i16) > 0 {
                        self.w.sw(v + W_STEP, 0);
                        self.w.sw(v + W_WAIT, self.g.w(H_NOTE_WAIT));
                        self.w.sw(v + W_STATE, 0xffff);
                        self.w.sw(v + W_LOOP, 0xffff);
                    }
                    let wt = self.w.w(v + W_WAIT);
                    if wt != 0 {
                        self.w.sw(v + W_WAIT, wt - 1);
                        return;
                    }
                }
                0x07 => {
                    self.w.sw(v + W_STATE, 0);
                    return;
                }
                0x08 | 0x09 => {
                    let per = if c == 0x08 {
                        let n = b1.wrapping_add(self.w.b(v + W_NOTE)) as usize;
                        self.period(n).wrapping_add(self.w.b(v + W_FINE + 1) as i8 as u16)
                    } else {
                        self.period(b1 as usize)
                    }
                    .wrapping_add(w1);
                    self.w.sw(v + W_PER, per);
                    if self.w.w(v + W_PORTA_RATE) == 0 {
                        self.poke_per(hw, ch, per);
                    }
                    step_on(self);
                    return;
                }
                0x0b => {
                    self.w.sb(v + W_PORTA_RESET, b1);
                    self.w.sb(v + W_PORTA_TIME, 1);
                    self.w.sw(v + W_PORTA_RATE, w1);
                    self.w.sw(v + W_PORTA_PER, self.w.w(v + W_PER));
                    step_on(self);
                }
                0x0c => {
                    let d = b1 & 0xfe;
                    self.w.sb(v + W_VIB_RESET, d);
                    self.w.sb(v + W_VIB_TIME, d >> 1);
                    self.w.sb(v + W_VIB_WIDTH, b3);
                    self.w.sb(v + W_VIB_WIDTH + 1, 1);
                    if self.w.w(v + W_PORTA_RATE) == 0 {
                        let p = self.w.w(v + W_PER);
                        self.poke_per(hw, ch, p);
                        self.w.sw(v + W_VIB_OFS, 0);
                    }
                    step_on(self);
                }
                0x0d => {
                    let x = self.w.w(v + W_VEL).wrapping_mul(3).wrapping_add(w1);
                    self.w.sb(v + W_VOL, x as u8);
                    step_on(self);
                }
                0x0e => {
                    self.w.sb(v + W_VOL, b3);
                    step_on(self);
                }
                0x0f => {
                    self.w.sb(v + W_ENV, b2);
                    self.w.sb(v + W_ENV + 3, b1);
                    self.w.sb(v + W_ENV + 1, b2);
                    self.w.sb(v + W_ENV + 2, b3);
                    step_on(self);
                }
                0x11 => {
                    let a = self.w.l(v + W_ADDR).wrapping_add(w1 as i16 as u32);
                    self.w.sl(v + W_ADDR, a);
                    self.poke_adr(hw, ch, a);
                    step_on(self);
                }
                0x12 => {
                    let l = self.w.w(v + W_LEN).wrapping_add(w1);
                    self.w.sw(v + W_LEN, l);
                    self.poke_len(hw, ch, l);
                    step_on(self);
                }
                0x14 => {
                    if self.w.b(v + W_KEY) == 0 {
                        step_on(self);
                        continue;
                    }
                    let l = self.w.b(v + W_LOOP);
                    if l == 0 {
                        self.w.sb(v + W_LOOP, 0xff);
                        step_on(self);
                        continue;
                    }
                    if l == 0xff {
                        self.w.sb(v + W_LOOP, b3.wrapping_sub(1));
                    } else {
                        self.w.sb(v + W_LOOP, l - 1);
                    }
                    return;
                }
                0x16 => {
                    self.w.sl(v + W_MACRO, self.w.l(v + W_RET));
                    self.w.sw(v + W_STEP, self.w.w(v + W_RET_STEP));
                    step_on(self);
                }
                0x17 => {
                    self.w.sw(v + W_PER, w1);
                    if self.w.w(v + W_PORTA_RATE) == 0 {
                        self.poke_per(hw, ch, w1);
                    }
                    step_on(self);
                }
                0x18 => {
                    let d0 = cmd & 0xffff;
                    let a = self.w.l(v + W_ADDR).wrapping_add(d0);
                    self.w.sl(v + W_ADDR, a);
                    let l = self.w.w(v + W_LEN).wrapping_sub((d0 as u16) >> 1);
                    self.w.sw(v + W_LEN, l);
                    self.poke_adr(hw, ch, a);
                    self.poke_len(hw, ch, l);
                    step_on(self);
                }
                0x19 => {
                    let a = self.g.l(H_SMPL);
                    self.w.sl(v + W_ADDR, a);
                    self.poke_adr(hw, ch, a);
                    self.w.sw(v + W_LEN, 1);
                    self.poke_len(hw, ch, 1);
                    step_on(self);
                }
                _ => {
                    step_on(self);
                    return;
                }
            }
        }
    }

    /// $1079E.
    fn interrupt(&mut self, hw: &mut Hw) {
        self.flush(hw, "t");
        self.at(B::Tick);
        if (self.g.w(H_START) as i16) >= 0 {
            self.start_song(hw);
        } else if self.g.w(H_ON) == 0 || self.g.w(H_PLAYING) == 0 {
            self.fade_lock = 0;
        } else {
            self.tracks(hw);
            self.dma_on = 0;
            for ch in 0..4 {
                self.voice(hw, ch);
            }
            if self.dma_on != 0 {
                self.at(B::DmaOn);
                let m = self.dma_on;
                self.dma_mask(hw, m);
            }
        }
        self.flush(hw, "e");
    }
}

impl Replayer for V15 {
    fn format(&self) -> &'static str {
        "TFMX 1.5"
    }

    fn title(&self) -> String {
        String::new()
    }

    fn tick(&mut self, hw: &mut Hw) {
        self.interrupt(hw);
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, _hw: &mut Hw) {
        self.init_sound(n);
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;

    /// An mdat with the old fixed layout: song 0 at track step 0 (speed 5),
    /// pattern 0 at $A00 on the first track, macro 0 at $B00; `id` is its first
    /// 10 bytes, `macro` macro 0's commands.
    fn mdat(id: &[u8], macro_: &[u32]) -> Vec<u8> {
        let mut d = vec![0u8; 0xc00];
        let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_be_bytes());
        d[..id.len()].copy_from_slice(id);
        d[0x180..0x182].copy_from_slice(&5u16.to_be_bytes());
        put(&mut d, 0x400, 0xa00); // pattern 0, then 1 ($20 apart: not empty)
        put(&mut d, 0x404, 0xa20);
        put(&mut d, 0x600, 0xb00); // macro 0, the last one
        put(&mut d, 0x7fc, 0xb00);
        // track step 0: pattern 0 on the first track, the others off
        for k in 1..8 {
            d[0x800 + 2 * k] = 0xff;
        }
        // pattern 0: note 30, macro 0, velocity 15, voice 0; wait 16; end
        put(&mut d, 0xa00, 0x1e00_f000);
        put(&mut d, 0xa04, 0xf310_0000);
        put(&mut d, 0xa08, 0xf000_0000);
        put(&mut d, 0xa20, 0xf000_0000);
        for (k, &c) in macro_.iter().enumerate() {
            put(&mut d, 0xb00 + 4 * k, c);
        }
        d
    }

    /// DMA off, SetBegin 0, SetLen 16, SetVol $30, AddNote 0, DMA on, Stop.
    const MACRO: [u32; 7] = [0x0000_0000, 0x0200_0000, 0x0300_0010, 0x0e00_0030, 0x0800_0000, 0x0100_0000, 0x0700_0000];

    /// Plays `mdat` (with a 64-byte square wave as smpl.test) for 0.2 s, and
    /// returns the format, the smpl address (LOC0's first value) and the writes.
    fn play(name: &str, mdat: Vec<u8>) -> (String, Vec<(u32, u16, u64)>) {
        let dir = std::env::temp_dir().join(format!("deliplayer-tfmx-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let smpl: Vec<u8> = (0..64).map(|i| if i < 32 { 0x40 } else { 0xc0 }).collect();
        std::fs::write(dir.join("smpl.test"), smpl).unwrap();
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        let r = e.play_data(&dir.join("mdat.test"), mdat);
        let _ = std::fs::remove_dir_all(&dir);
        r.unwrap();
        let format = e.playing.as_ref().unwrap().format.clone();
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
        (format, e.m.bus.write_log.take().unwrap())
    }

    fn reg(log: &[(u32, u16, u64)], a: u32) -> Vec<u16> {
        log.iter().filter(|w| w.0 == a).map(|w| w.1).collect()
    }

    #[test]
    fn pro_plays_a_note_on_paula() {
        let (format, log) = play("pro", mdat(b"TFMX-SONG ", &MACRO));
        assert_eq!(format, "TFMX Pro");
        // InitSound's all off, then the macro: DMA off at once, the sample's
        // start and length, the volume; the period and the DMA on at the ends
        // of the ticks (the first tick's DMACON is 0)
        assert_eq!(&reg(&log, 0xDFF096)[..4], &[0x000F, 0x0000, 0x0001, 0x0000]);
        assert!(reg(&log, 0xDFF096).contains(&0x8201));
        assert_eq!(&reg(&log, 0xDFF0A4)[..1], &[16]);
        assert!(reg(&log, 0xDFF0A8).contains(&0x30));
        assert!(reg(&log, 0xDFF0A6).contains(&super::PERIODS[30]));
        // the 9-line wait before the period's tick-end DMACON
        let per = log.iter().find(|w| w.0 == 0xDFF0A6 && w.1 == super::PERIODS[30]).unwrap().2;
        let on = log.iter().find(|w| w.0 == 0xDFF096 && w.1 == 0x8201).unwrap().2;
        assert!(on >= per + 8 * crate::paula::LINE);
    }

    #[test]
    fn v15_plays_a_note_on_paula() {
        let (format, log) = play("15", mdat(b"TFMX \0\0\x01\0\0", &MACRO));
        assert_eq!(format, "TFMX 1.5");
        // DMA off, the sample, the period at once; the volume ($30 at the master
        // volume $3F) and DMA on in the next tick
        assert_eq!(&reg(&log, 0xDFF096)[..2], &[0x0001, 0x8001]);
        assert_eq!(&reg(&log, 0xDFF0A4)[..1], &[16]);
        assert_eq!(&reg(&log, 0xDFF0A6)[..1], &[super::PERIODS[30]]);
        assert!(reg(&log, 0xDFF0A8).contains(&(0x30 * 4 * 0x3f >> 8)));
    }
}

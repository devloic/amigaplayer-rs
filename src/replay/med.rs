//! MED and OctaMED (MMD0, MMD1 and MMD2 modules on Paula's four channels),
//! ported from NostalgicPlayer's OctaMED player (Copyright (c) 2023 Thomas
//! Neumann, MIT licence: see LICENSE-NostalgicPlayer).
//!
//! NostalgicPlayer plays every module through its mixer, as OctaMED
//! SoundStudio's mix mode does: notes are frequencies, volumes go to 128 and
//! the tracks are not tied to Paula's channels. This port keeps its structure
//! (the track data, the row and pulse handling, the hold and decay, the effect
//! commands, the synth sound programs) but plays as the MED Eagleplayer does,
//! the proplayer of OctaMED 7, checked against its register writes: tracks 0-3
//! on Paula's channels 0-3, notes as periods from the ProTracker tables (two
//! octaves lower for synth sounds), slides, vibrato and portamento on the
//! period, volumes 0-64, and the commands as the proplayer knows them. See the
//! README for the list of places where the two differ.
//!
//! The module stays in guest memory where it was loaded, with its pointers
//! unrelocated: notes, commands, samples and synth waveforms are read from
//! there, so a module that points past its blocks reads what the original
//! would read.

use super::{Hw, Open, Replayer, be32};
use crate::bus::Bus;
use crate::machine::Machine;
use crate::paula::LINE;
use m68k::AddressBus;

mod med4;

pub const OPEN: Open = Open {
    name: "MED",
    detect,
    new,
};

/// ProTracker's period tables, finetunes 0 to 7 and -8 to -1 (NostalgicPlayer's
/// ModTracker tables).
const PT_PERIODS: [[u16; 36]; 16] = [
    [
        856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320,
        302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113,
    ],
    [
        850, 802, 757, 715, 674, 637, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337, 318,
        300, 284, 268, 253, 239, 225, 213, 201, 189, 179, 169, 159, 150, 142, 134, 126, 119, 113,
    ],
    [
        844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335, 316,
        298, 282, 266, 251, 237, 224, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118, 112,
    ],
    [
        838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332, 314,
        296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118, 111,
    ],
    [
        832, 785, 741, 699, 660, 623, 588, 555, 524, 495, 467, 441, 416, 392, 370, 350, 330, 312,
        294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 124, 117, 110,
    ],
    [
        826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328, 309,
        292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116, 109,
    ],
    [
        820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325, 307,
        290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115, 109,
    ],
    [
        814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323, 305,
        288, 272, 256, 242, 228, 216, 204, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114, 108,
    ],
    [
        907, 856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339,
        320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120,
    ],
    [
        900, 850, 802, 757, 715, 675, 636, 601, 567, 535, 505, 477, 450, 425, 401, 379, 357, 337,
        318, 300, 284, 268, 253, 238, 225, 212, 200, 189, 179, 169, 159, 150, 142, 134, 126, 119,
    ],
    [
        894, 844, 796, 752, 709, 670, 632, 597, 563, 532, 502, 474, 447, 422, 398, 376, 355, 335,
        316, 298, 282, 266, 251, 237, 223, 211, 199, 188, 177, 167, 158, 149, 141, 133, 125, 118,
    ],
    [
        887, 838, 791, 746, 704, 665, 628, 592, 559, 528, 498, 470, 444, 419, 395, 373, 352, 332,
        314, 296, 280, 264, 249, 235, 222, 209, 198, 187, 176, 166, 157, 148, 140, 132, 125, 118,
    ],
    [
        881, 832, 785, 741, 699, 660, 623, 588, 555, 524, 494, 467, 441, 416, 392, 370, 350, 330,
        312, 294, 278, 262, 247, 233, 220, 208, 196, 185, 175, 165, 156, 147, 139, 131, 123, 117,
    ],
    [
        875, 826, 779, 736, 694, 655, 619, 584, 551, 520, 491, 463, 437, 413, 390, 368, 347, 328,
        309, 292, 276, 260, 245, 232, 219, 206, 195, 184, 174, 164, 155, 146, 138, 130, 123, 116,
    ],
    [
        868, 820, 774, 730, 689, 651, 614, 580, 547, 516, 487, 460, 434, 410, 387, 365, 345, 325,
        307, 290, 274, 258, 244, 230, 217, 205, 193, 183, 172, 163, 154, 145, 137, 129, 122, 115,
    ],
    [
        862, 814, 768, 725, 684, 646, 610, 575, 543, 513, 484, 457, 431, 407, 384, 363, 342, 323,
        305, 288, 272, 256, 242, 228, 216, 203, 192, 181, 171, 161, 152, 144, 136, 128, 121, 114,
    ],
];

/// Words in one finetune's period table: two octaves below C-1 (synth sounds),
/// the three octaves, and the top octave three more times (MED clamps higher
/// notes so). The 16 tables follow each other (finetunes 0..7, -8..-1), and a
/// note far out of range reads into the next one.
const TABLE: usize = 96;
/// Index of C-1 in a table.
const C1: usize = 24;

/// SoundTracker-compatible tempos 1-10 as CIA timer values (NostalgicPlayer's
/// MED tables); higher tempos are 470000 / tempo, BPM tempos 1773447 / (tempo *
/// lines per beat) with the song advancing on every fourth timer interrupt.
const SPD_TEMPOS: [u16; 11] = [
    0x0f00, 2417, 4833, 7250, 9666, 12083, 14500, 16916, 19332, 21436, 24163,
];
const SPD_CLOCK: u32 = 470000;
const BPM_CLOCK: u32 = 1773447;

/// The vibrato sine (NostalgicPlayer's sineTable).
const SINE: [i8; 32] = [
    0, 25, 49, 71, 90, 106, 117, 125, 127, 125, 117, 106, 90, 71, 49, 25, 0, -25, -49, -71, -90,
    -106, -117, -125, -127, -125, -117, -106, -90, -71, -49, -25,
];

/// Multi-octave (IFF) samples: per type, per octave of the note, the shift of
/// the shortest octave's lengths, the octave's offset in shortest-octave
/// lengths, and the note added (NostalgicPlayer's MED ShiftCount,
/// MultiplyLengthCount and OctaveStart give the 5- and 3-octave rows; the
/// others follow the same rule for 2, 4, 6 and 7 octaves).
const OCT_SHIFT: [[u8; 6]; 6] = [
    [4, 3, 2, 1, 1, 0],
    [2, 2, 1, 1, 0, 0],
    [1, 1, 0, 0, 0, 0],
    [3, 3, 2, 2, 1, 0],
    [5, 4, 3, 2, 1, 0],
    [6, 5, 4, 3, 2, 1],
];
const OCT_NOTE: [[u8; 6]; 6] = [
    [12, 12, 12, 12, 24, 24],
    [0, 12, 12, 24, 24, 36],
    [0, 12, 12, 24, 36, 36],
    [0, 12, 12, 24, 24, 24],
    [12, 12, 12, 12, 12, 12],
    [12, 12, 12, 12, 12, 12],
];

/// Song flags.
const FLAG_FILTER: u8 = 0x01;
const FLAG_VOLHEX: u8 = 0x10;
const FLAG_STSLIDE: u8 = 0x20;
const FLAG_8CH: u8 = 0x40;
const FLAG2_BPM: u8 = 0x20;
const FLAG2_MIX: u8 = 0x80;

/// The channel the Paula writes of a one-shot sample loop on, after it ended:
/// one silent word (passed as length 0, so 65536 words, as the original does).
const SILENCE_SIZE: u32 = 0x20000;

/// Colour clocks the original spends between its writes (fitted to its logs;
/// by channel where they differ).
mod cost {
    pub const TICK: i64 = 4600;
    /// From a note's length to the next note's DMA off.
    pub const DMA_OFF: i64 = 330;
    /// A DMA off with no note before it in the tick.
    pub const DMA_OFF_FIRST: i64 = 200;
    pub const NOTE: [i64; 4] = [425, 440, 456, 460];
    pub const LEN: [i64; 4] = [240, 256, 272, 275];
    /// After the row's notes: the next position and the hold look-ahead, less
    /// per note played.
    pub const ROW_END: i64 = 1670;
    pub const ROW_END_NOTE: i64 = 30;
    pub const PER: i64 = 172;
    pub const VOL: [i64; 4] = [172, 183, 195, 190];
    /// A synth sound's new waveform: before its length, its start, and after.
    pub const WF_LEN: i64 = 60;
    /// A synth or hybrid sound's programs, each tick.
    pub const SYNTH: i64 = 450;
    pub const WF_LOC: i64 = 90;
    pub const WF_END: i64 = 74;
    pub const DMA_ON: i64 = 138;
    /// After the host's wait for the DMA, the first loop's start, the next ones'.
    pub const LOOP_FIRST: i64 = 211;
    pub const LOOP: i64 = 150;
    pub const LOOP_LEN: i64 = 112;
}

#[derive(Clone, Default)]
struct Track {
    prev_note: u8,
    instr: u8,
    vol: u8,
    hold_cnt: i8,
    hold: u8,
    decay: u8,
    strans: i8,
    finetune: i8,
    soffs: u16,
    keep_wf: bool,
    curr_note: u8,
    outdev: u8,
    /// The note is not played now (03, 1F...), and for tracks past 3 no effects.
    skip: bool,
    /// The song's sample struct of the instrument (0: none yet).
    smp: u32,
    /// The loop the note goes on with after its DMA is on (0: a synth sound).
    rptr: u32,
    rlen: u16,
    per: u16,
    porta_target: u16,
    porta_speed: u8,
    vib_shift: u8,
    vib_speed: u8,
    vib_depth: u16,
    vib_phase: u8,
    trem_phase: u8,
    trem_depth: u16,
    trem_speed: u8,
    /// This tick's period offsets (vibrato, arpeggio) and volume (0xff: none).
    vib_ofs: u16,
    arp_ofs: u16,
    out_vol: u8,
    trk_vol: u16,
    decay_speed: u8,
    fade_speed: u8,
    /// Index of the note's period table in `periods` (finetune table + C1;
    /// synth sounds two octaves lower), or none before a first note.
    per_tbl: Option<usize>,
    /// Synth or hybrid sound playing: its instrument, 1 synth, -1 hybrid.
    synth: u32,
    stype: i8,
    synth_note: u8,
    synth_vol: u8,
    arp_pos: u8,
    arp_start: u8,
    vol_cnt: i8,
    wf_cnt: i8,
    vol_pos: u8,
    wf_pos: u8,
    vol_wait: u8,
    wf_wait: u8,
    vol_speed: u8,
    wf_speed: u8,
    vol_chg: i8,
    per_chg: u16,
    per_chg_acc: u16,
    env: u32,
    env_loop: u32,
    env_cnt: i8,
    svib_speed: u16,
    svib_depth: u16,
    svib_wave: u32,
    svib_phase: u16,
}

pub struct Med {
    base: u32,
    /// The module header of the subsong playing (MMD files chain them).
    mmd: u32,
    ver: u8,
    song: u32,
    blockarr: u32,
    smplarr: u32,
    numblocks: u16,
    songlen: u16,
    deftempo: u16,
    playtransp: i8,
    flags: u8,
    flags2: u8,
    tempo2: u8,
    subsongs: u16,
    sub: u16,
    title: String,

    // the play position
    playing: bool,
    pline: u16,
    pblock: u16,
    pseqnum: u16,
    psecnum: u16,
    counter: u8,
    numtracks: u16,
    lastline: u16,
    numpages: u16,
    new_block: u8,
    keep_line: bool,
    next_line1: u16,
    loop_line: u16,
    loop_cnt: u16,
    block_delay: u16,
    fx_block: u16,
    fx_line: u16,
    dma_on: u16,
    // per instrument
    i_hold: [u8; 63],
    i_decay: [u8; 63],
    i_finetune: [u8; 63],
    i_flags: [u8; 63],
    i_outdev: [u8; 63],
    i_rep: [u32; 63],
    i_replen: [u32; 63],

    tracks: Vec<Track>,
    periods: Vec<u16>,
    silence: u32,
    /// The CIA timer's latch, and whether the song advances every fourth interrupt.
    timer: u16,
    bpm: bool,
    /// BPM tempos: timer interrupts until the song advances (every fourth).
    irqs: u8,
    songend: bool,
    spent: i64,
}

fn detect(d: &[u8]) -> bool {
    if d.len() >= 36 && &d[0..4] == b"MED\x04" {
        return med4::to_mmd0(d).is_some_and(|c| detect(&c));
    }
    if d.len() < 0x34 || &d[0..3] != b"MMD" || !(b'0'..=b'2').contains(&d[3]) {
        return false;
    }
    let song = be32(d, 8) as usize;
    if song == 0 || song + 0x314 > d.len() {
        return false;
    }
    // the 5-8 channel and mix modes have players of their own
    d[song + 0x2ff] & FLAG_8CH == 0 && d[song + 0x300] & FLAG2_MIX == 0
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    // a MED 2.10 module plays converted to MMD0, in memory of its own
    let conv;
    let (d, base) = if d.starts_with(b"MED\x04") {
        conv = med4::to_mmd0(d)?;
        let a = m.alloc(conv.len() as u32);
        if a == 0 {
            return None;
        }
        m.bus.write_bytes(a, &conv);
        (&conv[..], a)
    } else {
        (d, base)
    };
    let silence = m.alloc(SILENCE_SIZE);
    if silence == 0 {
        return None;
    }
    let periods = periods();
    let mut p = Med {
        base,
        mmd: base,
        ver: d[3],
        song: 0,
        blockarr: 0,
        smplarr: 0,
        numblocks: 0,
        songlen: 0,
        deftempo: 0,
        playtransp: 0,
        flags: 0,
        flags2: 0,
        tempo2: 6,
        subsongs: d[0x33] as u16,
        sub: 0,
        title: String::new(),
        playing: false,
        pline: 0,
        pblock: 0,
        pseqnum: 0,
        psecnum: 0,
        counter: 0,
        numtracks: 0,
        lastline: 0,
        numpages: 0,
        new_block: 0,
        keep_line: false,
        next_line1: 0,
        loop_line: 0,
        loop_cnt: 0,
        block_delay: 0,
        fx_block: 0,
        fx_line: 0,
        dma_on: 0,
        i_hold: [0; 63],
        i_decay: [0; 63],
        i_finetune: [0; 63],
        i_flags: [0; 63],
        i_outdev: [0; 63],
        i_rep: [0; 63],
        i_replen: [0; 63],
        tracks: vec![
            Track {
                out_vol: 0xff,
                ..Track::default()
            };
            64
        ],
        periods,
        silence,
        timer: 0,
        bpm: false,
        irqs: 0,
        songend: false,
        spent: 0,
    };
    p.title = p.song_name(&m.bus);
    p.play_module(&mut m.bus, 0);
    Some(Box::new(p))
}

impl Replayer for Med {
    fn format(&self) -> &'static str {
        "MED"
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn tick_clocks(&self) -> u64 {
        (self.timer as u64 + 1) * 5
    }

    fn songend(&self) -> bool {
        self.songend
    }

    fn subsongs(&self) -> (u16, u16, u16) {
        (0, self.subsongs, self.sub)
    }

    fn set_subsong(&mut self, n: u16, hw: &mut Hw) {
        // the song playing stops first, its channels from the last
        self.stop_all(hw);
        self.flush(hw);
        self.play_module(hw.bus, n);
    }

    fn tick(&mut self, hw: &mut Hw) {
        if self.bpm {
            // the first interrupt plays, then every fourth
            let n = self.irqs;
            self.irqs = (n + 1) & 3;
            if n != 0 {
                return;
            }
        }
        if !self.playing {
            return;
        }
        self.spent = cost::TICK;
        self.dma_on = 0;
        let c = self.counter.wrapping_add(1);
        if (c as i8) < (self.tempo2 as i8) && c != 0 {
            self.counter = c;
        } else {
            self.counter = 0;
            if self.block_delay == 0 || {
                self.block_delay -= 1;
                self.block_delay == 0
            } {
                if !self.row(hw) {
                    return;
                }
            }
        }
        self.every_tick(hw);
        self.dma_on_writes(hw);
    }
}

impl Med {
    /// A pointer of the module (an offset in the file, 0 for none).
    fn ptr(&self, b: &Bus, a: u32) -> u32 {
        match b.r32(a) {
            0 => 0,
            v => v.wrapping_add(self.base),
        }
    }

    fn song_name(&self, b: &Bus) -> String {
        let exp = self.ptr(b, self.base + 0x20);
        if exp == 0 {
            return String::new();
        }
        let (name, len) = (self.ptr(b, exp + 0x2c), b.r32(exp + 0x30));
        if name == 0 || len == 0 || len > 256 {
            return String::new();
        }
        let s: Vec<u8> = (0..len)
            .map(|k| b.r8(name + k))
            .take_while(|&c| c != 0)
            .collect();
        String::from_utf8_lossy(&s).trim().to_string()
    }

    /// InitSound and StartInt: the subsong's module header, its instruments, the
    /// track volumes, the filter, the position at its start and the tempo.
    fn play_module(&mut self, b: &mut Bus, sub: u16) {
        self.sub = sub;
        let mut mmd = self.base;
        for _ in 0..sub {
            let exp = self.ptr(b, mmd + 0x20);
            if exp == 0 {
                break;
            }
            let next = self.ptr(b, exp);
            if next == 0 {
                break;
            }
            mmd = next;
        }
        self.mmd = mmd;
        // a continuation module (MCNT) has the first one's format
        let id = b.r32(mmd);
        self.ver = if id >> 8 == 0x4d4d44 {
            id as u8
        } else {
            b.r8(self.base + 3)
        };
        self.song = self.ptr(b, mmd + 8);
        self.blockarr = self.ptr(b, mmd + 0x10);
        self.smplarr = self.ptr(b, mmd + 0x18);
        let s = self.song;
        self.numblocks = b.r16(s + 0x1f8);
        self.songlen = b.r16(s + 0x1fa);
        self.deftempo = b.r16(s + 0x2fc);
        self.playtransp = b.r8(s + 0x2fe) as i8;
        self.flags = b.r8(s + 0x2ff);
        self.flags2 = b.r8(s + 0x300);
        self.tempo2 = b.r8(s + 0x301);
        self.init_tracks(b);

        self.counter = self.tempo2;
        self.pline = 0;
        self.pseqnum = 0;
        self.psecnum = b.r16(mmd + 0xc);
        self.loop_line = 0;
        self.loop_cnt = 0;
        self.block_delay = 0;
        self.pblock = if self.ver >= b'2' {
            let pseq = self.play_seq(b);
            b.r16(pseq + 0x2a)
        } else {
            // a first block of 128 or more is taken as negative there
            b.r8(s + 0x1fc) as i8 as i16 as u16
        };
        // the filter: CIA-A PRA bit 1 set is off
        let pra = b.read_byte(0xbfe001);
        let pra = if self.flags & FLAG_FILTER == 0 {
            pra | 2
        } else {
            pra & !2
        };
        b.write_byte(0xbfe001, pra);
        self.playing = true;
        self.songend = false;
        self.irqs = 0;
        self.bpm = self.flags2 & FLAG2_BPM != 0;
        self.set_tempo(self.deftempo);
    }

    /// The play sequence of the current section (MMD2).
    fn play_seq(&self, b: &Bus) -> u32 {
        let sect = self.ptr(b, self.song + 0x200);
        let n = b.r16(sect.wrapping_add(2 * self.psecnum as u32));
        self.ptr(b, self.ptr(b, self.song + 0x1fc).wrapping_add(4 * n as u32))
    }

    /// The track volumes and the instruments' hold, decay, finetune, loop flag
    /// and loop, from the song and its InstrExt.
    fn init_tracks(&mut self, b: &Bus) {
        let s = self.song;
        let master = b.r8(s + 0x312) as i8 as i16 as u16;
        let (vols, n) = if self.ver >= b'2' {
            (self.ptr(b, s + 0x204), b.r16(s + 0x208) as usize)
        } else {
            (s + 0x302, 16)
        };
        for t in 0..n.min(64) {
            let v = b.r8(vols + t as u32) as i8 as i16 as u16;
            self.tracks[t].trk_vol = v.wrapping_mul(master) >> 4;
        }
        for t in &mut self.tracks[..4] {
            t.synth = 0;
            t.stype = 0;
        }
        // the first module's instrument extensions, for every song of the file
        let exp = self.ptr(b, self.base + 0x20);
        let iext = if exp != 0 { self.ptr(b, exp + 4) } else { 0 };
        let entries = if iext != 0 { b.r16(exp + 8) } else { 0 };
        if entries != 0 {
            let size = b.r16(exp + 0xa) as i16;
            let mut e = iext;
            for i in 0..(entries as i16).min(63) as usize {
                let smp = s + 8 * i as u32;
                self.i_finetune[i] = if size > 3 { b.r8(e + 3) } else { 0 };
                self.i_flags[i] = if size >= 6 {
                    b.r8(e + 5)
                } else {
                    (b.r16(smp + 2) > 1) as u8
                };
                self.i_outdev[i] = if size >= 9 { b.r8(e + 8) } else { 0 };
                if size >= 0x12 {
                    self.i_rep[i] = b.r32(e + 0xa);
                    self.i_replen[i] = b.r32(e + 0xe);
                } else {
                    self.i_rep[i] = 2 * b.r16(smp) as u32;
                    self.i_replen[i] = 2 * b.r16(smp + 2) as u32;
                }
                self.i_decay[i] = b.r8(e + 1);
                self.i_hold[i] = b.r8(e);
                e = e.wrapping_add(size as i32 as u32);
            }
        } else {
            for i in 0..63 {
                let smp = s + 8 * i as u32;
                self.i_hold[i] = 0;
                self.i_decay[i] = 0;
                self.i_finetune[i] = 0;
                // the original leaves the loop flags and output devices
                self.i_flags[i] = 0;
                if b.r16(smp + 2) > 1 {
                    self.i_flags[i] |= 1;
                }
                self.i_rep[i] = 2 * b.r16(smp) as u32;
                self.i_replen[i] = 2 * b.r16(smp + 2) as u32;
            }
        }
    }

    fn set_tempo(&mut self, tempo: u16) {
        self.deftempo = tempo;
        let t = if self.flags2 & FLAG2_BPM != 0 {
            let lpb = (self.flags2 & 0x1f) as u32 + 1;
            divu(BPM_CLOCK, (tempo as u32 * lpb) as u16)
        } else if tempo <= 10 {
            SPD_TEMPOS[tempo as usize]
        } else {
            divu(SPD_CLOCK, tempo)
        };
        self.timer = t;
    }

    fn block(&self, b: &Bus, n: u16) -> u32 {
        self.ptr(b, self.blockarr.wrapping_add((n as i16 as i32 * 4) as u32))
    }

    /// The block's track count, last line and command pages.
    fn block_dims(&mut self, b: &Bus, blk: u32) {
        if self.ver >= b'1' {
            self.numtracks = b.r16(blk);
            self.lastline = b.r16(blk + 2);
            self.numpages = 0;
            let info = self.ptr(b, blk + 4);
            if info != 0 {
                let pt = self.ptr(b, info + 0xc);
                if pt != 0 {
                    self.numpages = b.r16(pt);
                }
            }
        } else {
            self.numpages = 0;
            self.numtracks = b.r8(blk) as u16;
            self.lastline = b.r8(blk + 1) as u16;
        }
    }

    /// The command byte of `track` on `line`, page `page`, and the stride to the
    /// next track's.
    fn cmd_ptr(&self, b: &Bus, blk: u32, page: u16, line: u16) -> (u32, u32) {
        if self.ver >= b'1' {
            let off = line as u32 * b.r16(blk) as u32 * 2;
            if page > 0 {
                let info = self.ptr(b, blk + 4);
                let pt = self.ptr(b, info + 0xc);
                let pg = self.ptr(b, pt + 4 * page as u32);
                return (pg.wrapping_add(off), 2);
            }
            (blk + 0xa + 2 * off, 4)
        } else {
            let off = (line as u32 * b.r8(blk) as u32 * 3) & 0xffff;
            (blk + 3 + off, 3)
        }
    }

    /// A row tick: the notes and instruments, the row's commands, the notes
    /// played, and the next position. False if the song stopped (0FFE).
    fn row(&mut self, hw: &mut Hw) -> bool {
        let blk = self.block(hw.bus, self.pblock);
        self.block_dims(hw.bus, blk);
        let line = self.pline.min(self.lastline) as u32;
        let mmd1 = self.ver >= b'1';
        let mut a = if mmd1 {
            blk + 8 + line * 4 * self.numtracks as u32
        } else {
            blk + 2 + ((line * 3) & 0xffff) * self.numtracks as u32
        };
        for t in 0..self.numtracks as usize {
            let b = &*hw.bus;
            let (note, mut inum);
            if mmd1 {
                let n = b.r8(a);
                note = if n & 0x80 != 0 { 0 } else { n };
                inum = b.r8(a + 1);
                a += 4;
            } else {
                let n = b.r8(a);
                inum = b.r8(a + 1) >> 4;
                if n & 0x80 != 0 {
                    inum |= 0x10;
                }
                if n & 0x40 != 0 {
                    inum |= 0x20;
                }
                note = n & 0x3f;
                a += 3;
            }
            let smp_base = self.song;
            let tr = &mut self.tracks[t.min(63)];
            tr.curr_note = note;
            if note != 0 {
                tr.prev_note = note;
            }
            tr.skip = false;
            inum &= 0x3f;
            if inum != 0 {
                let i = (inum - 1) as usize;
                tr.instr = i as u8;
                let smp = smp_base + 8 * i as u32;
                tr.smp = smp;
                tr.vol = b.r8(smp + 6);
                tr.strans = b.r8(smp + 7) as i8;
                let i = i.min(62);
                tr.hold = self.i_hold[i];
                tr.decay = self.i_decay[i];
                tr.finetune = self.i_finetune[i] as i8;
                tr.outdev = self.i_outdev[i];
                tr.soffs = 0;
                tr.keep_wf = false;
            }
        }
        if !self.row_commands(hw, blk) {
            return false;
        }
        let mut notes = 0;
        for t in 0..self.numtracks.min(64) as usize {
            let tr = &mut self.tracks[t];
            if tr.skip || tr.curr_note == 0 {
                continue;
            }
            tr.hold_cnt = if tr.hold == 0 { -1 } else { tr.hold as i8 };
            let note = tr.curr_note;
            self.play_note(hw, t, note);
            notes += 1;
        }
        self.spend(cost::ROW_END - cost::ROW_END_NOTE * notes.min(4));
        self.advance(hw.bus);
        true
    }

    /// The commands of the row on every page that act once, on the row tick.
    fn row_commands(&mut self, hw: &mut Hw, blk: u32) -> bool {
        let mut page = 0u16;
        loop {
            let (mut a, stride) = self.cmd_ptr(hw.bus, blk, page, self.pline);
            for t in 0..self.numtracks.min(64) as usize {
                let c = hw.bus.r8(a);
                let arg = hw.bus.r8(a + 1);
                a = a.wrapping_add(stride);
                if c == 0 {
                    continue;
                }
                let cmd = if stride == 3 { c & 0xf } else { c & 0x3f };
                match self.row_command(hw, t, cmd, arg) {
                    Some(skip) => self.tracks[t].skip |= skip,
                    None => return false,
                }
            }
            page += 1;
            if page > self.numpages {
                return true;
            }
        }
    }

    /// One command on the row tick: whether it holds the note back (None: the
    /// song stopped).
    fn row_command(&mut self, hw: &mut Hw, t: usize, cmd: u8, arg: u8) -> Option<bool> {
        match cmd {
            0x03 => {
                if t >= 4 {
                    return Some(true);
                }
                let tr = &self.tracks[t];
                let n = tr.curr_note.wrapping_sub(1);
                if (n as i8) >= 0 {
                    let Some(tbl) = tr.per_tbl else {
                        return Some(true);
                    };
                    let n = n
                        .wrapping_add(self.playtransp as u8)
                        .wrapping_add(tr.strans as u8);
                    if (n as i8) < 0 {
                        return Some(true);
                    }
                    let target = self.period(tbl, n as isize);
                    self.tracks[t].porta_target = target;
                }
                if arg != 0 {
                    self.tracks[t].porta_speed = arg;
                }
                return Some(true);
            }
            0x08 => {
                let tr = &mut self.tracks[t];
                tr.decay = arg >> 4;
                tr.hold = arg & 0xf;
            }
            0x09 => {
                let v = arg & 0x1f;
                self.tempo2 = if v == 0 { 0x20 } else { v };
            }
            0x0b => {
                let len = if self.ver < b'2' {
                    self.songlen
                } else {
                    let pseq = self.play_seq(hw.bus);
                    hw.bus.r16(pseq + 0x28)
                };
                if arg as u16 > len {
                    return Some(false);
                }
                if !(arg as i16 > self.pseqnum as i16) {
                    self.songend = true;
                }
                self.pseqnum = arg as u16;
                self.new_block = 0xff;
            }
            0x0c => {
                let mut v = arg;
                if arg & 0x80 != 0 {
                    v &= 0x7f;
                    if v > 0x40 {
                        return Some(false);
                    }
                    // it sets the instrument's default volume in the song too
                    let smp = self.song + 8 * self.tracks[t].instr as u32 + 6;
                    hw.bus.w8(smp, v);
                } else {
                    if self.flags & FLAG_VOLHEX == 0 {
                        v = (arg >> 4).wrapping_mul(10).wrapping_add(arg & 0xf);
                    }
                    if v > 0x40 {
                        return Some(false);
                    }
                }
                self.tracks[t].vol = v;
            }
            0x0e => {
                if t < 4 {
                    let tr = &mut self.tracks[t];
                    tr.keep_wf = true;
                    tr.wf_pos = arg;
                }
            }
            0x0f => return self.row_misc(hw, t, arg),
            0x15 => {
                if (arg as i8) <= 7 && (arg as i8) >= -8 {
                    self.tracks[t].finetune = arg as i8;
                }
            }
            0x16 => {
                if arg == 0 {
                    self.loop_line = self.pline;
                    return Some(false);
                }
                if self.loop_cnt != 0 {
                    self.loop_cnt -= 1;
                    if self.loop_cnt == 0 {
                        return Some(false);
                    }
                } else {
                    self.loop_cnt = (self.loop_cnt & 0xff00) | arg as u16;
                }
                self.next_line1 = self.loop_line.wrapping_add(1);
            }
            0x19 => self.tracks[t].soffs = (arg as u16) << 8,
            0x1d => {
                self.new_block = 1;
                self.keep_line = true;
                self.next_line1 = arg as u16 + 1;
            }
            0x1e => {
                if self.block_delay == 0 {
                    self.block_delay = arg as u16 + 1;
                }
            }
            0x1f => return Some(self.delay_note(t)),
            _ => {}
        }
        Some(false)
    }

    fn delay_note(&mut self, t: usize) -> bool {
        let tr = &mut self.tracks[t];
        tr.hold_cnt = if tr.hold == 0 { -1 } else { tr.hold as i8 };
        true
    }

    fn row_misc(&mut self, hw: &mut Hw, t: usize, arg: u8) -> Option<bool> {
        match arg {
            0 => self.new_block = 1,
            1..=0xf0 => self.set_tempo(arg as u16),
            0xf2 | 0xf4 | 0xf5 => return Some(self.delay_note(t)),
            0xfe => {
                self.playing = false;
                self.songend = true;
                self.stop_all(hw);
                return None;
            }
            0xfd => {
                if t >= 4 {
                    return Some(false);
                }
                let tr = &self.tracks[t];
                let Some(tbl) = tr.per_tbl else {
                    return Some(true);
                };
                let n = tr.curr_note.wrapping_sub(1);
                if (n as i8) < 0 {
                    return Some(true);
                }
                let n = n
                    .wrapping_add(self.playtransp as u8)
                    .wrapping_add(tr.strans as u8);
                self.tracks[t].per = self.period(tbl, n as isize);
                return Some(true);
            }
            0xff => self.mute(hw, t),
            _ => {}
        }
        Some(false)
    }

    /// A period from the flat tables, `n` notes from `tbl`; past the last table
    /// the original reads its own variables, taken as 0 here.
    fn period(&self, tbl: usize, n: isize) -> u16 {
        let i = tbl as isize + n;
        if i < 0 {
            return 0;
        }
        self.periods.get(i as usize).copied().unwrap_or(0)
    }

    fn ft_table(ft: i8) -> usize {
        (ft as u8 & 15) as usize * TABLE + C1
    }

    /// The next line, block and play sequence position, then the hold of notes
    /// held on into the new row.
    fn advance(&mut self, b: &Bus) {
        self.fx_block = self.pblock;
        self.fx_line = self.pline;
        let mut line = if self.next_line1 != 0 {
            let l = self.next_line1 - 1;
            self.next_line1 = 0;
            l
        } else {
            self.pline.wrapping_add(1)
        };
        if line > self.lastline || self.new_block != 0 {
            if !self.keep_line {
                line = 0;
            }
            let blk = self.next_pos(b, self.new_block != 0xff);
            self.pblock = if (blk as i16) < self.numblocks as i16 {
                blk
            } else {
                0
            };
            self.new_block = 0;
            self.keep_line = false;
        }
        self.pline = line;

        let blk = self.block(b, self.pblock);
        let tempo2 = self.tempo2;
        if self.ver < b'1' {
            let n = b.r8(blk) as u32;
            let mut a = blk + 2 + ((self.pline as u32 * 3) & 0xffff) * n;
            for t in 0..n.max(1) as usize {
                let (b0, b1) = (b.r8(a), b.r8(a + 1));
                a += 3;
                let tr = &mut self.tracks[t.min(63)];
                if tr.hold_cnt < 0 {
                    continue;
                }
                let extend = if b0 != 0 {
                    b0 & 0x3f == 0 || b1 & 0xf == 3
                } else {
                    b1 & 0xf0 != 0
                };
                if extend {
                    tr.hold_cnt = tr.hold_cnt.wrapping_add(tempo2 as i8);
                }
            }
        } else {
            let n = b.r16(blk) as u32;
            let mut a = blk + 8 + self.pline as u32 * 4 * n;
            for t in 0..(n & 0xff).max(1) as usize {
                let (b0, b1, b2) = (b.r8(a), b.r8(a + 1), b.r8(a + 2));
                a += 4;
                let tr = &mut self.tracks[t.min(63)];
                if tr.hold_cnt < 0 {
                    continue;
                }
                let extend = if b0 != 0 {
                    b0 & 0x7f == 0 || b2 == 3
                } else {
                    b1 & 0x3f != 0
                };
                if extend {
                    tr.hold_cnt = tr.hold_cnt.wrapping_add(tempo2 as i8);
                }
            }
        }
    }

    /// The block at the next play sequence position (the same one after 0B).
    fn next_pos(&mut self, b: &Bus, inc: bool) -> u16 {
        if self.ver >= b'2' {
            // a command entry (bit 15) is skipped; at most a full turn
            for _ in 0..0x10000 {
                let mut pseq = self.play_seq(b);
                let mut n = self.pseqnum;
                if inc {
                    n = n.wrapping_add(1);
                }
                if !((n as i16) < b.r16(pseq + 0x28) as i16) {
                    let mut s = self.psecnum.wrapping_add(1);
                    if !((s as i16) < self.songlen as i16) {
                        s = 0;
                        self.songend = true;
                    }
                    self.psecnum = s;
                    pseq = self.play_seq(b);
                    n = 0;
                }
                self.pseqnum = n;
                let blk = b.r16(pseq + 0x2a + 2 * n as u32);
                if (blk as i16) >= 0 {
                    return blk;
                }
            }
            return 0;
        }
        let mut n = self.pseqnum;
        if inc {
            n = n.wrapping_add(1);
        }
        if !((n as i16) < self.songlen as i16) {
            n = 0;
            self.songend = true;
        }
        self.pseqnum = (self.pseqnum & 0xff00) | (n & 0xff);
        (n & 0xff00) | b.r8(self.song + 0x1fc + (n & 0xff) as u32) as u16
    }

    /// Plays a track's note: DMA off, the sample's start and length, its period
    /// and loop (written after the DMA goes on).
    fn play_note(&mut self, hw: &mut Hw, t: usize, note: u8) {
        let tr = &self.tracks[t];
        if tr.smp == 0 {
            return;
        }
        let i = tr.instr as u32;
        let ihdr = self.ptr(hw.bus, self.smplarr + 4 * i);
        if ihdr == 0 {
            return;
        }
        let strans = hw.bus.r8(tr.smp + 7);
        let mut n = note
            .wrapping_add(self.playtransp as u8)
            .wrapping_add(strans);
        let typ = hw.bus.r16(ihdr + 4);
        if tr.outdev != 0 || t >= 4 {
            return;
        }
        if !(tr.stype > 0 && typ == 0xffff) {
            self.dma_off(hw, t);
        }
        n = n.wrapping_sub(1);
        let tr = &mut self.tracks[t];
        tr.synth = 0;
        tr.fade_speed = 0;
        tr.decay_speed = tr.decay;
        tr.vib_phase = 0;
        tr.trem_phase = 0;
        self.dma_on |= 1 << t;
        let mut hdr = ihdr;
        if (typ as i16) < 0 {
            let tr = &mut self.tracks[t];
            tr.synth_note = n;
            tr.synth = ihdr;
            if typ == 0xfffe {
                // hybrid: its first waveform is the sample
                tr.stype = -1;
                hdr = self.wf(hw.bus, ihdr, 0);
            } else {
                tr.stype = 1;
                let tbl = Self::ft_table(tr.finetune) - C1;
                let per = self.period(tbl, n as isize);
                let tr = &mut self.tracks[t];
                tr.per_tbl = Some(tbl);
                tr.per = per;
                tr.rptr = 0;
                self.synth_init(hw.bus, t);
                return;
            }
        } else {
            self.tracks[t].stype = 0;
        }
        self.play_sample(hw, t, hdr, n);
        if self.tracks[t].stype != 0 {
            self.synth_init(hw.bus, t);
        }
    }

    /// The waveform `n` of a synth sound (relocated as the original does, only
    /// the sound's own waveforms).
    fn wf(&self, b: &Bus, ihdr: u32, n: i8) -> u32 {
        let raw = b.r32(ihdr.wrapping_add((0x116 + 4 * n as i32) as u32));
        if (n as i32) >= 0 && (n as u16) < b.r16(ihdr + 0x14) {
            raw.wrapping_add(ihdr)
        } else {
            raw
        }
    }

    fn play_sample(&mut self, hw: &mut Hw, t: usize, hdr: u32, mut n: u8) {
        while (n as i8) < 0 {
            n = n.wrapping_add(12);
        }
        if (n as i8) > 0x3e {
            n -= 12;
        }
        let b = &*hw.bus;
        let tr = &self.tracks[t];
        let i = (tr.instr as usize).min(62);
        let len = b.r32(hdr);
        let typ = b.r16(hdr + 4);
        let mut data = 6u32;
        if typ & 0x20 != 0 && (t & 3) != 0 && (t & 3) != 3 {
            data = data.wrapping_add(len);
        }
        let mut tbl = Self::ft_table(tr.finetune);
        let (start, end, rep, replen, per);
        let kind = typ & 0xf;
        if kind == 0 || kind >= 7 {
            if kind >= 7 {
                tbl -= C1;
            }
            per = self.period(tbl, n as isize);
            start = hdr.wrapping_add(data);
            end = start.wrapping_add(len);
            rep = self.i_rep[i];
            replen = self.i_replen[i];
        } else {
            let k = (kind - 1) as usize;
            let oct = (n / 12) as usize;
            let div = [31u32, 7, 3, 15, 63, 127][k];
            let short = if len / div > 0xffff {
                len & 0xffff
            } else {
                len / div
            };
            let sh = OCT_SHIFT[k][oct.min(5)];
            let mult = [
                [15u32, 7, 3, 1, 1, 0],
                [3, 3, 1, 1, 0, 0],
                [1, 1, 0, 0, 0, 0],
                [7, 7, 3, 3, 1, 0],
                [31, 15, 7, 3, 1, 0],
                [63, 31, 15, 7, 3, 1],
            ][k][oct.min(5)];
            rep = self.i_rep[i] << sh;
            replen = self.i_replen[i] << sh;
            start = (short & 0xffff)
                .wrapping_mul(mult)
                .wrapping_add(hdr)
                .wrapping_add(data);
            end = (short << sh).wrapping_add(start);
            per = self.period(tbl, (n % 12 + OCT_NOTE[k][oct.min(5)]) as isize);
        }
        let rep = rep.wrapping_add(start);
        let loop_end = if replen <= 2 {
            0
        } else {
            replen.wrapping_add(rep)
        };
        let mut s = start.wrapping_add(tr.soffs as u32);
        if !(end > s) {
            s = start;
        }
        let looped = self.i_flags[tr.instr as usize % 63] & 1 != 0;
        self.spend(cost::NOTE[t]);
        self.flush(hw);
        hw.loc(t, s);
        let tr = &mut self.tracks[t];
        tr.per_tbl = Some(tbl);
        let words = if !looped {
            tr.rptr = self.silence;
            tr.rlen = 1;
            end.wrapping_sub(s) >> 1
        } else {
            tr.rptr = rep;
            tr.rlen = (loop_end.wrapping_sub(rep) >> 1) as u16;
            loop_end.wrapping_sub(s) >> 1
        };
        tr.per = per;
        self.spend(cost::LEN[t]);
        self.poke_len(hw, t, words);
        self.spend(cost::DMA_OFF - cost::DMA_OFF_FIRST);
    }

    fn poke_len(&mut self, hw: &mut Hw, t: usize, words: u32) {
        self.flush(hw);
        let w = words as u16;
        hw.len(t, if w <= 1 { 0 } else { w });
    }

    /// Resets a synth or hybrid sound's programs for a new note.
    fn synth_init(&mut self, b: &Bus, t: usize) {
        let tr = &mut self.tracks[t];
        tr.arp_pos = 0;
        tr.arp_start = 0;
        tr.vol_cnt = 0;
        tr.wf_cnt = 0;
        tr.vol_pos = 0;
        if !tr.keep_wf {
            tr.wf_pos = 0;
        }
        tr.vol_wait = 0;
        tr.wf_wait = 0;
        tr.svib_speed = 0;
        tr.per_chg = 0;
        tr.per_chg_acc = 0;
        tr.env = 0;
        tr.svib_depth = 0;
        tr.svib_wave = 0;
        tr.svib_phase = 0;
        tr.vol_speed = b.r8(tr.synth + 0x12);
        tr.wf_speed = b.r8(tr.synth + 0x13);
        tr.vol_chg = 0;
    }

    fn dma_off(&mut self, hw: &mut Hw, t: usize) {
        self.spend(cost::DMA_OFF_FIRST);
        self.flush(hw);
        hw.dmacon(1 << t);
    }

    /// MuteChannel: the note stops.
    fn mute(&mut self, hw: &mut Hw, t: usize) {
        let tr = &mut self.tracks[t.min(63)];
        if tr.outdev != 0 || t >= 4 {
            return;
        }
        tr.synth = 0;
        tr.stype = 0;
        tr.soffs = 0;
        self.dma_off(hw, t);
    }

    fn stop_all(&mut self, hw: &mut Hw) {
        for t in (0..64).rev() {
            self.mute(hw, t);
        }
    }

    /// Every tick: hold and decay, the commands that run on every tick of the
    /// row last played, and each channel's period and volume.
    fn every_tick(&mut self, hw: &mut Hw) {
        let tick = self.counter;
        let n = self.numtracks.min(64) as usize;
        for t in 0..n {
            self.hold_decay(hw, t);
        }
        let blk = self.block(hw.bus, self.fx_block);
        self.block_dims(hw.bus, blk);
        let mut page = 0u16;
        loop {
            let (mut a, stride) = self.cmd_ptr(hw.bus, blk, page, self.fx_line);
            for t in 0..self.numtracks.min(64) as usize {
                let c = hw.bus.r8(a);
                let arg = hw.bus.r8(a + 1);
                a = a.wrapping_add(stride);
                let cmd = if stride == 3 { c & 0xf } else { c & 0x1f };
                if !self.tracks[t].skip {
                    self.tick_command(hw, t, cmd, arg, tick);
                }
            }
            page += 1;
            if page > self.numpages {
                break;
            }
        }
        for t in 0..self.numtracks.min(64) as usize {
            if !self.tracks[t].skip {
                self.output(hw, t);
            }
        }
    }

    fn hold_decay(&mut self, hw: &mut Hw, t: usize) {
        if t >= 4 {
            self.tracks[t].skip = true;
            return;
        }
        let tr = &mut self.tracks[t];
        if tr.hold_cnt >= 0 {
            tr.hold_cnt -= 1;
            if tr.hold_cnt < 0 {
                if tr.stype != 0 {
                    // a synth sound decays by its volume program
                    tr.vol_pos = tr.decay_speed;
                    tr.vol_wait = 0;
                } else {
                    tr.fade_speed = tr.decay_speed;
                    if tr.fade_speed == 0 {
                        self.dma_off(hw, t);
                    }
                }
            }
        }
        let tr = &mut self.tracks[t];
        if tr.fade_speed != 0 {
            tr.vol = tr.vol.wrapping_sub(tr.fade_speed);
            if (tr.vol as i8) < 0 {
                tr.vol = 0;
                tr.fade_speed = 0;
            }
        }
        tr.skip = false;
    }

    fn tick_command(&mut self, hw: &mut Hw, t: usize, cmd: u8, arg: u8, tick: u8) {
        let st = tick == 0 && self.flags & FLAG_STSLIDE != 0;
        let playtransp = self.playtransp as u8;
        let tempo2 = self.tempo2;
        match cmd {
            0x00 => {
                if arg == 0 {
                    return;
                }
                let k = (tick % 3) as i8 - 1;
                let x = if k > 0 {
                    0
                } else if k < 0 {
                    arg >> 4
                } else {
                    arg & 0xf
                };
                let tr = &self.tracks[t];
                let base = tr
                    .prev_note
                    .wrapping_sub(1)
                    .wrapping_add(playtransp)
                    .wrapping_add(tr.strans as u8);
                let other = x.wrapping_add(base);
                let Some(tbl) = tr.per_tbl else { return };
                let p0 = self.period(tbl, (base.wrapping_mul(2) / 2) as isize);
                let p1 = self.period(tbl, (other.wrapping_mul(2) / 2) as isize);
                self.tracks[t].arp_ofs = p0.wrapping_sub(p1);
            }
            0x01 => {
                if st {
                    return;
                }
                let tr = &mut self.tracks[t];
                let p = tr.per.wrapping_sub(arg as u16);
                tr.per = if (p as i16) < 0x71 { 0x71 } else { p };
            }
            0x02 => {
                if !st {
                    self.tracks[t].per = self.tracks[t].per.wrapping_add(arg as u16);
                }
            }
            0x11 => {
                if tick == 0 {
                    self.tracks[t].per = self.tracks[t].per.wrapping_sub(arg as u16);
                }
            }
            0x12 => {
                if tick == 0 {
                    self.tracks[t].per = self.tracks[t].per.wrapping_add(arg as u16);
                }
            }
            0x03 => {
                if !st {
                    self.porta(t);
                }
            }
            0x04 | 0x14 => {
                let tr = &mut self.tracks[t];
                tr.vib_shift = if cmd == 4 { 5 } else { 6 };
                if tick == 0 && arg != 0 {
                    if arg & 0xf != 0 {
                        tr.vib_depth = (arg & 0xf) as u16;
                    }
                    if arg & 0xf0 != 0 {
                        tr.vib_speed = (arg >> 3) & 0x3e;
                    }
                }
                self.vibrato(t);
            }
            0x05 => {
                if !st {
                    self.vol_slide(t, arg);
                    self.porta(t);
                }
            }
            0x06 => {
                if !st {
                    self.vol_slide(t, arg);
                    self.vibrato(t);
                }
            }
            0x07 => {
                let tr = &mut self.tracks[t];
                if tick == 0 && arg != 0 {
                    if arg & 0xf != 0 {
                        tr.trem_depth = (arg & 0xf) as u16;
                    }
                    if arg & 0xf0 != 0 {
                        tr.trem_speed = (arg >> 2) & 0x3c;
                    }
                }
                let s = SINE[((tr.trem_phase >> 3) & 0x1f) as usize] as i16;
                let d = (s.wrapping_mul(tr.trem_depth as i16) >> 7) as u8;
                tr.trem_phase = tr.trem_phase.wrapping_add(tr.trem_speed);
                let v = d.wrapping_add(tr.vol) as i8;
                tr.out_vol = v.clamp(0, 0x40) as u8;
            }
            0x0a | 0x0d => {
                if !st {
                    self.vol_slide(t, arg);
                }
            }
            0x0f => match arg {
                0xf1 | 0xf2 => {
                    if tick == 3 {
                        self.replay_note(hw, t, tick);
                    }
                }
                0xf3 => {
                    if tick != 0 && tick & 1 == 0 {
                        self.replay_note(hw, t, tick);
                    }
                }
                0xf4 => {
                    if tick == tempo2 / 3 {
                        self.replay_note(hw, t, tick);
                    }
                }
                0xf5 => {
                    if tick == ((tempo2 / 3) as u16 * 2) as u8 {
                        self.replay_note(hw, t, tick);
                    }
                }
                0xf8 => self.flags &= !FLAG_FILTER,
                0xf9 => self.flags |= FLAG_FILTER,
                _ => {}
            },
            0x13 => {
                if (tick as i8) < 3 {
                    self.tracks[t].vib_ofs = (arg as u16).wrapping_neg();
                }
            }
            0x18 => {
                if tick == arg {
                    self.tracks[t].vol = 0;
                }
            }
            0x1a => {
                if tick == 0 {
                    let tr = &mut self.tracks[t];
                    let v = tr.vol.wrapping_add(arg);
                    tr.vol = if (v as i8) > 0x40 { 0x40 } else { v };
                }
            }
            0x1b => {
                if tick == 0 {
                    let tr = &mut self.tracks[t];
                    let v = tr.vol.wrapping_sub(arg);
                    tr.vol = if (v as i8) < 0 { 0 } else { v };
                }
            }
            0x1f => {
                let delay = arg >> 4;
                if delay != 0 {
                    if (tick as i8) < (delay as i8) {
                        return;
                    }
                    if tick == delay {
                        self.replay_note(hw, t, tick);
                        return;
                    }
                }
                let every = arg & 0xf;
                if every != 0 && tick % every == 0 {
                    self.replay_note(hw, t, tick);
                }
            }
            _ => {}
        }
    }

    fn vol_slide(&mut self, t: usize, arg: u8) {
        let tr = &mut self.tracks[t];
        if arg & 0xf0 == 0 {
            let v = tr.vol.wrapping_sub(arg);
            tr.vol = if (v as i8) < 0 { 0 } else { v };
        } else {
            let v = tr.vol.wrapping_add(arg >> 4);
            tr.vol = if (v as i8) > 0x40 { 0x40 } else { v };
        }
    }

    fn vibrato(&mut self, t: usize) {
        let tr = &mut self.tracks[t];
        let s = SINE[((tr.vib_phase >> 2) & 0x1f) as usize] as i16;
        tr.vib_ofs = (s.wrapping_mul(tr.vib_depth as i16) >> tr.vib_shift) as u16;
        tr.vib_phase = tr.vib_phase.wrapping_add(tr.vib_speed);
    }

    fn porta(&mut self, t: usize) {
        let tr = &mut self.tracks[t];
        let target = tr.porta_target;
        if target == 0 {
            return;
        }
        let sp = tr.porta_speed as u16;
        let reached = if tr.per > target {
            let p = tr.per.wrapping_sub(sp);
            tr.per = p;
            !((p as i16) > target as i16)
        } else {
            let p = tr.per.wrapping_add(sp);
            tr.per = p;
            (p as i16) >= target as i16
        };
        if reached {
            tr.per = target;
            tr.porta_target = 0;
        }
    }

    /// A note played again from a command on a later tick of the row.
    fn replay_note(&mut self, hw: &mut Hw, t: usize, tick: u8) {
        let tr = &mut self.tracks[t];
        let note = tr.curr_note;
        if note == 0 {
            return;
        }
        tr.hold_cnt = if tr.hold_cnt >= 0 {
            tr.hold_cnt.wrapping_add(tick as i8)
        } else if tr.hold == 0 {
            -1
        } else {
            tr.hold as i8
        };
        self.play_note(hw, t, note);
    }

    /// A channel's period and volume for this tick (after its synth programs).
    fn output(&mut self, hw: &mut Hw, t: usize) {
        let mut per = self.tracks[t].per;
        if self.tracks[t].synth != 0 {
            self.spend(cost::SYNTH);
            per = self.synth(hw, t);
        }
        let tr = &mut self.tracks[t];
        per = per.wrapping_add(tr.vib_ofs).wrapping_sub(tr.arp_ofs);
        tr.vib_ofs = 0;
        tr.arp_ofs = 0;
        let v = if (tr.out_vol as i8) < 0 {
            tr.vol
        } else {
            tr.out_vol
        };
        tr.out_vol = 0xff;
        let vol = ((v as u32 * tr.trk_vol as u32) as u16 >> 8) & 0x7f;
        self.spend(cost::PER);
        self.flush(hw);
        hw.per(t, per);
        self.spend(cost::VOL[t & 3]);
        self.flush(hw);
        hw.vol(t, vol);
    }

    /// The volume and waveform programs of a synth or hybrid sound: the
    /// period for this tick (and a new waveform's start and length).
    fn synth(&mut self, hw: &mut Hw, t: usize) -> u16 {
        let s = self.tracks[t].synth;
        let mut per = self.tracks[t].per;
        // the volume program
        let tr = &mut self.tracks[t];
        tr.vol_cnt = tr.vol_cnt.wrapping_sub(1);
        if tr.vol_cnt <= 0 {
            tr.vol_cnt = tr.vol_speed as i8;
            if tr.vol_chg != 0 {
                let v = (tr.vol_chg as u8).wrapping_add(tr.synth_vol) as i8;
                tr.synth_vol = v.clamp(0, 0x40) as u8;
            }
            if tr.env != 0 {
                let e = hw.bus.r8(tr.env);
                tr.synth_vol = e.wrapping_add(0x80) >> 2;
                let mut next = tr.env + 1;
                tr.env_cnt = tr.env_cnt.wrapping_add(1);
                if tr.env_cnt < 0 {
                    tr.env_cnt = 0;
                    next = tr.env_loop;
                }
                tr.env = next;
            }
            let run = tr.vol_wait == 0 || {
                tr.vol_wait = tr.vol_wait.wrapping_sub(1);
                (tr.vol_wait as i8) <= 0
            };
            if run {
                let mut pos = tr.vol_pos;
                for _ in 0..256 {
                    pos = pos.wrapping_add(1);
                    let c = hw.bus.r8(s + 0x15 + pos as u32);
                    if (c as i8) >= 0 {
                        self.tracks[t].synth_vol = c;
                        break;
                    }
                    let arg = hw.bus.r8(s + 0x16 + pos as u32);
                    let tr = &mut self.tracks[t];
                    match c & 0xf {
                        0x0 => {
                            tr.vol_speed = arg;
                            pos = pos.wrapping_add(1);
                        }
                        0x1 => {
                            tr.vol_wait = arg;
                            pos = pos.wrapping_add(1);
                            break;
                        }
                        0x2 => {
                            tr.vol_chg = (arg as i8).wrapping_neg();
                            pos = pos.wrapping_add(1);
                        }
                        0x3 => {
                            tr.vol_chg = arg as i8;
                            pos = pos.wrapping_add(1);
                        }
                        0x4 | 0x5 => {
                            let w = self.wf(hw.bus, s, arg as i8).wrapping_add(2);
                            let tr = &mut self.tracks[t];
                            tr.env_loop = if c & 0xf == 5 { w } else { 0 };
                            tr.env = w;
                            tr.env_cnt = 0;
                            pos = pos.wrapping_add(1);
                        }
                        0x6 => tr.env = 0,
                        0xa => {
                            tr.wf_pos = arg;
                            tr.wf_wait = 0;
                            pos = pos.wrapping_add(1);
                        }
                        0xb | 0xf => {
                            pos = pos.wrapping_sub(1);
                            break;
                        }
                        0xe => pos = arg,
                        _ => break,
                    }
                }
                self.tracks[t].vol_pos = pos;
            }
        }
        let tr = &mut self.tracks[t];
        tr.out_vol = ((tr.vol as u16 * tr.synth_vol as u16) as i16 >> 6) as u8;
        // the waveform program
        tr.wf_cnt = tr.wf_cnt.wrapping_sub(1);
        if tr.wf_cnt <= 0 {
            tr.wf_cnt = tr.wf_speed as i8;
            if tr.per_chg != 0 {
                tr.per_chg_acc = tr.per_chg_acc.wrapping_add(tr.per_chg);
            }
            let run = tr.wf_wait == 0 || {
                tr.wf_wait = tr.wf_wait.wrapping_sub(1);
                tr.wf_wait == 0
            };
            if run {
                let mut pos = tr.wf_pos;
                for _ in 0..256 {
                    pos = pos.wrapping_add(1);
                    let c = hw.bus.r8(s + 0x95 + pos as u32);
                    if (c as i8) >= 0 {
                        let w = self.wf(hw.bus, s, c as i8);
                        let len = hw.bus.r16(w);
                        self.spend(cost::WF_LEN);
                        self.poke_len(hw, t, len as u32);
                        self.spend(cost::WF_LOC);
                        self.flush(hw);
                        hw.loc(t, w + 2);
                        self.spend(cost::WF_END);
                        break;
                    }
                    let arg = hw.bus.r8(s + 0x96 + pos as u32);
                    let tr = &mut self.tracks[t];
                    match c & 0xf {
                        0x0 => {
                            tr.wf_speed = arg;
                            pos = pos.wrapping_add(1);
                        }
                        0x1 => {
                            tr.wf_wait = arg;
                            pos = pos.wrapping_add(1);
                            break;
                        }
                        0x2 => {
                            tr.per_chg = arg as u16;
                            pos = pos.wrapping_add(1);
                        }
                        0x3 => {
                            tr.per_chg = (arg as i8).wrapping_neg() as i16 as u16;
                            pos = pos.wrapping_add(1);
                        }
                        0x4 => {
                            tr.svib_depth = (tr.svib_depth & 0xff00) | arg as u16;
                            pos = pos.wrapping_add(1);
                        }
                        0x5 => {
                            tr.svib_speed = (tr.svib_speed & 0xff00) | arg.wrapping_add(1) as u16;
                            pos = pos.wrapping_add(1);
                        }
                        0x6 => {
                            tr.per_chg_acc = 0;
                            per = tr.per;
                        }
                        0x7 => {
                            let w = self.wf(hw.bus, s, arg as i8).wrapping_add(2);
                            self.tracks[t].svib_wave = w;
                            pos = pos.wrapping_add(1);
                        }
                        0xa => {
                            tr.vol_pos = arg;
                            tr.vol_wait = 0;
                            pos = pos.wrapping_add(1);
                        }
                        0xb | 0xf => {
                            pos = pos.wrapping_sub(1);
                            break;
                        }
                        0xc => {
                            tr.arp_start = pos;
                            tr.arp_pos = pos;
                            loop {
                                pos = pos.wrapping_add(1);
                                if (hw.bus.r8(s + 0x95 + pos as u32) as i8) < 0 {
                                    break;
                                }
                            }
                        }
                        0xd => {}
                        0xe => pos = arg,
                        _ => break,
                    }
                }
                self.tracks[t].wf_pos = pos;
            }
        }
        // the arpeggio
        let tr = &self.tracks[t];
        if tr.arp_pos != 0 {
            let n = hw
                .bus
                .r8(s + 0x96 + tr.arp_pos as u32)
                .wrapping_add(tr.synth_note);
            if let Some(tbl) = tr.per_tbl {
                per = self.period(tbl, n as isize);
            }
            let mut p = tr.arp_pos.wrapping_add(1);
            if (hw.bus.r8(s + 0x96 + p as u32) as i8) < 0 {
                p = tr.arp_start;
            }
            self.tracks[t].arp_pos = p;
        }
        // the vibrato
        let tr = &mut self.tracks[t];
        if tr.svib_depth != 0 {
            let k = ((tr.svib_phase >> 4) & 0x1f) as usize;
            let w = if tr.svib_wave == 0 {
                SINE[k]
            } else {
                hw.bus.r8(tr.svib_wave + k as u32) as i8
            };
            let d = ((w as i16).wrapping_mul(tr.svib_depth as i16) >> 8) as u16;
            per = per.wrapping_add(d);
            tr.svib_phase = tr.svib_phase.wrapping_add(tr.svib_speed);
        }
        per.wrapping_add(tr.per_chg_acc)
    }

    /// The DMA of the notes played this tick goes on, then their loops follow.
    fn dma_on_writes(&mut self, hw: &mut Hw) {
        if self.dma_on == 0 {
            return;
        }
        self.spend(cost::DMA_ON);
        self.flush(hw);
        hw.dmacon(0x8000 | self.dma_on);
        // as ENPP_DMAMask does, the host waits for the DMA to start
        let c = hw.bus.clock;
        hw.wait((c / LINE + 10) * LINE - c);
        let mut first = true;
        for t in 0..4 {
            if self.dma_on & 1 << t == 0 || self.tracks[t].rptr == 0 {
                continue;
            }
            let (r, l) = (self.tracks[t].rptr, self.tracks[t].rlen);
            self.spend(if first { cost::LOOP_FIRST } else { cost::LOOP });
            first = false;
            self.flush(hw);
            hw.loc(t, r);
            self.spend(cost::LOOP_LEN);
            self.poke_len(hw, t, l as u32);
        }
    }

    fn spend(&mut self, c: i64) {
        self.spent += c;
    }

    fn flush(&mut self, hw: &mut Hw) {
        hw.wait(self.spent.max(0) as u64);
        self.spent = 0;
    }
}

/// The 16 period tables (finetunes 0..7, -8..-1), `TABLE` words each.
fn periods() -> Vec<u16> {
    let mut periods = vec![0u16; TABLE * 16];
    for (slot, pt) in PT_PERIODS.iter().enumerate() {
        let t = &mut periods[slot * TABLE..(slot + 1) * TABLE];
        // the two octaves below: ProTracker's octave times 4 and 2 for finetune
        // 0, otherwise equal-tempered steps down from 4 times its C-1 (finetune
        // -7 from 4 * 897, not 4 * 900), as the original's periods show
        let c = if slot == 9 { 4 * 897 } else { 4 * pt[0] as u32 };
        for n in 0..24 {
            t[n] = if slot == 0 {
                pt[n % 12] << (2 - n / 12)
            } else {
                (c as f64 * (-(n as f64) / 12.0).exp2()).round() as u16
            };
        }
        t[C1..C1 + 36].copy_from_slice(pt);
        for k in 0..3 {
            t[C1 + 36 + 12 * k..C1 + 48 + 12 * k].copy_from_slice(&pt[24..36]);
        }
    }
    periods
}

#[cfg(test)]
mod tests;

/// 68000 DIVU.W: the quotient, or the dividend's low word when it overflows.
fn divu(n: u32, d: u16) -> u16 {
    if d == 0 {
        return n as u16;
    }
    let q = n / d as u32;
    if q > 0xffff { n as u16 } else { q as u16 }
}

//! MusicMaker V8 (Thomas Winischhofer), 8-voice (EXT) songs: a translation of
//! the original 68k replayer, `MusicMaker8.asm`, "EaglePlayer for MusicMaker
//! EXT (8-channel) modules/songs" with its "MULTI-CHANNEL-PLAYER V1.9 PCS 1.2
//! FAST-VOL-MIX (C) DIRE CRACKS Inc.", "MusicMaker V8" Copyright (C) 1987-2017
//! Thomas Winischhofer, which its author published under the BSD licence: the
//! README beside it in UADE's amigasrc/players/other/music_maker says "Thomas
//! Winischhofer <thomas@winischhofer.net> was kind to publish MusicMaker[48]
//! under BSD license" (the source itself carries no licence text).
//!
//! The 8 voices are mixed in software, in pairs, onto Paula's 4 channels:
//! voices 7 and 6 onto channel 3, 5 and 4 onto 2, 3 and 2 onto 1, 1 and 0 onto
//! 0. Each channel plays one of its two buffers (8 of `_obtainmixbuflen` bytes,
//! allocated together) while the other is mixed; its period is the lower of the
//! pair's, the other voice stepped down to it by a 16-bit fraction; its volume
//! the louder of the two, the quieter voice scaled by a table of 64 volumes;
//! samples are halved once, in place, so that two add up without overflow.
//! Every buffer lasts `speed * 115` colour clocks: the byte count is that over
//! the period, rounded to an even number with the fraction carried, and
//! corrected by how late the interrupt took each channel.
//!
//! There is no timer: the replayer runs in the level 4 (audio) interrupt, which
//! waits, a raster line at a time, for all 4 channels' interrupts, sets each to
//! its next buffer, then plays a row of all voices and mixes the next buffers.
//! The mixing routines are the "dbra" ones, which the original takes from a
//! 68020 on (and for `mixertype` $0f, which all known songs have); the older
//! self-modifying 68000 ones are not translated.
//!
//! Checked against the original as UADE runs it (`MusicMaker-8V`, built from
//! that source); writes timed as UADE's run takes.

use super::musicmaker::{
    hardwareinit, instruments, legal_per, legal_vol, read_instruments, read_song, song, song_name, wait_line,
    Instruments, Last, Row, INSTNUM, LOUDNESS, NOTETABLE, STRTVOLUS,
};
use super::{Hw, Open, Replayer};
use crate::bus::Bus;
use crate::machine::Machine;
use m68k::AddressBus;

pub const OPEN: Open = Open { name: "MusicMaker 8V", detect, new };

/// Software voices.
const CH: usize = 8;

// Colour clocks between the replayer's writes, as UADE's run of the original
// takes them (a 68020).
/// From an audio interrupt to the first channel's location.
const T_IRQ: u64 = 42;
/// From a channel's location to its length, period and volume, and to the
/// acknowledge of its interrupt.
const T_LEN: u64 = 12;
const T_PER: u64 = 42;
const T_VOL: u64 = 96;
const T_ACK: u64 = 134;
/// From one channel's location to the next one's, in the same pass.
const T_NEXT: u64 = 152;
/// A pass's test of a channel not yet raised.
const T_SKIP: u64 = 6;
/// From the last acknowledge to the lengths written after the mixing: this,
/// and for each voice that starts a sample, each routine run and each byte
/// mixed (by routine, in `Mix` order), as fitted to UADE's runs of the 18
/// songs (within a median 150 colour clocks). Simple, ZeroPer and ZeroVol run
/// in none of them: theirs are estimates from the routines that are alike.
const T_MIX: u64 = 4268;
const T_NOTE: u64 = 614;
const T_RUN: u64 = 842;
const MIX_BYTE: [u64; 10] = [8, 16, 18, 20, 20, 11, 10, 10, 6, 4];
/// Between two of those lengths.
const T_LENS: u64 = 10;
/// From InitPlay's reset to StartSnd's.
const T_START: u64 = 50_000;

fn detect(d: &[u8]) -> bool {
    matches!(song(d), Some((_, true)))
}

/// `divu.w`: the quotient in the low word, the remainder in the high; on an
/// overflow (or a division by zero, which would trap) the register unchanged.
fn divu(d: u32, s: u16) -> u32 {
    if s == 0 {
        return d;
    }
    let q = d / s as u32;
    if q > 0xffff {
        d
    } else {
        (d % s as u32) << 16 | q
    }
}

/// The mixing routines (`returndbramixer`'s), by what they do with the two
/// voices s1 and s2: `Per`, s1 stepped down to the channel's rate; `Vol`, s1
/// scaled by the volume table; `Zero`, s2 silent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mix {
    Simple,
    PerPer,
    VolVol,
    PerVol,
    VolPer,
    ZeroPerVol,
    ZeroPer,
    ZeroVol,
    ZeroSimple,
    Clear,
}

impl Mix {
    fn index(self) -> usize {
        self as usize
    }
}

/// Each routine's header words before its code, which the mixer keeps state
/// in: -18 the step fraction, -10 and -8 the rates (256: one byte per byte) of
/// the two voices, as they were initialised.
const HEADERS: [[u16; 3]; 10] = [
    [0, 256, 256],
    [0, 0, 256],
    [0, 256, 256],
    [0, 0, 256],
    [0, 256, 0],
    [0, 0, 0],
    [0, 0, 0],
    [0, 256, 0],
    [0, 256, 0],
    [0, 0, 0],
];

#[derive(Clone, Copy, Default)]
struct Header {
    rca: u16,
    r10: u16,
    r8: u16,
}

impl Header {
    fn swap(&mut self) {
        std::mem::swap(&mut self.r10, &mut self.r8);
    }
}

/// `returndbramixer`: the routine for voices A and B (periods, volumes,
/// lengths), and whether they swap places (B is s1).
fn returndbramixer(mut d0: u16, mut d1: u16, mut d2: u16, mut d3: u16, d5: u16, d6: u16) -> (Mix, bool) {
    let zero = |d0: u16, d1: u16, d2: u16, d3: u16, x: bool| {
        // acheckfzm
        if d0 > d1 {
            (if d3 <= d2 { Mix::ZeroPer } else { Mix::ZeroPerVol }, x)
        } else {
            (if d3 <= d2 { Mix::ZeroSimple } else { Mix::ZeroVol }, x)
        }
    };
    if d6 == 0 {
        if d5 == 0 {
            return (Mix::Clear, false);
        }
        return zero(d0, d1, d2, d3, false);
    }
    if d5 == 0 {
        std::mem::swap(&mut d0, &mut d1);
        std::mem::swap(&mut d2, &mut d3);
        return zero(d0, d1, d2, d3, true);
    }
    if d0 == d1 {
        if d2 == d3 {
            return (Mix::Simple, false);
        }
        return (Mix::VolVol, d3 <= d2);
    }
    if d2 == d3 {
        return (Mix::PerPer, d0 <= d1);
    }
    if d0 > d1 {
        if d3 > d2 {
            (Mix::PerVol, false)
        } else {
            (Mix::VolPer, true)
        }
    } else if d3 > d2 {
        (Mix::VolPer, false)
    } else {
        (Mix::PerVol, true)
    }
}

/// The volume tables (`_maketables` with 64 volumes, packed): table p scales
/// a halved sample byte by p/64.
fn volume_tables() -> Vec<[u8; 256]> {
    (0..64u32)
        .map(|p| {
            let mut t = [0u8; 256];
            for s in 0..64u32 {
                let v = (s * p / 64) as u8;
                t[s as usize] = v;
                t[255 - s as usize] = 0u8.wrapping_sub(v);
            }
            t
        })
        .collect()
}

/// The table for two different volumes (`justgetvolumepointer`): the quieter
/// one relative to the louder.
fn table_index(a: u16, b: u16) -> u8 {
    let (mx, mn) = if a > b { (a, b) } else { (b, a) };
    let q = divu((mn as u32) << 16, mx) & 0xffff;
    ((q * 64) >> 16) as u8
}

/// Voice state (COUNTERS, MACRO_C, SCOUNT, SOFTMOD, VIBRATO).
#[derive(Default, Clone, Copy)]
struct Voice {
    melody: u32,
    melody_loop: u32,
    macro_ptr: u32,
    count: u16,
    softmod: [u16; 2],
    vibrato: [u8; 2],
}

/// A software channel's structure: the sample a row starts, its period and
/// volume, the loop after it, where it was left (for "(") and its backup.
#[derive(Default, Clone, Copy)]
struct Soft {
    ptr: u32,
    len: u16,
    per: u16,
    vol: u16,
    next: (u32, u16),
    old: (u32, u16),
    oldbak: (u32, u16),
}

/// One of a hardware channel's two voices: what is left of its sample.
#[derive(Default, Clone, Copy)]
struct Part {
    ptr: u32,
    per: u16,
    vol: u16,
    len: u16,
}

#[derive(Clone, Copy)]
enum Table {
    /// The channel's own volume buffer, which stays clear.
    Zero,
    P(u8),
}

/// A hardware channel (`hwchannelN`).
#[derive(Clone, Copy)]
struct HwCh {
    /// The buffer to mix (and set as the next) and the one playing.
    buf: [u32; 2],
    /// The period the byte count was last worked out for.
    w8: u16,
    /// Bytes to add to the next buffer (the interrupt's lateness, the fraction).
    corr: u16,
    a: Part,
    b: Part,
    per: u16,
    vol: u16,
    /// The buffer's length in words.
    len: u16,
    table: Table,
    /// The byte count's fraction, around $10000.
    acc: u32,
    /// The stepping fraction's running sum.
    rc: u16,
}

pub struct MusicMaker8 {
    name: String,
    ins: Instruments,
    macrovecs: Vec<u32>,
    currlfo: [u32; INSTNUM + 1],
    v: [Voice; CH],
    last: [Last; CH],
    soft: [Soft; CH],
    hw: [HwCh; 4],
    hdr: [Header; 10],
    tables: Vec<[u8; 256]>,
    /// lastvolumes: the voices' volumes times 2, then their base volumes.
    lastvol: [u16; 2 * CH],
    volumetable: [u16; 16],
    speed: u16,
    origspeed: u16,
    newspeed: u16,
    calcspeed: u16,
    mixcalcspeed: u32,
    pattlen: u16,
    fadespeedin: u16,
    fadespeedout: u16,
    channelshandle: u8,
    internfinished: u8,
    channelsenable: u8,
    channelsfinished: u8,
    dtfinished: u8,
    doloudness: u8,
    dofadein: u8,
    dofadeout: u8,
    speedtoupdate: bool,
    /// Sound off (`soundenable` set): after a play routine that overran.
    off: bool,
    songend: bool,
    /// The mixing's time this interrupt (colour clocks).
    work: u64,
}

fn new(d: &[u8], base: u32, m: &mut Machine) -> Option<Box<dyn Replayer>> {
    let (s, ext) = song(d)?;
    if !ext {
        return None;
    }
    let ia = instruments(d, base, m)?;
    let ins = read_instruments(&m.bus, ia, true)?;
    let sdata = base + s as u32;
    let bus = &m.bus;
    // internsndreset: after the name, channelsenable, mixertype, hi
    let channelsenable = bus.r8(sdata + 23);
    let mut a2 = sdata + 26;
    if bus.r8(a2) == 0xfe {
        a2 = a2.wrapping_add(bus.r16(a2 + 2) as u32);
    }
    a2 = a2.wrapping_add(4 + 226);
    let pattlen = bus.r16(a2);
    let speed = bus.r16(a2 + 2);
    let start = a2 + 4;
    // _obtainmixbuflen, as from a 68020 on: 113 the highest "hi" period
    let d1 = divu(3_579_545 / 113 * speed as u32, 31250) & 0xffff;
    let mixbuflen = (d1 + (d1 >> 2)) & !1;
    let mut song = read_song(bus, start, start + 4, CH, channelsenable);
    // a disabled voice plays mychanneloffmelody: macro 0, then the end
    let mut off = 0;
    if channelsenable != 0xff {
        off = m.alloc(4);
        if off == 0 {
            return None;
        }
        m.bus.w16(off, 0);
        m.bus.w16(off + 2, 999);
    }
    let mixbuf = m.alloc(8 * mixbuflen);
    if mixbuf == 0 {
        return None;
    }
    let bus = &mut m.bus;
    let mut v: [Voice; CH] = Default::default();
    for (i, vo) in v.iter_mut().enumerate() {
        if channelsenable & 1 << i == 0 {
            song.melody[i] = (off, off);
        }
        let (st, lp) = song.melody[i];
        vo.melody = st;
        vo.melody_loop = lp;
        vo.macro_ptr = song.macrovecs.get(bus.r16(st) as usize).copied().unwrap_or(0);
        vo.count = 1;
    }
    // _setmixbuffers: the buffers pushed last first
    let b = |k: u32| mixbuf + k * mixbuflen;
    let mk = |b0: u32, b1: u32| HwCh {
        buf: [b0, b1],
        w8: 0,
        corr: 0,
        a: Part::default(),
        b: Part::default(),
        per: 230,
        vol: 0,
        len: speed >> 2,
        table: Table::Zero,
        acc: 0x10000,
        rc: 0,
    };
    let hwch = [mk(b(7), b(6)), mk(b(5), b(4)), mk(b(3), b(2)), mk(b(1), b(0))];
    // calculateinstruments: the samples halved, once (the 4 bytes before them
    // mark it)
    if bus.r32(ins.defsnd) == 0 {
        bus.w32(ins.defsnd, 0xffff_ffff);
        for i in 0..INSTNUM {
            let (a, n) = (ins.vecs[i], ins.lens[i][0]);
            for k in 0..n as u32 {
                let x = bus.r8(a + k) as i8 >> 1;
                bus.w8(a + k, x as u8);
            }
        }
    }
    let mut hdr = [Header::default(); 10];
    for (h, i) in hdr.iter_mut().zip(HEADERS) {
        *h = Header { rca: i[0], r10: i[1], r8: i[2] };
    }
    let p = MusicMaker8 {
        name: song_name(bus, sdata),
        ins,
        macrovecs: song.macrovecs,
        currlfo: [0; INSTNUM + 1],
        v,
        last: [Last::reset(); CH],
        soft: [Soft::default(); CH],
        hw: hwch,
        hdr,
        tables: volume_tables(),
        lastvol: [0; 2 * CH],
        volumetable: STRTVOLUS,
        speed,
        origspeed: speed,
        newspeed: 0,
        calcspeed: speed,
        mixcalcspeed: speed as u32 * 115,
        pattlen,
        fadespeedin: 63,
        fadespeedout: 21,
        channelshandle: 0,
        internfinished: 0,
        channelsenable,
        channelsfinished: !channelsenable,
        dtfinished: !channelsenable,
        doloudness: 0,
        dofadein: 0,
        dofadeout: 0,
        speedtoupdate: false,
        off: false,
        songend: false,
        work: 0,
    };
    // InitPlay's and StartSnd's resets, then _soundon
    let mut hw = Hw { bus };
    // (InitPlay's apart from StartSnd's, as DeliTracker calls them)
    hardwareinit(&mut hw);
    hw.wait(T_START);
    for _ in 0..2 {
        hardwareinit(&mut hw);
        hw.wait(200);
    }
    hw.bus.write_word(0xdff09a, 0x0780);
    for ch in 0..4 {
        hw.wait(6);
        hw.len(ch, speed >> 2);
    }
    for ch in 0..4 {
        hw.wait(8);
        hw.per(ch, 230);
        hw.vol(ch, 0);
    }
    hw.wait(40);
    hw.bus.write_word(0xdff09c, 0x0780);
    hw.wait(8);
    hw.dmacon(0x800f);
    hw.wait(8);
    hw.bus.write_word(0xdff09a, 0x8780);
    Some(Box::new(p))
}

impl Replayer for MusicMaker8 {
    fn format(&self) -> &'static str {
        "MusicMaker 8V"
    }

    fn title(&self) -> String {
        self.name.clone()
    }

    /// No timer: the audio interrupt plays.
    fn tick_clocks(&self) -> u64 {
        1 << 40
    }

    fn tick(&mut self, _hw: &mut Hw) {}

    fn songend(&self) -> bool {
        self.songend
    }

    fn audio_irq(&mut self, hw: &mut Hw) -> bool {
        if self.off || hw.bus.read_word(0xdff01e) & hw.bus.read_word(0xdff01c) & 0x0780 == 0 {
            // taken with an earlier one's
            return true;
        }
        self.generalplay(hw);
        true
    }
}

impl MusicMaker8 {
    /// `_generalplay`, the level 4 interrupt.
    fn generalplay(&mut self, hw: &mut Hw) {
        hw.wait(T_IRQ);
        let mut left = 0x0780u16;
        let mut pass = 0u32;
        let mut when = [0u32; 4];
        loop {
            let r = hw.bus.read_word(0xdff01e);
            let mut first = true;
            #[allow(clippy::needless_range_loop)]
            for ch in 0..4 {
                let bit = 0x80u16 << ch;
                if r & bit == 0 {
                    hw.wait(T_SKIP);
                    continue;
                }
                if !first {
                    hw.wait(T_NEXT - T_ACK);
                }
                first = false;
                let c = self.hw[ch];
                let t = hw.bus.clock;
                let at = |hw: &mut Hw, d: u64| {
                    let now = hw.bus.clock;
                    hw.wait((t + d).saturating_sub(now));
                };
                hw.loc(ch, c.buf[0]);
                at(hw, T_LEN);
                hw.len(ch, c.len);
                at(hw, T_PER);
                hw.per(ch, c.per);
                at(hw, T_VOL);
                // the voices' volume (64 here) scales it
                hw.vol(ch, ((c.vol as u32 * 64) >> 6) as u16);
                at(hw, T_ACK);
                when[ch] = pass;
                left &= !bit;
                hw.bus.write_word(0xdff09c, bit);
            }
            pass += 1;
            if left == 0 || pass > 2000 {
                break;
            }
            wait_line(hw);
        }
        // calccorr: a channel taken late gets fewer bytes next
        for (c, &n) in self.hw.iter_mut().zip(&when) {
            let k = if n < 4 {
                0
            } else if n < 10 {
                1
            } else if n < 15 {
                2
            } else {
                5
            };
            c.corr = c.corr.wrapping_sub(k);
        }
        if self.speedtoupdate {
            self.speed = self.newspeed;
            self.calcspeed = self.newspeed;
            self.mixcalcspeed = self.newspeed as u32 * 115;
            self.speedtoupdate = false;
        }
        if self.dtfinished == 0xff {
            self.songend = true;
        }
        let t = hw.bus.clock;
        if self.channelsfinished & self.channelsenable == self.channelsenable {
            // channelsallfinished (only for one-shot songs)
            hw.bus.write_word(0xdff09a, 0x0780);
            hw.bus.write_word(0xdff09c, 0x0780);
            hw.dmacon(0x000f);
            return;
        }
        self.playsnd(hw);
        self.work = T_MIX + T_NOTE * self.channelshandle.count_ones() as u64;
        for h in (0..4).rev() {
            self.mix(h, hw.bus);
        }
        let now = hw.bus.clock;
        hw.wait((t + self.work).saturating_sub(now));
        for ch in 0..4 {
            if ch != 0 {
                hw.wait(T_LENS);
            }
            hw.len(ch, self.hw[ch].len);
        }
        // playalert: the next buffers already started
        if hw.bus.read_word(0xdff01e) & 0x0780 != 0 {
            self.off = true;
            hw.bus.write_word(0xdff09a, 0x0780);
            hw.bus.write_word(0xdff09c, 0x0780);
            hardwareinit(hw);
        }
    }

    /// `playsnd`: a row of each voice, 7 to 0.
    fn playsnd(&mut self, hw: &mut Hw) {
        self.channelshandle = 0;
        let d7 = self.volumetable[15];
        for ch in (0..CH).rev() {
            let bit = 1u8 << ch;
            let r = self.handle_channel(ch, hw);
            let (mut d4, d5, d6) = (r.d4, r.d5, r.d6);
            let mut d3: u8 = 0;
            if (d4 as i32) >= 0 {
                if (d5 as i32) >= 0 {
                    self.channelshandle |= bit;
                    self.soft[ch].len = d4 as u16;
                    self.soft[ch].ptr = d5;
                    let inst = (d4 >> 24) as usize;
                    let lfo = self.currlfo.get(inst).copied().unwrap_or(0);
                    let last = &mut self.last[ch];
                    last.hull_on = false;
                    last.hull_loop = false;
                    last.lfo = lfo;
                    if lfo != 0 {
                        last.hull_on = true;
                        last.hullcnt = 0;
                        last.hulllen = d4 as u16;
                    }
                }
                // pervolonly
                let w = (d6 as u16).rotate_left(8);
                self.lastvol[ch + CH] = (self.lastvol[ch + CH] & 0xff00) | (w & 0xff);
                self.lastvol[ch] = (w >> 8) * 2;
                self.last[ch].per = (d6 >> 16) as u16;
                self.last[ch].note2 = (d4 >> 16) as u16 & 0xff;
                d4 = 0xffff_ffff;
            }
            // absnothing
            let last = &mut self.last[ch];
            if (d4 as i16) >= 0 {
                last.fade = d4 as u16;
            }
            if self.dofadein & bit != 0 {
                if last.fade > 4096 {
                    last.fade = 4096u16.wrapping_sub(self.fadespeedin);
                } else if last.fade == 256 {
                    self.dofadein &= !bit;
                } else if (last.fade as i16) < 256 {
                    last.fade = 256;
                    self.dofadein &= !bit;
                } else {
                    last.fade = last.fade.wrapping_sub(self.fadespeedin);
                }
            } else if self.dofadeout & bit != 0 {
                if last.fade == 8192 {
                    self.dofadeout &= !bit;
                } else if last.fade as i16 > 8192 {
                    last.fade = 8192;
                    self.dofadeout &= !bit;
                } else {
                    last.fade = last.fade.wrapping_add(self.fadespeedout);
                }
            }
            // fadedone: the volume
            let v = &mut self.v[ch];
            if v.vibrato[0] as i8 > 0 {
                v.vibrato[1] = !v.vibrato[1];
                if v.vibrato[1] == 0 {
                    v.softmod[1] = v.softmod[1].wrapping_neg();
                }
            }
            let x = self.lastvol[ch + CH].wrapping_add(((v.softmod[1] as i16) >> 1) as u16);
            self.lastvol[ch + CH] = legal_vol(x, 64);
            let mut vol = legal_vol(self.lastvol[ch].wrapping_add(v.softmod[1]), d7.wrapping_mul(2));
            self.lastvol[ch] = vol;
            vol >>= 1;
            let last = &mut self.last[ch];
            if last.hull_on {
                let a2 = last.lfo.wrapping_add(((last.hullcnt / 40) as u32) * 2);
                d3 = hw.bus.r8(a2 + 1);
                let m = hw.bus.r8(a2) as i8;
                if m != 0 {
                    let x = (m as i16 as i32 * vol as i16 as i32) as i16 >> 5;
                    vol = legal_vol(vol.wrapping_add(x as u16), d7);
                }
            }
            if self.doloudness & bit != 0 {
                let l = LOUDNESS.get(last.note2 as usize / 2).copied().unwrap_or(256);
                let x = ((vol as u32) << 8 & 0xffff) / l as u32;
                vol = if x as u16 == (vol << 8) { 0 } else { x as u16 };
            }
            if last.fade != 0x100 {
                let num = (vol << 8) as u32;
                let q = divu(num, last.fade);
                vol = if q as u16 == (vol << 8) { 0 } else { q as u16 };
            }
            vol = legal_vol(vol, d7);
            self.soft[ch].vol = vol;
            // the period
            let v = &mut self.v[ch];
            if (v.vibrato[0] as i8) < 0 {
                v.vibrato[1] = !v.vibrato[1];
                if v.vibrato[1] == 0 {
                    v.softmod[0] = v.softmod[0].wrapping_neg();
                }
            }
            let mut per = legal_per(last.per.wrapping_add(v.softmod[0]));
            last.per = per;
            if last.hull_on {
                if d3 != 0 {
                    let x = (per as i16 as i32 * d3 as i8 as i32) >> 8;
                    per = legal_per(per.wrapping_add(x as u16));
                }
                let step = divu(self.calcspeed as u32 * 115, per) as u16;
                let (c, carry) = last.hullcnt.overflowing_add(step);
                last.hullcnt = c;
                if carry || c > last.hulllen {
                    if last.hull_loop {
                        last.hullcnt = last.hullcnt.wrapping_sub(last.looplen);
                    } else {
                        last.hull_on = false;
                    }
                }
            }
            self.soft[ch].per = per;
            if self.channelsfinished & bit != 0 {
                self.soft[ch].next = (0, 0);
            } else if self.channelshandle & bit != 0 && last.hull_on && self.soft[ch].next.0 != 0 {
                last.hull_loop = true;
                last.looplen = self.soft[ch].next.1;
            }
        }
    }

    fn handle_channel(&mut self, n: usize, hw: &mut Hw) -> Row {
        let bit = 1u8 << n;
        let nothing = Row { d4: 0xffff_ffff, d5: 0, d6: 0 };
        self.v[n].count = self.v[n].count.wrapping_sub(1);
        if self.v[n].count != 0 {
            return nothing;
        }
        if self.internfinished & bit != 0 {
            // the melody went back to its loop: the voice starts clear
            self.dofadein &= !bit;
            self.dofadeout &= !bit;
            self.channelshandle |= bit;
            self.lastvol[n] = 0;
            self.lastvol[n + CH] = 0;
            self.soft[n].ptr = 0;
            self.soft[n].len = 0;
            self.v[n].softmod = [0, 0];
            self.last[n] = Last::reset();
            self.soft[n].next = (0, 0);
            self.dtfinished |= bit;
            // (not a one-shot song)
            self.internfinished &= !bit;
        }
        let bus = &*hw.bus;
        let mut a0 = self.v[n].macro_ptr;
        let r8 = |a: u32| bus.r8(a);
        let mut d0 = r8(a0 + 2);
        if r8(a0 + 3) == 0xf2 || r8(a0 + 3) == 0xfb || r8(a0) == 0xf8 {
            d0 = r8(a0 + 5);
        }
        self.v[n].count = (((d0 & 0x3f) as u16) + 1) * 2;
        let mut d5: u32 = 0;
        let mut b0 = r8(a0);
        let mut b1 = r8(a0 + 1);
        let mut d4: u32 = 0xffff_ffff;
        let mut d6: u32 = 0;
        let mut control = false;
        if b0 > 0xf1 {
            control = true;
            match b0 {
                0xf4 => {
                    self.doloudness &= !bit;
                    if b1 != 0 {
                        self.doloudness |= bit;
                    }
                }
                0xfc => {
                    // a new speed, from the next interrupt on
                    let f2 = (b1 & 0x0f) as u16;
                    if f2 != 0 {
                        let mut s = divu((b1 >> 4) as u32 * self.origspeed as u32, f2) as u16;
                        if (s as i16) < 600 {
                            s = 600;
                        }
                        if s as i16 > self.origspeed as i16 {
                            s = self.origspeed;
                        }
                        self.newspeed = s;
                        self.speedtoupdate = true;
                    }
                }
                0xfd | 0xf5 => {}
                0xf6 => {
                    self.dofadein &= !bit;
                    self.dofadeout &= !bit;
                    if b1 != 0 {
                        if (b1 as i8) < 0 {
                            self.fadespeedin = (self.fadespeedin & 0xff00) | b1.wrapping_neg() as u16;
                            self.dofadein |= bit;
                        } else {
                            self.fadespeedout = (self.fadespeedout & 0xff00) | b1 as u16;
                            self.dofadeout |= bit;
                        }
                    }
                }
                0xf7 => d4 = if b1 != 0 { 0xffff_0100 } else { 0xffff_2000 },
                _ => {
                    control = false;
                    if b0 == 0xf8 {
                        d5 = b1 as u32;
                        a0 = a0.wrapping_add(3);
                        b0 = r8(a0);
                        b1 = r8(a0 + 1);
                    }
                    if b0 == 0xf9 {
                        control = true;
                        let d1 = b1 as i8 as i16 as u16;
                        let b2 = r8(a0 + 2);
                        let v = &mut self.v[n];
                        if b2 & 0x80 != 0 {
                            if b2 & 0x40 != 0 {
                                if (d1 as i16) < 0 {
                                    if (v.vibrato[0] as i8) < 0 {
                                        v.softmod[0] = 0;
                                    }
                                    v.softmod[1] = d1.wrapping_neg();
                                    v.vibrato = [1, 0];
                                } else {
                                    if v.vibrato[0] as i8 > 0 {
                                        v.softmod[1] = 0;
                                    }
                                    v.softmod[0] = d1;
                                    v.vibrato = [0xff, 0];
                                }
                            } else {
                                v.softmod[1] = d1;
                                if v.vibrato[0] as i8 > 0 {
                                    v.vibrato[0] = 0;
                                }
                            }
                        } else {
                            v.softmod[0] = d1;
                            if (v.vibrato[0] as i8) < 0 {
                                v.vibrato[0] = 0;
                            }
                        }
                    } else if b0 == 0xfa {
                        control = true;
                        let inst = ((b1 >> 4).wrapping_add(d5 as u8)) as u16;
                        let k = (b1 & 0x0f) as usize;
                        let mut p = self.ins.lfo[k];
                        if p != 0 {
                            if bus.r16(p) == inst {
                                p += 2;
                            } else {
                                p = 0;
                            }
                        }
                        if let Some(c) = self.currlfo.get_mut(inst as usize) {
                            *c = p;
                        }
                    }
                }
            }
        }
        if !control {
            // nocontrolbyte: a note
            let b2 = r8(a0 + 2);
            self.v[n].softmod = [0, 0];
            if b2 & 0x40 != 0 {
                let led = (b1 & 0x40) >> 5;
                let pra = hw.bus.read_byte(0xbfe001);
                hw.bus.write_byte(0xbfe001, pra & 0xfd);
                let pra = hw.bus.read_byte(0xbfe001);
                hw.bus.write_byte(0xbfe001, pra | led);
            }
            let bus = &*hw.bus;
            let r8 = |a: u32| bus.r8(a);
            let d1 = ((b1 & 0x3f) as u32) * 2;
            d4 = d1 << 16;
            d6 = (NOTETABLE[d1 as usize / 2] as u32) << 16;
            let vi = (b0 & 0x0f) as usize;
            let s = &mut self.soft[n];
            let novolumemod = |s: &mut Soft, d4: &mut u32, d5: &mut u32, d6: &mut u32| {
                *d6 &= 0xffff_0000;
                *d4 = (INSTNUM as u32) << 24;
                *d5 = 0;
                s.next = (0, 0);
            };
            if b0 == 0xf1 {
                s.oldbak = s.old;
                if d1 != 0 {
                    d4 = 0xffff_ffff;
                } else {
                    novolumemod(s, &mut d4, &mut d5, &mut d6);
                }
            } else if vi == 0 {
                novolumemod(s, &mut d4, &mut d5, &mut d6);
            } else {
                d6 |= (STRTVOLUS[vi] << 8) as u32 | (self.volumetable[vi] & 0xff) as u32;
                if b2 & 0x80 != 0 {
                    // modulate: no new sample, a soft modulation from the next entry
                    d5 = 0xffff_ffff;
                    let next = r8(a0 + 3);
                    if next == 0xf2 || next == 0xfb {
                        let sh = if next == 0xfb { 1 } else { 0 };
                        let x = r8(a0 + 4);
                        let hi = ((x as i8) >> 4) as i16;
                        let lo = (((x << 4) as i8) >> 4) as i16;
                        self.v[n].softmod = [(lo << sh) as u16, hi as u16];
                        a0 += 3;
                    }
                } else {
                    let inst = (b0 >> 4).wrapping_add(d5 as u8) as usize;
                    d4 = (inst as u32) << 24 | (d4 & 0x00ff_0000);
                    if inst == INSTNUM {
                        // setCONTFROMBREAK: "(" goes on where the voice was left
                        s.next = (0, 0);
                        d5 = s.oldbak.0;
                        d4 = (d4 & 0xffff_0000) | s.oldbak.1 as u32;
                    } else {
                        let (vec, lens) = match (self.ins.vecs.get(inst), self.ins.lens.get(inst)) {
                            (Some(&a), Some(&l)) => (a, l),
                            _ => (0, [0; 4]),
                        };
                        d5 = vec;
                        if b1 & 0x80 != 0 && lens[3] != 0 {
                            s.next = (vec.wrapping_add(lens[2] as u32), lens[3].wrapping_mul(2));
                            d4 = (d4 & 0xffff_0000) | lens[1] as u32;
                        } else {
                            s.next = (0, 0);
                            d4 = (d4 & 0xffff_0000) | lens[0] as u32;
                        }
                    }
                }
            }
        }
        // addcounthandle
        let bus = &*hw.bus;
        let mut guard = 0;
        loop {
            a0 = a0.wrapping_add(3);
            if bus.r8(a0) == 0xf3 {
                a0 = a0.wrapping_add(6);
            }
            if bus.r8(a0) != 0xff {
                break;
            }
            let (na0, d1) = self.getnewmacro(n, bus);
            a0 = na0;
            guard += 1;
            if d1 == 0 || bus.r8(a0) != 0xf0 || guard > 1000 {
                break;
            }
            let c = ((bus.r8(a0 + 2) & 0x3f) as u16 + 1) * 2;
            self.v[n].count = self.v[n].count.wrapping_add(c);
        }
        self.v[n].macro_ptr = a0;
        Row { d4, d5, d6 }
    }

    /// The next macro of voice `n`'s melody (pauses add to its count), and
    /// whether the melody went on (0: it went back to its loop).
    fn getnewmacro(&mut self, n: usize, bus: &Bus) -> (u32, u16) {
        let bit = 1u8 << n;
        let mut d1 = 1u16;
        let v = &mut self.v[n];
        for _ in 0..0x10000 {
            v.melody = v.melody.wrapping_add(2);
            if bus.r16(v.melody) != 0 {
                break;
            }
            v.count = v.count.wrapping_add(self.pattlen.wrapping_mul(2));
        }
        if bus.r16(v.melody) == 999 {
            v.melody = v.melody_loop;
            d1 = 0;
            self.internfinished |= bit;
        }
        let w = bus.r16(v.melody) as usize;
        (self.macrovecs.get(w).copied().unwrap_or(0), d1)
    }

    /// One mixing routine run for `n` output bytes into `out`: s1 and s2 the
    /// two voices' sample pointers.
    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, bus: &mut Bus, m: Mix, h: usize, s1: &mut u32, s2: &mut u32, out: &mut u32, n: u16) {
        let t = match self.hw[h].table {
            Table::Zero => [0u8; 256],
            Table::P(p) => self.tables[p as usize],
        };
        let rca = self.hdr[m.index()].rca;
        let mut rc = self.hw[h].rc;
        let fetch = |p: &mut u32| {
            let x = bus.r8(*p);
            *p = p.wrapping_add(1);
            x
        };
        self.work += T_RUN + MIX_BYTE[m.index()] * n as u64;
        let mut buf = Vec::with_capacity(n as usize);
        // getfirst: the stepped voice's byte
        let mut u: u8 = 0;
        match m {
            Mix::PerPer | Mix::ZeroPer => u = fetch(s1),
            Mix::PerVol | Mix::ZeroPerVol => u = t[fetch(s1) as usize],
            Mix::VolPer => u = fetch(s2),
            _ => {}
        }
        let step = |rc: &mut u16| {
            let (x, c) = rc.overflowing_add(rca);
            *rc = x;
            c
        };
        for _ in 0..n {
            let o = match m {
                Mix::Simple => fetch(s1).wrapping_add(fetch(s2)),
                Mix::PerPer => {
                    let o = fetch(s2).wrapping_add(u);
                    if step(&mut rc) {
                        u = fetch(s1);
                    }
                    o
                }
                Mix::VolVol => t[fetch(s1) as usize].wrapping_add(fetch(s2)),
                Mix::PerVol => {
                    let o = fetch(s2).wrapping_add(u);
                    if step(&mut rc) {
                        u = t[fetch(s1) as usize];
                    }
                    o
                }
                Mix::VolPer => {
                    let o = t[fetch(s1) as usize].wrapping_add(u);
                    if step(&mut rc) {
                        u = fetch(s2);
                    }
                    o
                }
                Mix::ZeroPerVol => {
                    if step(&mut rc) {
                        let o = u;
                        u = t[fetch(s1) as usize];
                        o
                    } else {
                        u
                    }
                }
                Mix::ZeroPer => {
                    let o = u;
                    if step(&mut rc) {
                        u = fetch(s1);
                    }
                    o
                }
                Mix::ZeroVol => t[fetch(s1) as usize],
                Mix::ZeroSimple => fetch(s1),
                Mix::Clear => 0,
            };
            buf.push(o);
        }
        // the routine before getfirst: the stepped voice's byte given back
        match m {
            Mix::PerPer | Mix::PerVol | Mix::ZeroPerVol | Mix::ZeroPer => *s1 = s1.wrapping_sub(1),
            Mix::VolPer => *s2 = s2.wrapping_sub(1),
            _ => {}
        }
        self.hw[h].rc = rc;
        bus.write_bytes(*out, &buf);
        *out = out.wrapping_add(n as u32);
    }

    /// A routine run in A/B terms (`exch`: B is s1): what A and B used, as the
    /// negative counts the original adds, and the bytes written.
    #[allow(clippy::too_many_arguments)]
    fn run_ab(
        &mut self,
        bus: &mut Bus,
        m: Mix,
        h: usize,
        exch: bool,
        a0: &mut u32,
        a1: &mut u32,
        a6: &mut u32,
        n: u16,
    ) -> (u16, u16, u16) {
        let (s1, s2) = if exch { (*a1, *a0) } else { (*a0, *a1) };
        let (mut p1, mut p2) = (s1, s2);
        let o = *a6;
        self.run(bus, m, h, &mut p1, &mut p2, a6, n);
        let (c1, c2) = (s1.wrapping_sub(p1) as u16, s2.wrapping_sub(p2) as u16);
        let w = o.wrapping_sub(*a6) as u16;
        if exch {
            *a0 = p2;
            *a1 = p1;
            (c2, c1, w)
        } else {
            *a0 = p1;
            *a1 = p2;
            (c1, c2, w)
        }
    }

    /// `MIXER` for hardware channel `h`: voices 2h+1 (A) and 2h (B).
    fn mix(&mut self, h: usize, bus: &mut Bus) {
        let (ca, cb) = (2 * h + 1, 2 * h);
        let mut d0 = self.soft[ca].per;
        let mut d2 = self.soft[ca].vol;
        let mut d1 = self.soft[cb].per;
        let mut d3 = self.soft[cb].vol;
        let handle = self.channelshandle;
        let (sa, sb) = (self.soft[ca], self.soft[cb]);
        let c = &mut self.hw[h];
        let mut d5: u16;
        let mut d6: u16;
        if handle & 1 << ca != 0 {
            c.a.ptr = sa.ptr;
            d5 = sa.len;
            self.soft[ca].len = 0;
            c.a.len = d5;
            c.rc = 0;
        } else {
            d5 = c.a.len;
            if d5 <= 3 {
                d5 = 0;
                c.a.len = 0;
                c.a.ptr = sa.next.0;
                if c.a.ptr != 0 {
                    d5 = sa.next.1;
                    c.a.len = d5;
                }
            }
        }
        if handle & 1 << cb != 0 {
            c.b.ptr = sb.ptr;
            d6 = sb.len;
            self.soft[cb].len = 0;
            c.b.len = d6;
            c.rc = 0;
        } else {
            d6 = c.b.len;
            if d6 <= 3 {
                d6 = 0;
                c.b.len = 0;
                c.b.ptr = sb.next.0;
                if c.b.ptr != 0 {
                    d6 = sb.next.1;
                    c.b.len = d6;
                }
            }
        }
        if d5 == 0 {
            d0 = 0xffff;
            d2 = 0;
            if d6 == 0 {
                // nochannelused
                c.per = 230;
                c.vol = 0;
                c.len = (self.speed >> 2).wrapping_add(c.corr);
                c.corr = 0;
                c.a.per = 0xffff;
                c.a.vol = 0;
                c.b.per = 0xffff;
                c.b.vol = 0;
                c.acc = 0x10000;
                c.w8 = 0;
                self.mix_end(h);
                return;
            }
        } else if d6 == 0 {
            d1 = 0xffff;
            d3 = 0;
        }
        let mut a6 = c.buf[0];
        c.a.per = d0;
        c.a.vol = d2;
        c.b.per = d1;
        c.b.vol = d3;
        let mut second = false;
        let (mut a0, mut a1): (u32, u32);
        let (mut n2, mut n3, mut n4): (u16, u16, u16);
        let mut myd2 = (0u16, 0u16, 0u16);
        let mut mya0 = (0u32, 0u32);
        let mut guard = 0;
        'mixagain: loop {
            guard += 1;
            let (m, exch) = returndbramixer(d0, d1, d2, d3, d5, d6);
            let mi = m.index();
            // make_dbra_mixer, for the routines that step
            let steps = matches!(m, Mix::PerPer | Mix::PerVol | Mix::VolPer | Mix::ZeroPerVol | Mix::ZeroPer);
            if steps && d0 != d1 {
                let (mx, mn) = if d0 > d1 { (d0, d1) } else { (d1, d0) };
                let q = divu((mn as u32) << 16, mx) as u16;
                self.hdr[mi].rca = q;
                let r = (q >> 8) + if q & 0xff != 0 { 4 } else { 0 };
                if m == Mix::VolPer {
                    self.hdr[mi].r8 = r;
                } else {
                    self.hdr[mi].r10 = r;
                }
            }
            let c = &mut self.hw[h];
            c.table = if d2 == d3 { Table::Zero } else { Table::P(table_index(d2, d3)) };
            if exch {
                self.hdr[mi].swap();
            }
            c.vol = d2.max(d3);
            if second {
                a0 = mya0.0;
                a1 = mya0.1;
                (n2, n3, n4) = myd2;
            } else {
                let (pa, pb) = (d0, d1);
                let mut x = self.mixcalcspeed;
                let adjust = |c: &mut HwCh, x: &mut u32, per: u16| {
                    if c.corr != 0 {
                        let k = c.corr;
                        c.corr = 0;
                        *x = (*x & 0xffff_0000) | (*x as u16).wrapping_add(k) as u32;
                    }
                    let frac = divu(*x & 0xffff_0000, per) & 0xffff;
                    if *x & 1 != 0 {
                        *x = (*x & 0xffff_0000) | (*x as u16).wrapping_add(1) as u32;
                        c.acc = c.acc.wrapping_sub(!(frac as u16) as u32);
                    } else {
                        c.acc = c.acc.wrapping_add(frac);
                    }
                    if (c.acc as i32) < 0 {
                        c.acc = c.acc.wrapping_add(0x20000);
                        c.corr = c.corr.wrapping_sub(2);
                    } else if c.acc > 0x1ffff {
                        c.acc = c.acc.wrapping_sub(0x20000);
                        c.corr = c.corr.wrapping_add(2);
                    }
                };
                if d0 > d1 {
                    // B the faster: the channel plays at its period
                    if c.w8 != 0 && pb != c.w8 {
                        c.acc = (divu(c.acc, pb) & 0xffff) * c.w8 as u32;
                    }
                    c.per = d1;
                    x = divu(x, d1);
                    c.w8 = pb;
                    adjust(c, &mut x, d1);
                    let n = x as u16;
                    (n2, n3, n4) = (n, n, n);
                    if (d0 as i16) >= 0 {
                        let q = divu((d1 as u32) << 16, d0) as u16;
                        n2 = ((n as u32 * q as u32) >> 16) as u16 + 1;
                    }
                } else {
                    if c.w8 != 0 && pa != c.w8 {
                        c.acc = (divu(c.acc, pa) & 0xffff) * c.w8 as u32;
                    }
                    c.per = d0;
                    x = divu(x, d0);
                    c.w8 = pa;
                    adjust(c, &mut x, d0);
                    let n = x as u16;
                    (n2, n3, n4) = (n, n, n);
                    if pa != pb && (d1 as i16) >= 0 {
                        let q = divu((d0 as u32) << 16, d1) as u16;
                        n3 = ((n as u32 * q as u32) >> 16) as u16 + 1;
                    }
                }
                c.len = n4 >> 1;
                a0 = c.a.ptr;
                a1 = c.b.ptr;
            }
            // wasfirstcall
            let myd0 = (d0, d1);
            #[derive(Clone, Copy)]
            enum L {
                Start,
                EnoughA,
                MixAA,
                MixBB,
                TempA(u16, u16),
                TempB(u16, u16),
                AOneshot,
                BOneshot,
            }
            let mut lbl = L::Start;
            let mut sv = (0u16, 0u16, 0u16);
            loop {
                guard += 1;
                if guard > 10_000 {
                    break 'mixagain;
                }
                let c = self.hw[h];
                let hd = self.hdr[mi];
                match lbl {
                    L::Start => {
                        if c.a.len == 0 || n2 <= c.a.len {
                            lbl = L::EnoughA;
                            continue;
                        }
                        sv = (n2, n3, n4);
                        if c.b.len == 0 || n3 <= c.b.len {
                            // neoAenoughofB: B has enough, A needs more
                            n4 = if myd0.0 == c.per && hd.r10 == 256 {
                                c.a.len
                            } else {
                                divu((c.a.len as u32) << 8, hd.r10) as u16
                            };
                            lbl = L::MixAA;
                        } else if myd0.0 == c.per {
                            // neofABperA
                            n4 = divu((c.b.len as u32) << 8, hd.r8) as u16;
                            let mut x = c.a.len;
                            if hd.r10 != 256 {
                                x = divu((c.a.len as u32) << 8, hd.r10) as u16;
                            }
                            if n4 <= x {
                                lbl = L::MixBB;
                            } else {
                                n4 = x;
                                lbl = L::MixAA;
                            }
                        } else {
                            // neofABperB
                            n4 = divu((c.a.len as u32) << 8, hd.r10) as u16;
                            let mut x = c.b.len;
                            if hd.r8 != 256 {
                                x = divu((c.b.len as u32) << 8, hd.r8) as u16;
                            }
                            if n4 <= x {
                                lbl = L::MixAA;
                            } else {
                                n4 = x;
                                lbl = L::MixBB;
                            }
                        }
                    }
                    L::EnoughA => {
                        if c.b.len == 0 || n3 <= c.b.len {
                            // enoughofAandB
                            if exch {
                                self.hdr[mi].swap();
                            }
                            let (x2, x3, _) = self.run_ab(bus, m, h, exch, &mut a0, &mut a1, &mut a6, n4);
                            let c = &mut self.hw[h];
                            c.a.len = c.a.len.wrapping_add(x2);
                            c.b.len = c.b.len.wrapping_add(x3);
                            c.a.ptr = a0;
                            c.b.ptr = a1;
                            break 'mixagain;
                        }
                        // A has enough, B needs more
                        sv = (n2, n3, n4);
                        n4 = if myd0.1 == c.per && hd.r8 == 256 {
                            c.b.len
                        } else {
                            divu((c.b.len as u32) << 8, hd.r8) as u16
                        };
                        lbl = L::MixBB;
                    }
                    L::MixAA | L::MixBB => {
                        if exch {
                            self.hdr[mi].swap();
                        }
                        let (x2, x3, w) = self.run_ab(bus, m, h, exch, &mut a0, &mut a1, &mut a6, n4);
                        n4 = sv.2.wrapping_add(w);
                        let c = &mut self.hw[h];
                        let (e0, e1) = (sv.0.wrapping_add(x2), sv.1.wrapping_add(x3));
                        if let L::MixAA = lbl {
                            c.b.len = c.b.len.wrapping_add(x3);
                            lbl = L::TempA(e0, e1);
                        } else {
                            c.a.len = c.a.len.wrapping_add(x2);
                            lbl = L::TempB(e0, e1);
                        }
                    }
                    L::TempA(e0, e1) => {
                        // A ran out: its loop, if it has one
                        myd2 = (e0, e1, n4);
                        a0 = self.soft[ca].next.0;
                        self.hw[h].a.ptr = a0;
                        if a0 == 0 {
                            lbl = L::AOneshot;
                            continue;
                        }
                        d5 = self.soft[ca].next.1;
                        self.hw[h].a.len = d5;
                        if d5 == 0 {
                            lbl = L::AOneshot;
                            continue;
                        }
                        d6 = c.b.len;
                        if d6 <= 3 {
                            a1 = self.soft[cb].next.0;
                            self.hw[h].b.ptr = a1;
                            if a1 == 0 {
                                lbl = L::BOneshot;
                                continue;
                            }
                            d6 = self.soft[cb].next.1;
                            self.hw[h].b.len = d6;
                        }
                        if exch {
                            self.hdr[mi].swap();
                        }
                        (n2, n3, n4) = myd2;
                        lbl = L::Start;
                    }
                    L::TempB(e0, e1) => {
                        myd2 = (e0, e1, n4);
                        a1 = self.soft[cb].next.0;
                        self.hw[h].b.ptr = a1;
                        if a1 == 0 {
                            lbl = L::BOneshot;
                            continue;
                        }
                        d6 = self.soft[cb].next.1;
                        self.hw[h].b.len = d6;
                        if d6 == 0 {
                            lbl = L::BOneshot;
                            continue;
                        }
                        d5 = c.a.len;
                        if d5 <= 3 {
                            a0 = self.soft[ca].next.0;
                            self.hw[h].a.ptr = a0;
                            if a0 == 0 {
                                lbl = L::AOneshot;
                                continue;
                            }
                            d5 = self.soft[ca].next.1;
                            self.hw[h].a.len = d5;
                        }
                        if exch {
                            self.hdr[mi].swap();
                        }
                        (n2, n3, n4) = myd2;
                        lbl = L::Start;
                    }
                    L::AOneshot | L::BOneshot => {
                        let c = &mut self.hw[h];
                        if let L::AOneshot = lbl {
                            c.a.len = 0;
                            d5 = 0;
                            d6 = c.b.len;
                            if d6 <= 3 {
                                d6 = 0;
                                a1 = self.soft[cb].next.0;
                                c.b.ptr = a1;
                                if a1 != 0 {
                                    d6 = self.soft[cb].next.1;
                                }
                                c.b.len = d6;
                            }
                        } else {
                            c.b.len = 0;
                            d6 = 0;
                            d5 = c.a.len;
                            if d5 <= 3 {
                                d5 = 0;
                                a0 = self.soft[ca].next.0;
                                c.a.ptr = a0;
                                if a0 != 0 {
                                    d5 = self.soft[ca].next.1;
                                }
                                c.a.len = d5;
                            }
                        }
                        // the rest with another routine
                        d0 = c.a.per;
                        d2 = c.a.vol;
                        d1 = c.b.per;
                        d3 = c.b.vol;
                        second = true;
                        mya0 = (a0, a1);
                        continue 'mixagain;
                    }
                }
            }
        }
        self.mix_end(h);
    }

    /// absmixloopsend and absend: the buffers exchanged, where the voices were
    /// left kept for "(".
    fn mix_end(&mut self, h: usize) {
        let c = &mut self.hw[h];
        c.buf.swap(0, 1);
        self.soft[2 * h + 1].old = (c.a.ptr, c.a.len);
        self.soft[2 * h].old = (c.b.ptr, c.b.len);
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::Engine;
    use crate::replay::musicmaker::tests::module;
    use std::path::Path;

    #[test]
    fn mixes_eight_voices_onto_paula() {
        // an EXT song: all 8 voices on, mixertype $0f, pattern length 64
        let mut extra = vec![0u8; 234];
        extra[0] = 0x0f;
        extra[1] = 123;
        extra[233] = 64;
        let d = module([0xff, 0xff], 8, &extra);
        assert!(super::detect(&d));
        assert!(!(crate::replay::musicmaker::OPEN.detect)(&d));
        let mut e = Engine::new(48000.0);
        e.open_replayers = true;
        e.m.bus.write_log = Some(Vec::new());
        e.play_data(Path::new("mm8.test"), d).unwrap();
        assert_eq!(e.playing.as_ref().unwrap().format, "MusicMaker 8V");
        let mut out = vec![0f32; 2 * 9600];
        e.render(&mut out);
        let log = e.m.bus.write_log.take().unwrap();
        let reg = |a: u32| log.iter().filter(|w| w.0 == a).map(|w| w.1).collect::<Vec<_>>();
        // _soundon, then the interrupts
        assert_eq!(reg(0xDFF09A)[..2], [0x0780, 0x8780]);
        assert!(reg(0xDFF096).contains(&0x800f));
        // the first interrupt sets the buffers _soundon left (speed / 4 words
        // at period 230), then mixes the notes': 600 * 115 / 604 bytes, at the
        // note's period
        assert_eq!(reg(0xDFF0A4)[..3], [150, 150, 57]);
        assert_eq!(reg(0xDFF0A6)[..3], [230, 230, super::NOTETABLE[12]]);
        // three resets and _soundon's 0, the first interrupt's, then the
        // louder voice's volume (both are 64)
        assert_eq!(reg(0xDFF0A8)[..6], [0, 0, 0, 0, 0, 64]);
        assert!(out.iter().any(|&s| s != 0.0), "no sound");
    }
}

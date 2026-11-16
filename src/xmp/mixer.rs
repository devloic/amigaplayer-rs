//! libxmp's software mixer (`mixer.c` and the stereo-output mixers of
//! `mix_all.c`): 16-bit stereo output with full panning, nearest, linear and
//! cubic spline interpolation, the resonant filter mixers (`QUIRK_FILTER`),
//! volume ramping and Hipolito's anticlick.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp). It is independent of the Paula
//! emulation the Amiga replayers use.
//!
//! Not ported yet: mono output, and libxmp's sound-effect samples.

use super::filter::FILTER_SHIFT;
use super::module::*;
use super::player::*;
use super::spline_lut::*;

pub const PAN_SURROUND: i32 = 0x8000;
pub const SMIX_SHIFT: i32 = 16;
const SMIX_MASK: i32 = 0xffff;
pub const ANTICLICK_SHIFT: i32 = 3;
const ANTICLICK_FPSHIFT: i32 = 24;
const C4_PERIOD: f64 = 428.0;
const DOWNMIX_SHIFT: i32 = 12;
pub const SMIX_NUMVOC: i32 = 128;
const XMP_MAX_FRAMESIZE: i32 = 5 * 768000 * 2 / XMP_MIN_BPM;

// voice flags
pub const VOICE_RELEASE: i32 = 1 << 0;
pub const ANTICLICK: i32 = 1 << 1;
pub const SAMPLE_LOOP: i32 = 1 << 2;
pub const VOICE_REVERSE: i32 = 1 << 3;
pub const VOICE_BIDIR: i32 = 1 << 4;
pub const SAMPLE_QUEUED: i32 = 1 << 5;
pub const SAMPLE_PAUSED: i32 = 1 << 6;

// mixer function index flags
const FLAG_16_BITS: i32 = 0x01;
const FLAG_STEREO: i32 = 0x02;
const FLAG_STEREOOUT: i32 = 0x04;
const FLAG_FILTER: i32 = 0x08;
const FLAG_ACTIVE: i32 = 0x10;

/// `XMP_DSP_LOWPASS` (`xmp.h`): the filter effect, on by default.
pub const XMP_DSP_LOWPASS: i32 = 1 << 0;

// libxmp_mixer_seteffect's types (common.h)
pub const DSP_EFFECT_CUTOFF: i32 = 0x02;
pub const DSP_EFFECT_RESONANCE: i32 = 0x03;
pub const DSP_EFFECT_FILTER_A0: i32 = 0xb0;
pub const DSP_EFFECT_FILTER_B0: i32 = 0xb1;
pub const DSP_EFFECT_FILTER_B1: i32 = 0xb2;

/// Sample pre-amplification of the filtered mixers (`PREAMP_BITS`, mix_all.c),
/// against filter rounding errors at high rates.
const PREAMP_BITS: i32 = 15;
/// IT's WAV output driver clamp (`FILTER_MIN`, `FILTER_MAX`).
const FILTER_MIN: i32 = -65536 * (1 << PREAMP_BITS);
const FILTER_MAX: i32 = 65535 * (1 << PREAMP_BITS);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Interp {
    Nearest,
    Linear,
    Spline,
}

/// `struct mixer_voice`.
#[derive(Clone, Copy, Default, Debug)]
pub struct MixerVoice {
    pub chn: i32,
    pub root: i32,
    pub note: i32,
    pub pan: i32,
    pub vol: i32,
    pub period: f64,
    pub pos: f64,
    pub pos0: i32,
    pub fidx: i32,
    pub ins: i32,
    pub smp: i32,
    pub start: i32,
    pub end: i32,
    pub act: i32,
    pub key: i32,
    pub old_vl: i32,
    pub old_vr: i32,
    pub sleft: i32,
    pub sright: i32,
    pub flags: i32,
    /// `sptr != NULL`: the voice has sample data.
    pub has_data: bool,
    pub queued_smp: i32,
    pub filter: VoiceFilter,
}

/// The `filter` of `struct mixer_voice`: the state and coefficients of the
/// voice's resonant filter.
#[derive(Clone, Copy, Default, Debug)]
pub struct VoiceFilter {
    pub r1: i32,
    pub r2: i32,
    pub l1: i32,
    pub l2: i32,
    pub a0: i32,
    pub b0: i32,
    pub b1: i32,
    pub cutoff: i32,
    pub resonance: i32,
}

/// `struct mixer_data` (stereo 16-bit output only).
#[derive(Clone, Debug)]
pub struct MixerData {
    pub freq: i32,
    pub amplify: i32,
    pub mix: i32,
    pub interp: Interp,
    /// `XMP_DSP_*` effects.
    pub dsp: i32,
    pub numvoc: i32,
    pub ticksize: i32,
    pub total_size: i32,
    pub bidir_adjust: i32,
    pub buf32: Vec<i32>,
    /// The last tick, interleaved stereo; `ticksize * 2` values are valid.
    pub buffer: Vec<i16>,
}

impl Default for MixerData {
    fn default() -> Self {
        MixerData {
            freq: 0,
            amplify: 1,
            mix: 100,
            interp: Interp::Linear,
            dsp: XMP_DSP_LOWPASS,
            numvoc: SMIX_NUMVOC,
            ticksize: 0,
            total_size: 0,
            bidir_adjust: 0,
            buf32: Vec::new(),
            buffer: Vec::new(),
        }
    }
}

/// `libxmp_mixer_get_ticksize`.
pub fn get_ticksize(freq: i32, time_factor: f64, rrate: f64, bpm: i32) -> i32 {
    if freq <= 0 || bpm <= 0 || time_factor <= 0.0 || rrate <= 0.0 {
        return -1;
    }
    let calc = freq as f64 * time_factor * rrate / bpm as f64 / 1000.0;
    if calc > i32::MAX as f64 || calc.is_nan() {
        return -1;
    }
    (calc as i32).max(1 << ANTICLICK_SHIFT)
}

/// The backed-up sample data around a loop (`struct loop_data`).
#[derive(Default)]
struct LoopData {
    smp: usize,
    start: i32,
    end: i32,
    first_loop: bool,
    b16: bool,
    active: bool,
    prologue_num: i32,
    epilogue_num: i32,
    prologue: [u8; 4],
    epilogue: [u8; 8],
}

/// Byte offset of sample element `i` (a frame of a channel) in a sample buffer.
#[inline(always)]
fn elem_off(i: i32, b16: bool) -> isize {
    SAMPLE_GUARD as isize + if b16 { 2 * i as isize } else { i as isize }
}

fn elem_get(d: &[u8], i: i32, b16: bool) -> u16 {
    let o = elem_off(i, b16);
    if o < 0 {
        return 0;
    }
    let o = o as usize;
    if b16 {
        match d.get(o..o + 2) {
            Some(b) => u16::from_le_bytes([b[0], b[1]]),
            None => 0,
        }
    } else {
        d.get(o).copied().unwrap_or(0) as u16
    }
}

fn elem_set(d: &mut [u8], i: i32, b16: bool, v: u16) {
    let o = elem_off(i, b16);
    if o < 0 {
        return;
    }
    let o = o as usize;
    if b16 {
        if let Some(b) = d.get_mut(o..o + 2) {
            b.copy_from_slice(&v.to_le_bytes());
        }
    } else if let Some(b) = d.get_mut(o) {
        *b = v as u8;
    }
}

fn init_sample_wraparound(interp: Interp, ld: &mut LoopData, vi: &MixerVoice, xxs: &mut Sample) {
    let mut prologue_num = 1;
    let mut epilogue_num = 2;
    if !vi.has_data || interp == Interp::Nearest || xxs.flg & XMP_SAMPLE_LOOP == 0 {
        ld.active = false;
        return;
    }
    let Some(d) = xxs.data.as_deref_mut() else {
        ld.active = false;
        return;
    };
    ld.smp = vi.smp as usize;
    ld.start = vi.start;
    ld.end = vi.end;
    ld.first_loop = vi.flags & SAMPLE_LOOP == 0;
    ld.b16 = xxs.flg & XMP_SAMPLE_16BIT != 0;
    ld.active = true;
    if xxs.flg & XMP_SAMPLE_STEREO != 0 {
        ld.start <<= 1;
        ld.end <<= 1;
        prologue_num <<= 1;
        epilogue_num <<= 1;
    }
    ld.prologue_num = prologue_num;
    ld.epilogue_num = epilogue_num;
    let bidir = vi.flags & VOICE_BIDIR != 0;
    let b16 = ld.b16;
    let w = if b16 { 2 } else { 1 };
    let (start, end) = (ld.start, ld.end);
    for i in 0..prologue_num {
        let v = elem_get(d, start - prologue_num + i, b16).to_le_bytes();
        ld.prologue[(i * w) as usize..((i + 1) * w) as usize].copy_from_slice(&v[..w as usize]);
    }
    for i in 0..epilogue_num {
        let v = elem_get(d, end + i, b16).to_le_bytes();
        ld.epilogue[(i * w) as usize..((i + 1) * w) as usize].copy_from_slice(&v[..w as usize]);
    }
    if !ld.first_loop {
        for i in 0..prologue_num {
            let j = i - prologue_num;
            let v = if bidir { elem_get(d, start - 1 - j, b16) } else { elem_get(d, end + j, b16) };
            elem_set(d, start + j, b16, v);
        }
    }
    for i in 0..epilogue_num {
        let v = if bidir { elem_get(d, end - 1 - i, b16) } else { elem_get(d, start + i, b16) };
        elem_set(d, end + i, b16, v);
    }
}

fn reset_sample_wraparound(ld: &LoopData, xxs: &mut [Sample]) {
    if !ld.active {
        return;
    }
    let Some(d) = xxs[ld.smp].data.as_deref_mut() else { return };
    let b16 = ld.b16;
    let w = if b16 { 2 } else { 1 };
    let get = |buf: &[u8], i: i32| -> u16 {
        let i = (i * w) as usize;
        if b16 { u16::from_le_bytes([buf[i], buf[i + 1]]) } else { buf[i] as u16 }
    };
    for i in 0..ld.prologue_num {
        elem_set(d, ld.start - ld.prologue_num + i, b16, get(&ld.prologue, i));
    }
    for i in 0..ld.epilogue_num {
        elem_set(d, ld.end + i, b16, get(&ld.epilogue, i));
    }
}

fn has_active_sustain_loop(vi: &MixerVoice, xxs: &Sample, mod_smp: usize) -> bool {
    (vi.smp as usize) < mod_smp && xxs.flg & XMP_SAMPLE_SLOOP != 0 && vi.flags & VOICE_RELEASE == 0
}

fn has_active_loop(vi: &MixerVoice, xxs: &Sample, mod_smp: usize) -> bool {
    xxs.flg & XMP_SAMPLE_LOOP != 0 || has_active_sustain_loop(vi, xxs, mod_smp)
}

fn adjust_voice_end(vi: &mut MixerVoice, xxs: &Sample, xtra: Option<&ExtraSample>, mod_smp: usize) {
    vi.flags &= !VOICE_BIDIR;
    if xtra.is_some() && has_active_sustain_loop(vi, xxs, mod_smp) {
        let x = xtra.unwrap();
        vi.start = x.sus;
        vi.end = x.sue;
        if xxs.flg & XMP_SAMPLE_SLOOP_BIDIR != 0 {
            vi.flags |= VOICE_BIDIR;
        }
    } else if xxs.flg & XMP_SAMPLE_LOOP != 0 {
        vi.start = xxs.lps;
        if xxs.flg & XMP_SAMPLE_LOOP_FULL != 0 && vi.flags & SAMPLE_LOOP == 0 {
            vi.end = xxs.len;
        } else {
            vi.end = xxs.lpe;
            if xxs.flg & XMP_SAMPLE_LOOP_BIDIR != 0 {
                vi.flags |= VOICE_BIDIR;
            }
        }
    } else {
        vi.start = 0;
        vi.end = xxs.len;
    }
}

fn loop_reposition(vi: &mut MixerVoice, xxs: &Sample, xtra: Option<&ExtraSample>, mod_smp: usize, bidir_adjust: i32) -> bool {
    let loop_changed = vi.flags & SAMPLE_LOOP == 0;
    vi.flags |= SAMPLE_LOOP;
    if loop_changed {
        adjust_voice_end(vi, xxs, xtra, mod_smp);
    }
    if vi.flags & VOICE_BIDIR == 0 {
        if vi.flags & VOICE_REVERSE == 0 {
            vi.pos -= (vi.end - vi.start) as f64;
        } else {
            vi.pos += (vi.end - vi.start) as f64;
        }
    } else {
        vi.flags ^= VOICE_REVERSE;
        if vi.flags & VOICE_REVERSE != 0 {
            vi.pos = (vi.end * 2 - bidir_adjust) as f64 - vi.pos;
        } else {
            vi.pos = (vi.start * 2) as f64 - vi.pos;
        }
    }
    if vi.pos > (xxs.len + 1) as f64 {
        vi.pos = (xxs.len + 1) as f64;
    }
    loop_changed
}

/// The `libxmp_mix_stereoout_*` handlers: mixes `count` output frames of the
/// voice into `buf` (interleaved stereo), through the voice's resonant filter
/// with `FILTER` (the `*_filter` handlers, which the nearest mixers lack).
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn mix_voice<const B16: bool, const STEREO: bool, const FILTER: bool>(
    interp: Interp,
    vi: &mut MixerVoice,
    d: &[u8],
    buf: &mut [i32],
    count: i32,
    vl: i32,
    vr: i32,
    step: i32,
    ramp: i32,
    delta_l: i32,
    delta_r: i32,
) {
    let chn: i32 = if STEREO { 2 } else { 1 };
    let mut pos = (vi.pos as i32) * chn;
    let mut frac = (((1 << SMIX_SHIFT) as f64) * (vi.pos - (vi.pos as i32) as f64)) as i32;
    let mut old_vl = vi.old_vl;
    let mut old_vr = vi.old_vr;
    let rd = |i: i32| -> i32 {
        let v = elem_get(d, i, B16);
        if B16 { v as i16 as i32 } else { v as u8 as i8 as i32 }
    };
    if interp == Interp::Nearest {
        frac += 1 << (SMIX_SHIFT - 1);
        pos += (frac >> SMIX_SHIFT) * chn;
        frac &= SMIX_MASK;
    }
    let sample = |pos: i32, frac: i32, off: i32| -> i32 {
        match interp {
            Interp::Nearest => {
                if B16 { rd(pos + off) } else { rd(pos + off) << 8 }
            }
            Interp::Linear => {
                let (a, b) = if B16 { (rd(pos + off), rd(pos + off + chn)) } else { (rd(pos + off) << 8, rd(pos + off + chn) << 8) };
                let dt = b - a;
                a + (((frac >> 1).wrapping_mul(dt)) >> (SMIX_SHIFT - 1))
            }
            Interp::Spline => {
                let f = (frac >> 6) as usize & 1023;
                let s = CUBIC_SPLINE_LUT0[f]
                    .wrapping_mul(rd(pos + off - chn))
                    .wrapping_add(CUBIC_SPLINE_LUT1[f].wrapping_mul(rd(pos + off)))
                    .wrapping_add(CUBIC_SPLINE_LUT3[f].wrapping_mul(rd(pos + off + (chn << 1))))
                    .wrapping_add(CUBIC_SPLINE_LUT2[f].wrapping_mul(rd(pos + off + chn)));
                if B16 { s >> 14 } else { s >> 6 }
            }
        }
    };
    // Volume ramping (no ramp in the nearest mixers)
    let ramp = if interp == Interp::Nearest { count } else { ramp };
    // VAR_FILTER_MONO, VAR_FILTER_STEREO
    let (mut fl1, mut fl2, mut fr1, mut fr2) = (vi.filter.l1, vi.filter.l2, vi.filter.r1, vi.filter.r2);
    let (a0, b0, b1) = (vi.filter.a0 as i64, vi.filter.b0 as i64, vi.filter.b1 as i64);
    // FILTER_LEFT, FILTER_RIGHT
    let filter = |smp: i32, f1: &mut i32, f2: &mut i32| -> i32 {
        let s64 = (a0 * (smp.wrapping_shl(PREAMP_BITS as u32)) as i64 + b0 * *f1 as i64 + b1 * *f2 as i64) >> FILTER_SHIFT;
        let s = s64.clamp(FILTER_MIN as i64, FILTER_MAX as i64) as i32;
        *f2 = *f1;
        *f1 = s;
        s >> PREAMP_BITS
    };
    let mut count = count;
    let mut o = 0usize;
    while count > 0 {
        let mut smpl = sample(pos, frac, 0);
        let mut smpr = if STEREO { sample(pos, frac, 1) } else { smpl };
        if FILTER {
            smpl = filter(smpl, &mut fl1, &mut fl2);
            smpr = if STEREO { filter(smpr, &mut fr1, &mut fr2) } else { smpl };
        }
        let (l, r) = if count > ramp { (old_vl >> 8, old_vr >> 8) } else { (vl, vr) };
        if let Some(b) = buf.get_mut(o..o + 2) {
            b[0] = b[0].wrapping_add(smpl.wrapping_mul(l));
            b[1] = b[1].wrapping_add(smpr.wrapping_mul(r));
        }
        o += 2;
        if count > ramp {
            old_vl = old_vl.wrapping_add(delta_l);
            old_vr = old_vr.wrapping_add(delta_r);
        }
        frac += step;
        pos += (frac >> SMIX_SHIFT) * chn;
        frac &= SMIX_MASK;
        count -= 1;
    }
    if FILTER {
        // SAVE_FILTER_MONO copies the left state to the right
        vi.filter.l1 = fl1;
        vi.filter.l2 = fl2;
        vi.filter.r1 = if STEREO { fr1 } else { fl1 };
        vi.filter.r2 = if STEREO { fr2 } else { fl2 };
    }
}

impl Context {
    /// `libxmp_mixer_on` (16-bit stereo).
    pub fn mixer_on(&mut self, rate: i32) {
        let s = &mut self.s;
        let mut total_size = 2 * get_ticksize(rate, DEFAULT_TIME_FACTOR * 2.0, PAL_RATE, XMP_MIN_BPM);
        total_size = total_size.clamp(5 * 49170 * 2 / 20, XMP_MAX_FRAMESIZE);
        s.buffer = vec![0; total_size as usize];
        s.buf32 = vec![0; total_size as usize];
        s.total_size = total_size;
        s.freq = rate;
        s.amplify = 1;
        s.mix = 100;
        s.bidir_adjust = 0;
    }

    fn do_anticlick(&mut self, voc: usize, buf: Option<usize>, mut count: i32) {
        let discharge = self.s.ticksize >> ANTICLICK_SHIFT;
        let vi = &mut self.p.virt.voice_array[voc];
        let smp_l = vi.sleft;
        let smp_r = vi.sright;
        vi.sleft = 0;
        vi.sright = 0;
        if smp_l == 0 && smp_r == 0 {
            return;
        }
        let mut at = match buf {
            None => {
                count = discharge;
                0
            }
            Some(b) => {
                if count > discharge {
                    count = discharge;
                }
                b
            }
        };
        if count <= 0 {
            return;
        }
        let stepval = (1 << ANTICLICK_FPSHIFT) / count;
        let mut stepmul = stepval * count;
        let b = &mut self.s.buf32;
        loop {
            stepmul -= stepval;
            if stepmul <= 0 {
                break;
            }
            let mut sq = (stepmul >> (ANTICLICK_FPSHIFT - 16)) as u32;
            sq = sq.wrapping_mul(sq);
            if at + 1 < b.len() {
                b[at] = b[at].wrapping_add(((sq as i64 * smp_l as i64) >> 32) as i32);
                b[at + 1] = b[at + 1].wrapping_add(((sq as i64 * smp_r as i64) >> 32) as i32);
            }
            at += 2;
        }
    }

    fn set_sample_end(&mut self, voc: usize, end: bool) {
        if voc as i32 >= self.p.virt.maxvoc {
            return;
        }
        let chn = self.p.virt.voice_array[voc].chn;
        if end {
            if let Some(xc) = self.p.xc_data.get_mut(chn as usize) {
                xc.set_note(NOTE_SAMPLE_END);
            }
            self.p.virt.voice_array[voc].fidx &= !FLAG_ACTIVE;
            if self.m.quirk & QUIRK_RSTCHN != 0 {
                self.virt_resetvoice(voc as i32, false);
            }
        } else if let Some(xc) = self.p.xc_data.get_mut(chn as usize) {
            xc.reset_note(NOTE_SAMPLE_END);
        }
    }

    fn hotswap_sample(&mut self, voc: usize, smp: i32) {
        let (vol, pan) = (self.p.virt.voice_array[voc].vol, self.p.virt.voice_array[voc].pan);
        self.mixer_setpatch(voc, smp, false);
        let vi = &mut self.p.virt.voice_array[voc];
        vi.flags |= SAMPLE_LOOP;
        vi.vol = vol;
        vi.pan = pan;
    }

    /// `get_current_sample`: the voice's sample and c5spd, with the voice ends
    /// adjusted.
    fn get_current_sample(&mut self, voc: usize) -> i32 {
        let mod_smp = self.m.module.smp;
        let vi = &mut self.p.virt.voice_array[voc];
        let smp = vi.smp as usize;
        let xxs = &self.m.module.xxs[smp];
        let xtra = &self.m.xtra[smp];
        adjust_voice_end(vi, xxs, Some(xtra), mod_smp);
        xtra.c5spd as i32
    }

    /// `libxmp_mixer_softmixer`: renders one tick into `s.buffer`.
    pub fn softmixer(&mut self) {
        let interp = self.s.interp;
        self.s.bidir_adjust = if self.m.read_event_type == ReadEvent::It { 1 } else { 0 };
        let bidir_adjust = self.s.bidir_adjust;
        let mod_smp = self.m.module.smp;

        // libxmp_mixer_prepare
        self.s.ticksize = get_ticksize(self.s.freq, self.m.time_factor * self.p.time_factor_relative, self.m.rrate, self.p.bpm);
        if self.s.ticksize < 0 || self.s.ticksize > self.s.total_size / 2 {
            self.s.ticksize = self.s.total_size / 2;
        }
        let ticksize = self.s.ticksize;
        for v in self.s.buf32[..(ticksize * 2) as usize].iter_mut() {
            *v = 0;
        }

        for voc in 0..self.p.virt.maxvoc as usize {
            if self.p.virt.voice_array[voc].flags & ANTICLICK != 0 {
                if interp != Interp::Nearest {
                    self.do_anticlick(voc, None, 0);
                }
                self.p.virt.voice_array[voc].flags &= !ANTICLICK;
            }
            if self.p.virt.voice_array[voc].chn < 0 {
                continue;
            }
            if self.p.virt.voice_array[voc].smp as u32 as usize >= mod_smp {
                // libxmp's sound effect samples are not supported
                self.virt_resetvoice(voc as i32, true);
                continue;
            }
            if self.p.virt.voice_array[voc].period < 1.0 {
                self.virt_resetvoice(voc as i32, true);
                continue;
            }
            {
                let vi = &mut self.p.virt.voice_array[voc];
                if vi.pos < 0.0 {
                    vi.pos = 0.0;
                }
                vi.pos0 = vi.pos as i32;
            }
            let mut buf_pos = 0usize;
            let mut vol = self.p.virt.voice_array[voc].vol;
            if self.m.mvolbase > 0 && self.m.mvol != self.m.mvolbase {
                vol = vol * self.m.mvol / self.m.mvolbase;
            }
            let pan = self.p.virt.voice_array[voc].pan;
            let (mut vol_l, mut vol_r) =
                if pan == PAN_SURROUND { (vol * 0x80, -vol * 0x80) } else { (vol * (0x80 - pan), vol * (0x80 + pan)) };

            let mut c5spd;
            if self.p.virt.voice_array[voc].flags & SAMPLE_PAUSED != 0 {
                let vi = &mut self.p.virt.voice_array[voc];
                if vi.flags & SAMPLE_QUEUED == 0 || vi.queued_smp < 0 {
                    vi.flags &= !SAMPLE_QUEUED;
                    continue;
                }
                let q = vi.queued_smp;
                self.hotswap_sample(voc, q);
                c5spd = self.get_current_sample(voc);
                let vi = &mut self.p.virt.voice_array[voc];
                vi.pos = vi.start as f64;
            } else {
                c5spd = self.get_current_sample(voc);
            }

            let step = C4_PERIOD * c5spd as f64 / self.s.freq as f64 / self.p.virt.voice_array[voc].period;
            if !(0.001..=i16::MAX as f64).contains(&step) {
                continue;
            }

            let mut ld = LoopData::default();
            {
                let vi = self.p.virt.voice_array[voc];
                let smp = vi.smp as usize;
                init_sample_wraparound(interp, &mut ld, &vi, &mut self.m.module.xxs[smp]);
            }

            let mut rampsize = ticksize >> ANTICLICK_SHIFT;
            let delta_l = (vol_l - self.p.virt.voice_array[voc].old_vl) / rampsize;
            let delta_r = (vol_r - self.p.virt.voice_array[voc].old_vr) / rampsize;

            let mut size = ticksize;
            let mut usmp = ticksize;
            while size > 0 {
                let vchn = self.p.virt.voice_array[voc].chn;
                let split_noloop = self.p.xc_data.get(vchn as usize).is_some_and(|x| x.split != 0);
                let samples;
                let step_dir;
                {
                    let vi = &self.p.virt.voice_array[voc];
                    if vi.flags & VOICE_REVERSE == 0 {
                        if vi.pos >= vi.end as f64 {
                            samples = 0;
                            usmp -= 1;
                            if usmp <= 0 {
                                break;
                            }
                        } else {
                            let c = ((vi.end as f64 - vi.pos) / step).ceil();
                            samples = if c > size as f64 { size } else { c as i32 };
                        }
                        step_dir = step;
                    } else {
                        if vi.pos <= vi.start as f64 {
                            samples = 0;
                            usmp -= 1;
                            if usmp <= 0 {
                                break;
                            }
                        } else {
                            let c = ((vi.pos - vi.start as f64) / step).ceil();
                            samples = if c > size as f64 { size } else { c as i32 };
                        }
                        step_dir = -step;
                    }
                }

                if self.p.virt.voice_array[voc].vol != 0 {
                    let mix_size = (samples * 2) as usize;
                    let (prev_l, prev_r) = if samples > 0 {
                        (self.s.buf32[buf_pos + mix_size - 2], self.s.buf32[buf_pos + mix_size - 1])
                    } else {
                        (0, 0)
                    };
                    if samples > 0 && self.p.virt.voice_array[voc].has_data {
                        let mut rsize = 0;
                        if rampsize > samples {
                            rampsize -= samples;
                        } else {
                            rsize = samples - rampsize;
                            rampsize = 0;
                        }
                        if delta_l == 0 && delta_r == 0 {
                            rsize = samples;
                        }
                        let vi = &mut self.p.virt.voice_array[voc];
                        let xxs = &self.m.module.xxs[vi.smp as usize];
                        if let Some(d) = xxs.data.as_deref() {
                            let buf = &mut self.s.buf32[buf_pos..];
                            let st = (step_dir * (1 << SMIX_SHIFT) as f64) as i32;
                            let (vl, vr) = (vol_l >> 8, vol_r >> 8);
                            // See OpenMPT env-flt-max.it; the nearest mixers
                            // have no filtered versions
                            let filter = vi.fidx & FLAG_FILTER != 0
                                && !(vi.filter.cutoff >= 0xfe && vi.filter.resonance == 0)
                                && interp != Interp::Nearest;
                            match (vi.fidx & FLAG_16_BITS != 0, vi.fidx & FLAG_STEREO != 0, filter) {
                                (false, false, false) => mix_voice::<false, false, false>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                                (true, false, false) => mix_voice::<true, false, false>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                                (false, true, false) => mix_voice::<false, true, false>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                                (true, true, false) => mix_voice::<true, true, false>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                                (false, false, true) => mix_voice::<false, false, true>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                                (true, false, true) => mix_voice::<true, false, true>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                                (false, true, true) => mix_voice::<false, true, true>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                                (true, true, true) => mix_voice::<true, true, true>(interp, vi, d, buf, samples, vl, vr, st, rsize, delta_l, delta_r),
                            }
                        }
                        buf_pos += mix_size;
                        vi.old_vl = vi.old_vl.wrapping_add(samples * delta_l);
                        vi.old_vr = vi.old_vr.wrapping_add(samples * delta_r);
                        vi.sleft = self.s.buf32[buf_pos - 2].wrapping_sub(prev_l);
                        vi.sright = self.s.buf32[buf_pos - 1].wrapping_sub(prev_r);
                    }
                }

                self.p.virt.voice_array[voc].pos += step_dir * samples as f64;
                size -= samples;

                let (active_loop, queued) = {
                    let vi = &self.p.virt.voice_array[voc];
                    let xxs = &self.m.module.xxs[vi.smp as usize];
                    (has_active_loop(vi, xxs, mod_smp), vi.flags & SAMPLE_QUEUED != 0)
                };
                if (!active_loop || split_noloop) && !queued {
                    if size > 0 {
                        self.do_anticlick(voc, Some(buf_pos), size);
                        self.set_sample_end(voc, true);
                        vol_l = 0;
                        vol_r = 0;
                    }
                    size = 0;
                    continue;
                }

                let past_end = {
                    let vi = &self.p.virt.voice_array[voc];
                    (vi.flags & VOICE_REVERSE == 0 && vi.pos >= vi.end as f64) || (vi.flags & VOICE_REVERSE != 0 && vi.pos <= vi.start as f64)
                };
                if size > 0 || past_end {
                    if queued {
                        self.do_anticlick(voc, Some(buf_pos), size);
                        let q = self.p.virt.voice_array[voc].queued_smp;
                        if q < 0 || (!active_loop && self.m.module.xxs[q as usize].flg & XMP_SAMPLE_LOOP == 0) {
                            let vi = &mut self.p.virt.voice_array[voc];
                            vi.flags &= !SAMPLE_QUEUED;
                            vi.flags |= SAMPLE_PAUSED;
                            self.set_sample_end(voc, true);
                            vol_l = 0;
                            vol_r = 0;
                            size = 0;
                            continue;
                        }
                        reset_sample_wraparound(&ld, &mut self.m.module.xxs);
                        self.hotswap_sample(voc, q);
                        c5spd = self.get_current_sample(voc);
                        let _ = c5spd;
                        let vi = self.p.virt.voice_array[voc];
                        init_sample_wraparound(interp, &mut ld, &vi, &mut self.m.module.xxs[vi.smp as usize]);
                        let vi = &mut self.p.virt.voice_array[voc];
                        vi.pos = vi.start as f64;
                        continue;
                    }
                    let changed = {
                        let vi = &mut self.p.virt.voice_array[voc];
                        let smp = vi.smp as usize;
                        loop_reposition(vi, &self.m.module.xxs[smp], Some(&self.m.xtra[smp]), mod_smp, bidir_adjust)
                    };
                    if changed {
                        reset_sample_wraparound(&ld, &mut self.m.module.xxs);
                        let vi = self.p.virt.voice_array[voc];
                        init_sample_wraparound(interp, &mut ld, &vi, &mut self.m.module.xxs[vi.smp as usize]);
                    }
                }
            }

            reset_sample_wraparound(&ld, &mut self.m.module.xxs);
            let vi = &mut self.p.virt.voice_array[voc];
            vi.old_vl = vol_l;
            vi.old_vr = vol_r;
        }

        // (not libxmp: the OPL of a module with AdLib instruments)
        if self.opl.is_some() {
            self.opl_mix(ticksize as usize);
        }

        // downmix_int_16bit
        let size = ((ticksize * 2).min(self.s.total_size)) as usize;
        let shift = DOWNMIX_SHIFT - self.s.amplify;
        for (o, &v) in self.s.buffer[..size].iter_mut().zip(self.s.buf32[..size].iter()) {
            *o = (v >> shift).clamp(-32768, 32767) as i16;
        }
    }

    /// `libxmp_mixer_voicepos`.
    pub fn mixer_voicepos(&mut self, voc: usize, pos: f64, ac: bool) {
        let mod_smp = self.m.module.smp;
        if self.p.virt.voice_array[voc].flags & SAMPLE_QUEUED != 0 {
            self.p.virt.voice_array[voc].flags &= !SAMPLE_QUEUED;
            let (q, smp) = (self.p.virt.voice_array[voc].queued_smp, self.p.virt.voice_array[voc].smp);
            if q < 0 {
                self.p.virt.voice_array[voc].flags |= SAMPLE_PAUSED;
            } else if smp != q {
                self.hotswap_sample(voc, q);
            }
            self.p.virt.voice_array[voc].flags |= SAMPLE_LOOP;
        }
        let vi = &mut self.p.virt.voice_array[voc];
        let smp = vi.smp as usize;
        let Some(xxs) = self.m.module.xxs.get(smp) else { return };
        let xtra = self.m.xtra.get(smp);
        vi.pos = pos;
        adjust_voice_end(vi, xxs, xtra, mod_smp);
        if vi.pos >= vi.end as f64 {
            vi.pos = vi.end as f64;
            if vi.flags & VOICE_REVERSE == 0 && has_active_loop(vi, xxs, mod_smp) {
                loop_reposition(vi, xxs, xtra, mod_smp, self.s.bidir_adjust);
            }
        } else if vi.flags & VOICE_REVERSE != 0 && vi.pos <= 0.1 {
            vi.pos = vi.end as f64;
        }
        if ac {
            vi.flags |= ANTICLICK;
            vi.old_vl = 0;
            vi.old_vr = 0;
        }
    }

    /// `libxmp_mixer_setpatch`.
    pub fn mixer_setpatch(&mut self, voc: usize, smp: i32, ac: bool) {
        let (b16, stereo, has_data) = match self.m.module.xxs.get(smp as usize) {
            Some(x) => (x.flg & XMP_SAMPLE_16BIT != 0, x.flg & XMP_SAMPLE_STEREO != 0, x.data.is_some()),
            None => (false, false, false),
        };
        {
            let vi = &mut self.p.virt.voice_array[voc];
            vi.smp = smp;
            vi.vol = 0;
            vi.pan = 0;
            vi.flags &= !(SAMPLE_LOOP | SAMPLE_QUEUED | SAMPLE_PAUSED | VOICE_REVERSE | VOICE_BIDIR);
            vi.fidx = FLAG_STEREOOUT;
        }
        self.set_sample_end(voc, false);
        let filter = self.m.quirk & QUIRK_FILTER != 0 && self.s.dsp & XMP_DSP_LOWPASS != 0;
        let vi = &mut self.p.virt.voice_array[voc];
        vi.has_data = has_data;
        vi.fidx |= FLAG_ACTIVE;
        if filter {
            vi.fidx |= FLAG_FILTER;
        }
        if b16 {
            vi.fidx |= FLAG_16_BITS;
        }
        if stereo {
            vi.fidx |= FLAG_STEREO;
        }
        self.mixer_voicepos(voc, 0.0, ac);
    }

    /// `libxmp_mixer_queuepatch`.
    pub fn mixer_queuepatch(&mut self, voc: usize, smp: i32) {
        let vi = &mut self.p.virt.voice_array[voc];
        if smp != vi.smp || vi.flags & SAMPLE_PAUSED != 0 {
            vi.queued_smp = smp;
            vi.flags |= SAMPLE_QUEUED;
        }
    }

    /// `libxmp_mixer_setnote`.
    pub fn mixer_setnote(&mut self, voc: usize, note: i32) {
        let note = note.min(149);
        let vi = &mut self.p.virt.voice_array[voc];
        vi.note = note;
        vi.period = note_to_period_mix(note, 0);
        vi.flags |= ANTICLICK;
        vi.old_vl = 0;
        vi.old_vr = 0;
    }

    /// `libxmp_mixer_seteffect`: a filter parameter of the voice.
    pub fn mixer_seteffect(&mut self, voc: usize, type_: i32, val: i32) {
        let f = &mut self.p.virt.voice_array[voc].filter;
        match type_ {
            DSP_EFFECT_CUTOFF => f.cutoff = val,
            DSP_EFFECT_RESONANCE => f.resonance = val,
            DSP_EFFECT_FILTER_A0 => f.a0 = val,
            DSP_EFFECT_FILTER_B0 => f.b0 = val,
            DSP_EFFECT_FILTER_B1 => f.b1 = val,
            _ => {}
        }
    }

    /// `libxmp_mixer_setvol`.
    pub fn mixer_setvol(&mut self, voc: usize, vol: i32) {
        let vi = &mut self.p.virt.voice_array[voc];
        if vol == 0 {
            vi.flags |= ANTICLICK;
            vi.old_vl = 0;
            vi.old_vr = 0;
        }
        vi.vol = vol;
    }

    /// `libxmp_mixer_release`.
    pub fn mixer_release(&mut self, voc: usize, rel: bool) {
        let mod_smp = self.m.module.smp;
        let vi = &mut self.p.virt.voice_array[voc];
        if rel {
            if vi.flags & VOICE_RELEASE == 0 {
                if let Some(xxs) = self.m.module.xxs.get(vi.smp as usize) {
                    if has_active_sustain_loop(vi, xxs, mod_smp) && xxs.flg & XMP_SAMPLE_LOOP_BIDIR == 0 {
                        vi.flags &= !VOICE_REVERSE;
                    }
                }
            }
            vi.flags |= VOICE_RELEASE;
        } else {
            vi.flags &= !VOICE_RELEASE;
        }
    }

    /// `libxmp_mixer_reverse`.
    pub fn mixer_reverse(&mut self, voc: usize, rev: bool) {
        let vi = &mut self.p.virt.voice_array[voc];
        if vi.fidx & FLAG_ACTIVE == 0 {
            return;
        }
        if rev {
            vi.flags |= VOICE_REVERSE;
        } else {
            vi.flags &= !VOICE_REVERSE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(pos: f64) -> MixerVoice {
        MixerVoice { pos, ..Default::default() }
    }

    fn sample8(v: &[i8]) -> Vec<u8> {
        let mut d = vec![0u8; SAMPLE_GUARD];
        d.extend(v.iter().map(|&x| x as u8));
        d.extend([0u8; 4]);
        d
    }

    #[test]
    fn ticksize_matches_libxmp() {
        assert_eq!(get_ticksize(48000, 10.0, 250.0, 125), 960);
        assert_eq!(get_ticksize(44100, 10.0, 250.0, 78), 1413);
        assert_eq!(get_ticksize(8, 10.0, 250.0, 255), 8);
    }

    #[test]
    fn linear_interpolates_halfway() {
        let d = sample8(&[0, 64, 64, 64]);
        let mut buf = vec![0i32; 4];
        // pos 0.5, step 0: both frames read halfway between 0 and 64 << 8
        mix_voice::<false, false, false>(Interp::Linear, &mut voice(0.5), &d, &mut buf, 2, 2, 3, 0, 2, 0, 0);
        assert_eq!(buf, vec![32 * 256 * 2, 32 * 256 * 3, 32 * 256 * 2, 32 * 256 * 3]);
    }

    #[test]
    fn nearest_rounds_and_steps() {
        let d = sample8(&[10, 20, 30, 40]);
        let mut buf = vec![0i32; 6];
        // start 0.6 rounds to frame 1; step 1.0
        mix_voice::<false, false, false>(Interp::Nearest, &mut voice(0.6), &d, &mut buf, 3, 1, 1, 1 << 16, 0, 0, 0);
        assert_eq!(buf, vec![20 << 8, 20 << 8, 30 << 8, 30 << 8, 40 << 8, 40 << 8]);
    }

    #[test]
    fn panning_and_ramp() {
        let mut d = vec![0u8; SAMPLE_GUARD];
        for _ in 0..8 {
            d.extend(1000i16.to_le_bytes());
        }
        d.extend([0u8; 8]);
        let mut buf = vec![0i32; 8];
        let mut vi = voice(0.0);
        vi.old_vl = 0;
        vi.old_vr = 256 * 10;
        // two ramped frames (count 4 > ramp 2), then fixed volumes 5 / 7
        mix_voice::<true, false, false>(Interp::Linear, &mut vi, &d, &mut buf, 4, 5, 7, 1 << 16, 2, 256, -256);
        assert_eq!(buf, vec![0, 10000, 1000, 9000, 5000, 7000, 5000, 7000]);
    }

    #[test]
    fn stereo_sample_reads_both_channels() {
        let mut d = vec![0u8; SAMPLE_GUARD];
        for f in 0..4i16 {
            d.extend((100 * f).to_le_bytes());
            d.extend((-100 * f).to_le_bytes());
        }
        d.extend([0u8; 16]);
        let mut buf = vec![0i32; 4];
        mix_voice::<true, true, false>(Interp::Linear, &mut voice(1.0), &d, &mut buf, 2, 1, 1, 1 << 16, 2, 0, 0);
        assert_eq!(buf, vec![100, -100, 200, -200]);
    }

    #[test]
    fn filter_runs_the_two_pole_recursion() {
        let mut d = vec![0u8; SAMPLE_GUARD];
        for v in [1000i16, 1000, 1000, 0] {
            d.extend(v.to_le_bytes());
        }
        d.extend([0u8; 8]);
        let mut buf = vec![0i32; 6];
        let mut vi = voice(0.0);
        // y[n] = x[n] / 2 + y[n-1] / 4 (a0 = 0.5, b0 = 0.25, b1 = 0), in the
        // pre-amplified domain
        vi.filter.a0 = 1 << (FILTER_SHIFT - 1);
        vi.filter.b0 = 1 << (FILTER_SHIFT - 2);
        mix_voice::<true, false, true>(Interp::Linear, &mut vi, &d, &mut buf, 3, 1, 1, 1 << 16, 3, 0, 0);
        let y1 = 1000 << 14;
        let y2 = (1000 << 14) + (y1 >> 2);
        let y3 = (1000 << 14) + (y2 >> 2);
        assert_eq!(buf, vec![y1 >> 15, y1 >> 15, y2 >> 15, y2 >> 15, y3 >> 15, y3 >> 15]);
        // the state is kept, the right copies the left for a mono sample
        assert_eq!((vi.filter.l1, vi.filter.l2, vi.filter.r1, vi.filter.r2), (y3, y2, y3, y2));
    }

    #[test]
    fn wraparound_copies_loop_start_after_end() {
        let mut xxs = Sample { len: 4, lps: 1, lpe: 3, flg: XMP_SAMPLE_LOOP, data: Some(sample8(&[1, 2, 3, 4])), ..Default::default() };
        let vi = MixerVoice { has_data: true, start: 1, end: 3, flags: SAMPLE_LOOP, ..Default::default() };
        let mut ld = LoopData::default();
        init_sample_wraparound(Interp::Linear, &mut ld, &vi, &mut xxs);
        let d = xxs.data.as_ref().unwrap();
        // end[0..2] = start[0..2], start[-1] = end[-1]
        assert_eq!(&d[SAMPLE_GUARD..SAMPLE_GUARD + 6], &[3, 2, 3, 2, 3, 0]);
        let mut all = vec![xxs];
        ld.smp = 0;
        reset_sample_wraparound(&ld, &mut all);
        assert_eq!(&all[0].data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 6], &[1, 2, 3, 4, 0, 0]);
    }
}

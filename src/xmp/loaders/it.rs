//! Impulse Tracker modules (`loaders/it_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! The three instrument kinds (IT 2.x "new" instruments with their volume, pan
//! and pitch/filter envelopes, IT 1.x "old" ones, and sample mode), compressed
//! samples (`itsex`), ModPlug ADPCM, stereo samples, MIDI macro configurations,
//! and the trackers libxmp tells apart by their version words (flow mode, type
//! string, ModPlug pre-amp). `load_at` takes the module's offset in the file, as
//! libxmp's loader does for the Unreal containers that embed ITs.

use super::itsex;
use crate::xmp::load::*;
use crate::xmp::module::*;

const IT_STEREO: u16 = 0x01;
const IT_USE_INST: u16 = 0x04;
const IT_LINEAR_FREQ: u16 = 0x08;
const IT_OLD_FX: u16 = 0x10;
const IT_LINK_GXX: u16 = 0x20;
const IT_MIDI_CONFIG: u16 = 0x80;

const IT_EDIT_HISTORY: u16 = 0x02;
const IT_SPEC_MIDICFG: u16 = 0x08;

const IT_SMP_SAMPLE: u8 = 0x01;
const IT_SMP_16BIT: u8 = 0x02;
const IT_SMP_STEREO: u8 = 0x04;
const IT_SMP_COMP: u8 = 0x08;
const IT_SMP_LOOP: u8 = 0x10;
const IT_SMP_SLOOP: u8 = 0x20;
const IT_SMP_BLOOP: u8 = 0x40;
const IT_SMP_BSLOOP: u8 = 0x80;

const IT_CVT_SIGNED: u8 = 0x01;
const IT_CVT_DIFF: u8 = 0x04;
const IT_CVT_ADPCM: u8 = 0xff;

const IT_ENV_ON: u8 = 0x01;
const IT_ENV_LOOP: u8 = 0x02;
const IT_ENV_SLOOP: u8 = 0x04;
const IT_ENV_CARRY: u8 = 0x08;
const IT_ENV_FILTER: u8 = 0x80;

const MAGIC_IMPI: u32 = 0x494d_5049;
const MAGIC_IMPS: u32 = 0x494d_5053;

const TEMP_BUFFER_LEN: usize = 65536;
const L_CHANNELS: usize = 64;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 4 && &d[..4] == b"IMPM"
}

const FX_NONE: u8 = 0xff;
const FX_XTND: u8 = 0xfe;

const FX: [u8; 32] = [
    FX_NONE,
    FX_S3M_SPEED,
    FX_JUMP,
    FX_IT_BREAK,
    FX_VOLSLIDE,
    FX_PORTA_DN,
    FX_PORTA_UP,
    FX_TONEPORTA,
    FX_VIBRATO,
    FX_TREMOR,
    FX_S3M_ARPEGGIO,
    FX_VIBRA_VSLIDE,
    FX_TONE_VSLIDE,
    FX_TRK_VOL,
    FX_TRK_VSLIDE,
    FX_OFFSET,
    FX_IT_PANSLIDE,
    FX_MULTI_RETRIG,
    FX_TREMOLO,
    FX_XTND,
    FX_IT_BPM,
    FX_FINE_VIBRATO,
    FX_GLOBALVOL,
    FX_GVOL_SLIDE,
    FX_SETPAN,
    FX_PANBRELLO,
    FX_MACRO,
    FX_NONE,
    FX_MACROSMOOTH,
    FX_NONE,
    FX_NONE,
    FX_NONE,
];

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn xlat_fx(c: usize, e: &mut Event, last_fxp: &mut [u8; 64], new_fx: bool) {
    let (mut h, mut l) = (msn(e.fxp), lsn(e.fxp));
    e.fxt = FX[e.fxt as usize];
    match e.fxt {
        FX_XTND => {
            e.fxt = FX_EXTENDED;
            if h == 0 && e.fxp == 0 {
                e.fxp = last_fxp[c];
                h = msn(e.fxp);
                l = lsn(e.fxp);
            } else {
                last_fxp[c] = e.fxp;
            }
            match h {
                0x1 => e.fxp = 0x30 | l,
                0x3 => e.fxp = 0x40 | l,
                0x4 => e.fxp = 0x70 | l,
                0x5 => {
                    if l <= 3 {
                        e.fxt = FX_PANBRELLO_WF;
                        e.fxp = l;
                    } else {
                        e.fxt = 0;
                        e.fxp = 0;
                    }
                }
                0x6 => e.fxp = 0xe0 | l,
                0x7 => {
                    e.fxt = FX_IT_INSTFUNC;
                    e.fxp &= 0x0f;
                }
                0x8 => {
                    e.fxt = FX_SETPAN;
                    e.fxp = l << 4;
                }
                0x9 => {
                    if l == 0 || l == 1 {
                        e.fxt = FX_SURROUND;
                        e.fxp = l;
                    } else if l == 0xe || l == 0xf {
                        e.fxt = FX_REVERSE;
                        e.fxp = l - 0xe;
                    }
                }
                0xa => {
                    e.fxt = FX_HIOFFSET;
                    e.fxp = l;
                }
                0xb => e.fxp = 0x60 | l,
                0xc | 0xd => {
                    e.fxp = l;
                    if e.fxp == 0 {
                        e.fxp += 1;
                    }
                    e.fxp |= h << 4;
                }
                0xe => {
                    e.fxt = FX_IT_ROWDELAY;
                    e.fxp = l;
                }
                0xf => {
                    e.fxt = FX_MACRO_SET;
                    e.fxp = l;
                }
                // 0x2 (finetune) and 0x0
                _ => {
                    e.fxt = 0;
                    e.fxp = 0;
                }
            }
        }
        FX_TREMOR => {
            if !new_fx && e.fxp != 0 {
                e.fxp = ((msn(e.fxp) + 1) << 4) | (lsn(e.fxp) + 1);
            }
        }
        FX_GLOBALVOL => {
            if e.fxp > 0x80 {
                e.fxt = 0;
                e.fxp = 0;
            }
        }
        FX_NONE => {
            e.fxt = 0;
            e.fxp = 0;
        }
        _ => {}
    }
}

fn xlat_volfx(e: &mut Event) {
    let b = e.vol;
    e.vol = 0;
    match b {
        0..=0x40 => e.vol = b + 1,
        65..=74 => {
            e.f2t = FX_F_VSLIDE_UP_2;
            e.f2p = b - 65;
        }
        75..=84 => {
            e.f2t = FX_F_VSLIDE_DN_2;
            e.f2p = b - 75;
        }
        85..=94 => {
            e.f2t = FX_VSLIDE_UP_2;
            e.f2p = b - 85;
        }
        95..=104 => {
            e.f2t = FX_VSLIDE_DN_2;
            e.f2p = b - 95;
        }
        105..=114 => {
            e.f2t = FX_PORTA_DN;
            e.f2p = (b - 105) << 2;
        }
        115..=124 => {
            e.f2t = FX_PORTA_UP;
            e.f2p = (b - 115) << 2;
        }
        128..=192 => {
            e.f2p = if b == 192 { 0xff } else { (b - 128) << 2 };
            e.f2t = FX_SETPAN;
        }
        193..=202 => {
            const VAL: [u8; 10] = [0x00, 0x01, 0x04, 0x08, 0x10, 0x20, 0x40, 0x60, 0x80, 0xff];
            e.f2t = FX_TONEPORTA;
            e.f2p = VAL[(b - 193) as usize];
        }
        203..=212 => {
            e.f2t = FX_VIBRATO;
            e.f2p = b - 203;
        }
        _ => {}
    }
}

/// `fix_name` + `libxmp_copy_adjust`: IT names may hold NULs before their end.
fn it_name(b: &[u8], n: usize) -> String {
    let mut s = b.to_vec();
    let l = s.len() - 1;
    let mut i = 0;
    while i < l {
        if s[i] == 0 {
            s[i] = b' ';
        }
        i += 1;
    }
    while i > 0 && s[i - 1] == b' ' {
        s[i - 1] = 0;
        i -= 1;
    }
    copy_adjust(&s, n)
}

fn load_it_midi_config(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut midi = Box::<MidiMacroData>::default();
    if !f.seek_cur(9 * 32) {
        return Err(());
    }
    for p in midi.param.iter_mut() {
        if f.read(p) < 32 {
            return Err(());
        }
        p[31] = 0;
    }
    for p in midi.fixed.iter_mut() {
        if f.read(p) < 32 {
            return Err(());
        }
        p[31] = 0;
    }
    m.midi = Some(midi);
    Ok(())
}

/// `read_envelope`; returns the IT envelope's flags and point count (the
/// `struct it_envelope` the caller looks at after the last one).
fn read_envelope(ei: &mut Envelope, f: &mut Hio) -> Result<(u8, usize), ()> {
    let mut buf = [0u8; 82];
    if f.read(&mut buf) != 82 {
        return Err(());
    }
    let flg = buf[0];
    let num = buf[1].min(25);
    ei.flg = if flg & IT_ENV_ON != 0 { XMP_ENVELOPE_ON } else { 0 };
    if flg & IT_ENV_LOOP != 0 {
        ei.flg |= XMP_ENVELOPE_LOOP;
    }
    if flg & IT_ENV_SLOOP != 0 {
        ei.flg |= XMP_ENVELOPE_SUS | XMP_ENVELOPE_SLOOP;
    }
    if flg & IT_ENV_CARRY != 0 {
        ei.flg |= XMP_ENVELOPE_CARRY;
    }
    ei.npt = num as i32;
    ei.sus = buf[4] as i32;
    ei.sue = buf[5] as i32;
    ei.lps = buf[2] as i32;
    ei.lpe = buf[3] as i32;
    if ei.npt > 0 && ei.npt <= 25 {
        for i in 0..ei.npt as usize {
            ei.data[i * 2] = rd16(&buf, 7 + i * 3) as i16;
            ei.data[i * 2 + 1] = buf[6 + i * 3] as i8 as i16;
        }
    } else {
        ei.flg &= !XMP_ENVELOPE_ON;
    }
    Ok((flg, num as usize))
}

/// The tracker libxmp names (`identify_tracker`): the type string, flow mode and
/// whether it is a ModPlug 1.16-era file (pre-amp); may rewrite `cmwt`.
fn identify_tracker(m: &mut ModuleData, h: &mut Header, pat_before_smp: bool) -> bool {
    let sample_mode = !h.flags & IT_USE_INST != 0;
    let mut is_mpt_116 = false;
    m.flow_mode = FLOW_MODE_IT_210;
    let tracker_name = match h.cwt >> 8 {
        0x00 => "unmo3".to_string(),
        0x01 | 0x02 => {
            if h.cmwt == 0x0200
                && h.cwt == 0x0214
                && h.flags == 9
                && h.special == 0
                && h.hilite_maj == 0
                && h.hilite_min == 0
                && h.insnum == 0
                && h.patnum as u32 + 1 == h.ordnum as u32
                && h.gv == 128
                && h.mv == 100
                && h.is == 1
                && h.sep == 128
                && h.pwd == 0
                && h.msglen == 0
                && h.msgofs == 0
                && h.rsvd == [0; 4]
            {
                "OpenSPC conversion".to_string()
            } else if h.cmwt == 0x0200 && h.cwt == 0x0217 {
                h.cmwt = if sample_mode { 0x100 } else { 0x214 };
                m.flow_mode = FLOW_MODE_MPT_116;
                is_mpt_116 = true;
                "ModPlug Tracker 1.16".to_string()
            } else if h.cmwt == 0x0200 && h.cwt == 0x0202 && pat_before_smp {
                h.cmwt = if sample_mode { 0x100 } else { 0x200 };
                m.flow_mode = FLOW_MODE_MPT_116;
                is_mpt_116 = true;
                "ModPlug Tracker 1.0 pre-alpha".to_string()
            } else if h.cwt == 0x0216 {
                "Impulse Tracker 2.14v3".to_string()
            } else if h.cwt == 0x0217 {
                "Impulse Tracker 2.14v5".to_string()
            } else if h.cwt == 0x0214 && &h.rsvd == b"CHBI" {
                "Chibi Tracker".to_string()
            } else {
                if h.cwt < 0x104 {
                    m.flow_mode = FLOW_MODE_IT_100;
                } else if h.cwt < 0x200 {
                    m.flow_mode = FLOW_MODE_IT_104;
                } else if h.cwt < 0x210 {
                    m.flow_mode = FLOW_MODE_IT_200;
                }
                format!("Impulse Tracker {}.{:02x}", (h.cwt & 0x0f00) >> 8, h.cwt & 0xff)
            }
        }
        0x08 | 0x7f => {
            if h.cwt == 0x0888 {
                m.flow_mode = FLOW_MODE_MPT_116;
                is_mpt_116 = true;
                "OpenMPT 1.17".to_string()
            } else if h.cwt == 0x7fff {
                "munch.py".to_string()
            } else {
                format!("unknown ({:04x})", h.cwt)
            }
        }
        _ => match h.cwt >> 12 {
            0x1 => schism_tracker_string((h.cwt & 0x0fff) as i32, u32::from_le_bytes(h.rsvd) as i32),
            0x5 => {
                let mut s = format!("OpenMPT {}.{:02x}", (h.cwt & 0x0f00) >> 8, h.cwt & 0xff);
                if &h.rsvd != b"OMPT" {
                    s += " (compat.)";
                }
                s
            }
            0x6 => format!("BeRoTracker {}.{:02x}", (h.cwt & 0x0f00) >> 8, h.cwt & 0xff),
            _ => format!("unknown ({:04x})", h.cwt),
        },
    };
    // char tracker_name[40]
    let tracker_name: String = tracker_name.chars().take(39).collect();
    m.module.type_ = format!("{} IT {}.{:02x}", tracker_name, h.cmwt >> 8, h.cmwt & 0xff);
    is_mpt_116
}

/// `load_old_it_instrument`: an IT 1.x instrument.
fn load_old_it_instrument(xxi: &mut Instrument, f: &mut Hio) -> Result<(), ()> {
    let mut buf = [0u8; 64];
    if f.read(&mut buf) != 64 {
        return Err(());
    }
    if u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) != MAGIC_IMPI {
        return Err(());
    }
    let flags = buf[17];
    let (vls, vle, sls, sle) = (buf[18], buf[19], buf[20], buf[21]);
    let fadeout = rd16(&buf, 24);
    let nna = buf[26];
    let dnc = buf[27];
    let mut keys = [0u8; 240];
    let mut epoint = [0u8; 200];
    let mut enode = [0u8; 50];
    if f.read(&mut keys) != 240 {
        return Err(());
    }
    if f.read(&mut epoint) != 200 {
        return Err(());
    }
    if f.read(&mut enode) != 50 {
        return Err(());
    }
    xxi.name = it_name(&buf[32..58], 25);
    xxi.rls = (fadeout as i32) << 7;
    xxi.aei.flg = 0;
    if flags & IT_ENV_ON != 0 {
        xxi.aei.flg |= XMP_ENVELOPE_ON;
    }
    if flags & IT_ENV_LOOP != 0 {
        xxi.aei.flg |= XMP_ENVELOPE_LOOP;
    }
    if flags & IT_ENV_SLOOP != 0 {
        xxi.aei.flg |= XMP_ENVELOPE_SUS | XMP_ENVELOPE_SLOOP;
    }
    if flags & IT_ENV_CARRY != 0 {
        xxi.aei.flg |= XMP_ENVELOPE_SUS | XMP_ENVELOPE_CARRY;
    }
    xxi.aei.lps = vls as i32;
    xxi.aei.lpe = vle as i32;
    xxi.aei.sus = sls as i32;
    xxi.aei.sue = sle as i32;

    let mut k = 0;
    while k < 25 && enode[k * 2] != 0xff {
        k += 1;
    }
    if k >= 25 || enode[k * 2] != 0xff {
        return Err(());
    }
    xxi.aei.npt = k as i32;
    for j in 0..k {
        xxi.aei.data[j * 2] = enode[j * 2] as i16;
        xxi.aei.data[j * 2 + 1] = enode[j * 2 + 1] as i16;
    }

    let mut inst_map = [0xffu8; 255];
    let mut inst_rmap = [0u8; XMP_MAX_KEYS];
    let mut k = 0usize;
    for j in 0..XMP_MAX_KEYS {
        let c = if j < 120 { keys[j * 2 + 1] as i32 - 1 } else { -1 };
        if c < 0 {
            xxi.map[j].ins = 0;
            xxi.map[j].xpo = 0;
            continue;
        }
        let c = c as usize;
        if inst_map[c] == 0xff {
            inst_map[c] = k as u8;
            inst_rmap[k] = c as u8;
            k += 1;
        }
        xxi.map[j].ins = inst_map[c];
        xxi.map[j].xpo = (keys[j * 2] as i32 - j as i32) as i8;
    }
    xxi.nsm = k as i32;
    xxi.vol = 0x40;
    xxi.sub = (0..k)
        .map(|j| SubInstrument {
            sid: inst_rmap[j] as i32,
            nna: nna as i32,
            dct: if dnc != 0 { XMP_INST_DCT_NOTE } else { XMP_INST_DCT_OFF },
            dca: XMP_INST_DCA_CUT,
            pan: XMP_INST_NO_DEFAULT_PAN,
            ..Default::default()
        })
        .collect();
    Ok(())
}

/// `load_new_it_instrument`: an IT 2.x instrument.
fn load_new_it_instrument(xxi: &mut Instrument, f: &mut Hio) -> Result<(), ()> {
    const DCA2NNA: [i32; 4] = [0, 2, 3, 3];
    let mut buf = [0u8; 64];
    if f.read(&mut buf) != 64 {
        return Err(());
    }
    if u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) != MAGIC_IMPI {
        return Err(());
    }
    let nna = buf[17];
    let dct = buf[18];
    let mut dca = buf[19];
    if dca > 3 {
        dca = 0;
    }
    let fadeout = rd16(&buf, 20);
    let gbv = buf[24];
    let dfp = buf[25];
    let rv = buf[26];
    let rp = buf[27];
    let ifc = buf[58];
    let ifr = buf[59];
    let mut keys = [0u8; 240];
    if f.read(&mut keys) != 240 {
        return Err(());
    }
    xxi.name = it_name(&buf[32..58], 25);
    xxi.rls = (fadeout as i32) << 6;

    read_envelope(&mut xxi.aei, f)?;
    read_envelope(&mut xxi.pei, f)?;
    let (env_flg, env_num) = read_envelope(&mut xxi.fei, f)?;

    if xxi.pei.flg & XMP_ENVELOPE_ON != 0 {
        for j in 0..xxi.pei.npt as usize {
            xxi.pei.data[j * 2 + 1] += 32;
        }
    }
    if xxi.aei.flg & XMP_ENVELOPE_ON != 0 && xxi.aei.npt == 0 {
        xxi.aei.npt = 1;
    }
    if xxi.pei.flg & XMP_ENVELOPE_ON != 0 && xxi.pei.npt == 0 {
        xxi.pei.npt = 1;
    }
    if xxi.fei.flg & XMP_ENVELOPE_ON != 0 && xxi.fei.npt == 0 {
        xxi.fei.npt = 1;
    }
    if env_flg & IT_ENV_FILTER != 0 {
        xxi.fei.flg |= XMP_ENVELOPE_FLT;
        for j in 0..env_num {
            let v = &mut xxi.fei.data[j * 2 + 1];
            *v = v.wrapping_add(32);
            *v = v.wrapping_mul(4);
        }
    } else {
        // the pitch envelope *50 for a fine interpolation
        for j in 0..env_num {
            let v = &mut xxi.fei.data[j * 2 + 1];
            *v = v.wrapping_mul(50);
        }
    }

    let mut inst_map = [0xffu8; 255];
    let mut inst_rmap = [0u8; XMP_MAX_KEYS];
    let mut k = 0usize;
    for j in 0..120 {
        let c = keys[j * 2 + 1] as i32 - 1;
        if c < 0 {
            xxi.map[j].ins = 0xff;
            xxi.map[j].xpo = 0;
            continue;
        }
        let c = c as usize;
        if inst_map[c] == 0xff {
            inst_map[c] = k as u8;
            inst_rmap[k] = c as u8;
            k += 1;
        }
        xxi.map[j].ins = inst_map[c];
        xxi.map[j].xpo = (keys[j * 2] as i32 - j as i32) as i8;
    }
    xxi.nsm = k as i32;
    xxi.vol = (gbv.min(128) >> 1) as i32;
    xxi.sub = (0..k)
        .map(|j| SubInstrument {
            sid: inst_rmap[j] as i32,
            nna: nna as i32,
            dct: dct as i32,
            dca: DCA2NNA[dca as usize],
            pan: if dfp & 0x80 != 0 { XMP_INST_NO_DEFAULT_PAN } else { dfp as i32 * 4 },
            ifc: ifc as i32,
            ifr: ifr as i32,
            rvv: ((rp as i32) << 8) | rv as i32,
            ..Default::default()
        })
        .collect();
    Ok(())
}

fn force_sample_length(xxs: &mut Sample, xtra: &mut ExtraSample, len: i32) {
    xxs.len = len;
    if xxs.lpe > xxs.len {
        xxs.lpe = xxs.len;
    }
    if xxs.lps >= xxs.len {
        xxs.flg &= !XMP_SAMPLE_LOOP;
    }
    if xtra.sue > xxs.len {
        xtra.sue = xxs.len;
    }
    if xtra.sus >= xxs.len {
        xxs.flg &= !(XMP_SAMPLE_SLOOP | XMP_SAMPLE_SLOOP_BIDIR);
    }
}

/// `unpack_it_sample`: the decompressed sample (per channel, not interleaved),
/// as little-endian bytes.
fn unpack_it_sample(len: usize, flags: u8, convert: u8, tmpbuf: &mut [u8], f: &mut Hio) -> Vec<u8> {
    let channels = if flags & IT_SMP_STEREO != 0 { 2 } else { 1 };
    let it215 = convert & IT_CVT_DIFF != 0;
    if flags & IT_SMP_16BIT != 0 {
        let mut dec = vec![0i16; len * channels];
        for i in 0..channels {
            itsex::decompress16(f, &mut dec[i * len..(i + 1) * len], len, tmpbuf, it215);
        }
        dec.iter().flat_map(|v| v.to_le_bytes()).collect()
    } else {
        let mut dec = vec![0u8; len * channels];
        for i in 0..channels {
            itsex::decompress8(f, &mut dec[i * len..(i + 1) * len], len, tmpbuf, it215);
        }
        dec
    }
}

fn load_it_sample(m: &mut ModuleData, i: usize, start: usize, sample_mode: bool, tmpbuf: &mut [u8], f: &mut Hio) -> Result<(), ()> {
    if sample_mode {
        m.module.xxi[i].sub = vec![SubInstrument::default()];
    }
    let mut buf = [0u8; 80];
    if f.read(&mut buf) != 80 {
        return Err(());
    }
    // not an error, for use-brdg.it and use-funk.it (IT 2.04)
    if u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) != MAGIC_IMPS {
        return Ok(());
    }
    let gvl = buf[17];
    let flags = buf[18];
    let vol = buf[19];
    let convert = buf[46];
    let dfp = buf[47];
    let length = rd32(&buf, 48);
    let loopbeg = rd32(&buf, 52);
    let loopend = rd32(&buf, 56);
    let c5spd = rd32(&buf, 60);
    let sloopbeg = rd32(&buf, 64);
    let sloopend = rd32(&buf, 68);
    let sample_ptr = rd32(&buf, 72);
    let (vis, vid, vir, vit) = (buf[76], buf[77], buf[78], buf[79]);
    let name = it_name(&buf[20..46], 25);

    {
        let xxs = &mut m.module.xxs[i];
        if flags & IT_SMP_16BIT != 0 {
            xxs.flg = XMP_SAMPLE_16BIT;
        }
        if flags & IT_SMP_STEREO != 0 {
            xxs.flg |= XMP_SAMPLE_STEREO;
        }
        xxs.len = length as i32;
        xxs.lps = loopbeg as i32;
        xxs.lpe = loopend as i32;
        if flags & IT_SMP_LOOP != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP;
        }
        if flags & IT_SMP_BLOOP != 0 {
            xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
        }
        if flags & IT_SMP_SLOOP != 0 {
            xxs.flg |= XMP_SAMPLE_SLOOP;
        }
        if flags & IT_SMP_BSLOOP != 0 {
            xxs.flg |= XMP_SAMPLE_SLOOP_BIDIR;
        }
    }
    if flags & IT_SMP_SLOOP != 0 {
        m.xtra[i].sus = sloopbeg as i32;
        m.xtra[i].sue = sloopend as i32;
    }

    if sample_mode {
        let has = m.module.xxs[i].len != 0;
        let xxi = &mut m.module.xxi[i];
        xxi.vol = 64;
        xxi.sub[0].vol = vol as i32;
        xxi.sub[0].pan = XMP_INST_NO_DEFAULT_PAN;
        xxi.sub[0].sid = i as i32;
        xxi.nsm = has as i32;
        xxi.name = name;
    } else {
        m.module.xxs[i].name = name;
    }

    // c5spd is a sample attribute: every subinstrument playing it gets the transpose
    let (xpo, fin) = c2spd_to_note(c5spd as i32);
    for j in 0..m.module.ins {
        let xxi = &mut m.module.xxi[j];
        for k in 0..(xxi.nsm.max(0) as usize).min(xxi.sub.len()) {
            let sub = &mut xxi.sub[k];
            if sub.sid == i as i32 {
                sub.vol = vol as i32;
                sub.gvl = gvl.min(64) as i32;
                sub.vra = vis as i32;
                sub.vde = (vid as i32) << 1;
                sub.vwf = vit as i32;
                sub.vsw = (0xff - vir as i32) >> 1;
                sub.xpo = xpo;
                sub.fin = fin;
                if dfp & 0x80 != 0 {
                    sub.pan = (dfp & 0x7f) as i32 * 4;
                } else if sample_mode {
                    sub.pan = XMP_INST_NO_DEFAULT_PAN;
                }
            }
        }
    }

    if flags & IT_SMP_SAMPLE != 0 && m.module.xxs[i].len > 1 {
        let mut cvt = 0;
        if m.module.xxs[i].len > MAX_SAMPLE_SIZE {
            return Err(());
        }
        f.seek(start + sample_ptr as usize);
        {
            let xxs = &mut m.module.xxs[i];
            if xxs.lpe > xxs.len || xxs.lps >= xxs.lpe {
                xxs.flg &= !XMP_SAMPLE_LOOP;
            }
        }
        if convert == IT_CVT_ADPCM {
            cvt |= SAMPLE_FLAG_ADPCM;
        }
        if !convert & IT_CVT_SIGNED != 0 {
            cvt |= SAMPLE_FLAG_UNS;
        }
        if flags & IT_SMP_COMP != 0 {
            let mut samples = m.module.xxs[i].len as i64;
            if flags & IT_SMP_STEREO != 0 {
                samples <<= 1;
            }
            let file_len = f.size() as i64;
            let min_size = samples >> 3;
            let left = file_len - sample_ptr as i64;
            if left <= 0 {
                return Ok(());
            }
            if file_len > 0 && left < min_size {
                let (xxs, xtra) = (&mut m.module.xxs[i], &mut m.xtra[i]);
                force_sample_length(xxs, xtra, (left << 3) as i32);
            }
            let len = m.module.xxs[i].len as usize;
            let decbuf = unpack_it_sample(len, flags, convert, tmpbuf, f);
            load_sample(m, f, SAMPLE_FLAG_NOLOAD | cvt, i, Some(&decbuf))?;
        } else {
            load_sample(m, f, cvt, i, None)?;
        }
    }
    Ok(())
}

/// `load_it_pattern`: pattern `i` (its offset already sought) into its tracks.
fn load_it_pattern(m: &mut ModuleData, i: usize, new_fx: bool, patbuf: &mut [u8], f: &mut Hio) -> Result<(), ()> {
    let md = &mut m.module;
    let mut last_fxp = [0u8; 64];
    let mut lastevent = [Event::default(); L_CHANNELS];
    let mut dummy = Event::default();
    let mut mask = [0u8; L_CHANNELS];

    let pat_len = f.read16l() as usize;
    let num_rows = f.read16l() as usize;
    if num_rows == 0 {
        return Err(());
    }
    let chn = md.chn;
    for c in 0..chn {
        md.xxt[i * chn + c] = Track::new(num_rows);
    }
    md.xxp[i] = Pattern { rows: num_rows, index: (i * chn..(i + 1) * chn).collect() };
    f.read16l();
    f.read16l();
    if f.read(&mut patbuf[..pat_len]) < pat_len {
        return Err(());
    }
    let end = pat_len;
    let mut pos = 0usize;
    let mut r = 0usize;
    while r < num_rows && pos < end {
        let mut b = patbuf[pos];
        pos += 1;
        if b == 0 {
            r += 1;
            continue;
        }
        let c = ((b as usize).wrapping_sub(1)) & 63;
        if b & 0x80 != 0 {
            if pos >= end {
                break;
            }
            mask[c] = patbuf[pos];
            pos += 1;
        }
        let event: &mut Event = if c >= chn { &mut dummy } else { &mut md.xxt[i * chn + c].event[r] };
        if mask[c] & 0x0f != 0 {
            if mask[c] & 0x01 != 0 {
                if pos >= end {
                    break;
                }
                b = patbuf[pos];
                pos += 1;
                b = match b {
                    0xff => XMP_KEY_OFF,
                    0xfe => XMP_KEY_CUT,
                    _ if b > 119 => XMP_KEY_FADE,
                    _ => b + 1,
                };
                event.note = b;
                lastevent[c].note = b;
            }
            if mask[c] & 0x02 != 0 {
                if pos >= end {
                    break;
                }
                b = patbuf[pos];
                pos += 1;
                event.ins = b;
                lastevent[c].ins = b;
            }
            if mask[c] & 0x04 != 0 {
                if pos >= end {
                    break;
                }
                b = patbuf[pos];
                pos += 1;
                event.vol = b;
                lastevent[c].vol = b;
                xlat_volfx(event);
            }
            if mask[c] & 0x08 != 0 {
                if pos + 1 >= end {
                    break;
                }
                b = patbuf[pos];
                pos += 1;
                if b as usize >= FX.len() {
                    pos += 1;
                } else {
                    event.fxt = b;
                    event.fxp = patbuf[pos];
                    pos += 1;
                    xlat_fx(c, event, &mut last_fxp, new_fx);
                    lastevent[c].fxt = event.fxt;
                    lastevent[c].fxp = event.fxp;
                }
            }
        }
        if mask[c] & 0xf0 == 0 {
            continue;
        }
        if mask[c] & 0x10 != 0 {
            event.note = lastevent[c].note;
        }
        if mask[c] & 0x20 != 0 {
            event.ins = lastevent[c].ins;
        }
        if mask[c] & 0x40 != 0 {
            event.vol = lastevent[c].vol;
            xlat_volfx(event);
        }
        if mask[c] & 0x80 != 0 {
            event.fxt = lastevent[c].fxt;
            event.fxp = lastevent[c].fxp;
        }
    }
    Ok(())
}

/// The header fields `identify_tracker` looks at (`struct it_file_header`).
struct Header {
    hilite_min: u8,
    hilite_maj: u8,
    ordnum: u16,
    insnum: u16,
    patnum: u16,
    cwt: u16,
    cmwt: u16,
    flags: u16,
    special: u16,
    gv: u8,
    mv: u8,
    is: u8,
    sep: u8,
    pwd: u8,
    msglen: u16,
    msgofs: u32,
    rsvd: [u8; 4],
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    load_at(m, f, 0)
}

/// `it_load` for a module that starts at `start` in `f`.
pub fn load_at(m: &mut ModuleData, f: &mut Hio, start: usize) -> Result<(), ()> {
    f.seek(start);
    if f.read32b() != 0x494d_504d {
        return Err(());
    }
    let mut name = [0u8; 26];
    f.read(&mut name);
    let hilite_min = f.read8();
    let hilite_maj = f.read8();
    let ordnum = f.read16l();
    let insnum = f.read16l();
    let smpnum = f.read16l();
    let patnum = f.read16l();
    let cwt = f.read16l();
    let cmwt = f.read16l();
    let flags = f.read16l();
    let special = f.read16l();
    let gv = f.read8();
    let mv = f.read8();
    let is = f.read8();
    let it = f.read8();
    let sep = f.read8();
    let pwd = f.read8();
    if gv > 0x80 {
        return Err(());
    }
    let msglen = f.read16l();
    let msgofs = f.read32l();
    let mut rsvd = [0u8; 4];
    f.read(&mut rsvd);
    let mut chpan = [0u8; 64];
    let mut chvol = [0u8; 64];
    f.read(&mut chpan);
    f.read(&mut chvol);
    if f.take_error() {
        return Err(());
    }
    let mut h = Header {
        hilite_min,
        hilite_maj,
        ordnum,
        insnum,
        patnum,
        cwt,
        cmwt,
        flags,
        special,
        gv,
        mv,
        is,
        sep,
        pwd,
        msglen,
        msgofs,
        rsvd,
    };

    m.module.name = String::from_utf8_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(26)]).into_owned();
    let mut len = ordnum as usize;
    m.module.ins = insnum as usize;
    m.module.smp = smpnum as usize;
    m.module.pat = patnum as usize;
    if m.module.ins > 255 || m.module.smp > 255 || m.module.pat > 255 {
        return Err(());
    }
    m.module.spd = is as i32;
    m.module.bpm = it as i32;
    let sample_mode = !flags & IT_USE_INST != 0;
    if flags & IT_LINEAR_FREQ != 0 {
        m.period_type = PeriodType::Linear;
    }
    for i in 0..64 {
        let xxc = &mut m.module.xxc[i];
        let pan = (chpan[i] & 0x7f) as i32;
        if pan == 100 {
            xxc.flg |= XMP_CHANNEL_SURROUND;
        }
        if chpan[i] & 0x80 != 0 {
            xxc.flg |= XMP_CHANNEL_MUTE;
        }
        if flags & IT_STEREO != 0 {
            xxc.pan = (pan * 0x80 >> 5).min(0xff);
        } else {
            xxc.pan = 0x80;
        }
        xxc.vol = chvol[i] as i32;
    }
    if len <= XMP_MAX_MOD_LENGTH {
        f.read(&mut m.module.xxo[..len]);
    } else {
        f.read(&mut m.module.xxo[..]);
        f.seek_cur((len - XMP_MAX_MOD_LENGTH) as i64);
        len = XMP_MAX_MOD_LENGTH;
    }
    m.module.len = len;
    let new_fx = flags & IT_OLD_FX == 0;

    let pp_ins: Vec<u32> = (0..m.module.ins).map(|_| f.read32l()).collect();
    let pp_smp: Vec<u32> = (0..m.module.smp).map(|_| f.read32l()).collect();
    let mut pp_pat: Vec<u32> = (0..m.module.pat).map(|_| f.read32l()).collect();

    if special & IT_EDIT_HISTORY != 0 {
        let skip = f.read16l() as i64 * 8;
        if f.take_error() || (skip != 0 && !f.seek_cur(skip)) {
            return Err(());
        }
    }
    if flags & IT_MIDI_CONFIG != 0 || special & IT_SPEC_MIDICFG != 0 {
        load_it_midi_config(m, f)?;
    }
    let pat_before_smp = m.module.smp != 0 && m.module.pat != 0 && pp_pat[0] != 0 && pp_pat[0] < pp_smp[0];

    m.c4rate = C4_NTSC_RATE;
    let is_mpt_116 = identify_tracker(m, &mut h, pat_before_smp);

    if sample_mode {
        m.module.ins = m.module.smp;
    }
    m.init_instrument();

    for i in 0..m.module.ins {
        if !sample_mode {
            f.seek(start + pp_ins[i] as usize);
            if h.cmwt >= 0x200 {
                load_new_it_instrument(&mut m.module.xxi[i], f)?;
            } else {
                load_old_it_instrument(&mut m.module.xxi[i], f)?;
            }
        }
    }

    let mut patbuf = vec![0u8; TEMP_BUFFER_LEN];
    for i in 0..m.module.smp {
        f.seek(start + pp_smp[i] as usize);
        load_it_sample(m, i, start, sample_mode, &mut patbuf, f)?;
    }
    f.take_error();

    // effects in muted channels are processed: the real channel count first
    let mut max_ch = 0usize;
    for i in 0..m.module.pat {
        const BYTES_IN_PACKED_EVENT: [usize; 16] = [0, 1, 1, 2, 1, 2, 2, 3, 2, 3, 3, 4, 3, 4, 4, 5];
        if pp_pat[i] == 0 {
            continue;
        }
        f.seek(start + pp_pat[i] as usize);
        let pat_len = f.read16l() as usize;
        let num_rows = f.read16l() as usize;
        let mut mask = [0u8; L_CHANNELS];
        f.read16l();
        f.read16l();
        if num_rows > 1024 {
            pp_pat[i] = 0;
            continue;
        }
        if f.read(&mut patbuf[..pat_len]) < pat_len {
            return Err(());
        }
        let (mut pos, end) = (0usize, pat_len);
        let mut row = 0;
        while row < num_rows && pos < end {
            let b = patbuf[pos];
            pos += 1;
            if b == 0 {
                row += 1;
                continue;
            }
            let c = ((b as usize).wrapping_sub(1)) & 63;
            if c > max_ch {
                max_ch = c;
            }
            if b & 0x80 != 0 {
                if pos >= end {
                    break;
                }
                mask[c] = patbuf[pos] & 0x0f;
                pos += 1;
            }
            pos += BYTES_IN_PACKED_EVENT[mask[c] as usize];
        }
    }
    m.module.chn = max_ch + 1;
    m.module.trk = m.module.pat * m.module.chn;
    m.module.xxt = vec![Track::default(); m.module.trk];
    m.module.xxp = vec![Pattern::default(); m.module.pat];

    for i in 0..m.module.pat {
        if pp_pat[i] == 0 {
            let chn = m.module.chn;
            for j in 0..chn {
                m.module.xxt[i * chn + j] = Track::new(64);
            }
            m.module.xxp[i] = Pattern { rows: 64, index: (i * chn..(i + 1) * chn).collect() };
            continue;
        }
        f.seek(start + pp_pat[i] as usize);
        load_it_pattern(m, i, new_fx, &mut patbuf, f)?;
    }

    m.quirk |= QUIRKS_IT | QUIRK_ARPMEM | QUIRK_INSVOL;
    if flags & IT_LINK_GXX != 0 {
        m.quirk |= QUIRK_PRENV;
    } else {
        m.quirk |= QUIRK_UNISLD;
    }
    if new_fx {
        m.quirk |= QUIRK_VIBHALF | QUIRK_VIBINV;
    } else {
        m.quirk &= !QUIRK_VIBALL;
        m.quirk |= QUIRK_ITOLDFX;
    }
    if sample_mode {
        m.quirk &= !(QUIRK_VIRTUAL | QUIRK_RSTCHN);
    }
    m.gvolbase = 0x80;
    m.gvol = gv as i32;
    m.mvolbase = 48;
    m.mvol = mv as i32;
    m.read_event_type = ReadEvent::It;
    if is_mpt_116 {
        apply_mpt_preamp(m);
    }
    Ok(())
}

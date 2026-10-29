//! Digital Tracker / Digital Home Studio modules (`loaders/dt_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::iff::{self, IffLoader};
use crate::xmp::load::*;
use crate::xmp::module::*;

// Values to compare against version_derived for compatibility. These don't
// directly correspond to real values in the format.
const DTM_V2015: i32 = 2015; // Digital Tracker 2.015 and 2.02
const DTM_V203: i32 = 2030; // Digital Tracker 2.03
const DTM_V204: i32 = 2040; // Digital Tracker 2.04 thru 1.9
const DTM_V19: i32 = 19 << 8; // Digital Tracker 1.9 (later vers)
const DTM_V21: i32 = 21 << 8; // Digital Home Studio

// Pattern format versions.
const FORMAT_MOD: u32 = 0;
const FORMAT_V204: u32 = u32::from_be_bytes(*b"2.04");
const FORMAT_V206: u32 = u32::from_be_bytes(*b"2.06");

// "Mono" mode has always been panoramic stereo.
const DTM_OLD_STEREO: i32 = 0x00;
const DTM_PANORAMIC_STEREO: i32 = 0xff;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 4 && &d[..4] == b"D.T."
}

#[derive(Default)]
struct Local {
    vers_flag: bool,
    patt_flag: bool,
    sv19_flag: bool,
    pflag: bool,
    sflag: bool,
    stereo: i32,
    depth: i32,
    c2spd: i32,
    version: u32,
    version_derived: i32,
    format: u32,
    last_pat: usize,
    insnum: usize,
    chn: usize,
}

fn translate_effect(event: &mut Event, data: &Local) {
    match event.fxt {
        // Protracker compatible
        0x1 | 0x2 | 0x3 | 0x4 | 0x5 | 0x6 | 0x7 | 0x9 | 0xa | 0xc => {}

        0x0 => {
            // arpeggio: DT beta through 2.04 does nothing.
            if data.version_derived <= DTM_V203 {
                event.fxp = 0;
            }
        }

        0x8 => {
            // set panning: DT 2.04+, only supported in panoramic stereo mode.
            if data.version_derived >= DTM_V204 && data.stereo == DTM_PANORAMIC_STEREO {
                if data.version_derived >= DTM_V19 {
                    event.fxp ^= 0xff;
                } else {
                    let left = msn(event.fxp) as i32;
                    let right = lsn(event.fxp) as i32;
                    let pan = 0x80 * (right - left) / (left + right + 2);
                    let vol = if right > left {
                        0x80 * ((right + 1) << 2) / (0x80 + pan)
                    } else if left > right {
                        0x80 * ((left + 1) << 2) / (0x80 - pan)
                    } else {
                        (left + 1) << 2
                    };
                    event.fxt = FX_SETPAN;
                    event.fxp = (pan + 0x80) as u8;
                    event.f2t = FX_TRK_VOL;
                    event.f2p = vol as u8;
                }
            } else {
                event.fxt = 0;
                event.fxp = 0;
            }
        }

        0xb => {
            // position jump
            if data.version_derived <= DTM_V203 && data.chn > 4 {
                // DT 2.04 and under: in 6 and 8 channel modules, the break
                // position is somewhere in the middle of a pattern unless
                // followed by a Dxx.
                event.f2t = FX_BREAK;
                event.f2p = 0x32;
            } else if data.version_derived == DTM_V204 {
                // DT commercial 1.0 through 1.2: effect is off-by-one.
                if event.fxp > 0 {
                    event.fxp -= 1;
                }
            }
        }

        0xd => {
            // pattern break: DT beta through commercial 1.2 ignore the parameter.
            if data.version_derived <= DTM_V203 {
                event.fxp = 0;
            }
        }

        0xf => {
            // set speed/tempo
            if data.version_derived <= DTM_V203 && (event.fxp == 0 || event.fxp == 0x20) {
                event.fxp = 1;
            }
        }

        0xe => match msn(event.fxp) {
            // Protracker compatible
            0x1 | 0x2 | 0x3 | 0x6 | 0xa | 0xb | 0xc | 0xd | 0xe => {}

            0x4 => {
                // vibrato waveform. DT 2.04 and up: sine rampup(period) square square
                if event.fxp & 3 == 3 {
                    event.fxp -= 1;
                }
                // Continuous variants are missing from all versions.
                event.fxp &= 3;
            }

            0x5 => {
                // set finetune. DT 2.04: only works if a note is present
                if event.note == 0 {
                    event.fxt = 0;
                    event.fxp = 0;
                }
            }

            _ => {
                event.fxt = 0;
                event.fxp = 0;
            }
        },

        _ => {
            event.fxt = 0;
            event.fxp = 0;
        }
    }
}

fn event_size(format: u32) -> Option<usize> {
    match format {
        FORMAT_MOD | FORMAT_V204 => Some(4),
        FORMAT_V206 => Some(6),
        _ => None,
    }
}

fn translate_event(event: &mut Event, input: &[u8], data: &Local) {
    match data.format {
        FORMAT_MOD => decode_protracker_event(event, input),
        FORMAT_V204 => {
            if input[0] > 0 && input[0] <= 0x7c {
                event.note = 12 * (input[0] >> 4) + (input[0] & 0x0f) + 12;
            }
            event.vol = (input[1] & 0xfc) >> 2;
            event.ins = ((input[1] & 0x03) << 4) + (input[2] >> 4);
            event.fxt = input[2] & 0xf;
            event.fxp = input[3];
        }
        // FIXME Digital Home Studio
        _ => {}
    }

    translate_effect(event, data);
}

fn get_d_t_(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    if !(14..=142).contains(&size) {
        return Err(());
    }
    let mut name_len = (size - 14) as usize;

    f.read16b(); // type
    data.stereo = f.read8() as i32; // 0:old stereo, ff:panoramic stereo
    data.depth = f.read8() as i32; // global sample bit depth (pre-2.04)
    f.read16b(); // reserved
    m.module.spd = f.read16b() as i32;
    let b = f.read16b() as i32;
    if b > 0 {
        // RAMBO.DTM has bpm 0. Not clamped by Digital Tracker.
        m.module.bpm = b;
    }
    data.c2spd = f.read32b() as i32; // global sample rate (pre-2.04)

    if data.stereo != DTM_OLD_STEREO && data.stereo != DTM_PANORAMIC_STEREO {
        data.stereo = DTM_OLD_STEREO;
    }

    // Global sample depth is applied to all samples pre-2.04. DT 2.015 and 2.02
    // will save a value of 0 in this field.
    if data.depth != 0 && data.depth != 8 && data.depth != 16 {
        data.depth = 8;
    }

    // Only known used values for global sample rate are 8400 and 24585, but
    // 12292 and 49170 are also supported and referenced in the UI.
    match data.c2spd {
        49170 | 24585 | 12292 | 8400 => {}
        _ => data.c2spd = 8400,
    }

    name_len = name_len.min(XMP_NAME_SIZE);
    let mut name = vec![0u8; name_len];
    f.read(&mut name);
    m.module.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(name_len)]).into_owned();
    m.module.type_ = "Digital Tracker DTM".to_string();

    Ok(())
}

fn get_s_q_(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if m.module.pat != 0 {
        return Err(());
    }

    m.module.len = f.read16b() as usize;
    m.module.rst = f.read16b() as usize;

    // Sanity check
    if m.module.len > 256 || m.module.rst > 255 {
        return Err(());
    }

    f.read32b(); // reserved

    let mut maxpat = 0;
    for i in 0..128 {
        m.module.xxo[i] = f.read8();
        if m.module.xxo[i] > maxpat {
            maxpat = m.module.xxo[i];
        }
    }
    m.module.pat = maxpat as usize + 1;

    Ok(())
}

fn get_vers(_id: &[u8], size: u32, _m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    if data.vers_flag || size < 4 {
        return Ok(());
    }
    data.vers_flag = true;

    data.version = f.read32b();
    Ok(())
}

fn get_sv19(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    if !data.vers_flag || data.version < 19 || size < 86 {
        // ignore in the extreme off chance a module with this exists.
        return Ok(());
    }
    data.sv19_flag = true;

    f.read16b(); // ticks per beat, by default 24
    let bpm_frac = f.read32b(); // initial BPM (fractional portion)

    // Round up to nearest for now.
    if bpm_frac >= 0x8000_0000 {
        m.module.bpm += 1;
    }

    // Initial pan table. 0=left 90=center 180=right.
    // Do not load in old stereo mode for now.
    let mut buf = [0u8; 64];
    if f.read(&mut buf) < 64 {
        return Err(());
    }
    for i in 0..32 {
        let val = u16::from_be_bytes([buf[2 * i], buf[2 * i + 1]]) as i32;
        if val <= 180 && data.stereo == DTM_PANORAMIC_STEREO {
            m.module.xxc[i].pan = val * 0x100 / 180;
        }
    }

    Ok(())
}

fn get_patt(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if data.patt_flag || data.pflag || size < 8 {
        return Err(());
    }
    data.patt_flag = true;

    let chn = f.read16b() as usize;
    data.chn = chn;
    m.module.chn = chn;
    f.read16b(); // realpat
    let mut ver = [0u8; 4];
    if f.read(&mut ver) < 4 {
        return Err(());
    }
    data.format = u32::from_be_bytes(ver);
    m.module.trk = m.module.chn * m.module.pat;

    if data.format != FORMAT_MOD {
        data.version_derived = if data.format == FORMAT_V206 { DTM_V21 } else { DTM_V204 };
    } else {
        // DTM 2.015/2.02 have depth=0 and 31 instruments instead of 63. There
        // are also modules with depth!=0 but 31 instruments.
        data.version_derived = if data.depth != 0 { DTM_V203 } else { DTM_V2015 };
    }

    if data.vers_flag && data.version != 0 {
        data.version_derived = (data.version << 8) as i32;
        m.flow_mode = FLOW_MODE_DTM_19;
    } else {
        // 6/8 channel mode's position jump bugs also affect its interactions
        // with pattern loop.
        m.flow_mode = if data.version_derived <= DTM_V204 && m.module.chn > 4 { FLOW_MODE_DTM_2015_6CH } else { FLOW_MODE_DTM_2015 };
    }

    // Sanity check
    if m.module.chn > XMP_MAX_CHANNELS {
        return Err(());
    }

    if event_size(data.format).is_none() {
        return Err(());
    }

    Ok(())
}

fn get_inst(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    // Sanity check
    if m.module.ins != 0 {
        return Err(());
    }

    let n = f.read16b() as usize;
    m.module.ins = n;
    m.module.smp = n;

    // Sanity check
    if n > MAX_INSTRUMENTS {
        return Err(());
    }

    m.init_instrument();

    for i in 0..n {
        m.module.xxi[i].sub = vec![SubInstrument::default()];

        let mut buf = [0u8; 50];
        if f.read(&mut buf) < 50 {
            return Err(());
        }

        let be32 = |b: &[u8; 50], o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as i32;

        let mut len = be32(&buf, 4);
        if len < 0 {
            len = 0;
        }
        let xxs = &mut m.module.xxs[i];
        xxs.len = len;
        m.module.xxi[i].nsm = (len > 0) as i32;
        let fine = ((buf[8] << 4) as i8) as i32; // finetune
        m.module.xxi[i].sub[0].vol = buf[9] as i32;
        m.module.xxi[i].sub[0].pan = XMP_INST_NO_DEFAULT_PAN;
        let mut repstart = be32(&buf, 10);
        let mut replen = be32(&buf, 14);

        if repstart >= len || repstart < 0 || replen < 0 {
            repstart = 0;
            replen = 0;
        }
        if replen > len - repstart {
            replen = len - repstart;
        }
        let xxs = &mut m.module.xxs[i];
        xxs.lps = repstart;
        xxs.lpe = repstart + replen;

        m.module.xxi[i].name = copy_adjust(&buf[18..], 22);

        // DT pre-2.04: global sample depth is used for playback; the sample
        // depth field seems to be for the resampler only.
        if data.version_derived <= DTM_V203 {
            buf[41] = if data.depth != 0 { data.depth as u8 } else { 8 };
        }

        let stereo = buf[40]; // stereo
        let xxs = &mut m.module.xxs[i];
        if buf[41] > 8 {
            // resolution (8, 16, or rarely 0)
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }

        if stereo != 0 {
            // TODO (libxmp): in old stereo mode, stereo samples do something
            // unusual; not emulated.
            xxs.flg |= XMP_SAMPLE_STEREO;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }
        if xxs.lpe - xxs.lps > 1 {
            xxs.flg |= XMP_SAMPLE_LOOP;
        }

        let midinote = u16::from_be_bytes([buf[42], buf[43]]) as i32; // midi note
        let mut c2spd = be32(&buf, 46); // frequency

        // DT pre-2.04: the sample rate is used for resampling only(?) and the
        // global sample rate is used for playback instead.
        if data.version_derived <= DTM_V203 {
            c2spd = data.c2spd;
        }
        let (xpo, fin) = c2spd_to_note(c2spd);
        let sub = &mut m.module.xxi[i].sub[0];
        sub.xpo = xpo;
        sub.fin = fin;

        // DT 2.03 through commercial 1.2: midi note resamples the sample and
        // changes the rate in some cases, but does nothing during normal playback.
        // DT 1.901+: changing the midi note acts as a transpose during playback.
        // Values under C-2 are clamped, over B-7 are ignored.
        if data.version_derived >= DTM_V19 && midinote < 95 {
            sub.xpo += 48 - midinote.max(24);
        }

        // It's strange that we have both c2spd and finetune
        sub.fin += fine;

        sub.sid = i as i32;
    }

    Ok(())
}

fn get_dapt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    if !data.patt_flag {
        return Err(());
    }

    if !data.pflag {
        data.pflag = true;
        data.last_pat = 0;

        // libxmp_init_pattern
        let md = &mut m.module;
        md.xxt = vec![Track::default(); md.trk];
        md.xxp = vec![Pattern { rows: 0, index: vec![0; md.chn] }; md.pat];
    }

    f.read32b(); // 0xffffffff
    let pat = f.read16b() as usize;
    let rows = f.read16b() as usize;

    // Sanity check
    // DT 2.04, all known versions of 1.9x: maximum rows is 96.
    // DHS: maximum configurable by interface is 4*99 = 396.
    if pat >= m.module.pat
        || (data.version_derived <= DTM_V19 && rows > 96)
        || (data.version_derived > DTM_V19 && rows > 396)
    {
        return Err(());
    }
    if pat < data.last_pat {
        return Err(());
    }

    // libxmp_alloc_pattern_tracks refuses 0 and more than 256 rows.
    if rows == 0 || rows > 256 {
        return Err(());
    }
    for i in data.last_pat..=pat {
        m.module.alloc_pattern_tracks(i, rows);
    }
    data.last_pat = pat + 1;

    let esize = event_size(data.format).ok_or(())?;
    let total_size = esize * rows * m.module.chn;
    let mut patbuf = vec![0u8; total_size];
    if f.read(&mut patbuf) < total_size {
        return Err(());
    }

    let mut pos = 0;
    for j in 0..rows {
        for k in 0..m.module.chn {
            let event = m.module.event_mut(pat, k, j);
            translate_event(event, &patbuf[pos..pos + esize], data);
            pos += esize;
        }
    }

    Ok(())
}

fn get_dait(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, data: &mut Local) -> Result<(), ()> {
    if !data.sflag {
        data.sflag = true;
        data.insnum = 0;
    }

    if size > 2 {
        // Sanity check
        if data.insnum >= m.module.ins {
            return Err(());
        }

        let sid = m.module.xxi[data.insnum].sub[0].sid as usize;
        load_sample(m, f, SAMPLE_FLAG_BIGEND | SAMPLE_FLAG_INTERLEAVED, sid, None)?;
    }

    data.insnum += 1;

    Ok(())
}

fn get_text(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, _data: &mut Local) -> Result<(), ()> {
    if size < 12 || (size & 1) != 0 || !m.comment.is_empty() {
        return Ok(());
    }
    f.read16b(); // type
    let len = f.read32b();
    f.read16b(); // tab
    f.read16b(); // res

    // "=$FFFF <=> length is odd"
    let skip_byte = if f.read16b() == 0xffff { 1u32 } else { 0 };

    if len != size - 12 - skip_byte {
        return Ok(());
    }
    if len == 0 {
        return Ok(());
    }
    if len as i64 > f.size() as i64 - f.tell() as i64 {
        return Ok(());
    }

    if skip_byte != 0 {
        f.read8();
    }
    let mut buf = vec![0u8; len as usize];
    let n = f.read(&mut buf);
    buf.truncate(n);
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    m.comment = buf[..end].iter().map(|&b| b as char).collect();
    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut data = Local::default();

    m.c4rate = C4_NTSC_RATE;

    let chunks: &[(&[u8; 4], IffLoader<Local>)] = &[
        (b"D.T.", get_d_t_),
        (b"S.Q.", get_s_q_),
        (b"VERS", get_vers),
        (b"PATT", get_patt),
        (b"INST", get_inst),
        (b"SV19", get_sv19),
        (b"DAPT", get_dapt),
        (b"DAIT", get_dait),
        (b"TEXT", get_text),
    ];
    iff::load(m, f, &mut data, chunks, 4, 0)?;

    // alloc remaining patterns
    if data.pflag {
        for i in data.last_pat..m.module.pat {
            m.module.alloc_pattern_tracks(i, 64);
        }
    }

    // Correct the module type now that the version fields are known.
    m.module.type_ = if data.version >= 20 {
        format!("Digital Home Studio DTM {}.{}", data.version / 10, data.version % 10)
    } else if data.version != 0 {
        format!("Digital Tracker {}.{} DTM", data.version / 10, data.version % 10)
    } else if data.format == FORMAT_V204 {
        "Digital Tracker 2.04 DTM".to_string()
    } else if data.depth != 0 {
        "Digital Tracker 2.03 DTM".to_string()
    } else {
        "Digital Tracker 2.015 DTM".to_string()
    };

    if data.version_derived >= DTM_V204 && data.stereo == DTM_PANORAMIC_STEREO {
        // Panoramic stereo mode: all channels default to center. In 1.9xx, the
        // SV19 chunk is used to specify initial values.
        if !data.sv19_flag {
            for i in 0..m.module.chn {
                m.module.xxc[i].pan = 0x80;
            }
        }
    }

    Ok(())
}

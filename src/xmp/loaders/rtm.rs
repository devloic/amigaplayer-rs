//! Real Tracker modules (`loaders/rtm_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! Real Tracker 1.x by Arnaud Hasenfratz: XM-style instruments with envelopes
//! and up to 16 delta-coded samples, packed patterns with two effect columns,
//! played with the FT2 event reader.

use crate::xmp::load::*;
use crate::xmp::module::*;
use crate::xmp::player::c2spd_to_note;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 5 && &d[..4] == b"RTMM" && d[4] == 0x20
}

/// `hio_seek`, which also clears the error flag.
fn seek(f: &mut Hio, pos: usize) {
    f.seek(pos);
    f.error = false;
}

const FX_NONE: u8 = 0xff;
const FX_EXTENDED_IT: u8 = 0xfe;
const FX_PORTA_UP_MOD: u8 = 0xf1;
const FX_PORTA_DN_MOD: u8 = 0xf2;
const FX_TONE_VSLIDE_MOD: u8 = 0xf5;
const FX_VIBRA_VSLIDE_MOD: u8 = 0xf6;
const FX_VOLSLIDE_MOD: u8 = 0xfa;

/// MOD and S3M effects have separate memories in Real Tracker; libxmp strips the
/// fine Axy/5xy/6xy slides and lets the two sets share memory.
const RTM_FX: [u8; 41] = [
    FX_ARPEGGIO,         // 0
    FX_PORTA_UP_MOD,     // 1
    FX_PORTA_DN_MOD,     // 2
    FX_TONEPORTA,        // 3
    FX_VIBRATO,          // 4
    FX_TONE_VSLIDE_MOD,  // 5
    FX_VIBRA_VSLIDE_MOD, // 6
    FX_TREMOLO,          // 7
    FX_SETPAN,           // 8 (S3M)
    FX_OFFSET,           // 9
    FX_VOLSLIDE_MOD,     // A
    FX_JUMP,             // B
    FX_VOLSET,           // C
    FX_BREAK,            // D
    FX_EXTENDED,         // E
    FX_SPEED,            // F
    FX_GLOBALVOL,        // G
    FX_GVOL_SLIDE,       // H
    FX_NONE,             // I
    FX_NONE,             // J
    FX_KEYOFF,           // K
    FX_ENVPOS,           // L
    FX_NONE,             // M MIDI controller
    FX_NONE,             // N
    FX_NONE,             // O
    FX_PANSLIDE,         // P
    FX_NONE,             // Q
    FX_MULTI_RETRIG,     // R
    FX_EXTENDED_IT,      // S (SA: high sample offset)
    FX_TREMOR,           // T
    FX_NONE,             // U
    FX_NONE,             // V MIDI controller value
    FX_NONE,             // W
    FX_XF_PORTA,         // X
    FX_NONE,             // Y
    FX_NONE,             // Z
    FX_VOLSLIDE,         // d (S3M)
    FX_PORTA_UP,         // f (S3M)
    FX_PORTA_DN,         // e (S3M)
    FX_VIBRA_VSLIDE,     // k (S3M)
    FX_S3M_SPEED,        // a (S3M)
];

fn translate_effect(fxt: &mut u8, fxp: &mut u8) {
    if *fxt as usize >= RTM_FX.len() {
        *fxt = 0;
        *fxp = 0;
        return;
    }
    *fxt = RTM_FX[*fxt as usize];
    match *fxt {
        FX_NONE => {
            *fxt = 0;
            *fxp = 0;
        }
        FX_SETPAN => {
            if *fxp == 0xa4 {
                *fxt = FX_SURROUND;
                *fxp = 1;
            } else {
                *fxp = ((*fxp as i32) << 1).min(255) as u8;
            }
        }
        FX_VOLSLIDE_MOD | FX_TONE_VSLIDE_MOD | FX_VIBRA_VSLIDE_MOD => {
            *fxt &= 0x0f;
            // no fine slides
            if lsn(*fxp) != 0 && msn(*fxp) != 0 {
                *fxp &= 0xf0;
            }
        }
        FX_PORTA_UP_MOD | FX_PORTA_DN_MOD => {
            // clamp the values that would read as fine
            *fxt &= 0x0f;
            *fxp = (*fxp).min(0xdf);
        }
        FX_EXTENDED => {
            if matches!(msn(*fxp), 0x0 | 0xf) {
                *fxt = 0;
                *fxp = 0;
            }
        }
        FX_EXTENDED_IT => {
            if msn(*fxp) == 0xa {
                *fxt = FX_HIOFFSET;
                *fxp = lsn(*fxp);
            } else {
                *fxt = 0;
                *fxp = 0;
            }
        }
        _ => {}
    }
}

/// Real Tracker's -64..64 pan into libxmp's 0..255.
fn convert_pan(pan: i8) -> i32 {
    (pan as i32 * 2 + 0x80).clamp(0, 255)
}

/// RTM envelope flags are XM's: enable, sustain, loop.
fn convert_envelope_flags(flags: u16) -> i32 {
    (flags & 0x07) as i32
}

struct ObjectHeader {
    name: [u8; 32],
    version: u16,
    header_size: u16,
}

fn read_object_header(f: &mut Hio, id: &[u8; 4]) -> Result<ObjectHeader, ()> {
    let mut hid = [0u8; 4];
    f.read(&mut hid);
    if &hid != id {
        return Err(());
    }
    if f.read8() != 0x20 {
        return Err(());
    }
    let mut name = [0u8; 32];
    if f.read(&mut name) != 32 {
        return Err(());
    }
    let _eof = f.read8();
    let version = f.read16l();
    let header_size = f.read16l();
    Ok(ObjectHeader { name, version, header_size })
}

struct RtEnvelope {
    npoint: u8,
    point: [(i32, i32); 12],
    sustain: u8,
    loopstart: u8,
    loopend: u8,
    flags: u16,
}

fn read_envelope(f: &mut Hio) -> Result<RtEnvelope, ()> {
    let npoint = f.read8();
    if npoint >= 12 {
        return Err(());
    }
    let mut point = [(0, 0); 12];
    for p in point.iter_mut() {
        *p = (f.read32l() as i32, f.read32l() as i32);
    }
    Ok(RtEnvelope { npoint, point, sustain: f.read8(), loopstart: f.read8(), loopend: f.read8(), flags: f.read16l() })
}

fn set_envelope(env: &mut Envelope, e: &RtEnvelope, pan: bool) {
    env.npt = e.npoint as i32;
    env.sus = e.sustain as i32;
    env.lps = e.loopstart as i32;
    env.lpe = e.loopend as i32;
    env.flg = convert_envelope_flags(e.flags);
    for j in 0..e.npoint as usize {
        let (x, y) = e.point[j];
        env.data[j * 2] = x as i16;
        env.data[j * 2 + 1] = if pan { 32 + y / 2 } else { y / 2 } as i16;
    }
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let oh = read_object_header(f, b"RTMM")?;
    let version = oh.version;

    let mut tracker_name = [0u8; 20];
    f.read(&mut tracker_name);
    let mut composer = [0u8; 32];
    f.read(&mut composer);
    let flags = f.read16l();
    let ntrack = f.read8() as usize;
    let ninstr = f.read8() as usize;
    let nposition = f.read16l() as usize;
    let npattern = f.read16l() as usize;
    let speed = f.read8();
    let tempo = f.read8();
    let mut panning = [0u8; 32];
    f.read(&mut panning);
    let extra_data_size = f.read32l() as usize;

    if f.error || nposition > 255 || ntrack > 32 || npattern > 255 {
        return Err(());
    }
    if version >= 0x0112 {
        // the original name
        let p = f.tell() + 32;
        seek(f, p);
    }
    for i in 0..nposition {
        // xxo is 8-bit
        m.module.xxo[i] = f.read16l() as u8;
        if m.module.xxo[i] as usize >= npattern {
            return Err(());
        }
    }

    let md = &mut m.module;
    md.name = cstr(&oh.name);
    md.type_ = format!("{} RTM {:x}.{:02x}", cstr(&tracker_name), version >> 8, version & 0xff);
    md.len = nposition;
    md.pat = npattern;
    md.chn = ntrack;
    md.trk = md.chn * md.pat;
    md.ins = ninstr;
    md.spd = speed as i32;
    md.bpm = tempo as i32;
    m.c4rate = C4_NTSC_RATE;
    m.period_type = if flags & 1 != 0 { PeriodType::Linear } else { PeriodType::Amiga };

    for i in 0..m.module.chn {
        m.module.xxc[i].pan = convert_pan(panning[i] as i8);
    }

    let mut offset = 42 + oh.header_size as usize + extra_data_size;
    for i in 0..m.module.pat {
        seek(f, offset);
        let ph = read_object_header(f, b"RTND")?;
        let _flags = f.read16l();
        let pntrack = f.read8() as usize;
        let nrows = f.read16l() as usize;
        let datasize = f.read32l() as usize;
        if pntrack > ntrack || nrows > 999 {
            return Err(());
        }
        offset += 42 + ph.header_size as usize + datasize;
        // libxmp_alloc_pattern_tracks_long
        if nrows == 0 {
            return Err(());
        }
        m.module.alloc_pattern_tracks(i, nrows);

        for r in 0..nrows {
            let mut j = 0;
            loop {
                let c = f.read8();
                if c == 0 {
                    break; // next row
                }
                if j >= pntrack {
                    return Err(());
                }
                if c & 0x01 != 0 {
                    // set track
                    j = f.read8() as usize;
                    if j >= pntrack {
                        return Err(());
                    }
                }
                let event = m.module.event_mut(i, j, r);
                if c & 0x02 != 0 {
                    event.note = f.read8().wrapping_add(1);
                    if event.note == 0xff {
                        event.note = XMP_KEY_OFF;
                    } else {
                        event.note = event.note.wrapping_add(12);
                    }
                }
                if c & 0x04 != 0 {
                    event.ins = f.read8();
                }
                if c & 0x08 != 0 {
                    event.fxt = f.read8();
                }
                if c & 0x10 != 0 {
                    event.fxp = f.read8();
                }
                if c & 0x20 != 0 {
                    event.f2t = f.read8();
                }
                if c & 0x40 != 0 {
                    event.f2p = f.read8();
                }
                if c & 0x18 != 0 {
                    translate_effect(&mut event.fxt, &mut event.fxp);
                }
                if c & 0x60 != 0 {
                    translate_effect(&mut event.f2t, &mut event.f2p);
                }
                j += 1;
            }
        }
    }

    // instruments
    seek(f, offset);
    // an estimate, the number is known at the end
    m.module.smp = MAX_SAMPLES;
    m.init_instrument();

    let mut smpnum = 0usize;
    for i in 0..m.module.ins {
        let oh = read_object_header(f, b"RTIN")?;
        m.module.xxi[i].name = copy_adjust(&oh.name, 31);
        if oh.header_size == 0 {
            continue;
        }
        let nsample = f.read8();
        let iflags = f.read16l();
        let mut table = [0u8; 120];
        if f.read(&mut table) != 120 {
            return Err(());
        }
        let vol_env = read_envelope(f)?;
        let pan_env = read_envelope(f)?;
        let vibflg = f.read8() as i8;
        let vibsweep = f.read8() as i8;
        let vibdepth = f.read8() as i8;
        let vibrate = f.read8() as i8;
        let volfade = f.read16l();
        if version >= 0x0110 {
            f.skip(4); // MIDI port, channel, program, enable
        }
        if version >= 0x0112 {
            f.skip(4); // MIDI transpose, bender range, base volume, use velocity
        }

        let volbase = m.volbase;
        let xxi = &mut m.module.xxi[i];
        xxi.nsm = (nsample as i32).min(16);
        xxi.vol = if iflags & 2 == 0 { volbase } else { 0 };
        xxi.sub = vec![SubInstrument::default(); xxi.nsm as usize];
        for j in 0..120 {
            xxi.map[j].ins = table[j];
        }
        xxi.rls = volfade as i32;
        set_envelope(&mut xxi.aei, &vol_env, false);
        set_envelope(&mut xxi.pei, &pan_env, true);

        for j in 0..m.module.xxi[i].nsm as usize {
            let sh = read_object_header(f, b"RTSM")?;
            let sflags = f.read16l();
            let basevolume = f.read8();
            let defaultvolume = f.read8();
            let length = f.read32l();
            // the loop type is a byte, read with the 3 reserved ones after it
            let lp = f.read32l() as u8;
            let loopbegin = f.read32l();
            let loopend = f.read32l();
            let basefreq = f.read32l();
            let basenote = f.read8();
            let span = f.read8() as i8;

            let (xpo, fin) = c2spd_to_note(basefreq as i32);
            // autovibrato: see rtm_load.c for Real Tracker's oddities
            let sub = SubInstrument {
                xpo: xpo + 48 - basenote as i32,
                fin,
                vol: defaultvolume as i32,
                gvl: basevolume as i32,
                pan: if iflags & 1 != 0 { convert_pan(span) } else { -1 },
                vwf: (vibflg as u8).min(3) as i32,
                vde: (vibdepth as i32).max(0) << 2,
                vra: (vibrate as i32 + (vibrate > 0) as i32) >> 1,
                vsw: vibsweep as u8 as i32,
                sid: smpnum as i32,
                ..Default::default()
            };
            m.module.xxi[i].sub[j] = sub;

            if smpnum >= m.module.smp {
                let n = m.module.smp * 3 / 2;
                m.realloc_samples(n);
            }
            let xxs = &mut m.module.xxs[smpnum];
            xxs.name = copy_adjust(&sh.name, 31);
            xxs.len = length as i32;
            xxs.lps = loopbegin as i32;
            xxs.lpe = loopend as i32;
            let mut lflags = 0;
            if sflags & 2 != 0 {
                xxs.flg |= XMP_SAMPLE_16BIT;
                xxs.len >>= 1;
                xxs.lps >>= 1;
                xxs.lpe >>= 1;
            }
            if sflags & 4 != 0 {
                lflags |= SAMPLE_FLAG_DIFF;
            }
            // any non-zero loop type loops; only 2 is bidirectional
            if lp >= 1 {
                xxs.flg |= XMP_SAMPLE_LOOP;
            }
            if lp == 2 {
                xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
            }
            load_sample(m, f, lflags, smpnum, None)?;
            smpnum += 1;
        }
    }
    m.realloc_samples(smpnum);

    m.quirk |= QUIRK_FINEFX | QUIRK_INSVOL;
    m.read_event_type = ReadEvent::Ft2;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_effects() {
        let t = |fxt: u8, fxp: u8| {
            let (mut a, mut b) = (fxt, fxp);
            translate_effect(&mut a, &mut b);
            (a, b)
        };
        assert_eq!(t(8, 0xa4), (FX_SURROUND, 1));
        assert_eq!(t(8, 0x90), (FX_SETPAN, 0xff));
        assert_eq!(t(0xa, 0x31), (FX_VOLSLIDE, 0x30));
        assert_eq!(t(1, 0xf0), (FX_PORTA_UP, 0xdf));
        assert_eq!(t(0x1c, 0xa5), (FX_HIOFFSET, 5));
        assert_eq!(t(0x1c, 0x15), (0, 0));
        assert_eq!(t(0xe, 0xf1), (0, 0));
        assert_eq!(t(41, 1), (0, 0));
        assert_eq!((convert_pan(-64), convert_pan(0), convert_pan(64)), (0, 0x80, 255));
    }

    #[test]
    fn detects_rtm() {
        assert!(test(b"RTMM Some song"));
        assert!(!test(b"RTMM\x1aSome song"));
        assert!(!test(b"RTM"));
    }
}

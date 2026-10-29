//! Imago Orpheus modules (`loaders/imf_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! libxmp's loader follows the format description by Lutz Roeder: S3M-like
//! packed patterns with two effect columns, multisample instruments with a
//! volume envelope, played with the ST3 event reader, the Orpheus flow mode
//! and the resonant filter (`QUIRK_FILTER`: its cutoff and resonance effects).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 64 && &d[60..64] == b"IM10"
}

const IMF_EOR: u8 = 0x00;
const IMF_CH_MASK: u8 = 0x1f;
const IMF_NI_FOLLOW: u8 = 0x20;
const IMF_FX_FOLLOWS: u8 = 0x80;
const IMF_F2_FOLLOWS: u8 = 0x40;

const IMF_SAMPLE_LOOP: u8 = 0x01;
const IMF_SAMPLE_BIDI: u8 = 0x02;
const IMF_SAMPLE_16BIT: u8 = 0x04;
const IMF_SAMPLE_DEFPAN: u8 = 0x08;

const MAGIC_IM10: u32 = u32::from_be_bytes(*b"IM10");
const MAGIC_II10: u32 = u32::from_be_bytes(*b"II10");
const MAGIC_IS10: u32 = u32::from_be_bytes(*b"IS10");
/// Found in "leaving all behind.imf".
const MAGIC_IW10: u32 = u32::from_be_bytes(*b"IW10");

const NONE: u8 = 0xff;

const FX: [u8; 36] = [
    NONE,
    FX_S3M_SPEED,
    FX_S3M_BPM,
    FX_TONEPORTA,
    FX_TONE_VSLIDE,
    FX_VIBRATO,
    FX_VIBRA_VSLIDE,
    FX_FINE_VIBRATO,
    FX_TREMOLO,
    FX_S3M_ARPEGGIO,
    FX_SETPAN,
    FX_PANSLIDE,
    FX_VOLSET,
    FX_VOLSLIDE,
    FX_F_VSLIDE,
    FX_FINETUNE,
    FX_NSLIDE_UP,
    FX_NSLIDE_DN,
    FX_PORTA_UP,
    FX_PORTA_DN,
    FX_IMF_FPORTA_UP,
    FX_IMF_FPORTA_DN,
    FX_FLT_CUTOFF,
    FX_FLT_RESN,
    FX_OFFSET,
    NONE, // fine offset
    FX_KEYOFF,
    FX_MULTI_RETRIG,
    FX_TREMOR,
    FX_JUMP,
    FX_BREAK,
    FX_GLOBALVOL,
    FX_GVOL_SLIDE,
    FX_EXTENDED,
    FX_CHORUS,
    FX_REVERB,
];

/// `xlat_fx`.
fn xlat_fx(fxt: &mut u8, fxp: &mut u8) {
    let (h, l) = (msn(*fxp), lsn(*fxp));
    if *fxt as usize >= FX.len() {
        *fxt = 0;
        *fxp = 0;
        return;
    }
    *fxt = FX[*fxt as usize];
    match *fxt {
        FX_EXTENDED => match h {
            // set filter, undefined, ignore envelope, invert loop
            0x1 | 0x2 | 0x4 | 0x6 | 0x7 | 0x9 | 0xe | 0xf => {
                *fxp = 0;
                *fxt = 0;
            }
            0x3 => *fxp = l | (EX_GLISS << 4),
            0x5 => *fxp = l | (EX_VIBRATO_WF << 4),
            0x8 => *fxp = l | (EX_TREMOLO_WF << 4),
            0xa => *fxp = l | (EX_PATTERN_LOOP << 4),
            0xb => *fxp = l | (EX_PATT_DELAY << 4),
            0xc if l == 0 => {
                *fxt = 0;
                *fxp = 0;
            }
            _ => {}
        },
        NONE => {
            *fxt = 0;
            *fxp = 0;
        }
        _ => {}
    }
}

/// The bytes of `r` up to a NUL, one char per byte.
fn latin1(r: &[u8]) -> String {
    r.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect()
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut name = [0u8; 32];
    f.read(&mut name);
    let len = f.read16l() as usize;
    let pat = f.read16l() as usize;
    let ins = f.read16l() as usize;
    let flg = f.read16l();
    f.skip(8);
    let tpo = f.read8();
    let bpm = f.read8();
    let vol = f.read8();
    let amp = f.read8();
    f.skip(8);
    let magic = f.read32b();

    // the UI allows 99 instruments, but Orpheus resaves XMs with 128
    if len > 256 || pat > 256 || ins > 128 {
        return Err(());
    }

    let mut chpan = [0u8; 32];
    let mut status = [0u8; 32];
    for i in 0..32 {
        let mut cname = [0u8; 12];
        f.read(&mut cname);
        let _chorus = f.read8();
        let _reverb = f.read8();
        chpan[i] = f.read8();
        status[i] = f.read8();
    }
    let mut pos = [0u8; 256];
    if f.read(&mut pos) < 256 {
        return Err(());
    }
    if magic != MAGIC_IM10 {
        return Err(());
    }

    m.module.name = copy_adjust(&name, 32);
    m.module.len = len;
    m.module.ins = ins;
    m.module.smp = MAX_SAMPLES;
    m.module.pat = pat;
    if flg & 0x01 != 0 {
        m.period_type = PeriodType::Linear;
    }
    m.module.spd = tpo as i32;
    m.module.bpm = bpm as i32;
    m.module.type_ = "Imago Orpheus 1.0 IMF".to_string();

    m.module.chn = 0;
    for i in 0..32 {
        // 0 = enabled, 1 = muted but still processed, 2 = disabled
        if status[i] >= 2 {
            continue;
        }
        m.module.chn = i + 1;
        m.module.xxc[i].pan = chpan[i] as i32;
    }
    m.module.trk = m.module.pat * m.module.chn;

    m.module.xxo[..len].copy_from_slice(&pos[..len]);
    for o in m.module.xxo[..len].iter_mut() {
        if *o == 0xff {
            *o -= 1;
        }
    }

    m.c4rate = C4_NTSC_RATE;

    // patterns
    let mut dummy = Event::default();
    for i in 0..m.module.pat {
        let mut pat_len = f.read16l() as i32 - 4;
        let rows = f.read16l() as usize;
        // libxmp_alloc_pattern_tracks: 1 to 256 rows
        if rows == 0 || rows > 256 {
            return Err(());
        }
        m.module.alloc_pattern_tracks(i, rows);

        let mut r = 0;
        loop {
            pat_len -= 1;
            if pat_len < 0 {
                break;
            }
            let b = f.read8();
            if b == IMF_EOR {
                r += 1;
                continue;
            }
            if r >= rows {
                return Err(());
            }
            let c = (b & IMF_CH_MASK) as usize;
            let event = if c >= m.module.chn { &mut dummy } else { m.module.event_mut(i, c, r) };
            if b & IMF_NI_FOLLOW != 0 {
                let n = match f.read8() {
                    255 => 0, // no note
                    160 => XMP_KEY_OFF,
                    n => 13 + 12 * msn(n) + lsn(n),
                };
                event.note = n;
                event.ins = f.read8();
                pat_len -= 2;
            }
            if b & IMF_FX_FOLLOWS != 0 {
                event.fxt = f.read8();
                event.fxp = f.read8();
                xlat_fx(&mut event.fxt, &mut event.fxp);
                pat_len -= 2;
            }
            if b & IMF_F2_FOLLOWS != 0 {
                event.f2t = f.read8();
                event.f2p = f.read8();
                xlat_fx(&mut event.f2t, &mut event.f2p);
                pat_len -= 2;
            }
        }
    }

    // instruments and samples
    m.init_instrument();
    let mut smp_num = 0usize;
    for i in 0..m.module.ins {
        let mut iname = [0u8; 32];
        f.read(&mut iname);
        iname[31] = 0;
        let mut map = [0u8; 120];
        f.read(&mut map);
        f.skip(8);
        let mut vol_env = [0u16; 32];
        for v in vol_env.iter_mut() {
            *v = f.read16l();
        }
        // the pan and pitch envelopes are not used
        for _ in 0..64 {
            f.read16l();
        }
        let mut env = [[0u8; 5]; 3];
        for e in env.iter_mut() {
            f.read(e);
            f.skip(3);
        }
        let _fadeout = f.read16l();
        let nsm = f.read16l() as usize;
        let imagic = f.read32b();

        // Orpheus ignores samples past 16
        if nsm > 16 {
            return Err(());
        }
        // Orpheus may emit blank instruments with a magic of four NULs
        // ("leaving all behind.imf" by Karsten Koch)
        if imagic != MAGIC_II10 && imagic != 0 {
            return Err(());
        }

        let xxi = &mut m.module.xxi[i];
        xxi.nsm = nsm as i32;
        xxi.sub = vec![SubInstrument::default(); nsm];
        xxi.name = latin1(&iname[..31]);
        for (k, &smp) in xxi.map[12..].iter_mut().zip(&map[..108]) {
            k.ins = smp;
        }

        let [npt, sus, lps, lpe, eflg] = env[0];
        xxi.aei.npt = npt as i32;
        xxi.aei.sus = sus as i32;
        xxi.aei.lps = lps as i32;
        xxi.aei.lpe = lpe as i32;
        xxi.aei.flg = if eflg & 0x01 != 0 { XMP_ENVELOPE_ON } else { 0 };
        xxi.aei.flg |= if eflg & 0x02 != 0 { XMP_ENVELOPE_SUS } else { 0 };
        xxi.aei.flg |= if eflg & 0x04 != 0 { XMP_ENVELOPE_LOOP } else { 0 };
        if xxi.aei.npt > 16 {
            return Err(());
        }
        for j in 0..xxi.aei.npt as usize {
            xxi.aei.data[j * 2] = vol_env[j * 2] as i16;
            xxi.aei.data[j * 2 + 1] = vol_env[j * 2 + 1] as i16;
        }

        for j in 0..nsm {
            // Orpheus is limited to 200 samples, but may emit more
            if smp_num >= m.module.smp {
                let n = m.module.smp * 3 / 2;
                m.realloc_samples(n);
            }

            let mut sname = [0u8; 13];
            f.read(&mut sname);
            f.skip(3);
            let slen = f.read32l();
            let slps = f.read32l();
            let slpe = f.read32l();
            let rate = f.read32l();
            let svol = f.read8();
            let span = f.read8();
            f.skip(14);
            let sflg = f.read8();
            f.skip(5);
            let _ems = f.read16l();
            let _dram = f.read32l();
            let smagic = f.read32b();

            // four NULs too (an XM of >= 13 instruments x 16 samples resaved)
            if smagic != MAGIC_IS10 && smagic != MAGIC_IW10 && smagic != 0 {
                return Err(());
            }
            if slen > 0x100000 || slps > 0x100000 || slpe > 0x100000 {
                return Err(());
            }

            let (xpo, fin) = c2spd_to_note(rate as i32);
            m.module.xxi[i].sub[j] = SubInstrument {
                sid: smp_num as i32,
                vol: svol as i32,
                pan: if sflg & IMF_SAMPLE_DEFPAN != 0 { span as i32 } else { XMP_INST_NO_DEFAULT_PAN },
                xpo,
                fin,
                ..Default::default()
            };
            let xxs = &mut m.module.xxs[smp_num];
            xxs.len = slen as i32;
            xxs.lps = slps as i32;
            xxs.lpe = slpe as i32;
            xxs.flg = 0;
            if sflg & IMF_SAMPLE_LOOP != 0 {
                xxs.flg |= XMP_SAMPLE_LOOP;
            }
            if sflg & IMF_SAMPLE_BIDI != 0 {
                xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
            }
            if sflg & IMF_SAMPLE_16BIT != 0 {
                xxs.flg |= XMP_SAMPLE_16BIT;
                xxs.len >>= 1;
                xxs.lps >>= 1;
                xxs.lpe >>= 1;
            }
            let has_data = xxs.len > 0;
            let sid = smp_num;
            smp_num += 1;
            if !has_data {
                continue;
            }
            load_sample(m, f, 0, sid, None)?;
        }
    }
    if smp_num < m.module.smp {
        m.realloc_samples(smp_num);
    }

    m.c4rate = C4_NTSC_RATE;
    m.quirk |= QUIRK_FILTER | QUIRK_MARKER | QUIRK_RSTCHN | QUIRK_ARPMEM;
    m.flow_mode = FLOW_MODE_ORPHEUS;
    m.read_event_type = ReadEvent::St3;

    m.gvol = vol as i32;
    m.mvol = (amp as i32).clamp(4, 127);
    m.mvolbase = 48;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One channel, one 1-row pattern, one instrument with one 4-byte sample.
    fn synth() -> Vec<u8> {
        let mut d = vec![0u8; 64 + 32 * 16 + 256];
        d[..4].copy_from_slice(b"song");
        d[32] = 1; // orders
        d[34] = 1; // patterns
        d[36] = 1; // instruments
        d[48] = 6; // tempo
        d[49] = 125; // bpm
        d[50] = 64; // master volume
        d[51] = 48; // amplification
        d[60..64].copy_from_slice(b"IM10");
        for i in 1..32 {
            d[64 + i * 16 + 15] = 2; // disabled
        }
        d[64 + 14] = 0x40; // pan of channel 0
        // pattern: C-5 instrument 1, filter cutoff 0x40; end of row
        let p = [0x20 | 0x80, 0x40, 1, 0x16, 0x40, 0];
        d.extend(((p.len() + 4) as u16).to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(p);
        // instrument
        let mut ins = vec![0u8; 32 + 120 + 8 + 192 + 24];
        ins[32 + 48] = 0; // C-4 -> sample 0
        d.append(&mut ins);
        d.extend(0u16.to_le_bytes()); // fadeout
        d.extend(1u16.to_le_bytes()); // samples
        d.extend(b"II10");
        // sample
        d.extend([0u8; 16]);
        for v in [4u32, 0, 0, 8363] {
            d.extend(v.to_le_bytes());
        }
        d.extend([64u8, 0x80]);
        d.extend([0u8; 14]);
        d.push(0);
        d.extend([0u8; 5 + 2 + 4]);
        d.extend(b"IS10");
        d.extend([1, 2, 3, 4]);
        d
    }

    #[test]
    fn loads_synthetic_imf() {
        let d = synth();
        assert!(test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!((m.module.name.as_str(), m.module.chn, m.module.len, m.module.smp), ("song", 1, 1, 1));
        assert_eq!(m.module.xxc[0].pan, 0x40);
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.ins, e.fxt, e.fxp), (61, 1, FX_FLT_CUTOFF, 0x40));
        assert_eq!(m.module.xxs[0].data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 4], [1, 2, 3, 4]);
        assert!(m.has_quirk(QUIRK_FILTER));
        assert_eq!((m.mvol, m.gvol, m.flow_mode), (48, 64, FLOW_MODE_ORPHEUS));
    }
}

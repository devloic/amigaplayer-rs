//! Farandole Composer modules (`loaders/far_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! Based on the Farandole Composer format specifications by Megan Potter.

use crate::xmp::extras::ModuleExtra;
use crate::xmp::far_extras::*;
use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 4 && d[..4] == [b'F', b'A', b'R', 0xfe]
}

fn translate_effect(event: &mut Event, fx: u8, param: u8, vol: u8) {
    match fx {
        0x0 => match param {
            // 0x01 / 0x02: ramp delay on/off control volume ramping, ignored.
            0x3 => {
                // Fulfill loop: intended as sustain release, but buggy and
                // just cuts most of the time.
                event.fxt = FX_KEYOFF;
            }
            0x4 => {
                event.fxt = FX_FAR_TEMPO;
                event.fxp = 0x10;
            }
            0x5 => {
                event.fxt = FX_FAR_TEMPO;
                event.fxp = 0x20;
            }
            _ => {}
        },
        0x1 => {
            event.fxt = FX_FAR_PORTA_UP;
            event.fxp = param;
        }
        0x2 => {
            event.fxt = FX_FAR_PORTA_DN;
            event.fxp = param;
        }
        0x3 => {
            event.fxt = FX_FAR_TPORTA;
            event.fxp = param;
        }
        0x4 => {
            event.fxt = FX_FAR_RETRIG;
            event.fxp = param;
        }
        0x5 => {
            event.fxt = FX_FAR_VIBDEPTH;
            event.fxp = param;
        }
        0x6 => {
            event.fxt = FX_FAR_VIBRATO;
            event.fxp = param;
        }
        0x7 => {
            event.fxt = FX_F_VSLIDE_UP;
            event.fxp = param << 4;
        }
        0x8 => {
            event.fxt = FX_F_VSLIDE_DN;
            event.fxp = param << 4;
        }
        0x9 => {
            event.fxt = FX_FAR_VIBRATO;
            event.fxp = 0x10 /* Vibrato sustain flag */ | param;
        }
        0xa => {
            if (0x01..=0x10).contains(&vol) {
                event.fxt = FX_FAR_SLIDEVOL;
                event.fxp = ((vol - 1) << 4) | param;
                event.vol = 0;
            }
        }
        0xb => {
            event.fxt = FX_SETPAN;
            event.fxp = (param << 4) | param;
        }
        0xc => {
            event.fxt = FX_FAR_DELAY;
            event.fxp = param;
        }
        0xd => {
            event.fxt = FX_FAR_F_TEMPO;
            event.fxp = param;
        }
        0xe => {
            event.fxt = FX_FAR_F_TEMPO;
            event.fxp = param << 4;
        }
        0xf => {
            event.fxt = FX_FAR_TEMPO;
            event.fxp = param;
        }
        _ => {}
    }
}

/// `COMMENT_MAXLINES`.
const COMMENT_MAXLINES: usize = 44;

/// `far_read_text`: the 132-character lines with no line breaks.
fn read_text(textlen: usize, f: &mut Hio) -> String {
    let mut dest: Vec<u8> = Vec::new();
    let mut textlen = textlen.min(COMMENT_MAXLINES * 132);

    while textlen > 0 {
        let end = textlen.min(132);
        textlen -= end;
        let mut line = [0u8; 132];
        let end = f.read(&mut line[..end]);

        let mut lastchar = 0;
        for i in 0..end {
            // Nulls in the text area are equivalent to spaces.
            if line[i] == 0 {
                line[i] = b' ';
            } else if line[i] != b' ' {
                lastchar = i;
            }
        }
        // dest += lastchar + 1 (the line is read in place in C)
        dest.extend_from_slice(&line[..lastchar + 1]);
        dest.push(b'\n');
    }
    dest.into_iter().map(|b| b as char).collect()
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.read32b(); // file magic
    let mut name = [0u8; 40];
    f.read(&mut name);
    f.skip(3); // 0x0d 0x0a 0x1a
    let headersize = f.read16l() as usize;
    let version = f.read8();
    let mut ch_on = [0u8; 16];
    f.read(&mut ch_on);
    f.skip(9); // current editing values
    let tempo = f.read8();
    let mut pan = [0u8; 16];
    f.read(&mut pan);
    f.read32l(); // grid, mode (for editor)
    let textlen = f.read16l() as usize;

    // Sanity check
    if tempo >= 16 {
        return Err(());
    }

    m.comment = read_text(textlen, f);

    let mut order = [0u8; 256];
    f.read(&mut order);
    let _patterns = f.read8();
    let songlen = f.read8() as usize;
    let restart = f.read8() as usize;
    let mut patsize = [0u16; 256];
    for p in patsize.iter_mut() {
        *p = f.read16l();
    }

    if f.error {
        return Err(());
    }

    // Skip unsupported header extension if it exists. The documentation claims
    // this field is the "remaining" header size, but it's the total size.
    if headersize > 869 + textlen {
        f.seek(headersize);
    }

    let md = &mut m.module;
    md.chn = 16;
    md.len = songlen;
    md.rst = restart;
    md.xxo = [0; XMP_MAX_MOD_LENGTH];
    md.xxo[..songlen].copy_from_slice(&order[..songlen]);

    md.pat = 0;
    for i in 0..256 {
        if patsize[i] != 0 {
            md.pat = i + 1;
        }
    }
    // Make sure referenced zero-sized patterns are also counted.
    for i in 0..md.len {
        if md.pat <= md.xxo[i] as usize {
            md.pat = md.xxo[i] as usize + 1;
        }
    }

    md.trk = md.chn * md.pat;

    let mut me = FarModuleExtras::new();
    me.coarse_tempo = tempo as i32;
    me.fine_tempo = 0;
    me.tempo_mode = 1;
    m.time_factor = FAR_TIME_FACTOR;
    if let Some((s, b)) = far_translate_tempo(1, 0, me.coarse_tempo, &mut me.fine_tempo) {
        m.module.spd = s;
        m.module.bpm = b;
    }
    m.extra = ModuleExtra::Far(me);

    m.period_type = PeriodType::Cspd;
    m.c4rate = C4_NTSC_RATE;

    m.quirk |= QUIRK_VSALL | QUIRK_PBALL | QUIRK_VIBALL;

    let md = &mut m.module;
    md.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(40)]).into_owned();
    md.type_ = format!("Farandole Composer {}.{}", msn(version), lsn(version));

    // libxmp_init_pattern: patterns without tracks until they are allocated
    md.xxt = vec![Track::default(); md.trk];
    md.xxp = vec![Pattern { rows: 0, index: vec![0; md.chn] }; md.pat];

    // Read and convert patterns
    let mut patbuf = vec![0u8; 256 * 16 * 4];

    for i in 0..m.module.pat {
        if patsize[i] == 0 {
            continue;
        }

        let rows = (patsize[i] as i32 - 2) / 64;

        // Sanity check
        if rows <= 0 || rows > 256 {
            return Err(());
        }
        let rows = rows as usize;

        m.module.alloc_pattern_tracks(i, rows);

        let brk = f.read8() as usize + 1;
        f.read8();

        if f.read(&mut patbuf[..rows * 64]) < rows * 64 {
            return Err(());
        }

        let mut pos = 0;
        for j in 0..rows {
            for k in 0..m.module.chn {
                let event = m.module.event_mut(i, k, j);

                if k == 0 && j == brk {
                    event.f2t = FX_BREAK;
                }

                let note = patbuf[pos];
                let ins = patbuf[pos + 1];
                let vol = patbuf[pos + 2];
                let fxb = patbuf[pos + 3];
                pos += 4;

                // The instrument number is completely ignored if no note is
                // present; whatever note is playing will continue.
                if note != 0 {
                    event.note = note.wrapping_add(48);
                    event.ins = ins.wrapping_add(1);
                }

                if (0x01..=0x10).contains(&vol) {
                    event.vol = (vol - 1).wrapping_mul(16).wrapping_add(1);
                } else if note != 0 || vol != 0 {
                    // aurora.far contains note+ins without volume. This is
                    // invalid and sets the volume to zero. Volumes greater than
                    // 0x10 also usually seem to be treated as zero.
                    event.vol = 1;
                }

                translate_effect(event, msn(fxb), lsn(fxb), vol);
            }
        }
    }

    // Allocate tracks for any patterns referenced with a size of 0. These use
    // the configured pattern break position, which is 64 by default.
    for i in 0..m.module.len {
        let pat = m.module.xxo[i] as usize;
        if m.module.xxp[pat].rows == 0 {
            m.module.alloc_pattern_tracks(pat, 64);
        }
    }

    let mut sample_map = [0u8; 8];
    if f.read(&mut sample_map) < 8 {
        return Err(());
    }
    let mut ins: i32 = -1;
    for i in 0..64 {
        if sample_map[i / 8] & (1 << (i % 8)) != 0 {
            ins = i as i32;
        }
    }
    ins += 1;
    m.module.ins = ins as usize;
    m.module.smp = ins as usize;

    m.init_instrument();

    // Read and convert instruments and samples
    for i in 0..m.module.ins {
        if sample_map[i / 8] & (1 << (i % 8)) == 0 {
            continue;
        }

        m.module.xxi[i].sub = vec![SubInstrument::default()];

        let mut iname = [0u8; 32];
        f.read(&mut iname);
        let length = f.read32l();
        f.read8(); // finetune (unsupported)
        f.read8(); // volume (unsupported?)
        let loop_start = f.read32l();
        let loopend = f.read32l();
        let sampletype = f.read8();
        let loopmode = f.read8();

        // Sanity check
        if length > 0x10000 || loop_start > 0x10000 || loopend > 0x10000 {
            return Err(());
        }

        let xxs = &mut m.module.xxs[i];
        xxs.len = length as i32;
        xxs.lps = loop_start as i32;
        xxs.lpe = loopend as i32;
        xxs.flg = 0;

        if xxs.len > 0 {
            m.module.xxi[i].nsm = 1;
        }

        let xxs = &mut m.module.xxs[i];
        if sampletype != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }

        xxs.flg |= if loopmode != 0 { XMP_SAMPLE_LOOP } else { 0 };
        let xxi = &mut m.module.xxi[i];
        xxi.sub[0].vol = 0xff;
        xxi.sub[0].pan = XMP_INST_NO_DEFAULT_PAN;
        xxi.sub[0].sid = i as i32;
        xxi.name = copy_adjust(&iname, 32);

        load_sample(m, f, 0, i, None)?;
    }

    // Panning map
    for i in 0..16 {
        if ch_on[i] == 0 {
            m.module.xxc[i].flg |= XMP_CHANNEL_MUTE;
        }
        if pan[i] < 0x10 {
            m.module.xxc[i].pan = ((pan[i] << 4) | pan[i]) as i32;
        }
    }

    m.volbase = 0xf0;

    Ok(())
}

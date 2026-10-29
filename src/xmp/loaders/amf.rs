//! DSMI Advanced Module Format (`loaders/amf_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;
use crate::xmp::player::c2spd_to_note;

pub fn test(d: &[u8]) -> bool {
    if d.len() < 4 || &d[..3] != b"AMF" {
        return false;
    }
    let ver = d[3];
    !((ver != 0x01 && ver < 0x08) || ver > 0x0e)
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
}

/// The volume slide of the AMF effects 0x82, 0x8a and 0x8b.
fn vslide(t3: u8) -> u8 {
    if (t3 as i8) > 0 { t3 << 4 } else { ((-(t3 as i8 as i32)) & 0x0f) as u8 }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut buf = [0u8; 1024];
    f.read(&mut buf[..3]);
    let ver = f.read8();

    if f.read(&mut buf[..32]) != 32 {
        return Err(());
    }
    m.module.name = cstr(&buf[..32]);
    m.module.type_ = format!("DSMI {}.{} AMF", ver / 10, ver % 10);

    m.module.ins = f.read8() as usize;
    m.module.len = f.read8() as usize;
    m.module.trk = f.read16l() as usize;
    m.module.chn = 4;

    if ver >= 0x09 {
        m.module.chn = f.read8() as usize;
    }

    // Sanity check
    let md = &m.module;
    if md.ins == 0 || md.len == 0 || md.trk == 0 || md.chn == 0 || md.chn > XMP_MAX_CHANNELS {
        return Err(());
    }
    m.module.smp = m.module.ins;
    m.module.pat = m.module.len;

    if ver == 0x09 || ver == 0x0a {
        f.read(&mut buf[..16]); // channel remap table
    }

    if ver >= 0x0b {
        let pan_len = if ver >= 0x0c { 32 } else { 16 };
        if f.read(&mut buf[..pan_len]) != pan_len {
            return Err(());
        }
        for i in 0..pan_len {
            m.module.xxc[i].pan = 0x80 + 2 * (buf[i] as i8 as i32);
        }
    }

    if ver >= 0x0d {
        m.module.bpm = f.read8() as i32;
        m.module.spd = f.read8() as i32;
    }

    m.c4rate = C4_NTSC_RATE;

    // Orders
    for i in 0..m.module.len {
        m.module.xxo[i] = i as u8;
    }

    let chn = m.module.chn;
    m.module.xxp = Vec::with_capacity(m.module.pat);
    for _ in 0..m.module.pat {
        let rows = if ver >= 0x0e { f.read16l() as usize } else { 64 };
        if rows > 256 {
            return Err(());
        }
        let mut index = vec![0usize; chn];
        for j in 0..chn {
            index[j] = f.read16l() as usize;
        }
        m.module.xxp.push(Pattern { rows, index });
    }

    // Instruments
    m.init_instrument();

    // Probe for the 2-byte loop start 1.0 format (facing_n.amf, sweetdrm.amf)
    let mut no_loopend = false;
    if ver == 0x0a {
        let ins = m.module.ins;
        let pos = f.tell();
        for _ in 0..ins {
            let b = f.read8();
            if b != 0 && b != 1 {
                no_loopend = true;
                break;
            }
            f.seek_cur(32 + 13);
            if f.read32l() > ins as u32 {
                no_loopend = true;
                break;
            }
            let len = f.read32l();
            if len > 0x100000 {
                no_loopend = true;
                break;
            }
            if f.read16l() == 0x0000 {
                no_loopend = true;
                break;
            }
            if f.read8() > 0x40 {
                no_loopend = true;
                break;
            }
            if f.read32l() > len {
                no_loopend = true;
                break;
            }
            if f.read32l() > len {
                no_loopend = true;
                break;
            }
        }
        f.seek(pos);
    }

    for i in 0..m.module.ins {
        f.read8();
        f.read(&mut buf[..32]);
        let name = copy_adjust(&buf[..32], 31);
        f.read(&mut buf[..13]); // sample name
        f.read32l(); // sample index

        let len = if ver >= 0x0a { f.read32l() as i32 } else { f.read16l() as i32 };
        let c2spd = f.read16l() as i32;
        let (xpo, fin) = c2spd_to_note(c2spd);
        let vol = f.read8() as i32;

        let xxs = &mut m.module.xxs[i];
        xxs.len = len;
        if no_loopend {
            xxs.lps = f.read16l() as i32;
            xxs.lpe = xxs.len;
        } else if ver >= 0x0a {
            xxs.lps = f.read32l() as i32;
            xxs.lpe = f.read32l() as i32;
        } else {
            // Non-looping samples are stored with lpe=-1, not 0.
            xxs.lps = f.read16l() as i32;
            xxs.lpe = f.read16l() as i32;
            if xxs.lpe == 0xffff {
                xxs.lpe = 0;
            }
        }
        xxs.flg = if no_loopend {
            if xxs.lps > 0 { XMP_SAMPLE_LOOP } else { 0 }
        } else if xxs.lpe > xxs.lps {
            XMP_SAMPLE_LOOP
        } else {
            0
        };

        let xxi = &mut m.module.xxi[i];
        xxi.name = name;
        xxi.nsm = 1;
        xxi.sub = vec![SubInstrument { sid: i as i32, pan: XMP_INST_NO_DEFAULT_PAN, xpo, fin, vol, ..Default::default() }];
    }

    if f.take_error() {
        return Err(());
    }

    // Tracks
    //
    // Index 0 is a blank track that isn't stored in the file: the mapping table
    // is loaded to index 1 so the table index is the logical track value.
    let ntrk = m.module.trk;
    let mut trkmap = vec![0usize; ntrk + 1];
    let mut newtrk = 0usize;
    for i in 1..=ntrk {
        let t = f.read16l() as usize;
        trkmap[i] = t;
        if t > newtrk {
            newtrk = t;
        }
    }

    for p in m.module.xxp.iter_mut() {
        for j in 0..chn {
            let mut k = p.index[j];
            // an invalid track is the empty track
            if k > ntrk {
                k = 0;
            }
            p.index[j] = trkmap[k];
        }
    }

    m.module.trk = newtrk + 1; // + empty track

    if f.take_error() {
        return Err(());
    }

    m.module.xxt = vec![Track::new(64); m.module.trk];

    for i in 1..m.module.trk {
        // The size is just a word; the third byte is ignored by DSMI.
        let mut size = f.read16l() as usize;
        f.read8();

        if f.take_error() {
            return Err(());
        }

        // Version 0.1 AMFs do not count the end-of-track marker
        if ver == 0x01 && size != 0 {
            size += 1;
        }

        for j in 0..size {
            let t1 = f.read8();
            let t2 = f.read8();
            let mut t3 = f.read8();

            if t1 == 0xff && t2 == 0xff && t3 == 0xff {
                break;
            }

            // An event past the end of the track is the track end ("Avoid.amf").
            if t1 as usize >= m.module.xxt[i].rows {
                f.seek_cur(((size - j - 1) * 3) as i64);
                break;
            }

            let event = &mut m.module.xxt[i].event[t1 as usize];

            if t2 < 0x7f {
                // note
                if t2 > 0 {
                    event.note = t2 + 1;
                }
                // a volume of 0xff reuses the old volume
                event.vol = if t3 != 0xff { t3.wrapping_add(1) } else { 0 };
            } else if t2 == 0x7f {
                // note retrigger: ignored
            } else if t2 == 0x80 {
                event.ins = t3.wrapping_add(1);
            } else {
                let mut fxp = 0u8;
                let mut fxt = 0u8;
                match t2 {
                    0x81 => {
                        fxt = FX_S3M_SPEED;
                        fxp = t3;
                    }
                    0x82 => {
                        fxt = FX_VOLSLIDE;
                        fxp = vslide(t3);
                    }
                    0x83 => {
                        event.vol = t3.wrapping_add(1);
                    }
                    0x84 => {
                        if (t3 as i8) >= 0 {
                            fxt = FX_PORTA_DN;
                            fxp = t3;
                        } else if t3 == 0x80 {
                            fxt = FX_PORTA_UP;
                            fxp = 0;
                        } else {
                            fxt = FX_PORTA_UP;
                            fxp = (-(t3 as i8 as i32)) as u8;
                        }
                    }
                    0x85 => {}
                    0x86 => {
                        fxt = FX_TONEPORTA;
                        fxp = t3;
                    }
                    0x87 => {
                        fxt = FX_TREMOLO;
                        fxp = t3;
                    }
                    0x88 => {
                        fxt = FX_ARPEGGIO;
                        fxp = t3;
                    }
                    0x89 => {
                        fxt = FX_VIBRATO;
                        fxp = t3;
                    }
                    0x8a => {
                        fxt = FX_TONE_VSLIDE;
                        fxp = vslide(t3);
                    }
                    0x8b => {
                        fxt = FX_VIBRA_VSLIDE;
                        fxp = vslide(t3);
                    }
                    0x8c => {
                        fxt = FX_BREAK;
                        fxp = t3;
                    }
                    0x8d => {
                        fxt = FX_JUMP;
                        fxp = t3;
                    }
                    0x8e => {}
                    0x8f => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_RETRIG << 4) | (t3 & 0x0f);
                    }
                    0x90 => {
                        fxt = FX_OFFSET;
                        fxp = t3;
                    }
                    0x91 => {
                        fxt = FX_EXTENDED;
                        fxp = if (t3 as i8) > 0 { (EX_F_VSLIDE_UP << 4) | (t3 & 0x0f) } else { (EX_F_VSLIDE_DN << 4) | (t3 & 0x0f) };
                    }
                    0x92 => {
                        // fxp is still 0 here, as in libxmp
                        fxt = if (t3 as i8) > 0 { FX_PORTA_DN } else { FX_PORTA_UP };
                        fxp = 0xf0 | (fxp & 0x0f);
                    }
                    0x93 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_DELAY << 4) | (t3 & 0x0f);
                    }
                    0x94 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_CUT << 4) | (t3 & 0x0f);
                    }
                    0x95 => {
                        fxt = FX_SPEED;
                        if t3 < 0x21 {
                            t3 = 0x21;
                        }
                        fxp = t3;
                    }
                    0x96 => {
                        fxt = if (t3 as i8) > 0 { FX_PORTA_DN } else { FX_PORTA_UP };
                        fxp = 0xe0 | (fxp & 0x0f);
                    }
                    0x97 => {
                        // Same as S3M pan, but the parameter is offset by -0x40.
                        if t3 == 0x64 {
                            fxt = FX_SURROUND;
                            fxp = 1;
                        } else if t3 >= 0xc0 || t3 <= 0x40 {
                            let pan = (t3 as i8 as i32) * 2 + 0x80;
                            fxt = FX_SETPAN;
                            fxp = pan.min(0xff) as u8;
                        }
                    }
                    _ => {}
                }
                event.fxt = fxt;
                event.fxp = fxp;
            }
        }
    }

    // Samples
    for i in 0..m.module.ins {
        load_sample(m, f, SAMPLE_FLAG_UNS, i, None)?;
    }

    m.quirk |= QUIRK_FINEFX;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-pattern AMF 1.3 with one 4-byte sample and a note, a volume slide
    /// and a pan event on one track.
    fn synth() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(b"AMF");
        d.push(0x0d);
        let mut name = [0u8; 32];
        name[..3].copy_from_slice(b"abc");
        d.extend(name);
        d.push(1); // ins
        d.push(1); // len
        d.extend(1u16.to_le_bytes()); // trk
        d.push(2); // chn
        d.extend([0u8; 32]); // pan table
        d.push(140); // bpm
        d.push(5); // speed
        d.extend(1u16.to_le_bytes()); // pattern 0 track 1
        d.extend(0u16.to_le_bytes()); // pattern 0 track 2 (empty)
        let mut ins = vec![0u8; 1 + 32 + 13 + 4];
        ins[0] = 1;
        d.extend(ins);
        d.extend(4u32.to_le_bytes()); // len
        d.extend(8363u16.to_le_bytes()); // c2spd
        d.push(40); // volume
        d.extend(0u32.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.extend(1u16.to_le_bytes()); // track map: logical 1 -> stored 1
        d.extend(3u16.to_le_bytes()); // 3 events
        d.push(0);
        for e in [[0u8, 24, 0x20], [1, 0x82, 0xfe], [2, 0x97, 0x20]] {
            d.extend(e);
        }
        d.extend([0xff, 0xff, 0xff]);
        d.extend([0x80, 0x00, 0x7f, 1]);
        d
    }

    #[test]
    fn loads_synthetic_amf() {
        let d = synth();
        assert!(test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.type_, "DSMI 1.3 AMF");
        assert_eq!((m.module.chn, m.module.trk, m.module.pat, m.module.spd, m.module.bpm), (2, 2, 1, 5, 140));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.vol), (25, 0x21));
        let e = m.module.event(0, 0, 1);
        assert_eq!((e.fxt, e.fxp), (FX_VOLSLIDE, 2));
        let e = m.module.event(0, 0, 2);
        assert_eq!((e.fxt, e.fxp), (FX_SETPAN, 0xc0));
    }
}

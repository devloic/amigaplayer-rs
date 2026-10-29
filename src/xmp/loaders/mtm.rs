//! MultiTracker modules (`loaders/mtm_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 4 && &d[..3] == b"MTM" && d[3] == 0x10
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.skip(3);
    let version = f.read8();
    let mut name = [0u8; 20];
    f.read(&mut name);
    let tracks = f.read16l() as usize;
    let patterns = f.read8() as usize;
    let modlen = f.read8() as usize;
    let extralen = f.read16l() as usize;
    let samples = f.read8() as usize;
    if samples > 63 {
        return Err(());
    }
    let _attr = f.read8();
    let rows = f.read8() as usize;
    if rows != 64 {
        return Err(());
    }
    let channels = f.read8() as usize;
    if channels > 32 {
        return Err(());
    }
    let mut pan = [0u8; 32];
    f.read(&mut pan);
    if f.error {
        return Err(());
    }

    let md = &mut m.module;
    md.trk = tracks + 1;
    md.pat = patterns + 1;
    md.len = modlen + 1;
    md.ins = samples;
    md.smp = samples;
    md.chn = channels;
    md.spd = 6;
    md.bpm = 125;
    md.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(20)]).into_owned();
    md.type_ = format!("MultiTracker {}.{:02} MTM", msn(version), lsn(version));

    m.init_instrument();
    for i in 0..m.module.ins {
        let mut iname = [0u8; 22];
        f.read(&mut iname);
        let length = f.read32l();
        if length > MAX_SAMPLE_SIZE as u32 {
            return Err(());
        }
        let loop_start = f.read32l();
        let loopend = f.read32l();
        let finetune = f.read8();
        let volume = f.read8();
        let attr = f.read8();
        let xxs = &mut m.module.xxs[i];
        xxs.len = length as i32;
        xxs.lps = loop_start as i32;
        xxs.lpe = loopend as i32;
        xxs.flg = if xxs.lpe > 2 { XMP_SAMPLE_LOOP } else { 0 };
        if attr & 1 != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument {
            vol: volume as i32,
            fin: (finetune << 4) as i8 as i32,
            pan: XMP_INST_NO_DEFAULT_PAN,
            sid: i as i32,
            ..Default::default()
        }];
        xxi.name = copy_adjust(&iname, 22);
        if has {
            xxi.nsm = 1;
        }
    }

    let mut order = [0u8; 128];
    f.read(&mut order);
    m.module.xxo = [0; XMP_MAX_MOD_LENGTH];
    m.module.xxo[..128].copy_from_slice(&order);

    let mut fxx = [false; 2];
    m.module.xxt = vec![Track::new(rows); m.module.trk];
    for i in 1..m.module.trk {
        let mut mt = [0u8; 192];
        if f.read(&mut mt) != 192 {
            return Err(());
        }
        for j in 0..64 {
            let d = &mt[j * 3..j * 3 + 3];
            let e = &mut m.module.xxt[i].event[j];
            e.note = d[0] >> 2;
            if e.note != 0 {
                e.note += 37;
            }
            e.ins = ((d[0] & 0x3) << 4) + msn(d[1]);
            e.fxt = lsn(d[1]);
            e.fxp = d[2];
            if e.fxt > FX_SPEED {
                e.fxt = 0;
                e.fxp = 0;
            }
            if e.fxt == FX_BREAK {
                e.fxt = FX_IT_BREAK;
            }
            if e.fxt == FX_SPEED {
                fxx[(e.fxp >= 0x20) as usize] = true;
            }
            if e.fxt == FX_EXTENDED && msn(e.fxp) == 0x8 {
                e.fxt = FX_SETPAN;
                e.fxp <<= 4;
            }
        }
    }

    let chn = m.module.chn;
    m.module.xxp = Vec::with_capacity(m.module.pat);
    for _ in 0..m.module.pat {
        let mut index = vec![0usize; chn];
        for j in 0..32 {
            let mut track = f.read16l() as usize;
            if track >= m.module.trk {
                track = 0;
            }
            if j < chn {
                index[j] = track;
            }
        }
        m.module.xxp.push(Pattern { rows: 64, index });
    }

    // Tempo mode detection: MultiTracker resets the tempo to 125 when the speed
    // is set and the speed to 6 when the tempo is set.
    if fxx[0] && fxx[1] {
        let md = &mut m.module;
        let mut dmp = false;
        'pats: for i in 0..md.pat {
            for j in 0..rows {
                let mut f2 = [false; 2];
                for k in 0..md.chn {
                    let e = md.event(i, k, j);
                    if e.fxt == FX_SPEED {
                        f2[(e.fxp >= 0x20) as usize] = true;
                    }
                }
                if f2[0] && f2[1] {
                    dmp = true;
                    break 'pats;
                }
            }
        }
        if !dmp {
            // tracks may be shared, so change each track once
            let mut done = vec![false; md.trk];
            for i in 0..md.pat {
                for k in 0..md.chn {
                    let t = md.xxp[i].index[k];
                    if done[t] {
                        continue;
                    }
                    done[t] = true;
                    for e in md.xxt[t].event.iter_mut() {
                        if e.fxt == FX_SPEED {
                            e.f2t = FX_SPEED;
                            e.f2p = if e.fxp < 0x20 { 125 } else { 6 };
                        }
                    }
                }
            }
        }
    }

    if extralen != 0 {
        let c = f.read_vec(extralen);
        let mut last_line = 0;
        let mut i = 0;
        while i + 40 <= c.len() {
            if c[i] != 0 {
                last_line = i + 40;
            }
            i += 40;
        }
        let mut s = String::new();
        let mut line = 0;
        while line < last_line {
            for &b in c[line..line + 39].iter().take_while(|&&b| b != 0) {
                s.push(b as char);
            }
            s.push('\n');
            line += 40;
        }
        m.comment = s;
    }

    for i in 0..m.module.ins {
        load_sample(m, f, SAMPLE_FLAG_UNS, i, None)?;
    }
    for i in 0..m.module.chn {
        m.module.xxc[i].pan = (pan[i] as i32) << 4;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-channel MTM with one 16-bit sample, one track and the speed/tempo
    /// effects MultiTracker resets.
    fn synth() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(b"MTM\x10");
        let mut name = [0u8; 20];
        name[..3].copy_from_slice(b"abc");
        d.extend(name);
        d.extend(1u16.to_le_bytes()); // tracks
        d.push(0); // patterns - 1
        d.push(0); // length - 1
        d.extend(0u16.to_le_bytes()); // comment
        d.push(1); // samples
        d.push(0);
        d.push(64); // rows
        d.push(2); // channels
        let mut pan = [0u8; 32];
        pan[0] = 3;
        pan[1] = 12;
        d.extend(pan);
        let mut ins = [0u8; 37];
        ins[22..26].copy_from_slice(&8u32.to_le_bytes());
        ins[30..34].copy_from_slice(&8u32.to_le_bytes());
        ins[34] = 0x0f; // finetune -1
        ins[35] = 48;
        ins[36] = 1; // 16-bit
        d.extend(ins);
        d.extend([0u8; 128]); // orders
        let mut t = [0u8; 192];
        t[0] = 12 << 2; // note
        t[1] = 0x1f; // sample 1, effect F
        t[2] = 0x03; // speed 3
        t[3 + 1] = 0x0f;
        t[3 + 2] = 0x90; // tempo 0x90 on row 1
        t[6 + 1] = 0x0e;
        t[6 + 2] = 0x85; // E85: pan
        d.extend(t);
        d.extend(1u16.to_le_bytes());
        d.extend(0u16.to_le_bytes());
        d.extend([0u8; 60]);
        for v in [0x8000u16, 0x9000, 0xa000, 0xb000] {
            d.extend(v.to_le_bytes());
        }
        d
    }

    #[test]
    fn loads_synthetic_mtm() {
        let d = synth();
        assert!(test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.type_, "MultiTracker 1.00 MTM");
        assert_eq!(m.module.name, "abc");
        assert_eq!((m.module.chn, m.module.trk, m.module.pat, m.module.len), (2, 2, 1, 1));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.ins, e.fxt, e.fxp), (49, 1, FX_SPEED, 3));
        // speed and tempo on different rows: MultiTracker resets
        assert_eq!((e.f2t, e.f2p), (FX_SPEED, 125));
        assert_eq!(m.module.event(0, 0, 1).f2p, 6);
        let e = m.module.event(0, 0, 2);
        assert_eq!((e.fxt, e.fxp), (FX_SETPAN, 0x50));
        assert_eq!(m.module.event(0, 1, 0).note, 0);
        let s = &m.module.xxs[0];
        assert_eq!((s.len, s.lpe, s.flg), (4, 4, XMP_SAMPLE_LOOP | XMP_SAMPLE_16BIT));
        assert_eq!(m.module.xxi[0].sub[0].fin, -16);
        assert_eq!(s.data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 4], [0, 0, 0, 0x10]);
        assert_eq!((m.module.xxc[0].pan, m.module.xxc[1].pan), (48, 192));
    }
}

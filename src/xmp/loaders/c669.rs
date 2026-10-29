//! Composer 669 and UNIS 669 modules (`loaders/669_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    let id = f.read16b();
    if id != 0x6966 && id != 0x4a4e {
        return false;
    }
    f.seek(110);
    if f.read8() > 64 || f.read8() > 128 {
        return false;
    }
    f.seek(240);
    f.read8() == 0xff
}

const FX: [u8; 6] = [FX_669_PORTA_UP, FX_669_PORTA_DN, FX_669_TPORTA, FX_669_FINETUNE, FX_669_VIBRATO, FX_SPEED_CP];

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut marker = [0u8; 2];
    f.read(&mut marker);
    let mut message = [0u8; 108];
    f.read(&mut message);
    let nos = f.read8() as usize;
    let nop = f.read8() as usize;
    if nos > 64 || nop > 128 {
        return Err(());
    }
    let _loop = f.read8();
    let mut order = [0u8; 128];
    let mut speed = [0u8; 128];
    let mut pbrk = [0u8; 128];
    if f.read(&mut order) != 128 || f.read(&mut speed) != 128 || f.read(&mut pbrk) != 128 {
        return Err(());
    }

    let md = &mut m.module;
    md.chn = 8;
    md.ins = nos;
    md.pat = nop;
    md.trk = md.chn * md.pat;
    md.len = order.iter().position(|&o| o as usize > nop).unwrap_or(128);
    md.xxo[..md.len].copy_from_slice(&order[..md.len]);
    md.spd = 6;
    md.bpm = 78;
    md.smp = md.ins;
    m.period_type = PeriodType::Cspd;
    m.c4rate = C4_NTSC_RATE;
    m.module.name = copy_adjust(&message, 36);
    m.module.type_ = if &marker == b"if" { "Composer 669" } else { "UNIS 669" }.to_string();
    m.comment = String::from_utf8_lossy(&message[..message.iter().position(|&b| b == 0).unwrap_or(108)]).into_owned();

    m.init_instrument();
    for i in 0..m.module.ins {
        let mut name = [0u8; 13];
        f.read(&mut name);
        let length = f.read32l();
        let loop_start = f.read32l();
        let loopend = f.read32l();
        if length > MAX_SAMPLE_SIZE as u32 {
            return Err(());
        }
        let xxs = &mut m.module.xxs[i];
        xxs.len = length as i32;
        xxs.lps = loop_start as i32;
        xxs.lpe = if loopend >= 0xfffff { 0 } else { loopend as i32 };
        xxs.flg = if xxs.lpe != 0 { XMP_SAMPLE_LOOP } else { 0 };
        let has = xxs.len > 0;
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument { vol: 0x40, pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, ..Default::default() }];
        if has {
            xxi.nsm = 1;
        }
        xxi.name = copy_adjust(&name, 13);
    }

    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
        {
            let e = m.module.event_mut(i, 0, 0);
            e.f2t = FX_SPEED_CP;
            e.f2p = speed[i];
        }
        let pb = pbrk[i] as usize;
        if pb >= 64 {
            return Err(());
        }
        {
            let e = m.module.event_mut(i, 1, pb);
            e.f2t = FX_BREAK;
            e.f2p = 0;
        }
        for j in 0..64 * 8 {
            let mut ev = [0u8; 3];
            if f.read(&mut ev) < 3 {
                return Err(());
            }
            let e = m.module.event_mut(i, j % 8, j / 8);
            if ev[0] & 0xfe != 0xfe {
                e.note = 1 + 36 + (ev[0] >> 2);
                e.ins = 1 + msn(ev[1]) + ((ev[0] & 0x03) << 4);
            }
            if ev[0] != 0xff {
                e.vol = (lsn(ev[1]) << 2) + 1;
            }
            if ev[2] != 0xff {
                if msn(ev[2]) as usize >= FX.len() {
                    continue;
                }
                e.fxt = FX[msn(ev[2]) as usize];
                e.fxp = lsn(ev[2]);
                if e.fxt == FX_SPEED_CP {
                    e.f2t = FX_PER_CANCEL;
                }
            }
        }
    }

    for i in 0..m.module.ins {
        if m.module.xxs[i].len <= 2 {
            continue;
        }
        load_sample(m, f, SAMPLE_FLAG_UNS, i, None)?;
    }
    for i in 0..m.module.chn {
        m.module.xxc[i].pan = m.defpan((i as i32 % 2) * 0xff);
    }
    m.quirk |= QUIRK_PBALL | QUIRK_PERPAT;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-pattern 669 with one 8-byte looped sample and a note on channel 0.
    pub fn synth() -> Vec<u8> {
        let mut d = vec![0u8; 0x1f1];
        d[0..2].copy_from_slice(b"if");
        d[2..6].copy_from_slice(b"test");
        d[110] = 1; // samples
        d[111] = 1; // patterns
        d[113] = 0; // order 0 = pattern 0
        for b in &mut d[114..241] {
            *b = 0xff;
        }
        d[241] = 4; // tempo
        d[369] = 63; // break row
        // sample header
        let mut s = vec![0u8; 25];
        s[..4].copy_from_slice(b"smp\0");
        s[13..17].copy_from_slice(&8u32.to_le_bytes());
        s[17..21].copy_from_slice(&0u32.to_le_bytes());
        s[21..25].copy_from_slice(&8u32.to_le_bytes());
        d.extend(s);
        let mut pat = vec![0xffu8; 64 * 8 * 3];
        for k in 0..64 * 8 {
            pat[k * 3] = 0xff;
            pat[k * 3 + 1] = 0x00;
            pat[k * 3 + 2] = 0xff;
        }
        // row 0, channel 0: note 24 (ev0 = 24 << 2), sample 1, volume 15
        pat[0] = 24 << 2;
        pat[1] = 0x0f;
        pat[2] = 0xff;
        d.extend(pat);
        d.extend([0x80u8, 0xc0, 0xff, 0xc0, 0x80, 0x40, 0x00, 0x40]);
        d
    }

    #[test]
    fn loads_synthetic_669() {
        let d = synth();
        assert!(test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.type_, "Composer 669");
        assert_eq!(m.module.name, "test");
        assert_eq!((m.module.chn, m.module.pat, m.module.len), (8, 1, 1));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.ins, e.vol), (61, 1, 61));
        assert_eq!((e.f2t, e.f2p), (FX_SPEED_CP, 4));
        assert_eq!(m.module.event(0, 1, 63).f2t, FX_BREAK);
        let s = &m.module.xxs[0];
        assert_eq!((s.len, s.lps, s.lpe, s.flg), (8, 0, 8, XMP_SAMPLE_LOOP));
        // unsigned to signed
        assert_eq!(s.data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 2], [0x00, 0x40]);
        assert_eq!(m.xtra[0].c5spd, 8363.0);
        assert_eq!(m.module.xxc[0].pan, 0);
        assert_eq!(m.module.xxc[1].pan, 0xff);
    }

    #[test]
    fn plays_synthetic_669_to_its_end() {
        let d = synth();
        let mut p = crate::xmp::XmpPlayer::new(&d, 48000).unwrap();
        let mut out = vec![0i16; 48000 * 2 * 10];
        let n = p.render(&mut out);
        assert!(p.ended());
        // 64 rows of 4 ticks at 78 BPM: 256 ticks of 48000 * 2.5 / 78 frames
        assert_eq!(n, 256 * 1538);
        // the note sounds on the left channel only (channel 0 pans hard left)
        assert!(out[..n * 2].chunks(2).any(|f| f[0] != 0));
        assert!(out[..n * 2].chunks(2).all(|f| f[1] == 0));
    }

    #[test]
    fn rejects_other_data() {
        let mut d = synth();
        d[240] = 0;
        assert!(!test(&d));
        assert!(!test(b"if"));
    }
}

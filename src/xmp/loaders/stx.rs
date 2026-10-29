//! Scream Tracker Music Interface Kit (STMIK 0.2) modules (`loaders/stx_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! STX is the Scream Tracker beta-V3.0 format STMIK plays, made by STM2STX from
//! STMs: S3M-style packed patterns with STM effects, played with the ST3 event
//! reader and quirks.

use crate::xmp::load::*;
use crate::xmp::module::*;
use crate::xmp::player::c2spd_to_note;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 64 && (&d[20..28] == b"!Scream!" || &d[20..28] == b"BMOD2STM") && &d[60..64] == b"SCRM"
}

/// `hio_seek`, which also clears the error flag.
fn seek(f: &mut Hio, pos: usize) {
    f.seek(pos);
    f.error = false;
}

const FX_NONE: u8 = 0xff;
const FX: [u8; 11] =
    [FX_NONE, FX_SPEED, FX_JUMP, FX_BREAK, FX_VOLSLIDE, FX_PORTA_DN, FX_PORTA_UP, FX_TONEPORTA, FX_VIBRATO, FX_TREMOR, FX_ARPEGGIO];

const S3M_EOR: u8 = 0;
const S3M_CH_MASK: u8 = 0x1f;
const S3M_NI_FOLLOW: u8 = 0x20;
const S3M_VOL_FOLLOWS: u8 = 0x40;
const S3M_FX_FOLLOWS: u8 = 0x80;

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut name = [0u8; 20];
    f.read(&mut name);
    let mut magic = [0u8; 8];
    f.read(&mut magic);
    let psize = f.read16l();
    let _unknown1 = f.read16l();
    let pp_pat = f.read16l() as usize;
    let pp_ins = f.read16l() as usize;
    let pp_chn = f.read16l() as usize;
    let _unknown2 = f.read16l();
    let _unknown3 = f.read16l();
    let _gvol = f.read8();
    let tempo = f.read8();
    let _unknown4 = f.read16l();
    let _unknown5 = f.read16l();
    let patnum = f.read16l() as usize;
    let insnum = f.read16l() as usize;
    let ordnum = f.read16l() as usize;
    let _unknown6 = f.read16l();
    let _unknown7 = f.read16l();
    let _unknown8 = f.read16l();
    let mut magic2 = [0u8; 4];
    f.read(&mut magic2);

    if patnum > 254 || insnum > MAX_INSTRUMENTS || ordnum > 256 {
        return Err(());
    }
    // BMOD2STM does not convert the pitch
    let bmod2stm = &magic == b"BMOD2STM";

    let md = &mut m.module;
    md.ins = insnum;
    md.pat = patnum;
    md.trk = md.pat * md.chn;
    md.len = ordnum;
    md.spd = msn(tempo) as i32;
    md.smp = md.ins;
    m.c4rate = C4_NTSC_RATE;

    // STM2STX 1.0 (released with STMIK 0.2) stores the pattern length in the
    // first two bytes of a pattern, like S3M
    seek(f, pp_pat << 4);
    let x16 = f.read16l() as usize;
    seek(f, x16 << 4);
    let broken = f.read16l() == psize;

    m.module.name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(20)]).into_owned();
    m.module.type_ = if bmod2stm { "BMOD2STM STX".to_string() } else { format!("STM2STX 1.{}", if broken { 0 } else { 1 }) };

    // parapointers to the patterns and instruments
    seek(f, pp_pat << 4);
    let ppp: Vec<usize> = (0..m.module.pat).map(|_| f.read16l() as usize).collect();
    seek(f, pp_ins << 4);
    let ppi: Vec<usize> = (0..m.module.ins).map(|_| f.read16l() as usize).collect();

    // skip the channel table (?), then the orders
    seek(f, (pp_chn << 4) + 32);
    for i in 0..m.module.len {
        m.module.xxo[i] = f.read8();
        f.skip(4);
    }

    m.init_instrument();
    for i in 0..m.module.ins {
        seek(f, ppi[i] << 4);
        let _type = f.read8();
        let mut dosname = [0u8; 13];
        f.read(&mut dosname);
        let _memseg = f.read16l();
        let length = f.read32l();
        let loopbeg = f.read32l();
        let loopend = f.read32l();
        let vol = f.read8();
        let _rsvd1 = f.read8();
        let _pack = f.read8();
        let _flags = f.read8();
        let c2spd = f.read16l();
        let _rsvd2 = f.read16l();
        f.skip(4);
        let _int_gp = f.read16l();
        let _int_512 = f.read16l();
        let _int_last = f.read32l();
        let mut iname = [0u8; 28];
        f.read(&mut iname);
        let mut imagic = [0u8; 4];
        f.read(&mut imagic);
        if f.error {
            return Err(());
        }

        let xxs = &mut m.module.xxs[i];
        xxs.len = length as i32;
        xxs.lps = loopbeg as i32;
        xxs.lpe = loopend as i32;
        if xxs.lpe == 0xffff {
            xxs.lpe = 0;
        }
        xxs.flg = if xxs.lpe > 0 { XMP_SAMPLE_LOOP } else { 0 };
        let (xpo, fin) = c2spd_to_note(c2spd as i32);
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument { vol: vol as i32, pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, xpo, fin, ..Default::default() }];
        xxi.nsm = 1;
        xxi.name = copy_adjust(&iname, 12);
    }

    let mut dummy = Event::default();
    for i in 0..m.module.pat {
        m.module.alloc_pattern_tracks(i, 64);
        if ppp[i] == 0 {
            continue;
        }
        seek(f, ppp[i] << 4);
        if broken {
            f.skip(2);
        }
        let mut r = 0;
        while r < 64 {
            let b = f.read8();
            if f.error {
                return Err(());
            }
            if b == S3M_EOR {
                r += 1;
                continue;
            }
            let c = (b & S3M_CH_MASK) as usize;
            let event = if c >= m.module.chn { &mut dummy } else { m.module.event_mut(i, c, r) };
            if b & S3M_NI_FOLLOW != 0 {
                let n = match f.read8() {
                    255 => 0, // empty note
                    254 => XMP_KEY_OFF,
                    n => 37 + 12 * msn(n) + lsn(n),
                };
                event.note = n;
                event.ins = f.read8();
            }
            if b & S3M_VOL_FOLLOWS != 0 {
                event.vol = f.read8().wrapping_add(1);
            }
            if b & S3M_FX_FOLLOWS != 0 {
                let t = f.read8() as usize;
                let p = f.read8();
                if t < FX.len() {
                    event.fxt = FX[t];
                    event.fxp = p;
                    match event.fxt {
                        FX_SPEED => event.fxp = msn(event.fxp),
                        FX_NONE => {
                            event.fxt = 0;
                            event.fxp = 0;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // the samples follow the last pattern read
    for i in 0..m.module.ins {
        load_sample(m, f, 0, i, None)?;
    }

    m.quirk |= QUIRK_VSALL | QUIRKS_ST3;
    m.read_event_type = ReadEvent::St3;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pattern, one 4-byte instrument, one order.
    fn synth() -> Vec<u8> {
        let mut d = vec![0u8; 0xe0];
        d[..3].copy_from_slice(b"abc");
        d[20..28].copy_from_slice(b"!Scream!");
        d[32..34].copy_from_slice(&4u16.to_le_bytes()); // pattern table at 0x40
        d[34..36].copy_from_slice(&5u16.to_le_bytes()); // instrument table at 0x50
        d[36..38].copy_from_slice(&6u16.to_le_bytes()); // channel table at 0x60
        d[43] = 0x60; // speed 6
        d[48] = 1; // patterns
        d[50] = 1; // instruments
        d[52] = 1; // orders
        d[60..64].copy_from_slice(b"SCRM");
        d[0x40] = 0x0e; // pattern at 0xe0
        d[0x50] = 0x09; // instrument at 0x90
        // the orders at 0x80: pattern 0
        let ins = &mut d[0x90..0xe0];
        ins[16..20].copy_from_slice(&4u32.to_le_bytes()); // length
        ins[28] = 48; // volume
        ins[32..34].copy_from_slice(&8363u16.to_le_bytes());
        ins[48..52].copy_from_slice(b"inst");
        // row 0, channel 1: C-4, instrument 1, volume 32, speed 3
        d.extend([0xe1, 0x40, 1, 32, 1, 0x30]);
        d.extend([0u8; 64]);
        d.extend([1, 2, 3, 4]);
        d
    }

    #[test]
    fn loads_synthetic_stx() {
        let d = synth();
        assert!(test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.type_, "STM2STX 1.1");
        assert_eq!((m.module.name.as_str(), m.module.chn, m.module.spd, m.module.len), ("abc", 4, 6, 1));
        let e = *m.module.event(0, 1, 0);
        assert_eq!((e.note, e.ins, e.vol, e.fxt, e.fxp), (85, 1, 33, FX_SPEED, 3));
        assert_eq!(m.module.xxi[0].name, "inst");
        assert_eq!(m.module.xxi[0].sub[0].vol, 48);
        assert_eq!(m.module.xxs[0].data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 4], [1, 2, 3, 4]);
        assert_eq!(m.read_event_type, ReadEvent::St3);
    }
}

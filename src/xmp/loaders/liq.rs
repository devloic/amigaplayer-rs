//! Liquid Tracker modules (`loaders/liq_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! libxmp's loader follows the format description by Nir Oren: packed
//! patterns stored track after track, LDSS samples, played with the ST3 event
//! reader and the Liquid Tracker flow mode (its Scream Tracker compatibility
//! flag picks the ST3-like pattern loop reset).

use crate::xmp::load::*;
use crate::xmp::module::*;

pub fn test(d: &[u8]) -> bool {
    d.len() >= 14 && &d[..14] == b"Liquid Module:"
}

const LIQ_FLAG_SCREAM_TRACKER_COMPAT: u32 = 0x02;
const LIQ_SAMPLE_16BIT: u8 = 0x01;
const LIQ_SAMPLE_STEREO: u8 = 0x02;

const NONE: u8 = 0xff;

const FX: [u8; 25] = [
    FX_ARPEGGIO,
    FX_S3M_BPM,
    FX_BREAK,
    FX_PORTA_DN,
    NONE,
    FX_FINE_VIBRATO,
    FX_GLOBALVOL,
    NONE,
    NONE,
    FX_JUMP,
    NONE,
    FX_VOLSLIDE,
    FX_EXTENDED,
    FX_TONEPORTA,
    FX_OFFSET,
    FX_SETPAN,
    NONE,
    FX_RETRIG,
    FX_S3M_SPEED,
    FX_TREMOLO,
    FX_PORTA_UP,
    FX_VIBRATO,
    NONE,
    FX_TONE_VSLIDE,
    FX_VIBRA_VSLIDE,
];

fn translate_effect(e: &mut Event) {
    let h = msn(e.fxp);
    let mut l = lsn(e.fxp);
    if e.fxt as usize >= FX.len() {
        e.fxt = 0;
        e.fxp = 0;
        return;
    }
    e.fxt = FX[e.fxt as usize];
    match e.fxt {
        // global volume (decimal)
        FX_GLOBALVOL => e.fxp = (e.fxp >> 4) * 10 + (e.fxp & 0x0f),
        FX_EXTENDED => match h {
            0x3 => e.fxp = l | (EX_GLISS << 4),
            0x4 => {
                if l & 3 == 3 {
                    l -= 1;
                }
                e.fxp = l | (EX_VIBRATO_WF << 4);
            }
            0x5 => e.fxp = l | (EX_FINETUNE << 4),
            0x6 => e.fxp = l | (EX_PATTERN_LOOP << 4),
            0x7 => {
                if l & 3 == 3 {
                    l -= 1;
                }
                e.fxp = l | (EX_TREMOLO_WF << 4);
            }
            0xc => e.fxp = l | (EX_CUT << 4),
            0xd => e.fxp = l | (EX_DELAY << 4),
            0xe => e.fxp = l | (EX_PATT_DELAY << 4),
            _ => {
                e.fxt = 0;
                e.fxp = 0;
            }
        },
        // pan control (decimal); 70 resets all channels (TODO in libxmp)
        FX_SETPAN => {
            let l = (e.fxp >> 4) * 10 + (e.fxp & 0x0f);
            if l == 70 {
                e.fxt = 0;
                e.fxp = 0;
            } else if l == 66 {
                e.fxt = FX_SURROUND;
                e.fxp = 1;
            } else if l <= 64 {
                e.fxp = (l as i32 * 0xff / 64) as u8;
            } else {
                e.fxt = 0;
                e.fxp = 0;
            }
        }
        NONE => {
            e.fxt = 0;
            e.fxp = 0;
        }
        _ => {}
    }
}

/// `liq_translate_note`: Err for a note past the 9 octaves.
fn translate_note(note: u8, e: &mut Event) -> Result<(), ()> {
    if note == 0xfe {
        e.note = XMP_KEY_OFF;
    } else {
        if note > 107 {
            return Err(());
        }
        // outside of libxmp's range: ignored
        if note as usize + 36 >= XMP_MAX_KEYS {
            return Ok(());
        }
        e.note = note + 1 + 36;
    }
    Ok(())
}

/// `decode_event`: a packed event of mask `x1`.
fn decode_event(x1: u8, e: &mut Event, f: &mut Hio) -> Result<(), ()> {
    *e = Event::default();
    if x1 & 0x01 != 0 {
        let x2 = f.read8();
        translate_note(x2, e)?;
    }
    if x1 & 0x02 != 0 {
        e.ins = f.read8().wrapping_add(1);
    }
    if x1 & 0x04 != 0 {
        e.vol = f.read8().wrapping_add(1);
    }
    e.fxt = if x1 & 0x08 != 0 { f.read8().wrapping_sub(b'A') } else { NONE };
    e.fxp = if x1 & 0x10 != 0 { f.read8() } else { 0xff };
    if e.ins > 100 || e.vol > 65 {
        return Err(());
    }
    translate_effect(e);
    Ok(())
}

/// The bytes of `r` up to a NUL, one char per byte.
fn latin1(r: &[u8]) -> String {
    r.iter().take_while(|&&b| b != 0).map(|&b| b as char).collect()
}

/// The states of libxmp's pattern decoding loop (its `goto` labels).
enum Step {
    ReadEvent,
    TestEvent,
    NextRow,
}

/// Reads pattern `i` (its header already read); `x1` and `x2` are the
/// decoder's variables, which libxmp keeps across patterns.
fn read_pattern(md: &mut Module, f: &mut Hio, i: usize, rows: usize, size: u32, x1: &mut u8, x2: &mut u8) -> Result<(), ()> {
    let chn = md.chn;
    let mut row: i32 = 0;
    let mut channel: usize = 0;
    let count = f.tell();
    // packed pattern data is stored track after track, left to right; 0xc0
    // ends the pattern
    let bad = |channel: usize, row: i32| channel >= chn || row < 0 || row as usize >= rows;
    let mut step = Step::ReadEvent;
    loop {
        match step {
            Step::ReadEvent => {
                if bad(channel, row) {
                    return Err(());
                }
                if *x2 != 0 {
                    decode_event(*x1, md.event_mut(i, channel, row as usize), f)?;
                    *x2 -= 1;
                    step = Step::NextRow;
                    continue;
                }
                *x1 = f.read8();
                step = Step::TestEvent;
            }
            Step::TestEvent => {
                if bad(channel, row) {
                    return Err(());
                }
                match *x1 {
                    // end of pattern
                    0xc0 => {
                        if f.tell() - count != size as usize {
                            return Err(());
                        }
                        return Ok(());
                    }
                    // skip channels, then next channel
                    0xe1 | 0xa0 => {
                        if *x1 == 0xe1 {
                            *x1 = f.read8();
                            channel += *x1 as usize;
                        }
                        channel += 1;
                        if channel >= chn {
                            channel -= 1;
                        }
                        row = -1;
                        step = Step::NextRow;
                        continue;
                    }
                    // skip rows, then next row
                    0xe0 | 0x80 => {
                        if *x1 == 0xe0 {
                            *x1 = f.read8();
                            row += *x1 as i32;
                        }
                        step = Step::NextRow;
                        continue;
                    }
                    _ => {}
                }
                let x = *x1;
                if x > 0xc0 && x < 0xe0 {
                    // packed data
                    decode_event(x, md.event_mut(i, channel, row as usize), f)?;
                    step = Step::NextRow;
                    continue;
                }
                if x > 0xa0 && x < 0xc0 {
                    // packed data, repeated
                    *x2 = f.read8();
                    decode_event(x, md.event_mut(i, channel, row as usize), f)?;
                    step = Step::NextRow;
                    continue;
                }
                if x > 0x80 && x < 0xa0 {
                    // packed data, repeated keeping the note
                    *x2 = f.read8();
                    decode_event(x, md.event_mut(i, channel, row as usize), f)?;
                    let e = *md.event(i, channel, row as usize);
                    while *x2 != 0 {
                        row += 1;
                        if row as usize >= rows {
                            return Err(());
                        }
                        *md.event_mut(i, channel, row as usize) = e;
                        *x2 -= 1;
                    }
                    step = Step::NextRow;
                    continue;
                }

                // unpacked data
                translate_note(x, md.event_mut(i, channel, row as usize))?;
                *x1 = f.read8();
                if *x1 > 100 {
                    row += 1;
                    step = Step::TestEvent;
                    continue;
                }
                let e = md.event_mut(i, channel, row as usize);
                if *x1 != 0xff {
                    e.ins = *x1 + 1;
                }
                *x1 = f.read8();
                if *x1 != 0xff {
                    e.vol = x1.wrapping_add(1);
                }
                *x1 = f.read8();
                if *x1 != 0xff {
                    e.fxt = x1.wrapping_sub(b'A');
                }
                *x1 = f.read8();
                e.fxp = *x1;
                if e.ins > 100 || e.vol > 65 {
                    return Err(());
                }
                translate_effect(e);
                step = Step::NextRow;
            }
            Step::NextRow => {
                row += 1;
                if row as usize >= rows {
                    row = 0;
                    *x2 = 0;
                    channel += 1;
                }
                if channel >= chn {
                    channel = 0;
                }
                step = Step::ReadEvent;
            }
        }
    }
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut magic = [0u8; 14];
    f.read(&mut magic);
    let mut name = [0u8; 30];
    f.read(&mut name);
    let mut author = [0u8; 20];
    f.read(&mut author);
    f.read8();
    let mut tracker = [0u8; 20];
    f.read(&mut tracker);
    let version = f.read16l();
    let speed = f.read16l();
    let bpm = f.read16l();
    let _low = f.read16l();
    let _high = f.read16l();
    let chn = f.read16l() as usize;
    let flags = f.read32l();
    let pat = f.read16l() as usize;
    let ins = f.read16l() as usize;
    let mut len = f.read16l() as usize;
    let mut hdrsz = f.read16l() as usize;

    if chn > XMP_MAX_CHANNELS || pat > 256 || ins > 256 {
        return Err(());
    }

    let (num_channels_stored, num_orders_stored);
    if version >> 8 == 0 {
        hdrsz = len;
        len = 0;
        // skip 3 of 5 undocumented bytes (2 already read)
        f.seek_cur(3);
        num_channels_stored = 64;
        num_orders_stored = 256;
        if chn > 64 {
            return Err(());
        }
    } else {
        num_channels_stored = chn;
        num_orders_stored = len;
    }
    if len > 256 {
        return Err(());
    }

    let md = &mut m.module;
    md.spd = speed as i32;
    md.bpm = bpm.min(255) as i32;
    md.chn = chn;
    md.pat = pat;
    md.ins = ins;
    md.smp = ins;
    md.len = len;
    md.trk = md.chn * md.pat;
    m.quirk |= QUIRK_INSVOL;

    m.module.name = latin1(&name);
    // the tracker name without its trailing spaces
    let mut tn = [0u8; 21];
    tn[..20].copy_from_slice(&tracker);
    for k in (0..=20).rev() {
        if tn[k] == 0x20 {
            tn[k] = 0;
        }
        if tn[k] != 0 {
            break;
        }
    }
    let t = format!("{} LIQ {}.{:02}", latin1(&tn), version >> 8, version & 0xff);
    m.module.type_ = t.chars().take(XMP_NAME_SIZE - 1).collect();

    for i in 0..chn {
        let mut pan = f.read8();
        if pan < 64 {
            pan <<= 2;
        } else if pan == 64 {
            pan = 0xff;
        } else if pan == 66 {
            pan = 0x80;
            m.module.xxc[i].flg |= XMP_CHANNEL_SURROUND;
        } else {
            return Err(());
        }
        m.module.xxc[i].pan = pan as i32;
    }
    if chn < num_channels_stored {
        f.seek_cur((num_channels_stored - chn) as i64);
    }
    for i in 0..chn {
        m.module.xxc[i].vol = f.read8() as i32;
    }
    if chn < num_channels_stored {
        f.seek_cur((num_channels_stored - chn) as i64);
    }
    let n = f.read(&mut m.module.xxo[..num_orders_stored]);
    let _ = n;

    // skip the 1.01 echo pools
    f.seek(hdrsz);

    // version 0.00 does not store the length
    if version < 0x100 {
        m.module.len = m.module.xxo.iter().position(|&o| o == 0xff).unwrap_or(256);
    }

    m.c4rate = C4_NTSC_RATE;

    let (mut x1, mut x2) = (0u8, 0u8);
    for i in 0..m.module.pat {
        // libxmp_alloc_pattern, then the tracks once the rows are known
        let pmag = f.read32b();
        // a magic of !!!! does nothing special, despite the documentation
        if pmag != u32::from_be_bytes(*b"LP\0\0") {
            return Err(());
        }
        let mut pname = [0u8; 30];
        f.read(&mut pname);
        let rows = f.read16l() as usize;
        let size = f.read32l();
        let _reserved = f.read32l();
        if rows > 256 {
            return Err(());
        }
        // libxmp_alloc_tracks_in_pattern fails (unchecked) for 0 rows; the
        // decoder's row check then rejects the module
        if rows == 0 {
            return Err(());
        }
        m.module.alloc_pattern_tracks(i, rows);
        read_pattern(&mut m.module, f, i, rows, size, &mut x1, &mut x2)?;
    }

    // instruments
    m.init_instrument();
    for i in 0..m.module.ins {
        m.module.xxi[i].sub = vec![SubInstrument::default()];

        let ldss_magic = f.read32b();
        if ldss_magic == u32::from_be_bytes(*b"????") {
            continue;
        }
        if ldss_magic != u32::from_be_bytes(*b"LDSS") {
            return Err(());
        }
        let ver = f.read16l();
        let mut iname = [0u8; 30];
        f.read(&mut iname);
        let mut editor = [0u8; 20];
        f.read(&mut editor);
        let mut iauthor = [0u8; 20];
        f.read(&mut iauthor);
        let _hw_id = f.read8();
        let length = f.read32l();
        let loopstart = f.read32l();
        let loopend = f.read32l();
        let c2spd = f.read32l();
        let vol = f.read8();
        let iflags = f.read8();
        let ipan = f.read8();
        let _midi_ins = f.read8();
        let mut gvl = f.read8();
        let _chord = f.read8();
        let ihdrsz = f.read16l();
        let _comp = f.read16l();
        let _crc = f.read32l();
        let _midi_ch = f.read8();
        let loop_type = f.read8();
        let mut rsvd = [0u8; 10];
        f.read(&mut rsvd);
        let mut filename = [0u8; 25];
        f.read(&mut filename);
        if f.take_error() {
            return Err(());
        }

        let xxi = &mut m.module.xxi[i];
        xxi.nsm = (length != 0) as i32;
        xxi.vol = 0x40;

        let xxs = &mut m.module.xxs[i];
        xxs.len = length as i32;
        xxs.lps = loopstart as i32;
        xxs.lpe = loopend as i32;
        // LIQ_SAMPLE_SIGNED is ignored by Liquid Tracker 1.50: all samples
        // are signed
        if iflags & LIQ_SAMPLE_16BIT != 0 {
            xxs.flg |= XMP_SAMPLE_16BIT;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }
        // stored like S3M: left, then right
        if iflags & LIQ_SAMPLE_STEREO != 0 {
            xxs.flg |= XMP_SAMPLE_STEREO;
            xxs.len >>= 1;
            xxs.lps >>= 1;
            xxs.lpe >>= 1;
        }
        if loopend > 0 {
            xxs.flg |= XMP_SAMPLE_LOOP;
            if loop_type == 1 {
                xxs.flg |= XMP_SAMPLE_LOOP_BIDIR;
            }
        }
        let has_data = xxs.len != 0;

        // global volume came with LDSS 1.01: 0-64, 32 = 100%
        if ver < 0x101 {
            gvl = 0x20;
        }
        let (xpo, fin) = c2spd_to_note(c2spd as i32);
        let xxi = &mut m.module.xxi[i];
        xxi.sub[0] = SubInstrument {
            vol: vol as i32,
            gvl: gvl as i32,
            // TODO in libxmp: 66 = surround
            pan: if ipan <= 64 { (ipan as i32 * 4).min(255) } else { XMP_INST_NO_DEFAULT_PAN },
            sid: i as i32,
            xpo,
            fin,
            ..Default::default()
        };
        xxi.name = copy_adjust(&iname, 30);

        f.seek_cur(ihdrsz as i64 - 0x90);
        if !has_data {
            continue;
        }
        load_sample(m, f, 0, i, None)?;
    }

    m.quirk |= QUIRK_FINEFX | QUIRK_RTONCE;
    m.flow_mode = if flags & LIQ_FLAG_SCREAM_TRACKER_COMPAT != 0 { FLOW_MODE_LIQUID_COMPAT } else { FLOW_MODE_LIQUID };
    m.read_event_type = ReadEvent::St3;

    // Channel volume and instrument global volume are both "normally" 32 and
    // go up to 64 (2x gain each): libxmp keeps them and raises the mix volume
    // 2x for each instead
    m.mvol = 48 * 2 * 2;
    m.mvolbase = 48;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1.00 module: 2 channels, one 2-row pattern, one 4-byte sample.
    fn synth() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(b"Liquid Module:");
        let mut name = [0u8; 30];
        name[..4].copy_from_slice(b"song");
        d.extend(name);
        d.extend([0u8; 20]);
        d.push(0x1a);
        let mut tr = [b' '; 20];
        tr[..12].copy_from_slice(b"LiquidTrackr");
        d.extend(tr);
        for v in [0x100u16, 6, 125, 0, 0, 2] {
            d.extend(v.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes()); // flags
        let hdrsz = 109 + 2 + 2 + 1;
        for v in [1u16, 1, 1, hdrsz] {
            d.extend(v.to_le_bytes());
        }
        d.extend([0u8, 64]); // pans
        d.extend([32u8, 32]); // volumes
        d.push(0); // orders
        assert_eq!(d.len(), hdrsz as usize);
        // pattern: channel 0 row 0 note C-4 (36) ins 0, then next channel
        let mut p = vec![0xc3u8, 36, 0, 0xa0, 0x80, 0xc0];
        d.extend(b"LP\0\0");
        d.extend([0u8; 30]);
        d.extend(2u16.to_le_bytes());
        d.extend((p.len() as u32).to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d.append(&mut p);
        // instrument
        d.extend(b"LDSS");
        d.extend(0x101u16.to_le_bytes());
        d.extend([0u8; 70]);
        d.push(0);
        for v in [4u32, 0, 0, 8363] {
            d.extend(v.to_le_bytes());
        }
        d.extend([64u8, 0, 32, 0, 32, 0]);
        d.extend(0x90u16.to_le_bytes());
        d.extend([0u8; 2 + 4 + 2 + 10 + 25]);
        d.extend([1, 2, 3, 4]);
        d
    }

    #[test]
    fn loads_synthetic_liq() {
        let d = synth();
        assert!(test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.type_, "LiquidTrackr LIQ 1.00");
        assert_eq!((m.module.chn, m.module.len, m.module.xxc[1].pan), (2, 1, 0xff));
        let e = *m.module.event(0, 0, 0);
        assert_eq!((e.note, e.ins), (73, 1));
        assert_eq!(m.module.xxs[0].data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 4], [1, 2, 3, 4]);
        assert_eq!((m.flow_mode, m.read_event_type), (FLOW_MODE_LIQUID, ReadEvent::St3));
    }
}

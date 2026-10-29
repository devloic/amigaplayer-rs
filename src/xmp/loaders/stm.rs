//! Scream Tracker 2 modules and songs (`loaders/stm_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! STM: four channels of 64-row unpacked patterns, the tempo derived from ST2's
//! mix buffer size, played with the ST3 event reader and quirks. A song (type 1)
//! keeps its samples in files named after the instruments, looked for next to
//! the song (or in `XMP_INSTRUMENT_PATH`) as libxmp does.

use crate::xmp::load::*;
use crate::xmp::module::*;

/// ST2's tempo depends on its mix buffer size, so on its mix rate: the highest
/// of the released versions.
const STM_MIX_RATE: i32 = 23863;
const STM_TYPE_SONG: u8 = 0x01;
const STM_TYPE_MODULE: u8 = 0x02;

fn known_version(v: i32) -> bool {
    matches!(v, 110 | 200 | 210 | 220 | 221)
}

/// `libxmp_test_name` with no flags: ASCII, no control characters but
/// backspace and 0x0e.
fn test_name(s: &[u8]) -> bool {
    s.iter().all(|&b| b <= 0x7f && !(b > 0 && b < 32 && b != 0x08 && b != 0x0e))
}

pub fn test(d: &[u8]) -> bool {
    if d.len() < 64 || !test_name(&d[20..28]) {
        return false;
    }
    // EOF should be 0x1a; putup10.stm and putup11.stm have 2 instead
    if d[28] != 0x1a && d[28] != 0x02 {
        return false;
    }
    if d[29] > STM_TYPE_MODULE {
        return false;
    }
    if !known_version(100 * d[30] as i32 + d[31] as i32) {
        return false;
    }
    // not STX files
    &d[60..64] != b"SCRM"
}

const FX_NONE: u8 = 0xff;
const FX: [u8; 16] = [
    FX_NONE,
    FX_SPEED,     // A - Set tempo to [INFO]. 60 normal.
    FX_JUMP,      // B - Break pattern and jmp to order [INFO]
    FX_BREAK,     // C - Break pattern
    FX_VOLSLIDE,  // D - Slide volume; Hi-nibble=up, Lo-nibble=down
    FX_PORTA_DN,  // E - Slide down at speed [INFO]
    FX_PORTA_UP,  // F - Slide up at speed [INFO]
    FX_TONEPORTA, // G - Slide to the note specified at speed [INFO]
    FX_VIBRATO,   // H - Vibrato; Hi-nibble, speed. Lo-nibble, size
    FX_TREMOR,    // I - Tremor; Hi-nibble, ontime. Lo-nibble, offtime
    FX_ARPEGGIO,  // J - Arpeggio
    FX_NONE,
    FX_NONE,
    FX_NONE,
    FX_NONE,
    FX_NONE,
];

/// `stm_calculate_bpm`, after cs127's research: the divisor may be negative,
/// and the effective BPM is at most 125.
fn calculate_bpm(spd: i32, tempo_factor: i32) -> i32 {
    const SPEED_FACTOR: [i32; 16] = [140, 50, 25, 15, 10, 7, 6, 4, 3, 3, 2, 2, 2, 2, 1, 1];
    let divisor = 50 - ((SPEED_FACTOR[spd as usize] * tempo_factor) >> 4);
    let tick_frames = if divisor != 0 { STM_MIX_RATE / divisor } else { -1 };
    let tick_frames = (tick_frames as u32 & 0xffff) as i32;
    (STM_MIX_RATE * 5 + tick_frames) / (tick_frames * 2)
}

/// `stm_convert_tempo`: speed and BPM of a tempo byte.
fn convert_tempo(tempo: u8, version: i32) -> (i32, i32) {
    let (spd, bpm) = if version >= 221 {
        let spd = msn(tempo) as i32;
        (spd, calculate_bpm(spd, lsn(tempo) as i32))
    } else if version >= 110 {
        let spd = tempo as i32 / 10;
        (spd, calculate_bpm(spd.min(15), tempo as i32 % 10))
    } else {
        (tempo as i32, 125)
    };
    (spd.max(1), bpm.max(XMP_MIN_BPM))
}

#[derive(Default, Clone, Copy)]
struct InsHeader {
    name: [u8; 13],
    rsvd1: u16,
    length: u16,
    loopbeg: u16,
    loopend: u16,
    volume: u8,
    c2spd: u16,
}

/// `libxmp_copy_name_for_fopen`: the instrument name as a relative file name,
/// or None for one that could reach outside the directory.
fn name_for_fopen(name: &str, n: usize) -> Option<String> {
    let b = name.as_bytes();
    if name == "." || name.contains("..") || b.is_empty() || matches!(b[0], b'\\' | b'/' | b':') {
        return None;
    }
    let mut out = String::new();
    let mut converted_colon = false;
    for i in 0..b.len().min(n - 1) {
        let t = b[i];
        if !(32..0x7f).contains(&t) {
            return None;
        }
        if i > 0 && t == b':' && !converted_colon {
            let t2 = b.get(i + 1).copied().unwrap_or(0);
            if t2 == 0 || t2 == b'/' || t2 == b'\\' {
                return None;
            }
            converted_colon = true;
            out.push('/');
            continue;
        }
        out.push(if t == b'\\' { '/' } else { t as char });
    }
    Some(out)
}

/// `libxmp_find_instrument_file` + `hio_open`: the sample file `name` in
/// `XMP_INSTRUMENT_PATH`, else in the song's directory. libxmp matches the
/// directory entry ignoring case, then opens the name as given.
fn read_instrument_file(m: &ModuleData, name: &str) -> Option<Vec<u8>> {
    let probe = |dir: &str| -> Option<Vec<u8>> {
        let dir = if dir.is_empty() { "." } else { dir };
        let found = std::fs::read_dir(dir).ok()?.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name));
        if !found {
            return None;
        }
        std::fs::read(std::path::Path::new(dir).join(name)).ok()
    };
    if let Ok(p) = std::env::var("XMP_INSTRUMENT_PATH") {
        if let Some(d) = probe(&p) {
            return Some(d);
        }
    }
    // a module loaded from memory has no directory
    if m.basename.is_empty() {
        return None;
    }
    probe(&m.dirname)
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut name = [0u8; 20];
    f.read(&mut name);
    let mut magic = [0u8; 8];
    f.read(&mut magic);
    let _rsvd1 = f.read8();
    let stype = f.read8();
    let vermaj = f.read8();
    let vermin = f.read8();
    let version = 100 * vermaj as i32 + vermin as i32;
    if !known_version(version) {
        return Err(());
    }

    if version >= 200 {
        let tempo = f.read8();
        let patterns = f.read8();
        let _gvol = f.read8();
        f.skip(13);
        let md = &mut m.module;
        md.chn = 4;
        md.pat = patterns as usize;
        (md.spd, md.bpm) = convert_tempo(tempo, version);
        md.ins = 31;
        md.len = if version == 200 { 64 } else { 128 };
    } else {
        let insnum = f.read16l() as usize;
        if insnum > 32 {
            return Err(());
        }
        let ordnum = f.read16l() as usize;
        if ordnum > XMP_MAX_MOD_LENGTH {
            return Err(());
        }
        let patnum = f.read16l() as usize;
        if patnum > XMP_MAX_MOD_LENGTH {
            return Err(());
        }
        let _srate = f.read16l();
        let tempo = f.read8();
        let channels = f.read8();
        if channels != 4 {
            return Err(());
        }
        if f.read16l() != 64 {
            return Err(());
        }
        let _rsvd2 = f.read16l();
        let skip = f.read16l();
        f.seek_cur(skip as i64);
        let md = &mut m.module;
        md.chn = channels as usize;
        md.pat = patnum;
        (md.spd, md.bpm) = convert_tempo(tempo, version);
        md.ins = insnum;
        md.len = ordnum;
    }

    let mut ins = [InsHeader::default(); 32];
    for h in ins.iter_mut().take(m.module.ins) {
        f.read(&mut h.name);
        let _idisk = f.read8();
        h.rsvd1 = f.read16l();
        h.length = f.read16l();
        h.loopbeg = f.read16l();
        h.loopend = f.read16l();
        h.volume = f.read8();
        let _rsvd2 = f.read8();
        h.c2spd = f.read16l();
        let _rsvd3 = f.read32l();
        let _paralen = f.read16l();
    }
    if f.take_error() {
        return Err(());
    }

    m.module.smp = m.module.ins;
    m.c4rate = C4_NTSC_RATE;
    m.module.name = copy_adjust(&name, 20);
    m.module.type_ = if magic[0] == 0 || &magic[..5] == b"PCSTV" || &magic == b"!Scream!" {
        format!("Scream Tracker {vermaj}.{vermin:02}")
    } else if &magic == b"SWavePro" {
        format!("SoundWave Pro {vermaj}.{vermin:02}")
    } else {
        copy_adjust(&magic, 8)
    };

    m.init_instrument();
    for (i, h) in ins.iter().enumerate().take(m.module.ins) {
        let xxs = &mut m.module.xxs[i];
        xxs.len = h.length as i32;
        xxs.lps = h.loopbeg as i32;
        xxs.lpe = h.loopend as i32;
        if xxs.lpe == 0xffff {
            xxs.lpe = 0;
        }
        xxs.flg = if xxs.lpe > 0 { XMP_SAMPLE_LOOP } else { 0 };
        let (xpo, fin) = c2spd_to_note(h.c2spd as i32);
        let xxi = &mut m.module.xxi[i];
        xxi.sub = vec![SubInstrument { vol: h.volume as i32, pan: XMP_INST_NO_DEFAULT_PAN, sid: i as i32, xpo, fin, ..Default::default() }];
        if h.length > 0 {
            xxi.nsm = 1;
        }
        xxi.name = copy_adjust(&h.name, 12);
    }

    let len = m.module.len;
    if f.read(&mut m.module.xxo[..len]) < len {
        return Err(());
    }
    let mut blank_pattern = false;
    let mut i = 0;
    while i < len {
        let o = m.module.xxo[i];
        if o >= 99 {
            break;
        }
        // patterns past the count are valid blank patterns (jimmy.stm,
        // Rauno/dogs.stm, Skaven/hevijanis istu maas.stm)
        if o as usize >= m.module.pat {
            m.module.xxo[i] = m.module.pat as u8;
            blank_pattern = true;
        }
        i += 1;
    }
    let stored_patterns = m.module.pat;
    if blank_pattern {
        m.module.pat += 1;
    }
    m.module.trk = m.module.pat * m.module.chn;
    m.module.len = i;

    if blank_pattern {
        m.module.alloc_pattern_tracks(stored_patterns, 64);
    }
    for i in 0..stored_patterns {
        m.module.alloc_pattern_tracks(i, 64);
        if f.take_error() {
            return Err(());
        }
        for j in 0..64 {
            for k in 0..m.module.chn {
                let event = m.module.event_mut(i, k, j);
                let b = f.read8();
                if b == 251 || b == 252 {
                    continue; // empty note
                }
                if b == 253 {
                    event.note = XMP_KEY_OFF;
                    continue;
                }
                event.note = match b {
                    254 => XMP_KEY_OFF,
                    255 => 0,
                    _ => 1 + lsn(b) + 12 * (3 + msn(b)),
                };
                let b = f.read8();
                event.vol = b & 0x07;
                event.ins = (b & 0xf8) >> 3;
                let b = f.read8();
                event.vol += (b & 0xf0) >> 1;
                if version >= 200 {
                    event.vol = if event.vol > 0x40 { 0 } else { event.vol + 1 };
                } else if event.vol > 0 {
                    event.vol = if event.vol > 0x40 { 1 } else { event.vol + 1 };
                }
                event.fxt = FX[lsn(b) as usize];
                event.fxp = f.read8();
                match event.fxt {
                    // ST2 always breaks to row 0
                    FX_BREAK => event.fxp = 0,
                    FX_SPEED => {
                        if event.fxp != 0 {
                            let (spd, bpm) = convert_tempo(event.fxp, version);
                            event.fxt = FX_S3M_SPEED;
                            event.fxp = spd as u8;
                            if version >= 110 {
                                event.f2t = FX_S3M_BPM;
                                event.f2p = bpm as u8;
                            }
                        } else {
                            // A00 is a no-op
                            event.fxt = 0;
                            event.fxp = 0;
                        }
                    }
                    FX_NONE => {
                        event.fxt = 0;
                        event.fxp = 0;
                    }
                    _ => {}
                }
            }
        }
    }

    for (i, h) in ins.iter().enumerate().take(m.module.ins) {
        if h.volume == 0 || h.length == 0 {
            m.module.xxi[i].nsm = 0;
            continue;
        }
        if stype == STM_TYPE_SONG {
            let Some(fname) = name_for_fopen(&m.module.xxi[i].name, 32) else { continue };
            let Some(data) = read_instrument_file(m, &fname) else { continue };
            load_sample(m, &mut Hio::new(&data), SAMPLE_FLAG_UNS, i, None)?;
        } else {
            f.seek((h.rsvd1 as usize) << 4);
            load_sample(m, f, 0, i, None)?;
        }
    }

    m.quirk |= QUIRK_VSALL | QUIRKS_ST3;
    m.flow_mode = FLOW_MODE_ST2;
    m.read_event_type = ReadEvent::St3;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2.21 module: one pattern, one 4-byte sample, one order.
    fn synth() -> Vec<u8> {
        let mut d = vec![0u8; 48];
        d[..3].copy_from_slice(b"abc");
        d[20..28].copy_from_slice(b"!Scream!");
        d[28] = 0x1a;
        d[29] = STM_TYPE_MODULE;
        d[30] = 2;
        d[31] = 21;
        d[32] = 0x60; // speed 6, tempo factor 0
        d[33] = 1; // patterns
        d[34] = 64;
        let mut ins = [0u8; 32 * 31];
        ins[..4].copy_from_slice(b"SMP1");
        ins[14..16].copy_from_slice(&(0x800u16 >> 4).to_le_bytes());
        ins[16..18].copy_from_slice(&4u16.to_le_bytes());
        ins[20..22].copy_from_slice(&0xffffu16.to_le_bytes());
        ins[22] = 64;
        ins[24..26].copy_from_slice(&8363u16.to_le_bytes());
        d.extend(ins);
        let mut ord = [99u8; 128];
        ord[0] = 0;
        d.extend(ord);
        // one byte for an empty event (251), four for the others
        for k in 0..64 * 4 {
            match k {
                // row 0, channel 1: C-4 (0x10 is octave 1), instrument 1, volume 32, A40
                1 => d.extend([0x10, 1 << 3, (4 << 4) | 1, 0x40]),
                // row 0, channel 2: no note, C10
                2 => d.extend([0xff, 0x00, 0x03, 0x10]),
                _ => d.push(251),
            }
        }
        d.resize(0x800, 0);
        d.extend([1, 2, 3, 4]);
        d
    }

    #[test]
    fn loads_synthetic_stm() {
        let d = synth();
        assert!(test(&d));
        assert!(!crate::xmp::loaders::stx::test(&d));
        let mut m = ModuleData::default();
        load(&mut m, &mut Hio::new(&d)).unwrap();
        assert_eq!(m.module.type_, "Scream Tracker 2.21");
        assert_eq!((m.module.name.as_str(), m.module.chn, m.module.pat, m.module.len), ("abc", 4, 1, 1));
        assert_eq!((m.module.spd, m.module.bpm), (6, 125));
        let e = *m.module.event(0, 1, 0);
        assert_eq!((e.note, e.ins, e.vol, e.fxt, e.fxp, e.f2t), (1 + 12 * 4, 1, 33, FX_S3M_SPEED, 4, FX_S3M_BPM));
        let e = *m.module.event(0, 2, 0);
        assert_eq!((e.note, e.fxt, e.fxp), (0, FX_BREAK, 0));
        assert_eq!(m.module.xxs[0].lpe, 0);
        assert_eq!(m.module.xxi[0].name, "SMP1");
        assert_eq!(m.module.xxs[0].data.as_ref().unwrap()[SAMPLE_GUARD..SAMPLE_GUARD + 4], [1, 2, 3, 4]);
        assert_eq!(m.flow_mode, FLOW_MODE_ST2);
    }

    #[test]
    fn tempos() {
        assert_eq!(convert_tempo(0x60, 221), (6, 125));
        assert_eq!(convert_tempo(60, 210), (6, 125));
        assert_eq!(calculate_bpm(0, 15), (STM_MIX_RATE * 5 + 65242) / (65242 * 2));
    }
}

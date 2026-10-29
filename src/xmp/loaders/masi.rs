//! Epic MegaGames MASI and Sinaria PSM modules (`loaders/masi_load.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).
//!
//! Originally based on the PSM loader from Modplug by Olivier Lapicque. Sinaria
//! PSMs use eight character pattern and instrument IDs, a different sample
//! header, plain note values and a finer effect scale.

use crate::xmp::iff::{self, IffLoader, IFF_LITTLE_ENDIAN};
use crate::xmp::load::*;
use crate::xmp::module::*;
use crate::xmp::player::c2spd_to_note;

const MAGIC_PSM: u32 = u32::from_be_bytes(*b"PSM ");
const MAGIC_FILE: u32 = u32::from_be_bytes(*b"FILE");
const MAGIC_OPLH: u32 = u32::from_be_bytes(*b"OPLH");
const MAGIC_PPAN: u32 = u32::from_be_bytes(*b"PPAN");

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    if f.read32b() != MAGIC_PSM {
        return false;
    }
    f.read8();
    f.read8();
    f.read8();
    if f.read8() != 0 {
        return false;
    }
    if f.read32b() != MAGIC_FILE {
        return false;
    }
    f.read32b();
    let val = f.read32l() as i32;
    f.seek_cur(val as i64);
    // the title (TITL chunk) is not needed to claim the file
    true
}

#[derive(Default)]
struct Local {
    sinaria: bool,
    cur_pat: usize,
    cur_ins: usize,
    pnam: Vec<u8>,
    pord: Vec<u8>,
}

fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]).into_owned()
}

fn get_sdft(_id: &[u8], _size: u32, _m: &mut ModuleData, _f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    Ok(())
}

fn get_titl(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    let size = (size as usize).min(XMP_NAME_SIZE - 1);
    let mut buf = [0u8; XMP_NAME_SIZE];
    let n = f.read(&mut buf[..size]);
    m.module.name = cstr(&buf[..n]);
    Ok(())
}

fn get_dsmp_cnt(_id: &[u8], _size: u32, m: &mut ModuleData, _f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    m.module.ins += 1;
    m.module.smp = m.module.ins;
    Ok(())
}

fn get_pbod_cnt(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    m.module.pat += 1;
    let mut buf = [0u8; 20];
    if f.read(&mut buf) < 20 {
        return Err(());
    }
    if buf[9] != 0 && buf[13] == 0 {
        d.sinaria = true;
    }
    Ok(())
}

fn get_dsmp(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    let flags = f.read8(); // flags
    f.skip(8); // songname
    f.skip(if d.sinaria { 8 } else { 4 }); // smpid

    let i = d.cur_ins;
    if i >= m.module.ins {
        return Err(());
    }

    let mut name = [0u8; 31];
    f.read(&mut name);
    f.skip(8);
    f.read8(); // insno
    f.read8();
    let len = f.read32l() as i32;
    let lps = f.read32l() as i32;
    let mut lpe = f.read32l() as i32;
    let flg = if flags & 0x80 != 0 { XMP_SAMPLE_LOOP } else { 0 };
    f.read16l();

    if lpe < 0 {
        lpe = 0;
    }

    let finetune = if d.sinaria { (f.read8() << 4) as i8 as i32 } else { 0 };

    let vol = (f.read8() / 2 + 1) as i32;
    f.read32l();
    let srate = f.read16l() as i32;

    let (xpo, fin) = c2spd_to_note(srate);

    let xxs = &mut m.module.xxs[i];
    xxs.len = len;
    xxs.lps = lps;
    xxs.lpe = lpe;
    xxs.flg = flg;
    let xxi = &mut m.module.xxi[i];
    xxi.name = cstr(&name);
    if len > 0 {
        xxi.nsm = 1;
    }
    xxi.sub = vec![SubInstrument {
        vol,
        pan: XMP_INST_NO_DEFAULT_PAN,
        sid: i as i32,
        xpo,
        fin: fin + finetune,
        ..Default::default()
    }];

    f.skip(16);
    load_sample(m, f, SAMPLE_FLAG_8BDIFF, i, None)?;

    d.cur_ins += 1;
    Ok(())
}

fn convert_porta(param: u8, sinaria: bool) -> u8 {
    if sinaria {
        param
    } else if param < 4 {
        param | 0xf0
    } else {
        param >> 2
    }
}

fn get_pbod(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    let i = d.cur_pat;

    f.read32l();
    let n = if d.sinaria { 8 } else { 4 };
    if i >= m.module.pat {
        return Err(());
    }
    f.read(&mut d.pnam[i * 8..i * 8 + n]);

    let rows = f.read16l() as usize;
    if f.take_error() {
        return Err(());
    }

    if rows == 0 || rows > 256 {
        return Err(());
    }
    m.module.alloc_pattern_tracks(i, rows);

    let chn = m.module.chn;
    let mut dummy = Event::default();
    let mut r = 0;

    loop {
        let mut rowlen = f.read16l() as i32 - 2;
        if f.take_error() {
            return Err(());
        }
        while rowlen > 0 {
            let flag = f.read8();

            if rowlen == 1 {
                break;
            }

            let chan = f.read8() as usize;
            rowlen -= 2;

            let event = if chan < chn { m.module.event_mut(i, chan, r) } else { &mut dummy };

            if flag & 0x80 != 0 {
                let mut note = f.read8();
                rowlen -= 1;
                if d.sinaria {
                    note = note.wrapping_add(36);
                } else {
                    note = ((note >> 4) as u32 * 12 + (note & 0x0f) as u32 + 1 + 12) as u8;
                }
                event.note = note;
            }

            if flag & 0x40 != 0 {
                event.ins = f.read8().wrapping_add(1);
                rowlen -= 1;
            }

            if flag & 0x20 != 0 {
                event.vol = f.read8() / 2 + 1;
                rowlen -= 1;
            }

            if flag & 0x10 != 0 {
                let mut fxt = f.read8();
                let mut fxp = f.read8();
                rowlen -= 2;

                match fxt {
                    // Volume slide
                    0x01 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_F_VSLIDE_UP << 4) | ((fxp / 2) & 0x0f);
                    }
                    0x02 => {
                        fxt = FX_VOLSLIDE;
                        fxp = (fxp / 2) << 4;
                    }
                    0x03 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_F_VSLIDE_DN << 4) | ((fxp / 2) & 0x0f);
                    }
                    0x04 => {
                        fxt = FX_VOLSLIDE;
                        fxp /= 2;
                    }

                    // Portamento
                    0x0b => {
                        fxt = FX_PORTA_UP;
                        fxp = (EX_F_PORTA_UP << 4) | convert_porta(fxp, d.sinaria);
                    }
                    0x0c => {
                        fxt = FX_PORTA_UP;
                        fxp = convert_porta(fxp, d.sinaria);
                    }
                    0x0d => {
                        fxt = FX_PORTA_DN;
                        fxp = (EX_F_PORTA_DN << 4) | convert_porta(fxp, d.sinaria);
                    }
                    0x0e => {
                        fxt = FX_PORTA_DN;
                        fxp = convert_porta(fxp, d.sinaria);
                    }
                    0x0f => {
                        fxt = FX_TONEPORTA;
                        fxp >>= 2;
                    }
                    0x10 => {
                        // toneporta + vslide up (libxmp masks the new effect type)
                        fxt = FX_TONE_VSLIDE;
                        fxp = fxt & 0xf0;
                    }
                    0x11 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_GLISS << 4) | (fxp & 0x0f);
                    }
                    0x12 => {
                        fxt = FX_TONE_VSLIDE;
                        fxp >>= 4;
                    }

                    // Vibrato
                    0x15 => {
                        fxt = if d.sinaria { FX_VIBRATO } else { FX_FINE_VIBRATO };
                    }
                    0x16 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_VIBRATO_WF << 4) | (fxp & 0x0f);
                    }
                    0x17 => {
                        fxt = FX_VIBRA_VSLIDE;
                        fxp >>= 4;
                    }
                    0x18 => {
                        fxt = FX_VIBRA_VSLIDE;
                        fxp &= 0x0f;
                    }

                    // Tremolo
                    0x1f => {
                        fxt = FX_TREMOLO;
                    }
                    0x20 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_TREMOLO_WF << 4) | (fxp & 0x0f);
                    }

                    // Sample commands
                    0x29 => {
                        // 3-byte offset: only the middle byte
                        fxt = FX_OFFSET;
                        fxp = f.read8();
                        f.read8();
                        rowlen -= 2;
                    }
                    0x2a => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_RETRIG << 4) | (fxp & 0x0f);
                    }
                    0x2b => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_CUT << 4) | (fxp & 0x0f);
                    }
                    0x2c => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_DELAY << 4) | (fxp & 0x0f);
                    }

                    // Position change
                    0x33 => {
                        fxt = FX_JUMP;
                        fxp >>= 1;
                        f.read8();
                        rowlen -= 1;
                    }
                    0x34 => {
                        fxt = FX_BREAK;
                    }
                    0x35 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_PATTERN_LOOP << 4) | (fxp & 0x0f);
                    }
                    0x36 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_PATT_DELAY << 4) | (fxp & 0x0f);
                    }

                    // Speed change
                    0x3d | 0x3e => {
                        fxt = FX_SPEED;
                    }

                    // Other
                    0x47 => {
                        fxt = FX_S3M_ARPEGGIO;
                    }
                    0x48 => {
                        fxt = FX_EXTENDED;
                        fxp = (EX_FINETUNE << 4) | (fxp & 0x0f);
                    }
                    0x49 => {
                        fxt = FX_SETPAN;
                        fxp <<= 4;
                    }

                    _ => {
                        fxt = 0;
                        fxp = 0;
                    }
                }

                event.fxt = fxt;
                event.fxp = fxp;
            }
        }
        r += 1;
        if r >= rows {
            break;
        }
    }

    d.cur_pat += 1;
    Ok(())
}

fn get_song(_id: &[u8], _size: u32, m: &mut ModuleData, f: &mut Hio, _d: &mut Local) -> Result<(), ()> {
    f.skip(10);
    m.module.chn = f.read8() as usize;
    Ok(())
}

fn set_pan(xxc: &mut ChannelInfo, type_: u8, pan: u8) {
    match type_ {
        0 => xxc.pan = (pan ^ 0x80) as i32, // use panning
        2 => {
            // surround
            xxc.pan = 0x80;
            xxc.flg |= XMP_CHANNEL_SURROUND;
        }
        4 => xxc.pan = 0x80, // center
        _ => {}
    }
}

fn subchunk_oplh(m: &mut ModuleData, size: u32, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    let mut first_order_chunk = i32::MAX;
    let mut size = size as i32;

    // First two bytes = number of chunks that follow
    let num_chunk = f.read16l() as i32;

    // Sub sub chunks
    let mut i = 0;
    while i < num_chunk && size > 0 {
        let opcode = f.read8();

        size -= 1;

        if opcode == 0 {
            // last sub sub chunk
            break;
        }

        // A playlist in theory, but no module uses more than the order items
        // and the global values (Saga Musix's note in OpenMPT).
        match opcode {
            0x01 => {
                // Play order list item
                if m.module.len >= XMP_MAX_MOD_LENGTH {
                    return Err(());
                }
                let n = if d.sinaria { 8 } else { 4 };
                let at = m.module.len * 8;
                f.read(&mut d.pord[at..at + n]);
                size -= n as i32;
                m.module.len += 1;
                if first_order_chunk == i32::MAX {
                    first_order_chunk = i;
                }
            }

            0x04 => {
                // Jump line (restart position)
                let restart_chunk = f.read16l() as i32;
                size -= 2;

                // only change the restart position if it is after the first order chunk
                if restart_chunk >= first_order_chunk {
                    m.module.rst = (restart_chunk - first_order_chunk) as usize;
                }
            }

            0x07 => {
                m.module.spd = f.read8() as i32;
                size -= 1;
            }
            0x08 => {
                m.module.bpm = f.read8() as i32;
                size -= 1;
            }
            0x0c => {
                f.read16l();
                f.read16l();
                f.read16l();
                size -= 6;
            }
            0x0d => {
                // Channel panning table
                let chn = f.read8() as usize;
                let pan = f.read8();
                let type_ = f.read8();

                if chn < XMP_MAX_CHANNELS {
                    size -= 3;
                    set_pan(&mut m.module.xxc[chn], type_, pan);
                }
            }
            0x0e => {
                // Channel volume table
                let chn = f.read8() as usize;
                let vol = f.read8();

                if chn < XMP_MAX_CHANNELS {
                    size -= 2;
                    m.module.xxc[chn].vol = (vol >> 2) as i32 + 1;
                }
            }
            _ => return Err(()),
        }
        i += 1;
    }

    Ok(())
}

/// Sinaria channel panning table.
fn subchunk_ppan(m: &mut ModuleData, size: u32, f: &mut Hio) -> Result<(), ()> {
    let mut size = size as i32;
    let mut i = 0;
    while i < XMP_MAX_CHANNELS && size > 0 {
        let type_ = f.read8();
        let pan = f.read8();
        size -= 2;
        set_pan(&mut m.module.xxc[i], type_, pan);
        i += 1;
    }
    Ok(())
}

/// Subchunk loader based on OpenMPT's LoadPSM.cpp.
fn get_song_2(_id: &[u8], size: u32, m: &mut ModuleData, f: &mut Hio, d: &mut Local) -> Result<(), ()> {
    let mut size = size as i32;
    let mut buf = [0u8; 9];

    f.read(&mut buf);
    f.read16l();
    size -= 11;

    // Iterate over subchunks. We want OPLH and PPAN
    while size > 0 {
        let magic = f.read32b();
        let subchunk_size = f.read32l() as i32;
        size -= 8;

        if subchunk_size <= 0 || f.take_error() {
            return Err(());
        }

        size = size.wrapping_sub(subchunk_size);

        match magic {
            MAGIC_OPLH => subchunk_oplh(m, subchunk_size as u32, f, d)?,
            MAGIC_PPAN => subchunk_ppan(m, subchunk_size as u32, f)?,
            _ => {
                f.seek_cur(subchunk_size as i64);
            }
        }
    }

    Ok(())
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    let mut data = Local::default();

    f.read32b();

    m.module.name.clear();

    f.skip(8); // file size and FILE
    m.module.smp = 0;
    m.module.ins = 0;
    let offset = f.tell();

    let first: &[(&[u8; 4], IffLoader<Local>)] = &[
        (b"TITL", get_titl),
        (b"SDFT", get_sdft),
        (b"SONG", get_song),
        (b"DSMP", get_dsmp_cnt),
        (b"PBOD", get_pbod_cnt),
    ];
    iff::load(m, f, &mut data, first, 4, IFF_LITTLE_ENDIAN)?;

    m.module.trk = m.module.pat * m.module.chn;
    data.pnam = vec![0u8; m.module.pat * 8]; // pattern names
    data.pord = vec![0u8; XMP_MAX_MOD_LENGTH * 8]; // pattern orders

    m.module.type_ = (if data.sinaria { "Sinaria PSM" } else { "Epic MegaGames MASI PSM" }).to_string();

    m.c4rate = C4_NTSC_RATE;

    m.init_instrument();
    m.module.xxt = vec![Track::default(); m.module.trk];
    m.module.xxp = vec![Pattern::default(); m.module.pat];

    f.seek(offset);

    m.module.len = 0;

    let second: &[(&[u8; 4], IffLoader<Local>)] = &[(b"SONG", get_song_2), (b"DSMP", get_dsmp), (b"PBOD", get_pbod)];
    iff::load(m, f, &mut data, second, 4, IFF_LITTLE_ENDIAN)?;

    let n = if data.sinaria { 8 } else { 4 };
    for i in 0..m.module.len {
        let mut j = 0;
        while j < m.module.pat {
            if data.pord[i * 8..i * 8 + n] == data.pnam[j * 8..j * 8 + n] {
                m.module.xxo[i] = j as u8;
                break;
            }
            j += 1;
        }

        if j == m.module.pat {
            break;
        }
    }

    Ok(())
}

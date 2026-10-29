//! Epic Games UMX, the Unreal package that wraps an IT, S3M, XM or MOD
//! (`loaders/umx_load.c`): the export table is parsed for the music object and
//! the inner loader runs at its offset.
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp). UPKG parsing is partially based
//! on Unreal Media Ripper (UMR) v0.3 by Andy Ward, with fixes by O. Sezer.

use crate::xmp::load::*;
use crate::xmp::module::*;

const UPKG_HDR_TAG: u32 = 0x9e2a_83c1;

const UMUSIC_IT: i32 = 0;
const UMUSIC_S3M: i32 = 1;
const UMUSIC_XM: i32 = 2;
const UMUSIC_MOD: i32 = 3;
const UMUSIC_WAV: i32 = 4;
const UMUSIC_MP2: i32 = 5;

const MUSTYPE: [&str; 6] = ["IT", "S3M", "XM", "MOD", "WAV", "MP2"];

struct UpkgHdr {
    file_version: i32,
    name_count: i32,
    name_offset: i32,
    export_offset: i32,
    import_offset: i32,
}

/// `get_fci`: decode an FCompactIndex at `pos` of `buf` (bytes past the
/// buffer read as 0), advancing `pos`.
fn get_fci(buf: &[u8], pos: &mut usize) -> i32 {
    let b = |i: usize| -> i32 { buf.get(*pos + i).copied().unwrap_or(0) as i8 as i32 };
    let mut size = 1;
    let mut a: i32 = b(0) & 0x3f;
    if b(0) & 0x40 != 0 {
        size += 1;
        a |= (b(1) & 0x7f) << 6;
        if b(1) & 0x80 != 0 {
            size += 1;
            a |= (b(2) & 0x7f) << 13;
            if b(2) & 0x80 != 0 {
                size += 1;
                a |= (b(3) & 0x7f) << 20;
                if b(3) & 0x80 != 0 {
                    size += 1;
                    a |= (((b(4) & 0x3f) as u32) << 27) as i32;
                }
            }
        }
    }
    if b(0) & 0x80 != 0 {
        a = a.wrapping_neg();
    }
    *pos += size;
    a
}

/// `get_objtype`
fn get_objtype(f: &mut Hio, ofs: i32, mut type_: i32) -> i32 {
    loop {
        let mut sig = [0u8; 16];
        f.seek(ofs as usize);
        f.read(&mut sig);
        if type_ == UMUSIC_IT {
            return if &sig[..4] == b"IMPM" { UMUSIC_IT } else { -1 };
        }
        if type_ == UMUSIC_XM {
            if &sig != b"Extended Module:" {
                return -1;
            }
            f.read(&mut sig);
            if sig[0] != b' ' {
                return -1;
            }
            f.read(&mut sig);
            if sig[5] != 0x1a {
                return -1;
            }
            return UMUSIC_XM;
        }
        if type_ == UMUSIC_MP2 {
            let u = (((sig[0] as u32) << 8) | sig[1] as u32) & 0xfffe;
            return if u == 0xfffc || u == 0xfff4 { UMUSIC_MP2 } else { -1 };
        }
        if type_ == UMUSIC_WAV {
            return if &sig[..4] == b"RIFF" && &sig[8..12] == b"WAVE" { UMUSIC_WAV } else { -1 };
        }

        f.seek(ofs as usize + 44);
        f.read(&mut sig[..4]);
        if type_ == UMUSIC_S3M {
            if &sig[..4] == b"SCRM" {
                return UMUSIC_S3M;
            }
            // SpaceMarines.umx and Starseek.umx from Return to NaPali report
            // as "s3m" whereas the actual music format is "it"
            type_ = UMUSIC_IT;
            continue;
        }

        f.seek(ofs as usize + 1080);
        f.read(&mut sig[..4]);
        if type_ == UMUSIC_MOD {
            return if &sig[..4] == b"M.K." || &sig[..4] == b"M!K!" { UMUSIC_MOD } else { -1 };
        }
        return -1;
    }
}

/// `read_export`: the type name index, and the offset and size of the data.
fn read_export(f: &mut Hio, hdr: &UpkgHdr, ofs: &mut i32, objsize: &mut i32) -> i32 {
    let mut buf = [0u8; 64];
    f.seek(*ofs as usize);
    if f.read(&mut buf[..40]) < 40 {
        return -1;
    }
    let mut idx = 0usize;
    if hdr.file_version < 40 {
        idx += 8;
    }
    if hdr.file_version < 60 {
        idx += 16;
    }
    get_fci(&buf, &mut idx);
    let t = get_fci(&buf, &mut idx);
    if hdr.file_version > 61 {
        idx += 4;
    }
    *objsize = get_fci(&buf, &mut idx);
    *ofs = ofs.wrapping_add(idx as i32);
    t
}

/// `read_typname`
fn read_typname(f: &mut Hio, hdr: &UpkgHdr, idx: i32) -> Option<Vec<u8>> {
    if idx >= hdr.name_count {
        return None;
    }
    let mut buf = [0u8; 64];
    let mut l: i64 = 0;
    for _ in 0..=idx {
        f.seek((hdr.name_offset as i64 + l) as usize);
        if f.read(&mut buf[..63]) == 0 {
            return None;
        }
        if hdr.file_version >= 64 {
            let s = buf[0] as i8 as i64;
            if s <= 0 {
                return None;
            }
            l += s + 5;
        } else {
            l += buf.iter().position(|&b| b == 0).unwrap_or(64) as i64;
            l += 5;
        }
    }
    let start = if hdr.file_version >= 64 { 1 } else { 0 };
    let s = &buf[start..];
    Some(s[..s.iter().position(|&b| b == 0).unwrap_or(s.len())].to_vec())
}

/// `probe_umx`: the music type, with the offset and size of its data.
fn probe_umx(f: &mut Hio, hdr: &UpkgHdr, ofs: &mut i32, objsize: &mut i32) -> i32 {
    let fsiz = f.size() as i64;
    if hdr.name_offset as i64 >= fsiz || hdr.export_offset as i64 >= fsiz || hdr.import_offset as i64 >= fsiz {
        return -1;
    }
    let mut buf = [0u8; 64];
    f.seek(hdr.export_offset as usize);
    f.read(&mut buf);

    let mut idx = 0usize;
    get_fci(&buf, &mut idx); // class_index
    get_fci(&buf, &mut idx); // super_index
    if hdr.file_version >= 60 {
        idx += 4; // package_index
    }
    get_fci(&buf, &mut idx); // object_name
    idx += 4; // object_flags

    let mut s = get_fci(&buf, &mut idx); // serial_size
    if s <= 0 {
        return -1;
    }
    let mut pos = get_fci(&buf, &mut idx); // serial_offset
    if pos < 0 || pos as i64 > fsiz - 40 {
        return -1;
    }

    let t = read_export(f, hdr, &mut pos, &mut s);
    if t < 0 {
        return -1;
    }
    if s <= 0 || s as i64 > fsiz - pos as i64 {
        return -1;
    }

    let Some(name) = read_typname(f, hdr, t) else { return -1 };
    let name = String::from_utf8_lossy(&name).to_ascii_uppercase();
    let Some(t) = MUSTYPE.iter().position(|&n| n == name) else { return -1 };
    let t = get_objtype(f, pos, t as i32);
    if t < 0 {
        return -1;
    }
    *ofs = pos;
    *objsize = s;
    t
}

/// `probe_header`
fn probe_header(f: &mut Hio) -> Option<UpkgHdr> {
    let tag = f.read32l();
    let file_version = f.read32l() as i32;
    let _pkg_flags = f.read32l();
    let name_count = f.read32l() as i32;
    let name_offset = f.read32l() as i32;
    let export_count = f.read32l() as i32;
    let export_offset = f.read32l() as i32;
    let import_count = f.read32l() as i32;
    let import_offset = f.read32l() as i32;
    if tag != UPKG_HDR_TAG {
        return None;
    }
    if name_count < 0
        || export_count < 0
        || import_count < 0
        || name_offset < 36
        || export_offset < 36
        || import_offset < 36
    {
        return None;
    }
    Some(UpkgHdr { file_version, name_count, name_offset, export_offset, import_offset })
}

/// `process_upkg`
fn process_upkg(f: &mut Hio) -> Option<(i32, i32)> {
    let hdr = probe_header(f)?;
    let (mut ofs, mut size) = (0, 0);
    let t = probe_umx(f, &hdr, &mut ofs, &mut size);
    if t < 0 {
        None
    } else {
        Some((t, ofs))
    }
}

pub fn test(d: &[u8]) -> bool {
    let mut f = Hio::new(d);
    matches!(process_upkg(&mut f), Some((t, _)) if (UMUSIC_IT..=UMUSIC_MOD).contains(&t))
}

pub fn load(m: &mut ModuleData, f: &mut Hio) -> Result<(), ()> {
    f.seek(0);
    let Some((t, ofs)) = process_upkg(f) else { return Err(()) };
    let ofs = ofs as usize;
    f.seek(ofs);
    match t {
        UMUSIC_IT => super::it::load_at(m, f, ofs),
        UMUSIC_S3M => super::s3m::load_at(m, f, ofs),
        UMUSIC_XM => super::xm::load_at(m, f, ofs),
        UMUSIC_MOD => super::mod_load::load_at(m, f, ofs),
        _ => Err(()),
    }
}

/// The music type `data` wraps ("IT", "S3M", "XM" or "MOD", the type the
/// object really holds), for the corpus statistics.
pub fn payload(data: &[u8]) -> Option<&'static str> {
    let mut f = Hio::new(data);
    let (t, _) = process_upkg(&mut f)?;
    MUSTYPE.get(t as usize).copied()
}

//! The IFF chunk reader of libxmp's loaders (`loaders/iff.c`).
//!
//! Ported from libxmp 4.7.4, MIT licence, Copyright (C) 1996-2026 Claudio Matsuoka
//! and Hipolito Carraro Jr (see LICENSE-libxmp).

use super::load::Hio;
use super::module::ModuleData;

pub const IFF_LITTLE_ENDIAN: u32 = 0x01;
pub const IFF_FULL_CHUNK_SIZE: u32 = 0x02;
pub const IFF_CHUNK_ALIGN2: u32 = 0x04;
pub const IFF_CHUNK_ALIGN4: u32 = 0x08;
pub const IFF_SKIP_EMBEDDED: u32 = 0x10;
pub const IFF_MAX_CHUNK_SIZE: u32 = 0x800000;

/// A chunk loader: the chunk's size, the file positioned at its data.
pub type IffLoader<P> = fn(&[u8], u32, &mut ModuleData, &mut Hio, &mut P) -> Result<(), ()>;

/// `libxmp_iff_load` over the chunks registered in `chunks` (id, loader; an id
/// matches on its first `id_size` bytes, the first registered match wins).
pub fn load<P>(
    m: &mut ModuleData,
    f: &mut Hio,
    parm: &mut P,
    chunks: &[(&[u8; 4], IffLoader<P>)],
    id_size: usize,
    flags: u32,
) -> Result<(), ()> {
    while !f.eof() {
        match chunk(m, f, parm, chunks, id_size, flags) {
            Ok(true) => break,
            Ok(false) => {}
            Err(()) => return Err(()),
        }
    }
    // end of file or a structural issue: clear the error flag to carry on
    f.take_error();
    Ok(())
}

/// `iff_chunk`: Ok(true) to stop without error.
fn chunk<P>(
    m: &mut ModuleData,
    f: &mut Hio,
    parm: &mut P,
    chunks: &[(&[u8; 4], IffLoader<P>)],
    id_size: usize,
    flags: u32,
) -> Result<bool, ()> {
    let mut id = [0u8; 16];
    if f.read(&mut id[..id_size]) != id_size {
        return Ok(true);
    }
    if flags & IFF_SKIP_EMBEDDED != 0 && &id[..4] == b"RIFF" {
        f.read32b();
        f.read32b();
        if f.read(&mut id[..id_size]) != id_size {
            return Ok(true);
        }
    }
    let mut size = if flags & IFF_LITTLE_ENDIAN != 0 { f.read32l() } else { f.read32b() };
    if f.take_error() {
        return Ok(true);
    }
    if flags & IFF_CHUNK_ALIGN2 != 0 {
        if size > 0xffff_fffe {
            return Ok(true);
        }
        size = (size + 1) & !1;
    }
    if flags & IFF_CHUNK_ALIGN4 != 0 {
        if size > 0xffff_fffc {
            return Ok(true);
        }
        size = (size + 3) & !3;
    }
    if flags & IFF_FULL_CHUNK_SIZE != 0 && &id[..4] != b"PTDT" {
        if (size as usize) < id_size + 4 {
            return Err(());
        }
        size -= id_size as u32 + 4;
    }

    let pos = f.tell();
    for (cid, loader) in chunks {
        if id[..id_size] == cid[..id_size] {
            if size > IFF_MAX_CHUNK_SIZE {
                return Err(());
            }
            loader(&id[..id_size], size, m, f, parm)?;
            break;
        }
    }
    f.seek(pos.saturating_add(size as usize));
    Ok(false)
}

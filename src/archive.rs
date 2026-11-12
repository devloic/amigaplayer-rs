//! Archives: recognise an archive from its bytes and get its files, in memory.
//!
//! Much Amiga music is kept in archives, often with the files a module needs beside it
//! (`smp.*`, `*.ins`, an `Instruments/` directory, TFMX `mdat.*` / `smpl.*` pairs). This module
//! lists and extracts the files of an archive; nothing touches the file system. A host plays an
//! entry with [`Song::open_with`](crate::Song::open_with), passing the other entries as its
//! companions (see [`Archive::companions`]).
//!
//! | [`Format`] | recognised by | methods |
//! |------------|---------------|---------|
//! | [`Lha`](Format::Lha)   | `-lh?-` / `-lz?-` / `-pm?-` at offset 2 | `-lh0-` `-lh1-` `-lh4-` .. `-lh7-` `-lhx-` `-lz4-` `-lz5-` `-lzs-` `-pm0-` (header levels 0 to 3) |
//! | [`Zip`](Format::Zip)   | `PK\3\4` / `PK\5\6` | stored, deflate; Zip64 (no encryption) |
//! | [`Gzip`](Format::Gzip) | `1F 8B 08` | deflate; several members are concatenated |
//! | [`Xz`](Format::Xz)     | `FD 37 7A 58 5A 00` | LZMA2 and the xz filters lzma-rs knows |
//! | [`Tar`](Format::Tar)   | `ustar` at offset 257 | regular files (also inside gzip and xz) |
//! | [`Lzx`](Format::Lzx)   | `LZX` | recognised only: [`extract`] says it is not supported |
//!
//! A gzip or xz file that does not hold a tar is a single file: [`extract`] gives one entry,
//! named after the name kept in the gzip header, or after the archive's name without its
//! `.gz` / `.xz` suffix. The crate's decrunchers also undo these two wrappers, so
//! [`Song::open`](crate::Song::open) plays a `tune.mod.gz` directly.
//!
//! ```
//! use amigaplayer_rs::archive::{self, Limits};
//!
//! assert_eq!(archive::format(b"plain data, not an archive"), None);
//! # let zip_bytes = amigaplayer_rs::archive::testdata::stored_zip(&[("mods/tune.mod", b"data")]);
//! let a = archive::extract("songs.zip", &zip_bytes, &Limits::default())?;
//! for e in &a.entries {
//!     println!("{} ({} bytes)", e.name, e.data.len());
//! }
//! # assert_eq!(a.entries[0].name, "mods/tune.mod");
//! # Ok::<(), archive::ArchiveError>(())
//! ```
//!
//! The input is untrusted: corrupt or truncated data gives an error or a shorter list (the
//! entries that could not be read are in [`Archive::skipped`]), never a panic, and the sizes
//! are capped by [`Limits`]. Entry names are made relative and cleaned of `.` and `..`
//! components, but a host that writes entries to disk should still treat them as untrusted.
//!
//! LhA is read with the `delharc` crate (MIT or Apache-2.0), deflate with `flate2` on
//! `miniz_oxide`, xz with `lzma-rs` (MIT). Amiga LZX is not extracted: the only free decoder
//! source at hand derives from `unlzx.c`, whose licence is unclear.

use std::fmt;
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// An archive format this module recognises. New variants may be added in a minor release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Format {
    /// LhA / LZH (LHarc, LhA, LZH archives of the Amiga and of MS-DOS).
    Lha,
    /// Zip (PKZIP).
    Zip,
    /// gzip: a single file, or a tar.
    Gzip,
    /// xz: a single file, or a tar.
    Xz,
    /// A POSIX (ustar) or GNU tar archive.
    Tar,
    /// Amiga LZX: recognised, not extracted.
    Lzx,
}

impl Format {
    /// A display name for the format.
    pub fn name(self) -> &'static str {
        match self {
            Format::Lha => "LhA",
            Format::Zip => "zip",
            Format::Gzip => "gzip",
            Format::Xz => "xz",
            Format::Tar => "tar",
            Format::Lzx => "LZX",
        }
    }

    /// Whether [`extract`] can get files out of this format.
    pub fn supported(self) -> bool {
        self != Format::Lzx
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why an archive could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveError(pub String);

impl fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ArchiveError {}

fn err<T>(s: impl Into<String>) -> Result<T, ArchiveError> {
    Err(ArchiveError(s.into()))
}

/// Size caps for [`extract`]. An entry larger than `max_entry` is skipped; once `max_total`
/// bytes or `max_entries` entries have been extracted the rest are skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest entry, in bytes (default 128 MiB).
    pub max_entry: usize,
    /// All entries together, in bytes (default 256 MiB).
    pub max_total: usize,
    /// The number of entries (default 10000).
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_entry: 128 << 20, max_total: 256 << 20, max_entries: 10000 }
    }
}

/// A file of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its path in the archive, components separated by `/`, relative, without `.` or `..`
    /// components (and never empty).
    pub name: String,
    /// Its content.
    pub data: Vec<u8>,
}

/// An entry that was not extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Its path in the archive (as for [`Entry::name`]; may be empty when the archive is too
    /// damaged to say).
    pub name: String,
    /// Why: an unsupported method, a checksum error, a size cap, ...
    pub reason: String,
}

/// The content of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Archive {
    /// The archive's format (the outer one: [`Format::Gzip`] for a `.tar.gz`).
    pub format: Format,
    /// Whether it is a gzip or xz wrapper around one file rather than an archive of files.
    pub single: bool,
    /// The files, in the archive's order (directories are left out).
    pub entries: Vec<Entry>,
    /// What could not be extracted.
    pub skipped: Vec<Skipped>,
}

impl Archive {
    /// The entries other than `entries[index]`, as `(name, bytes)` pairs for
    /// [`Song::open_with`](crate::Song::open_with): the companion files a module may need.
    pub fn companions(&self, index: usize) -> Vec<(&str, &[u8])> {
        self.entries
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != index)
            .map(|(_, e)| (e.name.as_str(), e.data.as_slice()))
            .collect()
    }

    /// The index of the entry named `name`: its full path or its file name, in any case
    /// (the full path wins).
    pub fn find(&self, name: &str) -> Option<usize> {
        let name = name.trim_start_matches('/');
        self.entries.iter().position(|e| e.name.eq_ignore_ascii_case(name)).or_else(|| {
            self.entries.iter().position(|e| e.name.rsplit('/').next().is_some_and(|b| b.eq_ignore_ascii_case(name)))
        })
    }
}

const XZ_MAGIC: &[u8] = &[0xFD, b'7', b'z', b'X', b'Z', 0];

fn is_lha(d: &[u8]) -> bool {
    // the method id "-lh5-" at offset 2, and a header level 0..3 at offset 20
    d.len() >= 22
        && d[2] == b'-'
        && d[6] == b'-'
        && matches!(&d[3..5], b"lh" | b"lz" | b"pm")
        && (d[5].is_ascii_alphanumeric())
        && d[20] <= 3
}

fn is_tar(d: &[u8]) -> bool {
    d.len() >= 512 && &d[257..262] == b"ustar"
}

/// The archive format of `data`, by its header alone (nothing is extracted).
pub fn format(data: &[u8]) -> Option<Format> {
    if data.len() >= 4 && (&data[..4] == b"PK\x03\x04" || &data[..4] == b"PK\x05\x06") {
        Some(Format::Zip)
    } else if data.len() >= 18 && data[0] == 0x1F && data[1] == 0x8B && data[2] == 8 {
        Some(Format::Gzip)
    } else if data.starts_with(XZ_MAGIC) && data.len() >= 24 {
        Some(Format::Xz)
    } else if is_lha(data) {
        Some(Format::Lha)
    } else if data.len() >= 10 && &data[..3] == b"LZX" {
        Some(Format::Lzx)
    } else if is_tar(data) {
        Some(Format::Tar)
    } else {
        None
    }
}

/// Extracts the files of the archive `data`. `name` is the archive's file name (a hint: it
/// names the file inside a gzip or xz wrapper that keeps no name). An error when `data` is not
/// an archive this module reads, or when nothing at all can be read from it; entries that fail
/// are listed in [`Archive::skipped`].
pub fn extract(name: &str, data: &[u8], limits: &Limits) -> Result<Archive, ArchiveError> {
    let Some(fmt) = format(data) else { return err("not an archive") };
    match catch_unwind(AssertUnwindSafe(|| extract_as(fmt, name, data, limits))) {
        Ok(r) => r,
        Err(_) => err(format!("{fmt} decoder failed on this data")),
    }
}

fn extract_as(fmt: Format, name: &str, data: &[u8], limits: &Limits) -> Result<Archive, ArchiveError> {
    let mut out = Out::new(limits);
    let mut single = false;
    match fmt {
        Format::Lha => lha(data, &mut out)?,
        Format::Zip => zip(data, &mut out)?,
        Format::Tar => tar(data, &mut out)?,
        Format::Gzip | Format::Xz => {
            let (inner, stored_name) = if fmt == Format::Gzip { gunzip(data, limits.max_entry)? } else { (unxz(data, limits.max_entry)?, None) };
            if is_tar(&inner) {
                tar(&inner, &mut out)?;
            } else {
                single = true;
                let n = stored_name.as_deref().and_then(|n| clean_name(n.rsplit(['/', '\\']).next().unwrap_or(n))).unwrap_or_else(|| strip_suffix(name, fmt));
                out.push(n, || Ok(inner));
            }
        }
        Format::Lzx => return err("Amiga LZX archives are not supported"),
    }
    Ok(Archive { format: fmt, single, entries: out.entries, skipped: out.skipped })
}

/// `name`'s file name without the wrapper's suffix (`tune.mod.gz` -> `tune.mod`).
fn strip_suffix(name: &str, fmt: Format) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = base.to_ascii_lowercase();
    let cut = match fmt {
        Format::Gzip => [".gz", ".gzip", ".z"].iter().find(|s| lower.ends_with(*s)).map(|s| s.len()),
        Format::Xz => lower.ends_with(".xz").then_some(3),
        _ => None,
    };
    let n = match cut {
        Some(c) if base.len() > c => &base[..base.len() - c],
        _ => base,
    };
    clean_name(n).unwrap_or_else(|| "data".into())
}

/// `raw` as a relative `/`-separated path without empty, `.` or `..` components (and without
/// an Amiga or MS-DOS volume name before a `:`); `None` when nothing is left.
pub(crate) fn clean_name(raw: &str) -> Option<String> {
    let raw = raw.replace('\\', "/");
    // "DH0:music/x" or "C:/x": the part after the volume
    let raw = match raw.split_once(':') {
        Some((vol, rest)) if !vol.contains('/') => rest.to_string(),
        _ => raw,
    };
    let parts: Vec<&str> = raw.split('/').filter(|p| !p.is_empty() && *p != "." && *p != ".." && !p.contains('\0')).collect();
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}

/// Text from bytes that need not be UTF-8: as UTF-8 if it is, else as Latin-1.
fn text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

/// The entries extracted so far, with the caps.
struct Out<'a> {
    limits: &'a Limits,
    total: usize,
    entries: Vec<Entry>,
    skipped: Vec<Skipped>,
}

impl<'a> Out<'a> {
    fn new(limits: &'a Limits) -> Self {
        Out { limits, total: 0, entries: Vec::new(), skipped: Vec::new() }
    }

    fn skip(&mut self, name: impl Into<String>, reason: impl Into<String>) {
        self.skipped.push(Skipped { name: name.into(), reason: reason.into() });
    }

    /// Room for an entry of `size` bytes (`None` when the reason it has none was recorded).
    fn room(&mut self, name: &str, size: u64) -> bool {
        if self.entries.len() >= self.limits.max_entries {
            self.skip(name, "too many entries");
            false
        } else if size > self.limits.max_entry as u64 {
            self.skip(name, format!("larger than {} bytes", self.limits.max_entry));
            false
        } else if self.total as u64 + size > self.limits.max_total as u64 {
            self.skip(name, format!("the archive's entries exceed {} bytes", self.limits.max_total));
            false
        } else {
            true
        }
    }

    /// Adds the entry `name` that `get` reads (`name` already cleaned).
    fn push(&mut self, name: String, get: impl FnOnce() -> Result<Vec<u8>, String>) {
        match get() {
            Ok(data) => {
                if self.room(&name, data.len() as u64) {
                    self.total += data.len();
                    self.entries.push(Entry { name, data });
                }
            }
            Err(why) => self.skip(name, why),
        }
    }
}

/// Reads at most `max` bytes from `r`: an error when there are more.
fn read_capped(r: impl Read, max: usize) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    r.take(max as u64 + 1).read_to_end(&mut v).map_err(|e| e.to_string())?;
    if v.len() > max {
        return Err(format!("larger than {max} bytes"));
    }
    Ok(v)
}

fn lha(data: &[u8], out: &mut Out) -> Result<(), ArchiveError> {
    use delharc::stub_io::Read as _;
    let mut rd = match delharc::LhaDecodeReader::new(data) {
        Ok(r) => r,
        Err(e) => return err(format!("LhA: {}", lha_err(e))),
    };
    loop {
        let h = rd.header();
        let raw = lha_name(h);
        let size = h.original_size;
        let is_dir = h.is_directory();
        match clean_name(&raw) {
            _ if is_dir => {}
            None => out.skip(raw, "no usable name"),
            Some(name) => {
                if !rd.is_decoder_supported() {
                    let m = String::from_utf8_lossy(&h.compression).into_owned();
                    out.skip(name, format!("unsupported LhA method {m}"));
                } else if out.room(&name, size) {
                    let mut buf = vec![0u8; size as usize];
                    let mut got = 0;
                    let mut failed = None;
                    while got < buf.len() {
                        match rd.read_all(&mut buf[got..]) {
                            Ok(0) => break,
                            Ok(n) => got += n,
                            Err(e) => {
                                failed = Some(lha_err(e));
                                break;
                            }
                        }
                    }
                    if let Some(f) = failed {
                        out.skip(name, format!("LhA data error: {f}"));
                    } else if got < buf.len() {
                        out.skip(name, "LhA data is truncated");
                    } else if !rd.crc_is_ok() {
                        out.skip(name, "LhA CRC error");
                    } else {
                        out.push(name, || Ok(buf));
                    }
                }
            }
        }
        match rd.next_file() {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => {
                // a truncated archive: what was read stays
                if out.entries.is_empty() && out.skipped.is_empty() {
                    return err(format!("LhA: {}", lha_err(e)));
                }
                out.skip("", format!("LhA archive damaged after this point: {}", lha_err(e)));
                break;
            }
        }
    }
    Ok(())
}

/// The path of an LhA entry, `/`-separated. Its name and directory come from the extended
/// headers when it has them (levels 1 to 3), else from the name field; either ends at a NUL
/// (an Amiga LhA keeps the file's comment after it, and there is no OS id in a level-0 header
/// to say so). Directory separators are 0xFF, `\` or `/`; the bytes are UTF-8 when they are
/// valid UTF-8, else Latin-1.
fn lha_name(h: &delharc::LhaHeader) -> String {
    let (mut file, mut dir): (Option<&[u8]>, Option<&[u8]>) = (None, None);
    for x in h.iter_extra() {
        match x {
            [1, d @ ..] => file = Some(d),
            [2, d @ ..] => dir = Some(d),
            _ => {}
        }
    }
    let cut = |b: &[u8]| -> Vec<u8> { b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())].to_vec() };
    let mut raw = Vec::new();
    if let Some(d) = dir {
        raw.extend(cut(d));
        raw.push(b'/');
    }
    raw.extend(cut(file.unwrap_or(&h.filename)));
    raw.split(|&c| c == 0xFF || c == b'\\' || c == b'/').filter(|c| !c.is_empty()).map(text).collect::<Vec<_>>().join("/")
}

/// A delharc error as text, short (in either of its builds, with or without `std`).
fn lha_err(e: impl fmt::Debug) -> String {
    format!("{e:?}").chars().take(160).collect()
}

fn u16le(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o.checked_add(2)?)?.try_into().ok()?))
}
fn u32le(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o.checked_add(4)?)?.try_into().ok()?))
}
fn u64le(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o.checked_add(8)?)?.try_into().ok()?))
}
/// `d[o..o + n]`, `None` when out of range (or when the end does not fit a `usize`).
fn span(d: &[u8], o: u64, n: u64) -> Option<&[u8]> {
    let o = usize::try_from(o).ok()?;
    let e = o.checked_add(usize::try_from(n).ok()?)?;
    d.get(o..e)
}

/// The upper half of code page 437, the encoding of zip names without the UTF-8 flag.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

/// A zip entry's name: UTF-8 when its flag (bit 11) says so or it is valid UTF-8, else CP437.
fn zip_text(b: &[u8], flags: u16) -> String {
    if flags & 0x800 != 0 || std::str::from_utf8(b).is_ok() {
        return String::from_utf8_lossy(b).into_owned();
    }
    let high: Vec<char> = CP437_HIGH.chars().collect();
    b.iter().map(|&c| if c < 0x80 { c as char } else { high.get(c as usize - 0x80).copied().unwrap_or('_') }).collect()
}

struct ZipEntry {
    name: String,
    flags: u16,
    method: u16,
    crc: u32,
    csize: u64,
    usize_: u64,
    local: u64,
}

/// The sizes and offset of a Zip64 entry from its extra field 0x0001: each of `fields` that is
/// 0xFFFFFFFF is replaced by the next 64-bit value there, in the order of the format
/// (uncompressed size, compressed size, local header offset).
fn zip64_extra(extra: &[u8], fields: [&mut u64; 3]) -> Option<()> {
    let mut p = 0;
    while p + 4 <= extra.len() {
        let id = u16le(extra, p)?;
        let len = u16le(extra, p + 2)? as usize;
        let body = extra.get(p + 4..p + 4 + len)?;
        if id == 1 {
            let mut q = 0;
            for f in fields {
                if *f == u32::MAX as u64 {
                    *f = u64le(body, q)?;
                    q += 8;
                }
            }
            return Some(());
        }
        p += 4 + len;
    }
    None
}

fn zip_central(data: &[u8]) -> Option<Vec<ZipEntry>> {
    // the end of central directory record: within the last 64 KiB + 22 bytes
    let lo = data.len().saturating_sub(65557);
    let eocd = (lo..=data.len().checked_sub(22)?).rev().find(|&i| &data[i..i + 4] == b"PK\x05\x06")?;
    let mut count = u16le(data, eocd + 10)? as u64;
    let mut p = u32le(data, eocd + 16)? as u64;
    if count == 0xFFFF || p == u32::MAX as u64 {
        // Zip64: the locator just before the record gives the Zip64 end record
        let loc = eocd.checked_sub(20)?;
        if data.get(loc..loc + 4)? == b"PK\x06\x07" {
            let z = usize::try_from(u64le(data, loc + 8)?).ok()?;
            if data.get(z..z.checked_add(4)?)? == b"PK\x06\x06" {
                count = u64le(data, z + 32)?;
                p = u64le(data, z + 48)?;
            }
        }
    }
    let mut v = Vec::new();
    for _ in 0..count {
        let q = usize::try_from(p).ok()?;
        if span(data, p, 4)? != b"PK\x01\x02" {
            return None;
        }
        let nlen = u16le(data, q + 28)? as u64;
        let xlen = u16le(data, q + 30)? as u64;
        let clen = u16le(data, q + 32)? as u64;
        let name = span(data, p + 46, nlen)?;
        let extra = span(data, p + 46 + nlen, xlen)?;
        let mut e = ZipEntry {
            name: zip_text(name, u16le(data, q + 8)?),
            flags: u16le(data, q + 8)?,
            method: u16le(data, q + 10)?,
            crc: u32le(data, q + 16)?,
            csize: u32le(data, q + 20)? as u64,
            usize_: u32le(data, q + 24)? as u64,
            local: u32le(data, q + 42)? as u64,
        };
        if [e.csize, e.usize_, e.local].contains(&(u32::MAX as u64)) {
            zip64_extra(extra, [&mut e.usize_, &mut e.csize, &mut e.local])?;
        }
        v.push(e);
        p += 46 + nlen + xlen + clen;
    }
    Some(v)
}

/// The entries from the local headers, for an archive whose central directory is missing
/// (truncated): only those that give their sizes up front.
fn zip_local(data: &[u8]) -> Vec<ZipEntry> {
    let mut v = Vec::new();
    let mut p = 0u64;
    while span(data, p, 4) == Some(b"PK\x03\x04") {
        let q = p as usize;
        let (Some(flags), Some(method), Some(crc), Some(csize), Some(usz), Some(nlen), Some(xlen)) = (
            u16le(data, q + 6),
            u16le(data, q + 8),
            u32le(data, q + 14),
            u32le(data, q + 18),
            u32le(data, q + 22),
            u16le(data, q + 26),
            u16le(data, q + 28),
        ) else {
            break;
        };
        if flags & 8 != 0 {
            break;
        }
        let (nlen, xlen) = (nlen as u64, xlen as u64);
        let (Some(name), Some(extra)) = (span(data, p + 30, nlen), span(data, p + 30 + nlen, xlen)) else { break };
        let mut e = ZipEntry { name: zip_text(name, flags), flags, method, crc, csize: csize as u64, usize_: usz as u64, local: p };
        if [e.csize, e.usize_].contains(&(u32::MAX as u64)) {
            let mut local = 0;
            if zip64_extra(extra, [&mut e.usize_, &mut e.csize, &mut local]).is_none() {
                break;
            }
        }
        p = p.saturating_add(30 + nlen + xlen).saturating_add(e.csize);
        v.push(e);
    }
    v
}

fn zip(data: &[u8], out: &mut Out) -> Result<(), ArchiveError> {
    let list = match zip_central(data) {
        Some(l) => l,
        None => {
            let l = zip_local(data);
            if l.is_empty() {
                return err("zip: no central directory and no readable entry");
            }
            out.skip("", "zip: central directory missing or damaged; entries read from their local headers");
            l
        }
    };
    for z in list {
        if z.name.ends_with('/') || z.name.ends_with('\\') {
            continue;
        }
        let Some(name) = clean_name(&z.name) else {
            out.skip(z.name, "no usable name");
            continue;
        };
        if z.flags & 1 != 0 {
            out.skip(name, "encrypted");
            continue;
        }
        if !out.room(&name, z.usize_) {
            continue;
        }
        let max = out.limits.max_entry;
        out.push(name, || {
            let p = z.local;
            if span(data, p, 4) != Some(b"PK\x03\x04") {
                return Err("zip: bad local header".into());
            }
            let q = p as usize;
            let nlen = u16le(data, q + 26).ok_or("zip: truncated")? as u64;
            let xlen = u16le(data, q + 28).ok_or("zip: truncated")? as u64;
            let raw = span(data, p + 30 + nlen + xlen, z.csize).ok_or("zip: entry data is truncated")?;
            let bytes = match z.method {
                0 => raw.to_vec(),
                8 => read_capped(flate2::read::DeflateDecoder::new(raw), max).map_err(|e| format!("zip: deflate data error: {e}"))?,
                m => return Err(format!("unsupported zip method {m}")),
            };
            if bytes.len() as u64 != z.usize_ {
                return Err("zip: size mismatch".into());
            }
            let mut c = flate2::Crc::new();
            c.update(&bytes);
            if c.sum() != z.crc {
                return Err("zip: CRC error".into());
            }
            Ok(bytes)
        });
    }
    Ok(())
}

/// The content of a gzip file (all its members), and the file name its first header keeps.
fn gunzip(data: &[u8], max: usize) -> Result<(Vec<u8>, Option<String>), ArchiveError> {
    let flags = data[3];
    let mut name = None;
    if flags & 8 != 0 {
        let mut p = 10;
        if flags & 4 != 0 {
            p += 2 + u16le(data, 10).unwrap_or(0) as usize;
        }
        if let Some(rest) = data.get(p..) {
            if let Some(end) = rest.iter().position(|&b| b == 0) {
                name = Some(text(&rest[..end]));
            }
        }
    }
    let bytes = read_capped(flate2::read::MultiGzDecoder::new(data), max).map_err(|e| ArchiveError(format!("gzip: {e}")))?;
    Ok((bytes, name))
}

/// A writer that refuses to grow past `max` bytes.
struct Capped {
    v: Vec<u8>,
    max: usize,
}

impl std::io::Write for Capped {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.v.len() + b.len() > self.max {
            return Err(std::io::Error::other(format!("larger than {} bytes", self.max)));
        }
        self.v.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn unxz(data: &[u8], max: usize) -> Result<Vec<u8>, ArchiveError> {
    let mut w = Capped { v: Vec::new(), max };
    let mut input = data;
    match lzma_rs::xz_decompress(&mut input, &mut w) {
        Ok(()) => Ok(w.v),
        Err(e) => err(format!("xz: {e}")),
    }
}

fn octal(f: &[u8]) -> Option<u64> {
    // (GNU's base-256 form for sizes of 8 GiB and more is refused: far past any cap)
    let s = std::str::from_utf8(f).ok()?.trim_matches(|c: char| c == '\0' || c == ' ');
    if s.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(s, 8).ok()
}

fn cstr(f: &[u8]) -> &[u8] {
    &f[..f.iter().position(|&b| b == 0).unwrap_or(f.len())]
}

fn tar(data: &[u8], out: &mut Out) -> Result<(), ArchiveError> {
    let mut p = 0;
    let mut long_name: Option<String> = None;
    let mut any = false;
    while p + 512 <= data.len() {
        let h = &data[p..p + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let Some(size) = octal(&h[124..136]) else {
            if !any {
                return err("tar: bad header");
            }
            out.skip("", "tar: damaged header; the rest is skipped");
            break;
        };
        any = true;
        let body = p + 512;
        let next = body.saturating_add(size.div_ceil(512).saturating_mul(512).min(usize::MAX as u64) as usize);
        let content = data.get(body..body.saturating_add(size as usize).min(data.len())).unwrap_or(&[]);
        let kind = h[156];
        match kind {
            b'L' => long_name = Some(text(cstr(content))),
            b'x' => {
                // a pax header: only its path matters here
                for rec in text(content).lines() {
                    if let Some((_, kv)) = rec.split_once(' ') {
                        if let Some(v) = kv.strip_prefix("path=") {
                            long_name = Some(v.to_string());
                        }
                    }
                }
            }
            b'0' | 0 | b'7' => {
                let raw = long_name.take().unwrap_or_else(|| {
                    let n = text(cstr(&h[0..100]));
                    let prefix = if &h[257..262] == b"ustar" { text(cstr(&h[345..500])) } else { String::new() };
                    if prefix.is_empty() { n } else { format!("{prefix}/{n}") }
                });
                match clean_name(&raw) {
                    None => out.skip(raw, "no usable name"),
                    Some(name) => {
                        if out.room(&name, size) {
                            if content.len() as u64 != size {
                                out.skip(name, "tar: entry data is truncated");
                            } else {
                                out.push(name, || Ok(content.to_vec()));
                            }
                        }
                    }
                }
            }
            _ => long_name = None,
        }
        p = next;
    }
    if !any {
        return err("tar: no entry");
    }
    Ok(())
}

/// Undoes a gzip or xz wrapper: `None` when `data` is neither, `Some(None)` when it is but
/// cannot be read, `Some(Some(bytes))` otherwise. For the decrunchers.
pub(crate) fn unwrap_single(data: &[u8], max: usize) -> Option<Option<Vec<u8>>> {
    let fmt = format(data).filter(|f| matches!(f, Format::Gzip | Format::Xz))?;
    let r = catch_unwind(AssertUnwindSafe(|| if fmt == Format::Gzip { gunzip(data, max).map(|r| r.0) } else { unxz(data, max) }));
    Some(r.ok().and_then(|r| r.ok()))
}

/// Whether `data` is gzip or xz, by its header (for the decrunchers' `identify`).
pub(crate) fn single_kind(data: &[u8]) -> Option<Format> {
    format(data).filter(|f| matches!(f, Format::Gzip | Format::Xz))
}

// --- the application's side: archives extracted into a cache directory -----------------

/// For the applications: an archive extracted into a directory of its own, where the players
/// find a module's companions as they find them next to a module on disk.
#[doc(hidden)]
#[derive(Debug)]
pub struct Expanded {
    pub format: Format,
    /// The directory the entries were written to.
    pub dir: std::path::PathBuf,
    /// The entries written, in the archive's order: (name in the archive, path on disk).
    pub entries: Vec<(String, std::path::PathBuf)>,
    pub skipped: Vec<Skipped>,
}

/// The largest archive file the applications read.
#[doc(hidden)]
pub const MAX_ARCHIVE_FILE: u64 = 512 << 20;

/// How many extracted archives the cache keeps (the least recently used go first).
const CACHE_KEEP: usize = 200;

/// Where the applications extract archives: `$AMIGAPLAYER_ARCHIVE_CACHE`,
/// `$XDG_CACHE_HOME/amigaplayer-rs/archives`, `~/.cache/amigaplayer-rs/archives`, else
/// `amigaplayer-rs-archives` in the system's temporary directory.
#[doc(hidden)]
pub fn default_cache_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Some(p) = std::env::var_os("AMIGAPLAYER_ARCHIVE_CACHE").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("XDG_CACHE_HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(p).join("amigaplayer-rs/archives");
    }
    if let Some(h) = std::env::var_os("HOME").filter(|p| !p.is_empty()) {
        return PathBuf::from(h).join(".cache/amigaplayer-rs/archives");
    }
    std::env::temp_dir().join("amigaplayer-rs-archives")
}

fn fnv64(d: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in d {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A path component fit for any host file system.
fn disk_component(c: &str) -> String {
    let s: String = c.chars().map(|ch| if ch.is_control() || "<>:\"|?*\\".contains(ch) { '_' } else { ch }).collect();
    // (Windows drops trailing dots and spaces)
    let s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "_".into() } else { s }
}

fn is_cache_key(n: &str) -> bool {
    let Some((h, len)) = n.split_once('-') else { return false };
    h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit()) && !len.is_empty() && len.bytes().all(|b| b.is_ascii_digit())
}

/// Removes the least recently used extractions past `CACHE_KEEP` (never `keep`).
fn prune(root: &std::path::Path, keep: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut dirs: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(is_cache_key) && e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()))
        .filter(|(_, p)| p != keep)
        .collect();
    if dirs.len() < CACHE_KEEP {
        return;
    }
    dirs.sort();
    let excess = dirs.len() + 1 - CACHE_KEEP;
    for (_, p) in dirs.into_iter().take(excess) {
        let _ = std::fs::remove_dir_all(&p);
    }
}

/// Extracts the archive at `path` into its own directory under `cache` (default:
/// [`default_cache_dir`]), named after a hash of its bytes, so a second visit finds it there.
/// `Ok(None)` when the file is not an archive of files (a gzip or xz wrapper around one file
/// is not: the engine's decrunchers undo it, next to the file's own companions).
#[doc(hidden)]
pub fn expand_file(path: &std::path::Path, cache: Option<&std::path::Path>) -> Result<Option<Expanded>, String> {
    use std::io::Read as _;
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut head = Vec::new();
    f.take(1024).read_to_end(&mut head).map_err(|e| format!("{}: {e}", path.display()))?;
    // (all the headers are recognised by their first 512 bytes)
    let Some(fmt) = format(&head) else { return Ok(None) };
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_ARCHIVE_FILE {
        return Err(format!("{}: archive larger than {MAX_ARCHIVE_FILE} bytes", path.display()));
    }
    if !fmt.supported() {
        return Err(format!("{}: {fmt} archives are not supported", path.display()));
    }
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let a = extract(&name, &data, &Limits::default()).map_err(|e| format!("{}: {e}", path.display()))?;
    if a.single {
        return Ok(None);
    }
    let root = cache.map(|c| c.to_path_buf()).unwrap_or_else(default_cache_dir);
    let key = format!("{:016x}-{}", fnv64(&data), data.len());
    let dir = root.join(&key);
    let disk: Vec<std::path::PathBuf> =
        a.entries.iter().map(|e| e.name.split('/').map(disk_component).fold(std::path::PathBuf::new(), |p, c| p.join(c))).collect();
    if !dir.is_dir() {
        std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        let tmp = root.join(format!(".{key}.{}.tmp", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for (e, rel) in a.entries.iter().zip(&disk) {
            let p = tmp.join(rel);
            let ok = p.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&p, &e.data).is_ok();
            if !ok {
                log::warn!("{}: could not write {}", path.display(), e.name);
            }
        }
        if std::fs::create_dir_all(&tmp).is_ok() && std::fs::rename(&tmp, &dir).is_err() {
            // (another process extracted it meanwhile)
            let _ = std::fs::remove_dir_all(&tmp);
        }
        prune(&root, &dir);
    } else {
        // the most recently used, for the pruning
        if let Ok(f) = std::fs::File::open(&dir) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    }
    let entries = a.entries.iter().zip(disk).map(|(e, rel)| (e.name.clone(), dir.join(rel))).filter(|(_, p)| p.is_file()).collect();
    Ok(Some(Expanded { format: a.format, dir, entries, skipped: a.skipped }))
}

/// Whether an entry is one no player wants (an icon, a text): the applications do not try it.
#[doc(hidden)]
pub fn is_not_music(name: &str) -> bool {
    let l = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
    [".info", ".txt", ".doc", ".nfo", ".diz", ".readme", ".guide", ".htm", ".html", ".jpg", ".png", ".gif", ".iff.info"]
        .iter()
        .any(|s| l.ends_with(s))
        || l == "readme"
}

/// Tiny archives built in memory, for the documentation's examples and the tests.
#[doc(hidden)]
pub mod testdata {
    fn crc(d: &[u8]) -> u32 {
        let mut c = flate2::Crc::new();
        c.update(d);
        c.sum()
    }

    /// A zip archive with `files` stored (not compressed).
    pub fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        zip(files, false)
    }

    /// A zip archive with `files` deflated.
    pub fn deflated_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        zip(files, true)
    }

    fn zip(files: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
        use std::io::Write;
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data) in files {
            let packed = if deflate {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let off = out.len() as u32;
            let mut h = Vec::new();
            h.extend_from_slice(&20u16.to_le_bytes()); // version needed
            h.extend_from_slice(&0u16.to_le_bytes()); // flags
            h.extend_from_slice(&method.to_le_bytes());
            h.extend_from_slice(&[0; 4]); // time, date
            h.extend_from_slice(&crc(data).to_le_bytes());
            h.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&h);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);
            cd.extend_from_slice(b"PK\x01\x02");
            cd.extend_from_slice(&20u16.to_le_bytes()); // version made by
            cd.extend_from_slice(&h);
            cd.extend_from_slice(&[0; 6]); // comment len, disk, internal attr
            cd.extend_from_slice(&[0; 4]); // external attr
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// A gzip file of `data`, keeping `name` in its header when given.
    pub fn gzip(data: &[u8], name: Option<&str>) -> Vec<u8> {
        use std::io::Write;
        let mut b = flate2::GzBuilder::new();
        if let Some(n) = name {
            b = b.filename(n);
        }
        let mut e = b.write(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    /// A ustar archive of `files`.
    pub fn tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data) in files {
            let mut h = [0u8; 512];
            h[..name.len()].copy_from_slice(name.as_bytes());
            h[100..107].copy_from_slice(b"0000644");
            h[108..115].copy_from_slice(b"0000000");
            h[116..123].copy_from_slice(b"0000000");
            h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
            h[136..147].copy_from_slice(b"00000000000");
            h[156] = b'0';
            h[257..263].copy_from_slice(b"ustar\0");
            h[263..265].copy_from_slice(b"00");
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|&b| b as u32).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            out.extend_from_slice(&h);
            out.extend_from_slice(data);
            out.resize(out.len().div_ceil(512) * 512, 0);
        }
        out.resize(out.len() + 1024, 0);
        out
    }
}

#[cfg(test)]
mod tests;

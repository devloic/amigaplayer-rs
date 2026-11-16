//! The open replayers for the browser, behind a C ABI a few lines of JavaScript can
//! drive (amigaplayer.mjs). The shape is libxmp's shim in the archive's web player:
//! open decodes the whole tune into memory, then the page copies the PCM out.
//!
//! ```text
//! p = dp_alloc(n)            copy the module (or a companion file) to p
//! dp_add_file(path, len, p, n)   a companion, at its path beside the module
//! dp_open(name, len, p, n, rate, max_bytes, subsong)   0, -2 not claimed, -1 failed
//! dp_info_ptr/len            "title\tformat\tsubsong\tfirst\tlast"; after -1, the reason
//! dp_pcm_ptr/len, dp_truncated, dp_close
//!
//! dp_archive(name, len, p, n)    an archive (LhA, LZX, zip, tar, gzip, xz) extracted into
//!                                the shim: its entry count, -2 not an archive, -1 unreadable;
//!                                dp_info: one line per entry, "index\tpath\tsize\tclaimed"
//! dp_open_entry(i, rate, max_bytes, subsong)   as dp_open, on entry i, the other entries
//!                                its companions
//! dp_archive_close               frees the extracted entries
//! ```
//!
//! An AIFF file is decoded by the library's AIFF decoder (a recording: played to its end).
//!
//! The PCM is interleaved stereo s16le. A tune ends where its replayer says, after
//! 10 s of silence once it has sounded (60 s before its first sound: some modules open
//! with that long a silent intro), or at `max_bytes` (truncated): many Amiga players loop
//! for ever.

use amigaplayer_rs::{Error, Options, Song};
use std::path::PathBuf;

const SILENCE_S: u32 = 10;
/// The silence before the first sound that ends a tune: S3Ms with silent intros of 10 to
/// 28 s (amp/Jakim, Xenoc, ZeOLiTe ...) were cut off at `SILENCE_S`.
const LEAD_SILENCE_S: u32 = 60;

struct State {
    files: Vec<(String, Vec<u8>)>,
    /// The entries of the archive `dp_archive` extracted, until `dp_archive_close`.
    archive: Vec<(String, Vec<u8>)>,
    info: String,
    pcm: Vec<i16>,
    truncated: bool,
}

static mut STATE: State =
    State { files: Vec::new(), archive: Vec::new(), info: String::new(), pcm: Vec::new(), truncated: false };

#[allow(static_mut_refs)]
fn st() -> &'static mut State {
    // SAFETY: wasm32-unknown-unknown without threads: one caller at a time.
    unsafe { &mut STATE }
}

unsafe fn bytes<'a>(p: *const u8, n: usize) -> &'a [u8] {
    if n == 0 { &[] } else { unsafe { std::slice::from_raw_parts(p, n) } }
}

#[unsafe(no_mangle)]
pub extern "C" fn dp_alloc(n: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(n.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `p` came from `dp_alloc(n)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_free(p: *mut u8, n: usize) {
    drop(unsafe { Vec::from_raw_parts(p, 0, n.max(1)) });
}

/// Whether an open replayer claims the module (1) or not (0).
///
/// # Safety
/// `p` points at `n` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_detect(p: *const u8, n: usize) -> i32 {
    // (a panic aborts under panic = "abort"; the guard is for builds without it)
    let d = unsafe { bytes(p, n) };
    // a custom module too (the module's own 68k code, which `dp_open` plays)
    std::panic::catch_unwind(|| (amigaplayer_rs::detect(d).is_some() || is_custom(d)) as i32).unwrap_or(0)
}

#[cfg(feature = "custom-modules")]
fn is_custom(d: &[u8]) -> bool {
    amigaplayer_rs::detect_custom(d)
}

#[cfg(not(feature = "custom-modules"))]
fn is_custom(_: &[u8]) -> bool {
    false
}

/// The options `dp_open` opens a song with: custom modules allowed when the build has them.
fn open_options() -> Options {
    #[cfg(feature = "custom-modules")]
    return Options::new().allow_custom(true);
    #[cfg(not(feature = "custom-modules"))]
    Options::new()
}

/// A companion file for the next `dp_open`, at `path` relative to the module.
///
/// # Safety
/// The pointers point at their lengths in bytes; `path` is UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_add_file(path: *const u8, path_len: usize, p: *const u8, n: usize) {
    let rel = String::from_utf8_lossy(unsafe { bytes(path, path_len) }).into_owned();
    let rel: PathBuf = rel.split(['/', '\\']).filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();
    st().files.push((rel.to_string_lossy().into_owned(), unsafe { bytes(p, n) }.to_vec()));
}

/// Decodes the module named `name` (its own file name, which name checks see) at
/// `rate` Hz from subsong `subsong` (-1: the first) into at most `max_bytes` of PCM.
///
/// # Safety
/// The pointers point at their lengths in bytes; `name` is UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_open(
    name: *const u8,
    name_len: usize,
    p: *const u8,
    n: usize,
    rate: u32,
    max_bytes: usize,
    subsong: i32,
) -> i32 {
    let files = std::mem::take(&mut st().files);
    let data = unsafe { bytes(p, n) };
    let name = String::from_utf8_lossy(unsafe { bytes(name, name_len) }).replace(['/', '\\'], "_");
    let companions: Vec<(&str, &[u8])> = files.iter().map(|(p, d)| (p.as_str(), d.as_slice())).collect();
    decode(&name, data, &companions, rate, max_bytes, subsong)
}

/// Extracts the archive `p` (its file name `name`, which names the file inside a gzip or xz
/// wrapper) into the shim for `dp_open_entry`. Returns the number of entries, -2 when it is
/// not an archive, -1 when nothing can be read from it (the reason in dp_info). dp_info then
/// lists the entries, one line each: "index\tpath\tsize\tclaimed", claimed 1 when an open
/// replayer claims the entry (as dp_detect) and its name is not a known non-music file.
///
/// # Safety
/// The pointers point at their lengths in bytes; `name` is UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_archive(name: *const u8, name_len: usize, p: *const u8, n: usize) -> i32 {
    use amigaplayer_rs::archive;
    let s = st();
    s.archive = Vec::new();
    s.info.clear();
    let data = unsafe { bytes(p, n) };
    if archive::format(data).is_none() {
        return -2;
    }
    let name = String::from_utf8_lossy(unsafe { bytes(name, name_len) }).into_owned();
    let a = match archive::extract(&name, data, &archive::Limits::default()) {
        Ok(a) => a,
        Err(e) => {
            s.info = e.to_string();
            return -1;
        }
    };
    for (i, e) in a.entries.iter().enumerate() {
        let base = e.name.rsplit('/').next().unwrap_or(&e.name);
        let claimed = !archive::is_not_music(base)
            && std::panic::catch_unwind(|| amigaplayer_rs::detect(&e.data).is_some() || is_custom(&e.data))
                .unwrap_or(false);
        s.info.push_str(&format!("{i}\t{}\t{}\t{}\n", e.name.replace(['\t', '\n'], " "), e.data.len(), claimed as i32));
    }
    s.archive = a.entries.into_iter().map(|e| (e.name, e.data)).collect();
    s.archive.len() as i32
}

/// As `dp_open`, on entry `index` of the archive `dp_archive` extracted, with the archive's
/// other entries as its companion files (at their paths relative to the entry's directory,
/// and at their full paths). -1 when there is no such entry.
#[unsafe(no_mangle)]
pub extern "C" fn dp_open_entry(index: i32, rate: u32, max_bytes: usize, subsong: i32) -> i32 {
    let entries = std::mem::take(&mut st().archive);
    let r = match usize::try_from(index).ok().and_then(|i| entries.get(i).map(|e| (i, e))) {
        None => {
            st().info = format!("no archive entry {index}");
            -1
        }
        Some((i, (path, data))) => {
            let dir = path.rsplit_once('/').map(|(d, _)| format!("{d}/")).unwrap_or_default();
            let mut companions: Vec<(&str, &[u8])> = Vec::new();
            for (j, (p, d)) in entries.iter().enumerate() {
                if j != i {
                    if let Some(rel) = p.strip_prefix(dir.as_str()).filter(|r| !dir.is_empty() && !r.is_empty()) {
                        companions.push((rel, d.as_slice()));
                    }
                    companions.push((p.as_str(), d.as_slice()));
                }
            }
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            decode(&name, data, &companions, rate, max_bytes, subsong)
        }
    };
    st().archive = entries;
    r
}

#[unsafe(no_mangle)]
pub extern "C" fn dp_archive_close() {
    st().archive = Vec::new();
}

/// Decodes `data` (its file name `name`) with its companions into the shim's PCM: dp_open's
/// work, shared by dp_open_entry.
fn decode(name: &str, data: &[u8], companions: &[(&str, &[u8])], rate: u32, max_bytes: usize, subsong: i32) -> i32 {
    let s = st();
    s.pcm = Vec::new();
    s.info.clear();
    s.truncated = false;
    let mut song = match Song::open_opts(name, data, companions, rate, &open_options()) {
        Ok(song) => song,
        Err(Error::NotRecognised) => return -2,
        Err(e) => {
            // the reason, for the page to show (AIFF names the compression it cannot decode)
            s.info = match e {
                Error::Invalid(why) => why,
                e => e.to_string(),
            };
            return -1;
        }
    };
    let (lo, hi, first) = {
        let r = song.subsongs();
        (r.first, r.last, r.current)
    };
    let want = if subsong < 0 { first } else { {
        let n = subsong.min(u16::MAX as i32) as u16;
        if lo <= hi { n.clamp(lo, hi) } else { first }
    } };
    if song.set_subsong(want).is_err() {
        return -1;
    }

    let max_frames = max_bytes / 4;
    let mut buf = vec![0i16; 2 * 1024];
    let mut quiet = 0u32;
    let mut sounded = false;
    // a recording is played to its end, silent stretches included
    let recorded = song.format().starts_with("AIFF");
    while !song.songend() {
        if s.pcm.len() / 2 >= max_frames {
            s.truncated = true;
            break;
        }
        song.render_i16(&mut buf);
        let mut silent = true;
        for &v in &buf {
            silent &= v == 0;
            s.pcm.push(v);
        }
        quiet = if silent { quiet + buf.len() as u32 / 2 } else { 0 };
        sounded |= !silent;
        let limit = if sounded { SILENCE_S } else { LEAD_SILENCE_S };
        if !recorded && quiet >= limit * rate {
            break;
        }
    }
    s.pcm.truncate(max_frames * 2);
    // the subsong played: an empty one at the start is skipped for the next
    let cur = song.subsongs().current;
    s.info = format!("{}\t{}\t{cur}\t{lo}\t{hi}", song.title(), song.format());
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn dp_info_ptr() -> *const u8 {
    st().info.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn dp_info_len() -> usize {
    st().info.len()
}

#[unsafe(no_mangle)]
pub extern "C" fn dp_pcm_ptr() -> *const u8 {
    st().pcm.as_ptr() as *const u8
}

/// In bytes.
#[unsafe(no_mangle)]
pub extern "C" fn dp_pcm_len() -> usize {
    st().pcm.len() * 2
}

#[unsafe(no_mangle)]
pub extern "C" fn dp_truncated() -> i32 {
    st().truncated as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn dp_close() {
    let s = st();
    s.pcm = Vec::new();
    s.info = String::new();
}

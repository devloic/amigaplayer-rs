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
//! ```
//!
//! An AIFF file is decoded by `amigaplayer_rs::aiff` (a recording: played to its end).
//!
//! The PCM is interleaved stereo s16le. A tune ends where its replayer says, after
//! 10 s of silence, or at `max_bytes` (truncated): many Amiga players loop for ever.

use amigaplayer_rs::open::{self, Song};
use std::path::PathBuf;

/// The module's directory: companions are added beside it, as `/mod/smpl.x` or
/// `/mod/Instruments/x.instr`.
const DIR: &str = "/mod";
const SILENCE_S: u32 = 10;

struct State {
    files: Vec<(PathBuf, Vec<u8>)>,
    info: String,
    pcm: Vec<i16>,
    truncated: bool,
}

static mut STATE: State = State { files: Vec::new(), info: String::new(), pcm: Vec::new(), truncated: false };

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
    open::detect(unsafe { bytes(p, n) }).is_some() as i32
}

/// A companion file for the next `dp_open`, at `path` relative to the module.
///
/// # Safety
/// The pointers point at their lengths in bytes; `path` is UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dp_add_file(path: *const u8, path_len: usize, p: *const u8, n: usize) {
    let rel = String::from_utf8_lossy(unsafe { bytes(path, path_len) }).into_owned();
    let rel: PathBuf = rel.split(['/', '\\']).filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();
    st().files.push((PathBuf::from(DIR).join(rel), unsafe { bytes(p, n) }.to_vec()));
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
    let s = st();
    s.pcm = Vec::new();
    s.info.clear();
    s.truncated = false;
    let files = std::mem::take(&mut s.files);
    let data = unsafe { bytes(p, n) };
    if open::detect(data).is_none() {
        return -2;
    }
    let name = String::from_utf8_lossy(unsafe { bytes(name, name_len) }).replace(['/', '\\'], "_");
    let path = PathBuf::from(DIR).join(name);
    let mut song = match Song::open_with(&path, data.to_vec(), files, rate) {
        Ok(song) => song,
        Err(e) => {
            // the reason, for the page to show (AIFF names the compression it cannot decode)
            s.info = e.to_string();
            return -1;
        }
    };
    let (lo, hi, first) = song.subsongs();
    let want = if subsong < 0 { first } else { (subsong as u16).clamp(lo, hi) };
    if song.set_subsong(want).is_err() {
        return -1;
    }

    let max_frames = max_bytes / 4;
    let mut buf = vec![0f32; 2 * 1024];
    let mut quiet = 0u32;
    // a recording is played to its end, silent stretches included
    let recorded = song.format().starts_with("AIFF");
    while !song.songend() {
        if s.pcm.len() / 2 >= max_frames {
            s.truncated = true;
            break;
        }
        song.render(&mut buf);
        let mut silent = true;
        for &x in &buf {
            let v = (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
            silent &= v == 0;
            s.pcm.push(v);
        }
        quiet = if silent { quiet + buf.len() as u32 / 2 } else { 0 };
        if !recorded && quiet >= SILENCE_S * rate {
            break;
        }
    }
    s.pcm.truncate(max_frames * 2);
    // the subsong played: an empty one at the start is skipped for the next
    let cur = song.subsongs().2;
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

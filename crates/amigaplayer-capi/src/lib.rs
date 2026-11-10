//! C API of the amigaplayer-rs open replayers. The header is `include/amigaplayer.h`; this file
//! is its implementation over [`amigaplayer_rs::Song`].
//!
//! Conventions: functions return a null pointer or a negative number on failure and record the
//! reason for [`ap_last_error`] / [`ap_last_error_code`] (per thread). Strings returned by
//! `ap_title` and `ap_format` belong to the song and stay valid until the next call of the same
//! function on that song, or `ap_close`. No Rust panic crosses the boundary.
//!
//! Licence: GPL-2.0-or-later, as the library.

#![allow(clippy::missing_safety_doc)]

use amigaplayer_rs::{Error, Song};
use std::cell::RefCell;
use std::ffi::{c_char, c_double, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Version of this C API, not of the crate: `major * 10000 + minor * 100 + patch`.
const API_VERSION: c_int = 100; // 0.1.0

thread_local! {
    static LAST: RefCell<(c_int, CString)> = RefCell::new((0, CString::default()));
}

const E_NOT_RECOGNISED: c_int = 1;
const E_INVALID: c_int = 2;
const E_RATE: c_int = 3;
const E_SUBSONG: c_int = 4;
const E_INTERNAL: c_int = 5;
const E_ARGUMENT: c_int = 6;

fn set_error(code: c_int, msg: &str) {
    let msg = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    LAST.with(|l| *l.borrow_mut() = (code, msg));
}

fn set_from(e: &Error) {
    let code = match e {
        Error::NotRecognised => E_NOT_RECOGNISED,
        Error::Invalid(_) => E_INVALID,
        Error::InvalidRate(_) => E_RATE,
        Error::SubsongOutOfRange { .. } => E_SUBSONG,
        _ => E_INTERNAL,
    };
    set_error(code, &e.to_string());
}

/// Runs `f`, turning a panic into an error and `fail`.
fn guard<T>(fail: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            set_error(E_INTERNAL, "internal error (panic)");
            fail
        }
    }
}

/// An open song: the Rust [`Song`] plus the strings handed out to C.
pub struct ApSong {
    song: Song,
    title: CString,
    format: CString,
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

impl ApSong {
    fn new(song: Song) -> Box<ApSong> {
        let (title, format) = (cstring(song.title()), cstring(song.format()));
        Box::new(ApSong { song, title, format })
    }
}

unsafe fn str_arg<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() { None } else { unsafe { CStr::from_ptr(p) }.to_str().ok() }
}

unsafe fn bytes<'a>(p: *const u8, n: usize) -> Option<&'a [u8]> {
    if n == 0 { Some(&[]) } else if p.is_null() { None } else { Some(unsafe { std::slice::from_raw_parts(p, n) }) }
}

/// The API version, `major * 10000 + minor * 100 + patch`.
#[unsafe(no_mangle)]
pub extern "C" fn ap_version() -> c_int {
    API_VERSION
}

/// The message of the last failed call on this thread (empty if none). Valid until the next
/// call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn ap_last_error() -> *const c_char {
    LAST.with(|l| l.borrow().1.as_ptr())
}

/// The code of the last failed call on this thread (`AP_ERR_*`), 0 if none.
#[unsafe(no_mangle)]
pub extern "C" fn ap_last_error_code() -> c_int {
    LAST.with(|l| l.borrow().0)
}

/// Asks which format plays `data`; copies its NUL-terminated name into `out` (cut to `cap`
/// bytes including the NUL). Returns the full length of the name, without the NUL, or 0 if no
/// replayer recognises the data. `out` may be null with `cap` 0 to ask for the length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_detect(data: *const u8, len: usize, out: *mut c_char, cap: usize) -> usize {
    let Some(d) = (unsafe { bytes(data, len) }) else {
        set_error(E_ARGUMENT, "null data");
        return 0;
    };
    guard(0, || match amigaplayer_rs::detect(d) {
        None => 0,
        Some(name) => {
            if !out.is_null() && cap > 0 {
                let n = name.len().min(cap - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(name.as_ptr(), out as *mut u8, n);
                    *out.add(n) = 0;
                }
            }
            name.len()
        }
    })
}

/// Opens a module. `name` is its file name (a hint). Returns null on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_open(name: *const c_char, data: *const u8, len: usize, rate: u32) -> *mut ApSong {
    unsafe { ap_open_with(name, data, len, rate, 0, std::ptr::null(), std::ptr::null(), std::ptr::null()) }
}

/// As `ap_open`, with `n` companion files in memory: `paths[i]` is the path relative to the
/// module, `datas[i]` / `lens[i]` its content.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_open_with(
    name: *const c_char,
    data: *const u8,
    len: usize,
    rate: u32,
    n: usize,
    paths: *const *const c_char,
    datas: *const *const u8,
    lens: *const usize,
) -> *mut ApSong {
    let (Some(name), Some(data)) = (unsafe { str_arg(name) }, unsafe { bytes(data, len) }) else {
        set_error(E_ARGUMENT, "null or invalid name or data");
        return std::ptr::null_mut();
    };
    let mut files: Vec<(&str, &[u8])> = Vec::new();
    for i in 0..n {
        if paths.is_null() || datas.is_null() || lens.is_null() {
            set_error(E_ARGUMENT, "null companion arrays");
            return std::ptr::null_mut();
        }
        let (p, d, l) = unsafe { (*paths.add(i), *datas.add(i), *lens.add(i)) };
        let (Some(p), Some(d)) = (unsafe { str_arg(p) }, unsafe { bytes(d, l) }) else {
            set_error(E_ARGUMENT, "null or invalid companion");
            return std::ptr::null_mut();
        };
        files.push((p, d));
    }
    guard(std::ptr::null_mut(), || match Song::open_with(name, data, &files, rate) {
        Ok(s) => Box::into_raw(ApSong::new(s)),
        Err(e) => {
            set_from(&e);
            std::ptr::null_mut()
        }
    })
}

/// Closes a song. Null is allowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_close(song: *mut ApSong) {
    if !song.is_null() {
        let b = unsafe { Box::from_raw(song) };
        let _ = catch_unwind(AssertUnwindSafe(move || drop(b)));
    }
}

macro_rules! song {
    ($p:expr, $fail:expr) => {
        match unsafe { $p.as_mut() } {
            Some(s) => s,
            None => {
                set_error(E_ARGUMENT, "null song");
                return $fail;
            }
        }
    };
}

/// Renders up to `frames` interleaved stereo frames (2 * `frames` samples) as signed 16 bit.
/// Returns the number of frames written, or 0 on a null argument.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_render_s16(song: *mut ApSong, out: *mut i16, frames: usize) -> usize {
    let s = song!(song, 0);
    if out.is_null() {
        set_error(E_ARGUMENT, "null buffer");
        return 0;
    }
    if frames > isize::MAX as usize / 4 {
        set_error(E_ARGUMENT, "frames too large");
        return 0;
    }
    let buf = unsafe { std::slice::from_raw_parts_mut(out, frames * 2) };
    guard(0, || s.song.render_i16(buf))
}

/// As `ap_render_s16`, as floats in -1..1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_render_f32(song: *mut ApSong, out: *mut f32, frames: usize) -> usize {
    let s = song!(song, 0);
    if out.is_null() {
        set_error(E_ARGUMENT, "null buffer");
        return 0;
    }
    if frames > isize::MAX as usize / 8 {
        set_error(E_ARGUMENT, "frames too large");
        return 0;
    }
    let buf = unsafe { std::slice::from_raw_parts_mut(out, frames * 2) };
    guard(0, || s.song.render_f32(buf))
}

/// The first, last and current subsong. Any pointer may be null. Returns 0, or -1 for a null song.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_subsongs(song: *mut ApSong, first: *mut u16, last: *mut u16, current: *mut u16) -> c_int {
    let s = song!(song, -1);
    let Some(r) = guard(None, || Some(s.song.subsongs())) else { return -1 };
    unsafe {
        if !first.is_null() {
            *first = r.first;
        }
        if !last.is_null() {
            *last = r.last;
        }
        if !current.is_null() {
            *current = r.current;
        }
    }
    0
}

/// Restarts at subsong `n`. Returns 0, or -1 (see `ap_last_error`) if `n` is out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_set_subsong(song: *mut ApSong, n: u16) -> c_int {
    let s = song!(song, -1);
    guard(-1, || match s.song.set_subsong(n) {
        Ok(()) => 0,
        Err(e) => {
            set_from(&e);
            -1
        }
    })
}

/// 1 if the replayer has reported the end of the song, else 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_songend(song: *mut ApSong) -> c_int {
    let s = song!(song, 0);
    guard(0, || s.song.songend() as c_int)
}

/// Seconds rendered since the subsong started.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_elapsed_secs(song: *mut ApSong) -> c_double {
    let s = song!(song, 0.0);
    guard(0.0, || s.song.elapsed_secs())
}

/// The length in seconds when the format states it, else a negative number.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_duration_hint(song: *mut ApSong) -> c_double {
    let s = song!(song, -1.0);
    guard(-1.0, || s.song.duration_hint().unwrap_or(-1.0))
}

/// Restarts the current subsong from its beginning. Returns 0 or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_restart(song: *mut ApSong) -> c_int {
    let s = song!(song, -1);
    guard(-1, || match s.song.restart() {
        Ok(()) => 0,
        Err(e) => {
            set_from(&e);
            -1
        }
    })
}

/// Positions the song `secs` seconds into the current subsong (by rendering up to there).
/// `secs` must be finite and not negative (capped at 24 hours). Returns 0 or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_seek(song: *mut ApSong, secs: c_double) -> c_int {
    let s = song!(song, -1);
    guard(-1, || match s.song.seek(secs) {
        Ok(()) => 0,
        Err(e) => {
            set_from(&e);
            -1
        }
    })
}

/// The sample rate the song renders at.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_sample_rate(song: *mut ApSong) -> u32 {
    let s = song!(song, 0);
    s.song.sample_rate()
}

/// The module's title (UTF-8, may be empty). Valid until the next `ap_title` call on `song`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_title(song: *mut ApSong) -> *const c_char {
    let s = song!(song, std::ptr::null());
    s.title = cstring(s.song.title());
    s.title.as_ptr()
}

/// The name of the format that plays the module. Valid until the next `ap_format` call on `song`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_format(song: *mut ApSong) -> *const c_char {
    let s = song!(song, std::ptr::null());
    s.format = cstring(s.song.format());
    s.format.as_ptr()
}

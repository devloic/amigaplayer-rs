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

use amigaplayer_rs::{Error, Options, Song};
use std::cell::RefCell;
use std::ffi::{c_char, c_double, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Version of this C API, not of the crate: `major * 10000 + minor * 100 + patch`.
const API_VERSION: c_int = 500; // 0.5.0

thread_local! {
    static LAST: RefCell<(c_int, CString)> = RefCell::new((0, CString::default()));
    /// The files the last AP_ERR_MISSING_COMPANION named.
    static MISSING: RefCell<Vec<CString>> = const { RefCell::new(Vec::new()) };
}

const E_NOT_RECOGNISED: c_int = 1;
const E_INVALID: c_int = 2;
const E_RATE: c_int = 3;
const E_SUBSONG: c_int = 4;
const E_INTERNAL: c_int = 5;
const E_ARGUMENT: c_int = 6;
const E_GUEST: c_int = 7;
#[cfg_attr(all(feature = "custom-modules", feature = "songdb"), allow(dead_code))]
const E_UNSUPPORTED: c_int = 8;
const E_MISSING_COMPANION: c_int = 9;

/// `ap_open_opts` flag: custom modules may play (their own 68k code is run).
const OPEN_CUSTOM: u32 = 1;

fn set_error(code: c_int, msg: &str) {
    let msg = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    LAST.with(|l| *l.borrow_mut() = (code, msg));
    MISSING.with(|m| m.borrow_mut().clear());
}

fn code_of(e: &Error) -> c_int {
    match e {
        Error::NotRecognised => E_NOT_RECOGNISED,
        Error::Invalid(_) => E_INVALID,
        Error::InvalidRate(_) => E_RATE,
        Error::SubsongOutOfRange { .. } => E_SUBSONG,
        Error::Guest(_) => E_GUEST,
        Error::MissingCompanion { .. } => E_MISSING_COMPANION,
        _ => E_INTERNAL,
    }
}

fn set_from(e: &Error) {
    set_error(code_of(e), &e.to_string());
    if let Error::MissingCompanion { wanted, .. } = e {
        MISSING.with(|m| *m.borrow_mut() = wanted.iter().map(|w| cstring(w)).collect());
    }
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
    error: CString,
    missing: Vec<CString>,
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

impl ApSong {
    fn new(song: Song) -> Box<ApSong> {
        let (title, format) = (cstring(song.title()), cstring(song.format()));
        Box::new(ApSong { song, title, format, error: CString::default(), missing: Vec::new() })
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

/// After `AP_ERR_MISSING_COMPANION`: the `i`-th file the format looked for and did not find
/// (a path relative to the module, as `ap_open_with` takes it), or null past the last. Valid
/// until the next failing call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn ap_last_missing(i: c_int) -> *const c_char {
    MISSING.with(|m| {
        let m = m.borrow();
        usize::try_from(i).ok().and_then(|i| m.get(i)).map_or(std::ptr::null(), |c| c.as_ptr())
    })
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
    unsafe { ap_open_opts(name, data, len, rate, n, paths, datas, lens, 0) }
}

/// 1 if `data` is a custom module (an executable that carries its own player, which
/// `ap_open_opts` plays with `AP_OPEN_CUSTOM`), else 0. Always 0 in a build without the
/// `custom-modules` feature.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_detect_custom(data: *const u8, len: usize) -> c_int {
    let Some(d) = (unsafe { bytes(data, len) }) else {
        set_error(E_ARGUMENT, "null data");
        return 0;
    };
    #[cfg(feature = "custom-modules")]
    return guard(0, || amigaplayer_rs::detect_custom(d) as c_int);
    #[cfg(not(feature = "custom-modules"))]
    {
        let _ = d;
        0
    }
}

/// As `ap_open_with`, with `flags` (`AP_OPEN_*`, or 0). The output stage is the default one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_open_opts(
    name: *const c_char,
    data: *const u8,
    len: usize,
    rate: u32,
    n: usize,
    paths: *const *const c_char,
    datas: *const *const u8,
    lens: *const usize,
    flags: u32,
) -> *mut ApSong {
    unsafe { ap_open_output(name, data, len, rate, n, paths, datas, lens, flags, 1, 1, 300) }
}

/// As `ap_open_opts`, with the output stage: `filter` 0 = A500, 1 = A1200 (the default), 2 =
/// none; `resampler` 0 = UADE's default (averaging) one, 1 = its Sinc one (the default);
/// `separation_milli` 0 (mono) to 1000 (hard Amiga panning), default 300. A value out of its
/// range is AP_ERR_ARGUMENT and a NULL result. New in 0.5.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_open_output(
    name: *const c_char,
    data: *const u8,
    len: usize,
    rate: u32,
    n: usize,
    paths: *const *const c_char,
    datas: *const *const u8,
    lens: *const usize,
    flags: u32,
    filter: c_int,
    resampler: c_int,
    separation_milli: c_int,
) -> *mut ApSong {
    if !(0..=2).contains(&filter) || !(0..=1).contains(&resampler) || !(0..=1000).contains(&separation_milli) {
        set_error(E_ARGUMENT, "output setting out of range");
        return std::ptr::null_mut();
    }
    if flags & !OPEN_CUSTOM != 0 {
        set_error(E_ARGUMENT, "unknown flags");
        return std::ptr::null_mut();
    }
    let out = Options::new()
        .filter(match filter {
            0 => amigaplayer_rs::paulaout::Filter::A500,
            2 => amigaplayer_rs::paulaout::Filter::None,
            _ => amigaplayer_rs::paulaout::Filter::A1200,
        })
        .resampler(if resampler == 0 { amigaplayer_rs::paulaout::Resampler::UadeDefault } else { amigaplayer_rs::paulaout::Resampler::Sinc })
        .separation(separation_milli as f32 / 1000.0);
    #[cfg(feature = "custom-modules")]
    let opts = out.allow_custom(flags & OPEN_CUSTOM != 0);
    #[cfg(not(feature = "custom-modules"))]
    let opts = {
        if flags & OPEN_CUSTOM != 0 {
            set_error(E_UNSUPPORTED, "this build cannot play custom modules (built without the custom-modules feature)");
            return std::ptr::null_mut();
        }
        out
    };
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
    guard(std::ptr::null_mut(), || match Song::open_opts(name, data, &files, rate, &opts) {
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

/// Why the song has stopped by itself and renders silence (the 68k code of a custom module
/// failed, or a replayer did), or null while all is well. Valid until the next `ap_error`
/// call on `song`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_error(song: *mut ApSong) -> *const c_char {
    let s = song!(song, std::ptr::null());
    match s.song.error() {
        Some(e) => {
            s.error = cstring(&e.to_string());
            s.error.as_ptr()
        }
        None => std::ptr::null(),
    }
}

/// The `AP_ERR_*` code of `ap_error` (`AP_ERR_GUEST` or `AP_ERR_INTERNAL`), 0 while all is well.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_error_code(song: *mut ApSong) -> c_int {
    let s = song!(song, 0);
    s.song.error().map_or(0, code_of)
}

/// The `i`-th library function (from 0, in the order first called) the 68k code of a custom
/// module called that the emulated AmigaOS does not implement, as
/// `"exec.library/AddTask() (LVO -282)"`, or null past the last (and for every other format).
/// Valid until the next `ap_missing_call` call on `song`. New in 0.4.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_missing_call(song: *mut ApSong, i: c_int) -> *const c_char {
    let s = song!(song, std::ptr::null());
    s.missing = s.song.missing_calls().iter().map(|c| cstring(&c.to_string())).collect();
    usize::try_from(i).ok().and_then(|i| s.missing.get(i)).map_or(std::ptr::null(), |c| c.as_ptr())
}

/// Whether the `i`-th call of `ap_missing_call` is one the replay cannot work without (it has
/// stopped the song, and `ap_error` names it): 1, else 0; -1 past the last. New in 0.4.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_missing_call_fatal(song: *mut ApSong, i: c_int) -> c_int {
    let s = song!(song, -1);
    usize::try_from(i).ok().and_then(|i| s.song.missing_calls().get(i)).map_or(-1, |c| c.fatal as c_int)
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

// ------------------------------------------------------------------ song database ---

/// One subsong of an `ap_songdb_info`.
#[repr(C)]
pub struct ApSongDbSubsong {
    pub subsong: u8,
    pub is_duplicate: u8,
    /// `AP_SONGEND_*`
    pub status: i8,
    pub length_ms: u32,
}

/// What `ap_songdb_lookup` fills in. The pointers belong to the database handle.
#[repr(C)]
pub struct ApSongDbInfo {
    pub key: *const c_char,
    pub format: *const c_char,
    pub author: *const c_char,
    pub album: *const c_char,
    pub publisher: *const c_char,
    pub year: u16,
    pub channels: u8,
    pub subsong_count: usize,
    pub subsongs: *const ApSongDbSubsong,
}

/// An open song database, and the strings of its last lookup.
pub struct ApSongDb {
    #[cfg(feature = "songdb")]
    db: amigaplayer_rs::songdb::SongDb,
    #[cfg_attr(not(feature = "songdb"), allow(dead_code))]
    strings: [CString; 5],
    #[cfg_attr(not(feature = "songdb"), allow(dead_code))]
    subsongs: Vec<ApSongDbSubsong>,
}

/// Opens the song database in directory `dir` (audacious-uade's `conf/songdb`). Null on
/// failure: `AP_ERR_INVALID` if it cannot be read, `AP_ERR_UNSUPPORTED` in a library built
/// without the `songdb` feature.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_songdb_open(dir: *const c_char) -> *mut ApSongDb {
    let Some(dir) = (unsafe { str_arg(dir) }) else {
        set_error(E_ARGUMENT, "null or invalid directory");
        return std::ptr::null_mut();
    };
    #[cfg(feature = "songdb")]
    return guard(std::ptr::null_mut(), || match amigaplayer_rs::songdb::SongDb::open(dir) {
        Ok(db) => Box::into_raw(Box::new(ApSongDb { db, strings: Default::default(), subsongs: Vec::new() })),
        Err(e) => {
            set_error(E_INVALID, &e.to_string());
            std::ptr::null_mut()
        }
    });
    #[cfg(not(feature = "songdb"))]
    {
        let _ = dir;
        set_error(E_UNSUPPORTED, "this build has no song database support (built without the songdb feature)");
        std::ptr::null_mut()
    }
}

/// Closes a song database. Null is allowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_songdb_free(db: *mut ApSongDb) {
    if !db.is_null() {
        let b = unsafe { Box::from_raw(db) };
        let _ = catch_unwind(AssertUnwindSafe(move || drop(b)));
    }
}

/// The number of files the database knows, 0 for a null handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_songdb_count(db: *mut ApSongDb) -> usize {
    let Some(d) = (unsafe { db.as_mut() }) else {
        set_error(E_ARGUMENT, "null database");
        return 0;
    };
    #[cfg(feature = "songdb")]
    return d.db.len();
    #[cfg(not(feature = "songdb"))]
    {
        let _ = d;
        0
    }
}

/// The database key of `data` (12 hex digits and a NUL) into `out`, which holds 13 bytes.
/// Returns 0, or -1: a null argument, or (`AP_ERR_UNSUPPORTED`) a build without the `songdb`
/// feature. Needs no database.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_songdb_key(data: *const u8, len: usize, out: *mut c_char) -> c_int {
    let (Some(d), false) = (unsafe { bytes(data, len) }, out.is_null()) else {
        set_error(E_ARGUMENT, "null data or buffer");
        return -1;
    };
    #[cfg(feature = "songdb")]
    return guard(-1, || {
        let k = amigaplayer_rs::songdb::key(d);
        unsafe {
            std::ptr::copy_nonoverlapping(k.as_ptr(), out as *mut u8, 12);
            *out.add(12) = 0;
        }
        0
    });
    #[cfg(not(feature = "songdb"))]
    {
        let _ = d;
        set_error(E_UNSUPPORTED, "this build has no song database support (built without the songdb feature)");
        -1
    }
}

#[cfg(feature = "songdb")]
fn songdb_fill(d: &mut ApSongDb, found: Option<amigaplayer_rs::songdb::Info>, out: *mut ApSongDbInfo) -> c_int {
    let Some(i) = found else { return 0 };
    d.strings = [cstring(&i.key), cstring(&i.format), cstring(&i.author), cstring(&i.album), cstring(&i.publisher)];
    d.subsongs = i
        .subsongs
        .iter()
        .map(|s| ApSongDbSubsong { subsong: s.subsong, is_duplicate: s.is_duplicate as u8, status: s.status.as_i8(), length_ms: s.length_ms })
        .collect();
    let s = &d.strings;
    let info = ApSongDbInfo {
        key: s[0].as_ptr(),
        format: s[1].as_ptr(),
        author: s[2].as_ptr(),
        album: s[3].as_ptr(),
        publisher: s[4].as_ptr(),
        year: i.year,
        channels: i.channels,
        subsong_count: d.subsongs.len(),
        subsongs: d.subsongs.as_ptr(),
    };
    unsafe { out.write(info) };
    1
}

/// Looks the file `data` up. Returns 1 and fills `out` if the database knows it, 0 if it
/// does not (`out` is left alone), -1 on a null argument. The strings and the subsong array
/// of `out` belong to `db`: valid until the next lookup on it, or `ap_songdb_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_songdb_lookup(db: *mut ApSongDb, data: *const u8, len: usize, out: *mut ApSongDbInfo) -> c_int {
    let (Some(d), Some(bytes), false) = (unsafe { db.as_mut() }, unsafe { bytes(data, len) }, out.is_null()) else {
        set_error(E_ARGUMENT, "null database, data or result");
        return -1;
    };
    #[cfg(feature = "songdb")]
    return guard(-1, || {
        let found = d.db.lookup(bytes);
        songdb_fill(d, found, out)
    });
    #[cfg(not(feature = "songdb"))]
    {
        let _ = (d, bytes);
        0
    }
}

/// As `ap_songdb_lookup`, by key (12 hex digits; anything else is not found).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_songdb_lookup_key(db: *mut ApSongDb, key: *const c_char, out: *mut ApSongDbInfo) -> c_int {
    let (Some(d), false, false) = (unsafe { db.as_mut() }, key.is_null(), out.is_null()) else {
        set_error(E_ARGUMENT, "null database, key or result");
        return -1;
    };
    // (a key that is not UTF-8 is not a key)
    let Some(key) = (unsafe { str_arg(key) }) else { return 0 };
    #[cfg(feature = "songdb")]
    return guard(-1, || {
        let found = d.db.lookup_key(key);
        songdb_fill(d, found, out)
    });
    #[cfg(not(feature = "songdb"))]
    {
        let _ = (d, key);
        0
    }
}

/// The database tools' name of an `AP_SONGEND_*` status ("player", "loop+silence", ...), or
/// "unknown". A static string.
#[unsafe(no_mangle)]
pub extern "C" fn ap_songdb_status_name(status: c_int) -> *const c_char {
    let names: [&CStr; 13] = [
        c"error", c"none", c"player", c"timeout", c"silence", c"loop", c"volume", c"repeat", c"player+silence",
        c"player+volume", c"loop+silence", c"loop+volume", c"nosound",
    ];
    let at = status.checked_add(1).and_then(|v| usize::try_from(v).ok());
    at.and_then(|i| names.get(i)).unwrap_or(&c"unknown").as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use amigaplayer_rs::testdata::{minimal_custom, minimal_mod};

    #[test]
    fn status_names() {
        let name = |s| unsafe { CStr::from_ptr(ap_songdb_status_name(s)) }.to_str().unwrap();
        assert_eq!((name(-1), name(0), name(1), name(8), name(11)), ("error", "none", "player", "player+volume", "nosound"));
        assert_eq!((name(12), name(-2), name(c_int::MAX), name(c_int::MIN)), ("unknown", "unknown", "unknown", "unknown"));
        #[cfg(feature = "songdb")]
        for v in -1..=11i8 {
            let s = amigaplayer_rs::songdb::SongEndStatus::from_i8(v).unwrap();
            assert_eq!(name(v as c_int), s.as_str());
        }
    }

    #[cfg(not(feature = "songdb"))]
    #[test]
    fn the_song_database_is_refused_without_the_feature() {
        unsafe {
            assert!(ap_songdb_open(c"/".as_ptr()).is_null());
            assert_eq!(last_error().0, E_UNSUPPORTED);
            let mut key = [0 as c_char; 13];
            assert_eq!(ap_songdb_key(b"x".as_ptr(), 1, key.as_mut_ptr()), -1);
            ap_songdb_free(std::ptr::null_mut());
            assert_eq!(ap_songdb_count(std::ptr::null_mut()), 0);
        }
    }

    #[cfg(feature = "songdb")]
    #[test]
    fn the_song_database_through_c() {
        // (next to the test binary: in the build's own directory)
        let dir = std::env::current_exe().unwrap().parent().unwrap().join(format!("capi-songdb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (minimal_mod(), minimal_custom());
        amigaplayer_rs::testdata::write_songdb(&dir, &[&a, &b]).unwrap();
        let cdir = CString::new(dir.to_str().unwrap()).unwrap();
        unsafe {
            assert!(ap_songdb_open(c"/nonexistent/songdb".as_ptr()).is_null());
            assert_eq!(last_error().0, E_INVALID);
            assert!(ap_songdb_open(std::ptr::null()).is_null());
            assert_eq!(last_error().0, E_ARGUMENT);

            let db = ap_songdb_open(cdir.as_ptr());
            assert!(!db.is_null(), "{:?}", last_error());
            assert!(ap_songdb_count(db) >= 2);
            let mut info = std::mem::MaybeUninit::<ApSongDbInfo>::uninit();
            assert_eq!(ap_songdb_lookup(db, b.as_ptr(), b.len(), info.as_mut_ptr()), 1);
            let i = info.assume_init_read();
            let text = |p| CStr::from_ptr(p).to_str().unwrap().to_string();
            assert_eq!(text(i.key), amigaplayer_rs::songdb::key(&b));
            assert_eq!((text(i.format), text(i.author), text(i.album), text(i.publisher)), ("Synthetic 1".into(), "Author 1".into(), "Album 1".into(), "Publisher 1".into()));
            assert_eq!((i.year, i.channels, i.subsong_count), (1991, 4, 2));
            let subs = std::slice::from_raw_parts(i.subsongs, i.subsong_count);
            assert_eq!((subs[0].subsong, subs[0].length_ms, subs[0].status, subs[0].is_duplicate), (0, 3000, 1, 0));
            assert_eq!((subs[1].subsong, subs[1].length_ms, subs[1].status), (1, 500, 3));

            // by key: the same; the key function gives it
            let mut key = [0 as c_char; 13];
            assert_eq!(ap_songdb_key(a.as_ptr(), a.len(), key.as_mut_ptr()), 0);
            assert_eq!(text(key.as_ptr()), amigaplayer_rs::songdb::key(&a));
            assert_eq!(ap_songdb_lookup_key(db, key.as_ptr(), info.as_mut_ptr()), 1);
            let i = info.assume_init_read();
            assert_eq!((text(i.author), i.year), ("Author 0".to_string(), 1990));

            // not found, and bad arguments
            assert_eq!(ap_songdb_lookup(db, b"nothing".as_ptr(), 7, info.as_mut_ptr()), 0);
            assert_eq!(ap_songdb_lookup_key(db, c"xyz".as_ptr(), info.as_mut_ptr()), 0);
            assert_eq!(ap_songdb_lookup(std::ptr::null_mut(), a.as_ptr(), a.len(), info.as_mut_ptr()), -1);
            assert_eq!(ap_songdb_lookup(db, a.as_ptr(), a.len(), std::ptr::null_mut()), -1);
            assert_eq!(ap_songdb_lookup_key(db, std::ptr::null(), info.as_mut_ptr()), -1);
            assert_eq!(ap_songdb_key(std::ptr::null(), 3, key.as_mut_ptr()), -1);
            ap_songdb_free(db);
            ap_songdb_free(std::ptr::null_mut());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn last_error() -> (c_int, String) {
        (ap_last_error_code(), unsafe { CStr::from_ptr(ap_last_error()) }.to_string_lossy().into_owned())
    }

    unsafe fn open(data: &[u8], flags: u32) -> *mut ApSong {
        unsafe { ap_open_opts(c"cust.test".as_ptr(), data.as_ptr(), data.len(), 48000, 0, std::ptr::null(), std::ptr::null(), std::ptr::null(), flags) }
    }

    #[test]
    fn open_output_selects_the_output_stage() {
        let m = minimal_mod();
        let pcm = |filter, resampler, sep| unsafe {
            let s = ap_open_output(c"a.mod".as_ptr(), m.as_ptr(), m.len(), 48000, 0, std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, filter, resampler, sep);
            assert!(!s.is_null());
            let mut v = vec![0i16; 2 * 9000];
            assert_eq!(ap_render_s16(s, v.as_mut_ptr(), 9000), 9000);
            ap_close(s);
            v
        };
        unsafe {
            // ap_open_opts is the defaults: A1200, Sinc, 300
            let d = ap_open_opts(c"a.mod".as_ptr(), m.as_ptr(), m.len(), 48000, 0, std::ptr::null(), std::ptr::null(), std::ptr::null(), 0);
            let mut v = vec![0i16; 2 * 9000];
            assert_eq!(ap_render_s16(d, v.as_mut_ptr(), 9000), 9000);
            ap_close(d);
            assert_eq!(v, pcm(1, 1, 300));
        }
        let old = pcm(0, 0, 700);
        assert!(old.iter().any(|&x| x != 0));
        assert_ne!(old, pcm(1, 1, 300));
        assert_ne!(pcm(1, 1, 300), pcm(1, 0, 300));
        assert_ne!(pcm(1, 1, 300), pcm(0, 1, 300));
        assert_ne!(pcm(1, 1, 300), pcm(1, 1, 1000));
        for bad in [(3, 1, 300), (-1, 1, 300), (1, 2, 300), (1, 1, 1001), (1, 1, -1)] {
            unsafe {
                let s = ap_open_output(c"a.mod".as_ptr(), m.as_ptr(), m.len(), 48000, 0, std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, bad.0, bad.1, bad.2);
                assert!(s.is_null(), "{bad:?}");
            }
            assert_eq!(last_error().0, E_ARGUMENT);
        }
    }

    #[test]
    fn open_opts_without_flags_is_open_with() {
        let m = minimal_mod();
        unsafe {
            let (a, b) = (open(&m, 0), ap_open(c"a.mod".as_ptr(), m.as_ptr(), m.len(), 48000));
            assert!(!a.is_null() && !b.is_null());
            let (mut x, mut y) = (vec![0i16; 2000], vec![0i16; 2000]);
            assert_eq!(ap_render_s16(a, x.as_mut_ptr(), 1000), 1000);
            assert_eq!(ap_render_s16(b, y.as_mut_ptr(), 1000), 1000);
            assert_eq!(x, y);
            assert!(ap_error(a).is_null());
            assert_eq!(ap_error_code(a), 0);
            ap_close(a);
            ap_close(b);
            assert!(open(&m, 0x8000).is_null());
            assert_eq!(last_error().0, E_ARGUMENT);
            assert_eq!(ap_detect_custom(m.as_ptr(), m.len()), 0);
            assert_eq!(ap_detect_custom(std::ptr::null(), 4), 0);
        }
    }

    #[test]
    fn a_custom_module_is_refused_without_the_flag() {
        let m = minimal_custom();
        unsafe {
            assert!(open(&m, 0).is_null());
            assert_eq!(last_error().0, E_NOT_RECOGNISED);
            assert!(ap_open(c"cust.test".as_ptr(), m.as_ptr(), m.len(), 48000).is_null());
        }
    }

    #[cfg(feature = "custom-modules")]
    #[test]
    fn a_custom_module_plays_with_the_flag() {
        let m = minimal_custom();
        unsafe {
            assert_eq!(ap_detect_custom(m.as_ptr(), m.len()), 1);
            let s = open(&m, OPEN_CUSTOM);
            assert!(!s.is_null(), "{:?}", last_error());
            assert_eq!(CStr::from_ptr(ap_format(s)).to_str(), Ok("Custom"));
            assert_eq!(CStr::from_ptr(ap_title(s)).to_str(), Ok("synthetic"));
            let (mut first, mut last, mut cur) = (9u16, 9u16, 9u16);
            assert_eq!(ap_subsongs(s, &mut first, &mut last, &mut cur), 0);
            assert_eq!((first, last, cur), (0, 1, 0));
            let mut pcm = vec![0i16; 2 * 4800];
            assert_eq!(ap_render_s16(s, pcm.as_mut_ptr(), 4800), 4800);
            assert!(pcm.iter().any(|&v| v.abs() > 1000));
            assert!(ap_error(s).is_null());
            assert_eq!(ap_set_subsong(s, 1), 0);
            ap_close(s);
        }
    }

    #[cfg(feature = "custom-modules")]
    #[test]
    fn a_custom_module_that_hangs_reports_it() {
        // its interrupt routine is `bra.s *`
        let m = amigaplayer_rs::testdata::custom_with(&[], &[0x60, 0xFE]);
        unsafe {
            let s = open(&m, OPEN_CUSTOM);
            assert!(!s.is_null(), "{:?}", last_error());
            let mut pcm = vec![0f32; 2 * 48000];
            assert_eq!(ap_render_f32(s, pcm.as_mut_ptr(), 48000), 48000);
            assert_eq!(ap_error_code(s), E_GUEST);
            let why = CStr::from_ptr(ap_error(s)).to_string_lossy().into_owned();
            assert!(why.contains("did not return"), "{why}");
            assert_eq!(ap_songend(s), 0);
            assert_eq!(ap_set_subsong(s, 1), -1);
            assert_eq!(last_error().0, E_GUEST);
            assert_eq!(ap_restart(s), 0);
            assert!(ap_error(s).is_null());
            ap_close(s);
        }
    }

    /// 68000 code calling exec function `lvo` (negative), its registers kept.
    #[cfg(feature = "custom-modules")]
    fn exec_call_code(lvo: i16) -> Vec<u8> {
        let mut c = vec![0x48, 0xE7, 0xFF, 0xFE, 0x2C, 0x78, 0x00, 0x04, 0x4E, 0xAE];
        c.extend_from_slice(&lvo.to_be_bytes());
        c.extend_from_slice(&[0x4C, 0xDF, 0x7F, 0xFF]);
        c
    }

    #[cfg(feature = "custom-modules")]
    #[test]
    fn calls_the_host_does_not_implement_are_listed() {
        // InitSound calls exec AllocEntry: a warning, the module plays
        let m = amigaplayer_rs::testdata::custom_with(&exec_call_code(-222), &[]);
        unsafe {
            let s = open(&m, OPEN_CUSTOM);
            assert!(!s.is_null(), "{:?}", last_error());
            let mut pcm = vec![0f32; 2 * 4800];
            ap_render_f32(s, pcm.as_mut_ptr(), 4800);
            assert!(ap_error(s).is_null());
            let c = CStr::from_ptr(ap_missing_call(s, 0)).to_string_lossy().into_owned();
            assert_eq!(c, "exec.library/AllocEntry() (LVO -222)");
            assert_eq!(ap_missing_call_fatal(s, 0), 0);
            assert!(ap_missing_call(s, 1).is_null());
            assert_eq!(ap_missing_call_fatal(s, 1), -1);
            ap_close(s);
        }
        // the interrupt calls ColdReboot: the song stops, and ap_error names the call
        let mut irq = exec_call_code(-726);
        irq.extend_from_slice(&[0x4E, 0x75]);
        let m = amigaplayer_rs::testdata::custom_with(&[], &irq);
        unsafe {
            let s = open(&m, OPEN_CUSTOM);
            assert!(!s.is_null(), "{:?}", last_error());
            let mut pcm = vec![0f32; 2 * 48000];
            ap_render_f32(s, pcm.as_mut_ptr(), 48000);
            assert_eq!(ap_error_code(s), E_GUEST);
            let why = CStr::from_ptr(ap_error(s)).to_string_lossy().into_owned();
            assert!(why.contains("exec.library/ColdReboot() (LVO -726)"), "{why}");
            assert_eq!(ap_missing_call_fatal(s, 0), 1);
            ap_close(s);
        }
    }

    #[cfg(not(feature = "custom-modules"))]
    #[test]
    fn the_flag_is_refused_without_the_feature() {
        let m = minimal_custom();
        unsafe {
            assert!(open(&m, OPEN_CUSTOM).is_null());
            assert_eq!(last_error().0, E_UNSUPPORTED);
            assert_eq!(ap_detect_custom(m.as_ptr(), m.len()), 0);
        }
    }
}

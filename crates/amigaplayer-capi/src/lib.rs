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
const API_VERSION: c_int = 200; // 0.2.0

thread_local! {
    static LAST: RefCell<(c_int, CString)> = RefCell::new((0, CString::default()));
}

const E_NOT_RECOGNISED: c_int = 1;
const E_INVALID: c_int = 2;
const E_RATE: c_int = 3;
const E_SUBSONG: c_int = 4;
const E_INTERNAL: c_int = 5;
const E_ARGUMENT: c_int = 6;
const E_GUEST: c_int = 7;
#[cfg_attr(all(feature = "host-68k", feature = "songdb"), allow(dead_code))]
const E_UNSUPPORTED: c_int = 8;

/// `ap_open_opts` flag: custom modules may play (their own 68k code is run).
const OPEN_CUSTOM: u32 = 1;

fn set_error(code: c_int, msg: &str) {
    let msg = CString::new(msg.replace('\0', " ")).unwrap_or_default();
    LAST.with(|l| *l.borrow_mut() = (code, msg));
}

fn code_of(e: &Error) -> c_int {
    match e {
        Error::NotRecognised => E_NOT_RECOGNISED,
        Error::Invalid(_) => E_INVALID,
        Error::InvalidRate(_) => E_RATE,
        Error::SubsongOutOfRange { .. } => E_SUBSONG,
        Error::Guest(_) => E_GUEST,
        _ => E_INTERNAL,
    }
}

fn set_from(e: &Error) {
    set_error(code_of(e), &e.to_string());
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
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

impl ApSong {
    fn new(song: Song) -> Box<ApSong> {
        let (title, format) = (cstring(song.title()), cstring(song.format()));
        Box::new(ApSong { song, title, format, error: CString::default() })
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
    unsafe { ap_open_opts(name, data, len, rate, n, paths, datas, lens, 0) }
}

/// 1 if `data` is a custom module (an executable that carries its own player, which
/// `ap_open_opts` plays with `AP_OPEN_CUSTOM`), else 0. Always 0 in a build without the
/// `host-68k` feature.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ap_detect_custom(data: *const u8, len: usize) -> c_int {
    let Some(d) = (unsafe { bytes(data, len) }) else {
        set_error(E_ARGUMENT, "null data");
        return 0;
    };
    #[cfg(feature = "host-68k")]
    return guard(0, || amigaplayer_rs::detect_custom(d) as c_int);
    #[cfg(not(feature = "host-68k"))]
    {
        let _ = d;
        0
    }
}

/// As `ap_open_with`, with `flags` (`AP_OPEN_*`, or 0).
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
    if flags & !OPEN_CUSTOM != 0 {
        set_error(E_ARGUMENT, "unknown flags");
        return std::ptr::null_mut();
    }
    #[cfg(feature = "host-68k")]
    let opts = Options::new().allow_custom(flags & OPEN_CUSTOM != 0);
    #[cfg(not(feature = "host-68k"))]
    let opts = {
        if flags & OPEN_CUSTOM != 0 {
            set_error(E_UNSUPPORTED, "this build cannot play custom modules (built without the host-68k feature)");
            return std::ptr::null_mut();
        }
        Options::new()
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

    #[cfg(feature = "host-68k")]
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

    #[cfg(feature = "host-68k")]
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

    #[cfg(not(feature = "host-68k"))]
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

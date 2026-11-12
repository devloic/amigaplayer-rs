//! High-level emulation of the parts of AmigaOS that players use: exec
//! (memory, lists, interrupts, libraries, devices), dos (files mapped to the
//! host filesystem), utility (32-bit math, tags), cia resources and an
//! audio.device that plays CMD_WRITE on Paula. Any other library is loaded from
//! disk when a file of it is found (see `disk_library`), else a stub library whose
//! calls return 0 and are logged once.

use crate::machine::{Machine, Trap};
use m68k::AddressBus;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibKind {
    Exec,
    Dos,
    Utility,
    Graphics,
    Intuition,
    CiaA,
    CiaB,
    AudioDev,
    TimerDev,
    Stub,
    /// A library of 68k code loaded from disk (`disk_library`), or one guest code made and
    /// added itself (MakeLibrary + AddLibrary): its vectors jump into that code, none traps.
    Disk,
}

pub struct Library {
    pub name: String,
    pub base: u32,
    pub kind: LibKind,
}

struct OpenFile {
    file: Handle,
    path: PathBuf,
}

/// What a dos file handle reads: a file of the host, or (with `OsState::no_host_files`) one
/// of the files held in memory, which cannot be written.
enum Handle {
    Disk(File),
    Mem(std::io::Cursor<Vec<u8>>),
}

impl Read for Handle {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Handle::Disk(f) => f.read(buf),
            Handle::Mem(c) => c.read(buf),
        }
    }
}

impl Seek for Handle {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        match self {
            Handle::Disk(f) => f.seek(to),
            Handle::Mem(c) => c.seek(to),
        }
    }
}

impl Handle {
    fn write_all(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Handle::Disk(f) => f.write_all(data),
            Handle::Mem(_) => Err(std::io::ErrorKind::PermissionDenied.into()),
        }
    }
}

/// The room behind an AllocVec of no bytes (see exec's AllocVec).
const ALLOCVEC0_ROOM: u32 = 0x2000;
/// The most the guest console keeps (what players print with PutStr and the like).
const CONSOLE_MAX: usize = 1 << 20;
/// The most interrupt servers one interrupt takes.
const INT_SERVERS_MAX: usize = 256;

#[derive(Default)]
pub struct OsState {
    pub libs: Vec<Library>,
    pub exec_base: u32,
    /// DeliTracker's own Process.
    pub process: u32,
    /// The task running now (FindTask(0)): `process`, or a player's own process.
    pub current_task: u32,
    /// audio.device: what each channel plays and has queued.
    pub audio: [AudioChan; 4],
    pub audio_alloc: AudioAlloc,
    /// SetIntVector() handlers, by interrupt number (INTB_*)
    pub int_vectors: [u32; 16],
    /// AddIntServer() chains, by interrupt number
    pub int_servers: [Vec<u32>; 16],
    /// cia.resource ICR vectors [cia][bit]
    pub icr_vectors: [[u32; 5]; 2],
    files: HashMap<u32, OpenFile>,
    locks: HashMap<u32, PathBuf>,
    current_dir: u32,
    io_err: u32,
    pub progdir: PathBuf,
    /// What Env: and EnvArc: are: the `ENV` folder of the Eagleplayer directory, where UADE
    /// keeps the players' settings (`ENV/EaglePlayer/EP-Hippel_7V.cfg`, the mixing rate of
    /// Hippel 7V: 24 kHz, not its own 16).
    pub env_dir: PathBuf,
    /// The current module's directory, where the `Instruments:` volume is.
    pub module_dir: PathBuf,
    /// Files held in memory, by host path, read before the disk: a host with no
    /// file system (the browser) hands a module's companions over this way.
    pub mem_files: HashMap<PathBuf, Vec<u8>>,
    /// The guest sees `mem_files` and nothing else: no file or directory of the host is
    /// read, looked up or created, whatever name guest code asks dos.library for. Set for
    /// code that comes with the module (a custom module played by the library's `Song`).
    pub no_host_files: bool,
    /// The LIBS: directory: where a disk library is looked for after the module's own
    /// directories (see `disk_library`). None: there is no LIBS: of the host's.
    pub libs_dir: Option<PathBuf>,
    logged: HashSet<(usize, i32)>,
    pub console: String,
}

const NT_LIBRARY: u8 = 9;
const NT_PROCESS: u8 = 13;
const MEMF_LARGEST: u32 = 1 << 17;

pub const INTB_AUD0: usize = 7;
pub const INTB_VERTB: usize = 5;

pub fn init(m: &mut Machine) {
    let exec = make_library(m, "exec.library", LibKind::Exec, 140, 0x280, 40);
    m.os.exec_base = exec;
    m.bus.w32(4, exec);
    let attn: u16 = match m.cpu.cpu_type {
        m68k::CpuType::M68000 => 0,
        m68k::CpuType::M68010 => 1,
        _ => 3,
    };
    m.bus.w16(exec + 0x128, attn); // AttnFlags
    m.bus.w32(exec + 0x3E, 0x20_0000); // MaxLocMem (chip size)
    m.bus.w8(exec + 0x212, 50); // VBlankFrequency
    m.bus.w8(exec + 0x213, 50); // PowerSupplyFrequency
    m.bus.w32(exec + 0x238, 709_379); // ex_EClockFrequency (PAL)

    // a fake Process for FindTask(0)
    let pr = m.alloc(0x100);
    m.bus.w8(pr + 8, NT_PROCESS);
    let name = m.alloc_cstr("DeliTracker");
    m.bus.w32(pr + 10, name);
    m.bus.w32(pr + 0xB8, 0xFFFF_FFFF); // pr_WindowPtr: no requesters
    m.os.process = pr;
    m.os.current_task = pr;
    m.bus.w32(exec + 0x114, pr); // ThisTask

    let gfx = make_library(m, "graphics.library", LibKind::Graphics, 200, 0x240, 40);
    m.bus.w16(gfx + 0xCE, 4); // DisplayFlags = PAL
    make_library(m, "dos.library", LibKind::Dos, 170, 0x50, 40);
    make_library(m, "utility.library", LibKind::Utility, 60, 0x40, 40);
    make_library(m, "intuition.library", LibKind::Intuition, 140, 0x60, 40);
    make_library(m, "ciaa.resource", LibKind::CiaA, 8, 0x40, 40);
    make_library(m, "ciab.resource", LibKind::CiaB, 8, 0x40, 40);
    make_library(m, "audio.device", LibKind::AudioDev, 8, 0x40, 40);
    make_library(m, "timer.device", LibKind::TimerDev, 12, 0x40, 40);
}

pub fn make_library(m: &mut Machine, name: &str, kind: LibKind, nvec: u32, possize: u32, ver: u16) -> u32 {
    let negsize = nvec * 6;
    let mem = m.alloc(negsize + possize);
    let base = mem + negsize;
    let idx = m.os.libs.len();
    let id = m.new_trap_id(Trap::Lib(idx));
    for i in 1..=nvec {
        let v = base - i * 6;
        m.bus.w16(v, 0xA000 | id);
        m.bus.w16(v + 2, 0x4E75);
        m.bus.w16(v + 4, 0x4E71);
    }
    let name_ptr = m.alloc_cstr(name);
    m.bus.w8(base + 8, NT_LIBRARY);
    m.bus.w32(base + 10, name_ptr);
    m.bus.w16(base + 16, negsize as u16);
    m.bus.w16(base + 18, possize as u16);
    m.bus.w16(base + 20, ver);
    m.bus.w32(base + 24, name_ptr);
    m.os.libs.push(Library { name: name.to_string(), base, kind });
    base
}

fn find_lib(m: &Machine, name: &str) -> Option<u32> {
    let n = name.rsplit([':', '/']).next().unwrap_or(name).to_ascii_lowercase();
    m.os.libs.iter().find(|l| l.name == n).map(|l| l.base)
}

/// The lower-case file name of library name `name` ("LIBS:replayers/X.library" ->
/// "x.library"): what the host's list of libraries knows it by.
fn lib_key(name: &str) -> String {
    name.rsplit([':', '/']).next().unwrap_or(name).to_ascii_lowercase()
}

#[cfg(test)]
fn open_library(m: &mut Machine, name: &str) -> u32 {
    open_library_version(m, name, 0)
}

/// exec OpenLibrary(name, version): a library the host emulates; else one loaded from disk
/// (`disk_library`), opened through its Open vector each time, as exec does; else a stub
/// whose calls answer 0.
fn open_library_version(m: &mut Machine, name: &str, version: u32) -> u32 {
    let key = lib_key(name);
    if let Some(l) = m.os.libs.iter().find(|l| l.name == key && l.kind != LibKind::Stub) {
        let (base, kind) = (l.base, l.kind);
        return if kind == LibKind::Disk { open_disk_library(m, name, base, version) } else { base };
    }
    let lname = name.to_ascii_lowercase();
    // Decrunchers are refused so players fall back to plain data.
    if lname.contains("xpk") || lname.contains("powerpacker") || lname.contains("decrunch") {
        log::info!("OpenLibrary(\"{name}\") refused");
        return 0;
    }
    if let Some(base) = disk_library(m, name) {
        return open_disk_library(m, name, base, version);
    }
    if let Some(b) = find_lib(m, name) {
        return b;
    }
    log::info!("OpenLibrary(\"{name}\") -> stub library");
    make_library(m, &lname, LibKind::Stub, 200, 0x100, 40)
}

// ------------------------------------------------------- disk libraries ---
//
// A library a player opens that the host does not emulate is looked for as a file, as
// ramlib would in LIBS:, and run: LoadSeg, the RomTag (RTC_MATCHWORD) found in its segments,
// InitResident (RTF_AUTOINIT: MakeLibrary with the vector table, the InitStruct table and the
// init routine; else the RomTag's init routine, which makes the library itself), AddLibrary,
// then its Open vector. Where it is looked for, in order (the name as given, with any volume
// dropped, then its file name alone; names in any case):
//   1. the module's directory
//   2. `libs/` in the module's directory
//   3. `libs/` in the directory above it (an archive's modules/ beside its libs/)
//   4. LIBS: (`OsState::libs_dir`: $AMIGAPLAYER_LIBS, ~/.config/amigaplayer-rs/Libs)
// The libraries the host emulates are never loaded (they are in the list before any file is
// looked for), nor are the system libraries of `NEVER_FROM_DISK`: their stubs' answers are what
// players are known to work with.

/// System libraries a player may open that are never loaded from disk.
const NEVER_FROM_DISK: &[&str] = &[
    "exec.library", "dos.library", "graphics.library", "utility.library", "intuition.library",
    "reqtools.library", "req.library", "asl.library", "gadtools.library", "layers.library",
    "icon.library", "workbench.library", "diskfont.library", "commodities.library",
    "locale.library", "keymap.library", "expansion.library", "mathffp.library",
    "mathtrans.library", "mathieeesingbas.library", "mathieeesingtrans.library",
    "mathieeedoubbas.library", "mathieeedoubtrans.library", "datatypes.library",
    "iffparse.library", "rexxsyslib.library", "cybergraphics.library", "identify.library",
];
/// The largest library file loaded.
const DISK_LIB_MAX: u64 = 4 << 20;
/// The most vectors MakeLibrary takes from a table, and the most InitStruct commands run.
const VECTORS_MAX: u32 = 1024;
const INITSTRUCT_MAX: u32 = 4096;
const RTC_MATCHWORD: u16 = 0x4AFC;
const RTF_AUTOINIT: u8 = 0x80;

/// The directories a disk library is looked for in, in order.
fn lib_dirs(os: &OsState) -> Vec<PathBuf> {
    let mut v = Vec::new();
    // (a module named without a directory is in the current one)
    let d = if os.module_dir.as_os_str().is_empty() { Path::new(".") } else { &os.module_dir };
    v.push(d.to_path_buf());
    v.push(d.join("libs"));
    let up = d.parent().filter(|p| !p.as_os_str().is_empty() && d.file_name().is_some());
    v.push(up.map_or_else(|| d.join(".."), Path::to_path_buf).join("libs"));
    v.extend(os.libs_dir.clone());
    v
}

/// The file of library `name`, if one is found (see above).
fn find_disk_library(os: &OsState, name: &str) -> Option<(PathBuf, Vec<u8>)> {
    let rel = name.split_once(':').map_or(name, |x| x.1).trim_start_matches('/');
    let file = rel.rsplit('/').next().unwrap_or(rel);
    if file.is_empty() || rel.split('/').any(|c| c == "..") {
        return None;
    }
    for dir in lib_dirs(os) {
        for r in [rel, file] {
            let p = os.host_path(r, Some(&dir));
            // (a directory or an outsize file is not read; one of `mem_files` is never on disk)
            if !os.no_host_files && std::fs::metadata(&p).is_ok_and(|md| !md.is_file() || md.len() > DISK_LIB_MAX) {
                continue;
            }
            if let Ok(d) = os.read_file(&p) {
                if crate::hunk::is_hunk_file(&d) {
                    return Some((p, d));
                }
            }
        }
    }
    None
}

/// Loads library `name` from disk and initialises it (see above): its base, before any
/// Open. None: no file, or it would not load or initialise.
fn disk_library(m: &mut Machine, name: &str) -> Option<u32> {
    let key = lib_key(name);
    if NEVER_FROM_DISK.contains(&key.as_str()) {
        return None;
    }
    let (path, data) = find_disk_library(&m.os, name)?;
    let (seg, _) = match crate::deli::load_seg(m, &data) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("{}: {e}", path.display());
            return None;
        }
    };
    let Some(rt) = find_romtag(m, seg) else {
        log::warn!("{}: no RomTag", path.display());
        return None;
    };
    let base = match init_resident(m, rt, seg) {
        Ok(b) if b != 0 => b,
        Ok(_) => {
            log::warn!("{}: its init answered 0", path.display());
            return None;
        }
        Err(e) => {
            log::warn!("{}: init: {e}", path.display());
            return None;
        }
    };
    // (known by the name it was asked for too, should its own name be another)
    if !m.os.libs.iter().any(|l| l.base == base && l.name == key) {
        m.os.libs.push(Library { name: key, base, kind: LibKind::Disk });
    }
    log::info!("OpenLibrary(\"{name}\"): {} loaded, base ${base:06x}", path.display());
    Some(base)
}

/// The library's Open vector (a6 = base, d0 = version): what OpenLibrary answers. A library
/// older than `version` is not opened.
fn open_disk_library(m: &mut Machine, name: &str, base: u32, version: u32) -> u32 {
    let have = m.bus.r16(base + 20) as u32;
    if have < version {
        log::info!("OpenLibrary(\"{name}\", {version}): version {have} only");
        return 0;
    }
    let mut r = [0u32; 16];
    r[0] = version;
    r[14] = base;
    match m.call(base.wrapping_sub(6), &r) {
        Ok(o) => o[0],
        Err(e) => {
            log::warn!("OpenLibrary(\"{name}\"): its Open: {e}");
            0
        }
    }
}

/// The disk library at `base`, if it is one: its Close vector, as exec CloseLibrary calls it
/// (what it answers, a segment list to unload when the library is expunged, is ignored: the
/// library stays loaded, as one that is not expunged does).
fn close_disk_library(m: &mut Machine, base: u32) {
    if base == 0 || !m.os.libs.iter().any(|l| l.base == base && l.kind == LibKind::Disk) {
        return;
    }
    let mut r = [0u32; 16];
    r[14] = base;
    if let Err(e) = m.call(base.wrapping_sub(12), &r) {
        log::warn!("CloseLibrary(${base:06x}): its Close: {e}");
    }
}

/// The first RomTag in segment list `seg`: RTC_MATCHWORD at an even address followed by
/// rt_MatchTag pointing at it, as InitResident's callers find one.
fn find_romtag(m: &Machine, seg: u32) -> Option<u32> {
    let mut s = seg;
    for _ in 0..256 {
        if s == 0 {
            break;
        }
        let a = s << 2; // the segment's next pointer; its size is the long before it
        let end = (a - 4).wrapping_add(m.bus.r32(a.wrapping_sub(4)));
        let mut p = a + 4;
        while p + 26 <= end && (p as usize) < crate::bus::RAM_SIZE {
            if m.bus.r16(p) == RTC_MATCHWORD && m.bus.r32(p + 2) == p {
                return Some(p);
            }
            p += 2;
        }
        s = m.bus.r32(a);
    }
    None
}

/// exec InitResident(resident, segList): the library's base, 0 when it refuses.
fn init_resident(m: &mut Machine, rt: u32, seg: u32) -> Result<u32, String> {
    let flags = m.bus.r8(rt + 10);
    let init = m.bus.r32(rt + 22);
    if init == 0 {
        return Ok(0);
    }
    if flags & RTF_AUTOINIT == 0 {
        // the init routine makes and adds the library itself
        let mut r = [0u32; 16];
        r[8] = seg;
        r[14] = m.os.exec_base;
        return m.call(init, &r).map(|o| o[0]);
    }
    let (dsize, vectors, structure, initf) = (m.bus.r32(init), m.bus.r32(init + 4), m.bus.r32(init + 8), m.bus.r32(init + 12));
    let base = make_lib(m, vectors, structure, initf, dsize, seg)?;
    if base != 0 && m.bus.r8(rt + 12) == NT_LIBRARY {
        add_library(m, base);
    }
    Ok(base)
}

/// exec MakeLibrary(vectors, structure, init, dataSize, segList): the library (its vectors
/// before it, `dataSize` bytes from it, cleared), its node filled by `structure` (InitStruct)
/// when there is one, then `init` called (d0 = library, a0 = segList, a6 = exec), whose d0 is
/// the answer; without one, the library.
fn make_lib(m: &mut Machine, vectors: u32, structure: u32, init: u32, dsize: u32, seg: u32) -> Result<u32, String> {
    let (rel, mut a) = if m.bus.r16(vectors) == 0xFFFF { (true, vectors + 2) } else { (false, vectors) };
    let mut n = 0;
    while n < VECTORS_MAX && if rel { m.bus.r16(a) != 0xFFFF } else { m.bus.r32(a) != 0xFFFF_FFFF } {
        n += 1;
        a += if rel { 2 } else { 4 };
    }
    let negsize = (n * 6 + 3) & !3;
    let dsize = dsize.clamp(34, 0xFFFF);
    let mem = m.alloc(negsize + dsize);
    if mem == 0 {
        return Ok(0);
    }
    let base = mem + negsize;
    make_functions(m, base, if rel { vectors + 2 } else { vectors }, if rel { vectors } else { 0 });
    m.bus.w16(base + 16, negsize as u16);
    m.bus.w16(base + 18, dsize as u16);
    if structure != 0 {
        init_struct(m, structure, base, 0);
    }
    if init == 0 {
        return Ok(base);
    }
    let mut r = [0u32; 16];
    r[0] = base;
    r[8] = seg;
    r[14] = m.os.exec_base;
    m.call(init, &r).map(|o| o[0])
}

/// exec MakeFunctions(target, functionArray, funcDispBase): a JMP to each function before
/// `target`, the first at -6. With `disp` the array holds word offsets from it, else
/// addresses; -1 ends it. Answers the size of the vectors.
fn make_functions(m: &mut Machine, target: u32, array: u32, disp: u32) -> u32 {
    let mut a = array;
    let mut n = 0;
    while n < VECTORS_MAX {
        let f = if disp != 0 {
            let w = m.bus.r16(a);
            if w == 0xFFFF {
                break;
            }
            a += 2;
            disp.wrapping_add(w as i16 as i32 as u32)
        } else {
            let l = m.bus.r32(a);
            if l == 0xFFFF_FFFF {
                break;
            }
            a += 4;
            l
        };
        n += 1;
        let v = target.wrapping_sub(n * 6);
        m.bus.w16(v, 0x4EF9); // jmp f
        m.bus.w32(v + 2, f);
    }
    n * 6
}

/// exec InitStruct(initTable, memory, size): `size` bytes of `memory` cleared, then the
/// table's commands run. A command byte: bits 7-6 the destination (0 the next bytes, copied;
/// 1 the next bytes, one value repeated; 2 a byte offset follows; 3 a 24-bit offset follows),
/// bits 5-4 the size (long, word, byte), bits 3-0 the count less one. Word and long data
/// start at an even address of the table, and each command does.
fn init_struct(m: &mut Machine, table: u32, mem: u32, size: u32) {
    for i in 0..size.min(0x1_0000) {
        m.bus.w8(mem + i, 0);
    }
    let mut t = table;
    let mut dst = mem;
    for _ in 0..INITSTRUCT_MAX {
        let cmd = m.bus.r8(t);
        if cmd == 0 {
            break;
        }
        t += 1;
        let (dest, sz, cnt) = (cmd >> 6, (cmd >> 4) & 3, (cmd & 15) as u32 + 1);
        match dest {
            2 => {
                dst = mem + m.bus.r8(t) as u32;
                t += 1;
            }
            3 => {
                dst = mem + ((m.bus.r8(t) as u32) << 16 | m.bus.r16(t + 1) as u32);
                t += 3;
            }
            _ => {}
        }
        let w = match sz {
            0 => 4,
            1 => 2,
            2 => 1,
            _ => break,
        };
        if w > 1 {
            t = (t + 1) & !1;
        }
        let rd = |m: &Machine, a: u32| match w {
            4 => m.bus.r32(a),
            2 => m.bus.r16(a) as u32,
            _ => m.bus.r8(a) as u32,
        };
        let wr = |m: &mut Machine, a: u32, v: u32| match w {
            4 => m.bus.w32(a, v),
            2 => m.bus.w16(a, v as u16),
            _ => m.bus.w8(a, v as u8),
        };
        if dest == 1 {
            let v = rd(m, t);
            t += w;
            for _ in 0..cnt {
                wr(m, dst, v);
                dst += w;
            }
        } else {
            for _ in 0..cnt {
                let v = rd(m, t);
                t += w;
                wr(m, dst, v);
                dst += w;
            }
        }
        t = (t + 1) & !1;
    }
}

/// exec AddLibrary(library): the host knows it by its name (ln_Name), and OpenLibrary of
/// that name opens it.
fn add_library(m: &mut Machine, base: u32) {
    let np = m.bus.r32(base + 10);
    if np == 0 || m.os.libs.iter().any(|l| l.base == base) {
        return;
    }
    let name = lib_key(&m.bus.read_cstr(np));
    if !name.is_empty() {
        m.os.libs.push(Library { name, base, kind: LibKind::Disk });
    }
}

pub fn lib_call(m: &mut Machine, lib: usize, lvo: i32) {
    let kind = m.os.libs[lib].kind;
    log::trace!("{} LVO -{lvo} d0=${:x} a0=${:x} a1=${:x}", m.os.libs[lib].name, m.d(0), m.a(0), m.a(1));
    let handled = match kind {
        LibKind::Exec => exec_call(m, lvo),
        LibKind::Dos => dos_call(m, lvo),
        LibKind::Utility => utility_call(m, lvo),
        LibKind::CiaA => cia_call(m, 0, lvo),
        LibKind::CiaB => cia_call(m, 1, lvo),
        LibKind::AudioDev | LibKind::TimerDev => device_call(m, lib, lvo),
        LibKind::Intuition => intuition_call(m, lvo),
        LibKind::Stub => stub_call(m, lib, lvo),
        LibKind::Graphics | LibKind::Disk => false,
    };
    if !handled {
        if m.os.logged.insert((lib, lvo)) {
            log::warn!("unimplemented {} LVO -{}", m.os.libs[lib].name, lvo);
        }
        m.set_d0(0);
    }
    // (a player that prints in a loop does not grow the host's memory)
    if m.os.console.len() > CONSOLE_MAX {
        m.os.console.clear();
    }
}

/// The few calls of a stub library whose answer 0 makes a player give up.
///
/// stonepacker.library (the StoneTracker Eagleplayer, before it loads the SPS sample file):
/// LVO -30 takes no argument but the base and its result is a handle the player keeps and hands
/// back to LVO -36 in a0 when the file is read; 0 is refused ("could not load additional
/// files"). Observed black-box: with any non-zero answer the player reads the SPS through
/// dos.library itself and plays, and it never reads or writes the handle's memory (read watch
/// over it, its bytes unchanged at -36). So -30 answers a zeroed block of its own (not freed:
/// -36 stays a stub returning 0, which the player ignores).
fn stub_call(m: &mut Machine, lib: usize, lvo: i32) -> bool {
    let name = m.os.libs[lib].name.rsplit([':', '/']).next().unwrap_or("").to_string();
    match (name.as_str(), lvo) {
        ("stonepacker.library", 30) => {
            let p = m.alloc(STONEPACKER_HANDLE);
            m.set_d0(p);
            true
        }
        _ => false,
    }
}

/// The size of the block stonepacker.library's LVO -30 answers (never touched by the player).
const STONEPACKER_HANDLE: u32 = 0x100;

// ---------------------------------------------------------------- exec ---

const EXEC_NAMES: &[&str] = &[
    "Supervisor", "ExitIntr", "Schedule", "Reschedule", "Switch", "Dispatch", "Exception",
    "InitCode", "InitStruct", "MakeLibrary", "MakeFunctions", "FindResident", "InitResident",
    "Alert", "Debug", "Disable", "Enable", "Forbid", "Permit", "SetSR", "SuperState", "UserState",
    "SetIntVector", "AddIntServer", "RemIntServer", "Cause", "Allocate", "Deallocate", "AllocMem",
    "AllocAbs", "FreeMem", "AvailMem", "AllocEntry", "FreeEntry", "Insert", "AddHead", "AddTail",
    "Remove", "RemHead", "RemTail", "Enqueue", "FindName", "AddTask", "RemTask", "FindTask",
    "SetTaskPri", "SetSignal", "SetExcept", "Wait", "Signal", "AllocSignal", "FreeSignal",
    "AllocTrap", "FreeTrap", "AddPort", "RemPort", "PutMsg", "GetMsg", "ReplyMsg", "WaitPort",
    "FindPort", "AddLibrary", "RemLibrary", "OldOpenLibrary", "CloseLibrary", "SetFunction",
    "SumLibrary", "AddDevice", "RemDevice", "OpenDevice", "CloseDevice", "DoIO", "SendIO",
    "CheckIO", "WaitIO", "AbortIO", "AddResource", "RemResource", "OpenResource", "RawIOInit",
    "RawMayGetChar", "RawPutChar", "RawDoFmt", "GetCC", "TypeOfMem", "Procure", "Vacate",
    "OpenLibrary", "InitSemaphore", "ObtainSemaphore", "ReleaseSemaphore", "AttemptSemaphore",
    "ObtainSemaphoreList", "ReleaseSemaphoreList", "FindSemaphore", "AddSemaphore",
    "RemSemaphore", "SumKickData", "AddMemList", "CopyMem", "CopyMemQuick", "CacheClearU",
    "CacheClearE", "CacheControl", "CreateIORequest", "DeleteIORequest", "CreateMsgPort",
    "DeleteMsgPort", "ObtainSemaphoreShared", "AllocVec", "FreeVec", "CreatePrivatePool",
    "DeletePrivatePool", "AllocPooled", "FreePooled", "AttemptSemaphoreShared", "ColdReboot",
    "StackSwap", "ChildFree", "ChildOrphan", "ChildStatus", "ChildWait", "CachePreDMA",
    "CachePostDMA",
];

fn lvo_name(names: &[&'static str], lvo: i32) -> Option<&'static str> {
    if lvo < 30 || (lvo - 30) % 6 != 0 {
        return None;
    }
    names.get(((lvo - 30) / 6) as usize).copied()
}

fn exec_call(m: &mut Machine, lvo: i32) -> bool {
    let Some(name) = lvo_name(EXEC_NAMES, lvo) else { return false };
    let (d0, d1, a0, a1) = (m.d(0), m.d(1), m.a(0), m.a(1));
    match name {
        "Disable" | "Enable" | "Forbid" | "Permit" | "UserState" | "CacheClearU" | "CacheClearE"
        | "InitSemaphore" | "ObtainSemaphore" | "ReleaseSemaphore" | "ObtainSemaphoreShared"
        | "ObtainSemaphoreList" | "ReleaseSemaphoreList" | "AddPort" | "RemPort" | "AddTask"
        | "CloseDevice" | "CachePreDMA"
        | "CachePostDMA" | "SumLibrary" | "AddResource" | "AddDevice"
        | "AddSemaphore" | "RemSemaphore" | "Debug" | "DeletePrivatePool" => m.set_d0(0),
        "SuperState" => m.set_d0(0),
        "SetSR" => {
            let old = m.cpu.get_sr();
            let new = (old & !(d1 as u16)) | (d0 as u16 & d1 as u16);
            m.cpu.set_sr(new | 0x2000);
            m.set_d0(old as u32);
        }
        "GetCC" => m.set_d0((m.cpu.get_sr() & 0x1F) as u32),
        "Supervisor" => {
            // a5 = code ending in RTE; run it as a subroutine
            let code = m.a(5);
            let regs = m.cpu.dar;
            match call_rte(m, code, &regs) {
                Ok(r) => m.cpu.dar[..15].copy_from_slice(&r[..15]),
                Err(e) => m.crashed = Some(e),
            }
        }
        "Alert" => log::warn!("guest Alert(${d0:08x})"),
        "SetIntVector" => {
            let n = d0 as usize & 15;
            let old = m.os.int_vectors[n];
            m.os.int_vectors[n] = a1;
            m.set_d0(old);
        }
        "AddIntServer" => {
            let n = d0 as usize & 15;
            if !m.os.int_servers[n].contains(&a1) && m.os.int_servers[n].len() < INT_SERVERS_MAX {
                m.os.int_servers[n].push(a1);
            }
        }
        "RemIntServer" => {
            let n = d0 as usize & 15;
            m.os.int_servers[n].retain(|&x| x != a1);
        }
        "Cause" => {
            let code = m.bus.r32(a1 + 18);
            let data = m.bus.r32(a1 + 14);
            call_int_code(m, code, data, 0);
        }
        "AllocMem" | "AllocVec" => {
            // AllocVec of no bytes still returns a block, and a player may write there: UADE's
            // Octa-MED-Ss asks for 0 bytes and fills 76 a mixing channel (up to 64 channels,
            // 4864 bytes; measured). Under UADE nothing it needs lies behind; here the next
            // allocation would (its mixing tables), so such a block gets room behind it.
            let d0 = if name == "AllocVec" && d0 == 0 { ALLOCVEC0_ROOM } else { d0 };
            let p = m.alloc(d0);
            m.set_d0(p);
        }
        "Allocate" => {
            let p = m.alloc(d0);
            m.set_d0(p);
        }
        "AllocAbs" => m.set_d0(0),
        "FreeMem" | "FreeVec" | "Deallocate" => {
            if a1 != 0 && m.heap.free_containing(a1).is_none() {
                log::debug!("{name} of unknown block ${a1:06x}");
            }
        }
        "FreePooled" => {
            m.heap.free_containing(a1);
        }
        "AvailMem" => {
            let v = if d1 & MEMF_LARGEST != 0 { m.heap.largest() } else { m.heap.available() };
            m.set_d0(v);
        }
        "TypeOfMem" => m.set_d0(if (a1 as usize) < crate::bus::RAM_SIZE { 3 } else { 0 }),
        "CopyMem" | "CopyMemQuick" => {
            let (s, d, n) = (a0 as usize & 0xFF_FFFF, a1 as usize & 0xFF_FFFF, d0 as usize);
            let r = &mut m.bus.ram;
            if s + n <= r.len() && d + n <= r.len() {
                r.copy_within(s..s + n, d);
            }
        }
        "CreatePrivatePool" => {
            let p = m.alloc(16);
            m.set_d0(p);
        }
        "AllocPooled" => {
            let p = m.alloc(d0);
            m.set_d0(p);
        }
        "Insert" => list_insert(m, a0, a1, m.a(2)),
        "AddHead" => list_insert(m, a0, a1, a0),
        "AddTail" => {
            let pred = m.bus.r32(a0 + 8);
            list_insert(m, a0, a1, pred);
        }
        "Remove" => node_remove(m, a1),
        "RemHead" => {
            let n = m.bus.r32(a0);
            if m.bus.r32(n) != 0 {
                node_remove(m, n);
                m.set_d0(n);
            } else {
                m.set_d0(0);
            }
        }
        "RemTail" => {
            let n = m.bus.r32(a0 + 8);
            if m.bus.r32(n + 4) != 0 {
                node_remove(m, n);
                m.set_d0(n);
            } else {
                m.set_d0(0);
            }
        }
        "Enqueue" => {
            let pri = m.bus.r8(a1 + 9) as i8;
            let mut n = m.bus.r32(a0);
            while m.bus.r32(n) != 0 && (m.bus.r8(n + 9) as i8) >= pri {
                n = m.bus.r32(n);
            }
            let pred = m.bus.r32(n + 4);
            list_insert(m, a0, a1, pred);
        }
        "FindName" => {
            let want = m.bus.read_cstr(m.a(1));
            let mut n = m.bus.r32(a0);
            let mut found = 0;
            while n != 0 && m.bus.r32(n) != 0 {
                let np = m.bus.r32(n + 10);
                if np != 0 && m.bus.read_cstr(np) == want {
                    found = n;
                    break;
                }
                n = m.bus.r32(n);
            }
            m.set_d0(found);
        }
        "FindTask" => m.set_d0(if a1 == 0 { m.os.current_task } else { 0 }),
        "SetTaskPri" => {
            // a1 = task, d0 = priority -> old priority
            if a1 == 0 {
                m.set_d0(0);
            } else {
                let old = m.bus.r8(a1 + 9) as i8;
                m.bus.w8(a1 + 9, d0 as u8);
                m.set_d0(old as i32 as u32);
            }
        }
        "SetExcept" => m.set_d0(0),
        "SetSignal" => {
            let t = m.os.current_task;
            let old = m.bus.r32(t + TC_SIGRECVD);
            m.bus.w32(t + TC_SIGRECVD, (old & !d1) | (d0 & d1));
            m.set_d0(old);
        }
        "Signal" => signal(m, a1, d0),
        "Wait" => {
            let t = m.os.current_task;
            let got = m.bus.r32(t + TC_SIGRECVD) & d0;
            if got != 0 || !m.can_yield() {
                // DeliTracker's own context cannot wait for anything: everything has arrived
                let got = if got != 0 { got } else { d0 };
                m.bus.w32(t + TC_SIGRECVD, m.bus.r32(t + TC_SIGRECVD) & !got);
                m.set_d0(got);
            } else {
                m.bus.w32(t + TC_SIGWAIT, d0);
                m.yield_now(None);
            }
        }
        "AllocSignal" => {
            let t = m.os.current_task;
            let used = m.bus.r32(t + TC_SIGALLOC);
            let s = if (d0 as i8) >= 0 { d0 & 31 } else { (16..32).find(|b| used & 1 << b == 0).unwrap_or(31) };
            m.bus.w32(t + TC_SIGALLOC, used | 1 << s);
            m.set_d0(s);
        }
        "FreeSignal" => {
            if (d0 as i8) >= 0 {
                let t = m.os.current_task;
                m.bus.w32(t + TC_SIGALLOC, m.bus.r32(t + TC_SIGALLOC) & !(1 << (d0 & 31)));
            }
        }
        "GetMsg" => {
            let n = get_msg(m, a0);
            m.set_d0(n);
        }
        "WaitPort" => {
            let head = if a0 == 0 { 0 } else { m.bus.r32(a0 + 20) };
            if head != 0 && m.bus.r32(head) != 0 {
                m.set_d0(head);
            } else if a0 != 0 && head != 0 && m.can_yield() {
                let t = m.os.current_task;
                m.bus.w32(t + TC_SIGWAIT, 1 << (m.bus.r8(a0 + 15) & 31));
                m.yield_now(None);
            } else {
                m.set_d0(0);
            }
        }
        "FindPort" | "FindSemaphore" | "FindResident" => m.set_d0(0),
        "PutMsg" => put_msg(m, a0, a1, 5),   // NT_MESSAGE
        "ReplyMsg" => {
            let port = m.bus.r32(a1 + 14); // mn_ReplyPort
            put_msg(m, port, a1, 7); // NT_REPLYMSG
            m.set_d0(0);
        }
        "AttemptSemaphore" | "AttemptSemaphoreShared" => m.set_d0(1),
        "OldOpenLibrary" | "OpenLibrary" => {
            let n = m.bus.read_cstr(a1);
            let b = open_library_version(m, &n, if name == "OpenLibrary" { d0 } else { 0 });
            m.set_d0(b);
        }
        "CloseLibrary" => {
            close_disk_library(m, a1);
            m.set_d0(0);
        }
        "AddLibrary" => {
            add_library(m, a1);
            m.set_d0(0);
        }
        "MakeLibrary" => {
            let (a2, seg) = (m.a(2), d1);
            let r = make_lib(m, a0, a1, a2, d0, seg).unwrap_or_else(|e| {
                log::warn!("MakeLibrary: its init: {e}");
                0
            });
            m.set_d0(r);
        }
        "MakeFunctions" => {
            let a2 = m.a(2);
            let r = make_functions(m, a0, a1, a2);
            m.set_d0(r);
        }
        "InitStruct" => {
            let a2 = m.a(2);
            init_struct(m, a1, a2, d0);
        }
        "InitResident" => {
            let r = init_resident(m, a1, d1).unwrap_or_else(|e| {
                log::warn!("InitResident: {e}");
                0
            });
            m.set_d0(r);
        }
        "OpenResource" => {
            let n = m.bus.read_cstr(a1);
            let b = find_lib(m, &n).unwrap_or(0);
            if b == 0 {
                log::info!("OpenResource(\"{n}\") failed");
            }
            m.set_d0(b);
        }
        "SetFunction" => {
            // a1 = library, a0 = offset (negative), d0 = new function
            let v = a1.wrapping_add(a0);
            let old_copy = m.alloc(8);
            for i in 0..6 {
                let b = m.bus.r8(v + i);
                m.bus.w8(old_copy + i, b);
            }
            m.bus.w16(v, 0x4EF9);
            m.bus.w32(v + 2, d0);
            m.set_d0(old_copy);
        }
        "OpenDevice" => {
            let n = m.bus.read_cstr(a0);
            let r = open_device(m, &n, d0, a1);
            m.set_d0(r as u32);
        }
        "DoIO" | "SendIO" => {
            // DoIO completes synchronously (IOF_QUICK); SendIO replies to the port
            let f = m.bus.r8(a1 + 30);
            m.bus.w8(a1 + 30, if name == "DoIO" { f | 1 } else { f & !1 });
            let r = begin_io(m, a1);
            m.set_d0(r as u32);
            if name == "DoIO" && io_pending(m, a1) {
                // a write that has to play first: the rest of DoIO is a WaitIO
                wait_io(m, a1, Some(m.os.exec_base.wrapping_sub(474)));
            }
        }
        "CheckIO" => m.set_d0(if io_pending(m, a1) { 0 } else { a1 }),
        "WaitIO" => wait_io(m, a1, None),
        "AbortIO" => {
            abort_io(m, a1);
            m.set_d0(m.bus.r8(a1 + 31) as i8 as i32 as u32);
        }
        "CreateMsgPort" => {
            let p = m.alloc(34);
            if p != 0 {
                let t = m.os.current_task;
                let used = m.bus.r32(t + TC_SIGALLOC);
                let bit = (16..32).find(|b| used & 1 << b == 0).unwrap_or(16);
                m.bus.w32(t + TC_SIGALLOC, used | 1 << bit);
                m.bus.w8(p + 8, 4); // NT_MSGPORT
                m.bus.w8(p + 15, bit as u8); // mp_SigBit
                m.bus.w32(p + 16, t);
                let l = p + 20; // mp_MsgList: empty list
                m.bus.w32(l, l + 4);
                m.bus.w32(l + 8, l);
            }
            m.set_d0(p);
        }
        "DeleteMsgPort" => {
            m.heap.free(a0);
        }
        "CreateIORequest" => {
            let p = m.alloc(d0.max(32));
            if p != 0 {
                m.bus.w32(p + 14, a0);
                m.bus.w16(p + 18, d0 as u16);
            }
            m.set_d0(p);
        }
        "DeleteIORequest" => {
            m.heap.free(a0);
        }
        "RawDoFmt" => raw_do_fmt(m),
        "CacheControl" => m.set_d0(0),
        "RawPutChar" => m.os.console.push(d0 as u8 as char),
        _ => return false,
    }
    true
}

fn list_insert(m: &mut Machine, list: u32, node: u32, pred: u32) {
    let pred = if pred == 0 { list } else { pred };
    let succ = m.bus.r32(pred);
    m.bus.w32(node, succ);
    m.bus.w32(node + 4, pred);
    m.bus.w32(succ + 4, node);
    m.bus.w32(pred, node);
}

/// Appends a message to a port's mp_MsgList. As in UADE's score, a port whose list was never
/// initialised (lh_Head 0) is left alone, which is what a stub ReplyMsg did for every port.
pub fn put_msg(m: &mut Machine, port: u32, msg: u32, kind: u8) {
    m.bus.w8(msg + 8, kind);
    if port == 0 || m.bus.r32(port + 20) == 0 {
        return;
    }
    let l = port + 20;
    let pred = m.bus.r32(l + 8);
    list_insert(m, l, msg, pred);
    // PA_SIGNAL: wake the port's task
    if m.bus.r8(port + 14) & 3 == 0 {
        let task = m.bus.r32(port + 16);
        signal(m, task, 1 << (m.bus.r8(port + 15) & 31));
    }
}

// Task structure offsets
pub const TC_SIGALLOC: u32 = 18;
pub const TC_SIGWAIT: u32 = 22;
pub const TC_SIGRECVD: u32 = 26;

pub fn signal(m: &mut Machine, task: u32, sigs: u32) {
    if task != 0 {
        m.bus.w32(task + TC_SIGRECVD, m.bus.r32(task + TC_SIGRECVD) | sigs);
    }
}

/// Whether a task waits for a signal it has received, and should run.
pub fn task_ready(m: &Machine, task: u32) -> bool {
    m.bus.r32(task + TC_SIGRECVD) & m.bus.r32(task + TC_SIGWAIT) != 0
}

/// Removes and returns the first message of a port (0: none, or the list was never initialised).
pub fn get_msg(m: &mut Machine, port: u32) -> u32 {
    if port == 0 {
        return 0;
    }
    let n = m.bus.r32(port + 20);
    if n == 0 || m.bus.r32(n) == 0 {
        return 0;
    }
    node_remove(m, n);
    n
}

fn node_remove(m: &mut Machine, node: u32) {
    let succ = m.bus.r32(node);
    let pred = m.bus.r32(node + 4);
    m.bus.w32(pred, succ);
    m.bus.w32(succ + 4, pred);
}

/// Runs an interrupt handler the way exec does: A1 = is_Data, A5 = is_Code,
/// A0 = custom chip base, A6 = ExecBase, D1 = active interrupt bits.
pub fn call_int_code(m: &mut Machine, code: u32, data: u32, d1: u32) -> u32 {
    if code == 0 {
        return 0;
    }
    let mut r = [0u32; 16];
    r[1] = d1;
    r[8] = 0xDF_F000;
    r[9] = data;
    r[13] = code;
    r[14] = m.os.exec_base;
    match m.call(code, &r) {
        Ok(o) => o[0],
        Err(e) => {
            m.crashed = Some(format!("interrupt code: {e}"));
            0
        }
    }
}

/// Calls code that returns with RTE (Supervisor()).
fn call_rte(m: &mut Machine, code: u32, regs: &crate::machine::Regs) -> Result<crate::machine::Regs, String> {
    // A small trampoline: the code is entered via JSR from a stub that
    // fakes the exception frame the RTE will pop.
    let tramp = m.alloc(32);
    let sr = m.cpu.get_sr();
    let is_000 = matches!(m.cpu.cpu_type, m68k::CpuType::M68000);
    let mut b: Vec<u8> = Vec::new();
    let ret = tramp + 24;
    if !is_000 {
        b.extend_from_slice(&[0x3F, 0x3C, 0x00, 0x00]); // move.w #0,-(sp) format word
    }
    b.extend_from_slice(&[0x2F, 0x3C]); // move.l #ret,-(sp)
    b.extend_from_slice(&ret.to_be_bytes());
    b.extend_from_slice(&[0x3F, 0x3C]); // move.w #sr,-(sp)
    b.extend_from_slice(&sr.to_be_bytes());
    b.extend_from_slice(&[0x4E, 0xF9]); // jmp code
    b.extend_from_slice(&code.to_be_bytes());
    m.bus.write_bytes(tramp, &b);
    m.bus.write_bytes(ret, &[0x4E, 0x75]);
    let r = m.call(tramp, regs);
    m.heap.free(tramp);
    r
}

fn raw_do_fmt(m: &mut Machine) {
    let fmt = m.bus.read_cstr(m.a(0));
    let mut data = m.a(1);
    let putch = m.a(2);
    let mut a3 = m.a(3);
    let mut out = String::new();
    let fb = fmt.as_bytes();
    let mut i = 0;
    while i < fb.len() {
        let c = fb[i] as char;
        i += 1;
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut spec = String::new();
        while i < fb.len() && "-0123456789.".contains(fb[i] as char) {
            spec.push(fb[i] as char);
            i += 1;
        }
        let mut long = false;
        if i < fb.len() && fb[i] == b'l' {
            long = true;
            i += 1;
        }
        let Some(&conv) = fb.get(i) else { break };
        i += 1;
        let mut arg = || {
            let v = if long { m.bus.r32(data) } else { m.bus.r16(data) as u32 };
            data += if long { 4 } else { 2 };
            v
        };
        let width: usize = spec.trim_start_matches(['-', '0']).split('.').next().unwrap_or("").parse().unwrap_or(0);
        let s = match conv {
            b'd' => {
                let v = arg();
                if long { (v as i32).to_string() } else { (v as u16 as i16).to_string() }
            }
            b'u' => arg().to_string(),
            b'x' | b'X' => format!("{:x}", arg()),
            b'c' => ((arg() as u8) as char).to_string(),
            b's' => {
                let p = m.bus.r32(data);
                data += 4;
                m.bus.read_cstr(p)
            }
            b'b' => {
                let p = m.bus.r32(data);
                data += 4;
                let bp = p << 2;
                let n = m.bus.r8(bp) as u32;
                (0..n).map(|k| m.bus.r8(bp + 1 + k) as char).collect()
            }
            b'%' => "%".into(),
            _ => String::new(),
        };
        let pad = width.saturating_sub(s.len());
        if spec.starts_with('-') {
            out.push_str(&s);
            out.extend(std::iter::repeat_n(' ', pad));
        } else {
            let p = if spec.starts_with('0') { '0' } else { ' ' };
            out.extend(std::iter::repeat_n(p, pad));
            out.push_str(&s);
        }
    }
    for ch in out.bytes().chain(std::iter::once(0)) {
        let mut r = m.cpu.dar;
        r[0] = ch as u32;
        r[11] = a3;
        match m.call(putch, &r) {
            Ok(o) => a3 = o[11],
            Err(e) => {
                m.crashed = Some(e);
                return;
            }
        }
    }
    m.set_d0(data);
}

// ------------------------------------------------------------- devices ---

fn open_device(m: &mut Machine, name: &str, unit: u32, ioreq: u32) -> i8 {
    let Some(base) = find_lib(m, name) else {
        log::info!("OpenDevice(\"{name}\") failed");
        m.bus.w8(ioreq + 31, 0xFF);
        return -1;
    };
    m.bus.w32(ioreq + 20, base);
    m.bus.w32(ioreq + 24, unit);
    m.bus.w8(ioreq + 31, 0);
    if name.contains("audio") {
        // the open hands out the allocation key; copies of the request share it
        if m.bus.r16(ioreq + 32) == 0 {
            m.os.audio_alloc.next_key = m.os.audio_alloc.next_key.wrapping_add(1).max(1);
            m.bus.w16(ioreq + 32, m.os.audio_alloc.next_key);
        }
        // an allocation request may be part of the open (it never waits)
        if m.bus.r32(ioreq + 38) != 0 {
            m.bus.w8(ioreq + 30, m.bus.r8(ioreq + 30) | ADIOF_NOWAIT);
            if !audio_allocate(m, ioreq) {
                return m.bus.r8(ioreq + 31) as i8;
            }
        }
    }
    0
}

// audio.device. IOAudio extends IORequest (not IOStdReq): ln_Pri 9 (allocation precedence),
// io_Unit 24 (the channel bitmask), ioa_AllocKey 32, ioa_Data 34, ioa_Length 38 (bytes),
// ioa_Period 42, ioa_Volume 44, ioa_Cycles 46 (0: forever), ioa_WriteMsg 48.

const ADIOF_PERVOL: u8 = 0x10;
const ADIOF_SYNCCYCLE: u8 = 0x20;
const ADIOF_NOWAIT: u8 = 0x40;
const ADIOF_WRITEMESSAGE: u8 = 0x80;
const IOERR_ABORTED: u8 = (-2i8) as u8;
const ADIOERR_NOALLOCATION: u8 = (-10i8) as u8;
const ADIOERR_ALLOCFAILED: u8 = (-11i8) as u8;
const ADIOERR_CHANNELSTOLEN: u8 = (-12i8) as u8;

/// One audio.device channel. Paula raises the channel's interrupt each time it reloads
/// AUDxLC/AUDxLEN and starts a pass through the sample; the next request's registers are
/// written then, so it follows without a gap, as the real device does.
#[derive(Default, Clone, Debug)]
pub struct AudioChan {
    /// The allocation that owns the channel: key (0: free) and precedence.
    key: u16,
    pri: i8,
    /// The request whose passes Paula is playing.
    pub playing: Option<u32>,
    /// Passes of `playing` started so far.
    passes: u32,
    /// The request whose sample is in the registers, to start at the next reload.
    latched: Option<u32>,
    /// Writes waiting for their turn.
    queue: VecDeque<u32>,
    /// CMD_STOP: writes queue up until CMD_START.
    stopped: bool,
    /// Requests answered at the end of the current pass: ADCMD_WAITCYCLE, and ADCMD_PERVOL
    /// and ADCMD_FINISH with ADIOF_SYNCCYCLE.
    at_cycle: Vec<u32>,
    /// ADCMD_LOCK requests, answered when the channel is stolen or freed.
    locks: Vec<u32>,
}

impl AudioChan {
    fn holds(&self, io: u32) -> bool {
        self.playing == Some(io)
            || self.latched == Some(io)
            || self.queue.contains(&io)
            || self.at_cycle.contains(&io)
            || self.locks.contains(&io)
    }
    fn busy(&self) -> bool {
        self.playing.is_some() || self.latched.is_some()
    }
    /// Its writes and pass requests, once each (a repeating write is both playing and
    /// latched, and must not be replied twice).
    fn requests(&self) -> Vec<u32> {
        let mut v: Vec<u32> = Vec::new();
        for io in self.playing.into_iter().chain(self.latched).chain(self.queue.iter().copied()).chain(self.at_cycle.iter().copied()) {
            if !v.contains(&io) {
                v.push(io);
            }
        }
        v
    }
}

/// Channel allocations that have to wait (ADCMD_ALLOCATE without ADIOF_NOWAIT).
#[derive(Default, Debug)]
pub struct AudioAlloc {
    next_key: u16,
    waiting: Vec<u32>,
}

/// ADCMD_ALLOCATE: the first combination in the ioa_Data array of ioa_Length masks whose
/// channels are free, already this key's, or held at a lower precedence (those are
/// stolen). Returns false when it failed (ADIOF_NOWAIT) or has to wait.
fn audio_allocate(m: &mut Machine, ioreq: u32) -> bool {
    let (masks, n) = (m.bus.r32(ioreq + 34), m.bus.r32(ioreq + 38));
    let pri = m.bus.r8(ioreq + 9) as i8;
    let mut key = m.bus.r16(ioreq + 32);
    if key == 0 {
        m.os.audio_alloc.next_key = m.os.audio_alloc.next_key.wrapping_add(1).max(1);
        key = m.os.audio_alloc.next_key;
        m.bus.w16(ioreq + 32, key);
    }
    let combos: Vec<u32> = if n == 0 { vec![0xF] } else { (0..n.min(16)).map(|i| m.bus.r8(masks + i) as u32 & 0xF).collect() };
    let fits = |c: &AudioChan| c.key == 0 || c.key == key || c.pri < pri;
    let Some(&mask) = combos.iter().find(|&&mk| (0..4).all(|ch| mk & 1 << ch == 0 || fits(&m.os.audio[ch]))) else {
        if m.bus.r8(ioreq + 30) & ADIOF_NOWAIT != 0 {
            m.bus.w8(ioreq + 31, ADIOERR_ALLOCFAILED);
        } else if !m.os.audio_alloc.waiting.contains(&ioreq) {
            m.os.audio_alloc.waiting.push(ioreq);
        }
        return false;
    };
    for ch in 0..4 {
        if mask & 1 << ch != 0 {
            let c = &m.os.audio[ch];
            if c.key != 0 && c.key != key {
                audio_release(m, ch, ADIOERR_CHANNELSTOLEN);
            }
            let c = &mut m.os.audio[ch];
            c.key = key;
            c.pri = pri;
        }
    }
    m.bus.w32(ioreq + 24, mask);
    m.bus.w8(ioreq + 31, 0);
    true
}

/// A channel leaves its owner (stolen or freed): its writes end with `err`, its locks are
/// answered (with ADIOERR_CHANNELSTOLEN when stolen), and the sound stops.
fn audio_release(m: &mut Machine, ch: usize, err: u8) {
    let c = std::mem::take(&mut m.os.audio[ch]);
    audio_dma(m, ch, false);
    for io in c.requests() {
        m.bus.w8(io + 31, err);
        reply_io(m, io);
    }
    for io in c.locks {
        m.bus.w8(io + 31, if err == ADIOERR_CHANNELSTOLEN { err } else { 0 });
        reply_io(m, io);
    }
}

/// Allocations that waited may fit now.
fn audio_retry_waiting(m: &mut Machine) {
    for io in std::mem::take(&mut m.os.audio_alloc.waiting) {
        if audio_allocate(m, io) {
            reply_io(m, io);
        }
    }
}

fn begin_io(m: &mut Machine, ioreq: u32) -> i8 {
    let dev = m.bus.r32(ioreq + 20);
    let cmd = m.bus.r16(ioreq + 28);
    let kind = m.os.libs.iter().find(|l| l.base == dev).map(|l| l.kind);
    match kind {
        Some(LibKind::AudioDev) => {
            m.bus.w8(ioreq + 31, 0);
            if audio_command(m, ioreq, cmd) {
                // done at once; unless IOF_QUICK, it comes back on its reply port
                reply_io(m, ioreq);
            } else {
                // answered later: never quick
                m.bus.w8(ioreq + 30, m.bus.r8(ioreq + 30) & !1);
                m.bus.w8(ioreq + 8, 5); // NT_MESSAGE
            }
            return 0;
        }
        Some(LibKind::TimerDev) => {}
        _ => log::warn!("IO request for unknown device ${dev:06x}"),
    }
    m.bus.w8(ioreq + 31, 0);
    0
}

/// Completes an IO request: with IOF_QUICK clear, it is replied to its port.
fn reply_io(m: &mut Machine, ioreq: u32) {
    if m.bus.r8(ioreq + 30) & 1 == 0 {
        let port = m.bus.r32(ioreq + 14);
        put_msg(m, port, ioreq, 7);
    }
}

/// A request audio.device has accepted but not answered yet.
fn io_pending(m: &Machine, ioreq: u32) -> bool {
    m.os.audio.iter().any(|c| c.holds(ioreq)) || m.os.audio_alloc.waiting.contains(&ioreq)
}

/// WaitIO: a request still playing blocks the task until it is done; a replied one is taken
/// off its reply port. `resume` is where a blocked task continues (None: WaitIO again).
fn wait_io(m: &mut Machine, ioreq: u32, resume: Option<u32>) {
    if io_pending(m, ioreq) {
        if m.can_yield() {
            let port = m.bus.r32(ioreq + 14);
            let t = m.os.current_task;
            m.bus.w32(t + TC_SIGWAIT, 1 << (m.bus.r8(port + 15) & 31));
            m.yield_now(resume);
            return;
        }
        // DeliTracker's own context cannot wait: finish it now
        if m.os.logged.insert((usize::MAX - 1, 0)) {
            log::warn!("WaitIO on a pending audio request outside a task; aborted");
        }
        abort_io(m, ioreq);
    }
    if m.bus.r8(ioreq + 8) == 7 && m.bus.r32(ioreq) != 0 {
        node_remove(m, ioreq);
        m.bus.w32(ioreq, 0);
    }
    m.set_d0(m.bus.r8(ioreq + 31) as i8 as i32 as u32);
}

fn abort_io(m: &mut Machine, ioreq: u32) {
    if let Some(i) = m.os.audio_alloc.waiting.iter().position(|&w| w == ioreq) {
        m.os.audio_alloc.waiting.remove(i);
        m.bus.w8(ioreq + 31, IOERR_ABORTED);
        reply_io(m, ioreq);
        return;
    }
    for ch in 0..4 {
        if m.os.audio[ch].holds(ioreq) {
            let c = &mut m.os.audio[ch];
            c.queue.retain(|&q| q != ioreq);
            c.at_cycle.retain(|&q| q != ioreq);
            c.locks.retain(|&q| q != ioreq);
            if c.latched == Some(ioreq) {
                c.latched = c.queue.pop_front();
                if let Some(n) = c.latched {
                    audio_latch(m, ch, n);
                }
            }
            let c = &mut m.os.audio[ch];
            if c.playing == Some(ioreq) {
                c.playing = None;
                if c.latched.is_none() {
                    audio_dma(m, ch, false);
                }
            }
            m.bus.w8(ioreq + 31, IOERR_ABORTED);
            reply_io(m, ioreq);
        }
    }
}

fn audio_reg(ch: usize) -> u32 {
    0xDFF0A0 + 16 * ch as u32
}

fn audio_dma(m: &mut Machine, ch: usize, on: bool) {
    m.bus.write_word(0xDFF096, if on { 0x8000 } else { 0 } | 1 << ch);
}

/// Puts a write's sample in the channel's registers, for Paula's next reload.
fn audio_latch(m: &mut Machine, ch: usize, io: u32) {
    let (data, len) = (m.bus.r32(io + 34), m.bus.r32(io + 38));
    m.bus.write_long(audio_reg(ch), data);
    m.bus.write_word(audio_reg(ch) + 4, (len / 2) as u16); // Paula counts words
    m.os.audio[ch].latched = Some(io);
}

/// A write starts to play: its period and volume (with ADIOF_PERVOL; UADE's score always
/// sets them, but players such as IFF-8SVX send ADCMD_PERVOL instead), and its
/// ioa_WriteMsg when it asked for one.
fn audio_start(m: &mut Machine, ch: usize, io: u32) {
    let flags = m.bus.r8(io + 30);
    if flags & ADIOF_PERVOL != 0 {
        audio_pervol(m, ch, io);
    }
    if flags & ADIOF_WRITEMESSAGE != 0 {
        let port = m.bus.r32(io + 48 + 14);
        put_msg(m, port, io + 48, 7);
    }
}

fn audio_pervol(m: &mut Machine, ch: usize, io: u32) {
    let (per, vol) = (m.bus.r16(io + 42), m.bus.r16(io + 44));
    m.bus.write_word(audio_reg(ch) + 6, per);
    m.bus.write_word(audio_reg(ch) + 8, vol);
}

/// The playing write ends now (ADCMD_FINISH): it is aborted, the next one starts.
fn audio_finish(m: &mut Machine, ch: usize) {
    if let Some(p) = m.os.audio[ch].playing.take() {
        if m.os.audio[ch].latched == Some(p) {
            m.os.audio[ch].latched = None; // it had passes left
        }
        m.bus.w8(p + 31, IOERR_ABORTED);
        reply_io(m, p);
    }
    audio_dma(m, ch, false);
    if m.os.audio[ch].latched.is_none() {
        if let Some(n) = m.os.audio[ch].queue.pop_front() {
            audio_latch(m, ch, n);
        }
    }
    if m.os.audio[ch].latched.is_some() && !m.os.audio[ch].stopped {
        audio_dma(m, ch, true);
    }
}

/// audio.device commands on Paula. Returns whether the request is done (a CMD_WRITE is not
/// until it has played).
fn audio_command(m: &mut Machine, ioreq: u32, cmd: u16) -> bool {
    let unit = m.bus.r32(ioreq + 24);
    log::trace!(
        "audio.device ${ioreq:06x} cmd {cmd} unit {unit:x} key {} flags {:x} data ${:x} len {} per {} vol {} cycles {}",
        m.bus.r16(ioreq + 32),
        m.bus.r8(ioreq + 30),
        m.bus.r32(ioreq + 34),
        m.bus.r32(ioreq + 38),
        m.bus.r16(ioreq + 42),
        m.bus.r16(ioreq + 44),
        m.bus.r16(ioreq + 46)
    );
    if cmd == 32 {
        // ADCMD_ALLOCATE
        return audio_allocate(m, ioreq) || !m.os.audio_alloc.waiting.contains(&ioreq);
    }
    // The channels named that belong to the request's allocation key. A channel nobody
    // allocated is let through, for players that never allocate.
    let key = m.bus.r16(ioreq + 32);
    let mine = |c: &AudioChan| c.key == key || c.key == 0;
    let chans: Vec<usize> = (0..4).filter(|&c| unit & 1 << c != 0 && mine(&m.os.audio[c])).collect();
    if chans.len() != (0..4).filter(|c| unit & 1 << c != 0).count() {
        m.bus.w32(ioreq + 24, chans.iter().fold(0, |a, &c| a | 1 << c));
    }
    let Some(&ch) = chans.first() else {
        m.bus.w8(ioreq + 31, ADIOERR_NOALLOCATION);
        return true;
    };
    let sync = m.bus.r8(ioreq + 30) & ADIOF_SYNCCYCLE != 0;
    match cmd {
        3 => {
            // CMD_WRITE (one channel)
            let c = &m.os.audio[ch];
            if !c.busy() {
                audio_latch(m, ch, ioreq);
                if !m.os.audio[ch].stopped {
                    // starting DMA loads the registers and raises the interrupt
                    audio_dma(m, ch, false);
                    audio_dma(m, ch, true);
                }
            } else if c.latched.is_none() {
                audio_latch(m, ch, ioreq);
            } else {
                m.os.audio[ch].queue.push_back(ioreq);
            }
            return false;
        }
        12 if sync && m.os.audio[ch].playing.is_some() => {
            // ADCMD_PERVOL at the end of the pass
            m.os.audio[ch].at_cycle.push(ioreq);
            return false;
        }
        12 => {
            // ADCMD_PERVOL (CMD_NONSTD + 3)
            for &ch in &chans {
                audio_pervol(m, ch, ioreq);
            }
        }
        1 | 8 => {
            // CMD_RESET, CMD_FLUSH: every write ends, aborted; the allocation stays
            for &ch in &chans {
                let (key, pri, stopped, locks) = {
                    let c = &m.os.audio[ch];
                    (c.key, c.pri, c.stopped && cmd == 8, c.locks.clone())
                };
                let c = std::mem::take(&mut m.os.audio[ch]);
                audio_dma(m, ch, false);
                for io in c.requests() {
                    if io != ioreq {
                        m.bus.w8(io + 31, IOERR_ABORTED);
                        reply_io(m, io);
                    }
                }
                let c = &mut m.os.audio[ch];
                (c.key, c.pri, c.stopped, c.locks) = (key, pri, stopped, locks);
            }
        }
        6 => {
            // CMD_STOP
            for &ch in &chans {
                m.os.audio[ch].stopped = true;
                audio_dma(m, ch, false);
            }
        }
        7 => {
            // CMD_START
            for &ch in &chans {
                m.os.audio[ch].stopped = false;
                if m.os.audio[ch].busy() {
                    audio_dma(m, ch, true);
                }
            }
        }
        11 if sync && m.os.audio[ch].playing.is_some() => {
            // ADCMD_FINISH at the end of the pass
            m.os.audio[ch].at_cycle.push(ioreq);
            return false;
        }
        11 => {
            // ADCMD_FINISH: the playing write ends now, the next one starts
            for &ch in &chans {
                audio_finish(m, ch);
            }
        }
        14 if m.os.audio[ch].playing.is_some() => {
            // ADCMD_WAITCYCLE: answered when the pass ends
            m.os.audio[ch].at_cycle.push(ioreq);
            return false;
        }
        14 => {} // nothing playing: no cycle to wait for
        9 => {
            // ADCMD_FREE: the channels stop and are free again
            for &ch in &chans {
                if m.os.audio[ch].key == key {
                    audio_release(m, ch, IOERR_ABORTED);
                }
            }
            audio_retry_waiting(m);
        }
        10 => {
            // ADCMD_SETPREC
            let pri = m.bus.r8(ioreq + 9) as i8;
            for &ch in &chans {
                if m.os.audio[ch].key == key {
                    m.os.audio[ch].pri = pri;
                }
            }
            audio_retry_waiting(m);
        }
        13 => {
            // ADCMD_LOCK: answered when a channel is stolen or freed
            if m.os.audio[ch].key == key {
                m.os.audio[ch].locks.push(ioreq);
                return false;
            }
        }
        _ => {
            if m.os.logged.insert((usize::MAX, cmd as i32)) {
                log::warn!("audio.device command {cmd} ignored");
            }
        }
    }
    true
}

/// Paula reloaded a channel audio.device plays on: a pass of the latched write starts,
/// and the one before it ended. Called after each output sample; takes the interrupts of
/// those channels.
pub fn audio_service(m: &mut Machine) {
    for ch in 0..4 {
        let bit = 0x80u16 << ch;
        if m.bus.paula.intreq & bit == 0 || !m.os.audio[ch].busy() {
            continue;
        }
        m.bus.paula.write_intreq(bit);
        // the requests that waited for the end of a pass
        let mut finish = false;
        for io in std::mem::take(&mut m.os.audio[ch].at_cycle) {
            match m.bus.r16(io + 28) {
                12 => audio_pervol(m, ch, io),
                11 => finish = true,
                _ => {}
            }
            reply_io(m, io);
        }
        if finish {
            audio_finish(m, ch);
            continue;
        }
        match m.os.audio[ch].latched {
            Some(l) => {
                if m.os.audio[ch].playing != Some(l) {
                    if let Some(p) = m.os.audio[ch].playing.take() {
                        reply_io(m, p);
                    }
                    audio_start(m, ch, l);
                    let c = &mut m.os.audio[ch];
                    c.playing = Some(l);
                    c.passes = 0;
                }
                let cycles = m.bus.r16(l + 46) as u32;
                let c = &mut m.os.audio[ch];
                c.passes += 1;
                if cycles != 0 && c.passes >= cycles {
                    // its last pass: the next write goes in the registers
                    c.latched = None;
                    if let Some(n) = c.queue.pop_front() {
                        audio_latch(m, ch, n);
                    }
                }
            }
            None => {
                // the finished write's sample started over: nothing follows it
                audio_dma(m, ch, false);
                m.bus.paula.write_intreq(bit);
                if let Some(p) = m.os.audio[ch].playing.take() {
                    reply_io(m, p);
                }
            }
        }
    }
}

fn device_call(m: &mut Machine, _lib: usize, lvo: i32) -> bool {
    match lvo {
        30 => {
            // BeginIO(a1)
            let r = begin_io(m, m.a(1));
            m.set_d0(r as u32);
            true
        }
        36 => {
            // AbortIO(a1)
            abort_io(m, m.a(1));
            m.set_d0(0);
            true
        }
        _ => false,
    }
}

// ----------------------------------------------------------- resources ---

fn cia_call(m: &mut Machine, cia: usize, lvo: i32) -> bool {
    let (d0, a1) = (m.d(0), m.a(1));
    match lvo {
        6 => {
            // AddICRVector(d0 bit, a1 interrupt) -> d0 = 0 or current owner
            let bit = d0 as usize % 5;
            let cur = m.os.icr_vectors[cia][bit];
            if cur != 0 && cur != a1 {
                m.set_d0(cur);
            } else {
                m.os.icr_vectors[cia][bit] = a1;
                let c = if cia == 0 { &mut m.bus.ciaa } else { &mut m.bus.ciab };
                c.icr_mask |= 1 << bit;
                m.set_d0(0);
            }
        }
        12 => {
            let bit = d0 as usize % 5;
            if m.os.icr_vectors[cia][bit] == a1 {
                m.os.icr_vectors[cia][bit] = 0;
                let c = if cia == 0 { &mut m.bus.ciaa } else { &mut m.bus.ciab };
                c.icr_mask &= !(1 << bit);
            }
        }
        18 => {
            // AbleICR(d0 mask) -> old mask
            let c = if cia == 0 { &mut m.bus.ciaa } else { &mut m.bus.ciab };
            let old = c.icr_mask;
            let v = d0 as u8;
            if v & 0x80 != 0 {
                c.icr_mask |= v & 0x1F;
            } else {
                c.icr_mask &= !(v & 0x1F);
            }
            m.set_d0(old as u32);
        }
        24 => {
            // SetICR(d0 mask) -> old data
            let c = if cia == 0 { &mut m.bus.ciaa } else { &mut m.bus.ciab };
            let old = c.icr_data;
            let v = d0 as u8;
            if v & 0x80 != 0 {
                c.icr_data |= v & 0x1F;
            } else {
                c.icr_data &= !(v & 0x1F);
            }
            m.set_d0(old as u32);
        }
        _ => return false,
    }
    true
}

fn intuition_call(m: &mut Machine, lvo: i32) -> bool {
    match lvo {
        588 => {
            // EasyRequestArgs: answer the first gadget
            let es = m.a(1);
            if es != 0 {
                let t = m.bus.r32(es + 12);
                log::info!("EasyRequest: {}", m.bus.read_cstr(t));
            }
            m.set_d0(1);
            true
        }
        _ => false,
    }
}

// ------------------------------------------------------------- utility ---

fn utility_call(m: &mut Machine, lvo: i32) -> bool {
    let (d0, d1, a0) = (m.d(0), m.d(1), m.a(0));
    match lvo {
        30 => {
            let t = find_tag(m, a0, d0);
            m.set_d0(t);
        }
        36 => {
            let t = find_tag(m, a0, d0);
            m.set_d0(if t != 0 { m.bus.r32(t + 4) } else { d1 });
        }
        48 => {
            // NextTagItem(a0 = **tagitem)
            let mut p = m.bus.r32(a0);
            loop {
                if p == 0 {
                    m.set_d0(0);
                    break;
                }
                match m.bus.r32(p) {
                    0 => {
                        m.bus.w32(a0, 0);
                        m.set_d0(0);
                        break;
                    }
                    1 => p += 8,                // TAG_IGNORE
                    2 => p = m.bus.r32(p + 4),  // TAG_MORE
                    3 => p += 8 + m.bus.r32(p + 4) * 8, // TAG_SKIP
                    _ => {
                        m.bus.w32(a0, p + 8);
                        m.set_d0(p);
                        break;
                    }
                }
            }
        }
        138 | 144 => m.set_d0(d0.wrapping_mul(d1)),
        150 => {
            let (a, b) = (d0 as i32, d1 as i32);
            if b != 0 {
                m.set_d0(a.wrapping_div(b) as u32);
                m.cpu.dar[1] = a.wrapping_rem(b) as u32;
            }
        }
        156 => {
            if d1 != 0 {
                m.set_d0(d0 / d1);
                m.cpu.dar[1] = d0 % d1;
            }
        }
        198 => {
            let r = (d0 as i32 as i64).wrapping_mul(d1 as i32 as i64) as u64;
            m.set_d0(r as u32);
            m.cpu.dar[1] = (r >> 32) as u32;
        }
        204 => {
            let r = d0 as u64 * d1 as u64;
            m.set_d0(r as u32);
            m.cpu.dar[1] = (r >> 32) as u32;
        }
        162 | 168 => {
            let a = m.bus.read_cstr(a0).to_ascii_lowercase();
            let b = m.bus.read_cstr(m.a(1)).to_ascii_lowercase();
            let (a, b) = if lvo == 168 {
                let n = d0 as usize;
                (a.chars().take(n).collect::<String>(), b.chars().take(n).collect::<String>())
            } else {
                (a, b)
            };
            m.set_d0(match a.cmp(&b) {
                std::cmp::Ordering::Less => -1i32 as u32,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            });
        }
        174 => m.set_d0((d0 as u8).to_ascii_uppercase() as u32),
        180 => m.set_d0((d0 as u8).to_ascii_lowercase() as u32),
        _ => return false,
    }
    true
}

pub fn find_tag(m: &Machine, mut p: u32, tag: u32) -> u32 {
    for _ in 0..1000 {
        if p == 0 {
            return 0;
        }
        match m.bus.r32(p) {
            0 => return 0,
            1 => p += 8,
            2 => p = m.bus.r32(p + 4),
            3 => p += 8 + m.bus.r32(p + 4) * 8,
            t if t == tag => return p,
            _ => p += 8,
        }
    }
    0
}

// ----------------------------------------------------------------- dos ---

const DOS_NAMES: &[&str] = &[
    "Open", "Close", "Read", "Write", "Input", "Output", "Seek", "DeleteFile", "Rename", "Lock",
    "UnLock", "DupLock", "Examine", "ExNext", "Info", "CreateDir", "CurrentDir", "IoErr",
    "CreateProc", "Exit", "LoadSeg", "UnLoadSeg", "GetPacket", "QueuePacket", "DeviceProc",
    "SetComment", "SetProtection", "DateStamp", "Delay", "WaitForChar", "ParentDir",
    "IsInteractive", "Execute", "AllocDosObject", "FreeDosObject", "DoPkt", "SendPkt", "WaitPkt",
    "ReplyPkt", "AbortPkt", "LockRecord", "LockRecords", "UnLockRecord", "UnLockRecords",
    "SelectInput", "SelectOutput", "FGetC", "FPutC", "UnGetC", "FRead", "FWrite", "FGets",
    "FPuts", "VFWritef", "VFPrintf", "Flush", "SetVBuf", "DupLockFromFH", "OpenFromLock",
    "ParentOfFH", "ExamineFH", "SetFileDate", "NameFromLock", "NameFromFH", "SplitName",
    "SameLock", "SetMode", "ExAll", "ReadLink", "MakeLink", "ChangeMode", "SetFileSize",
    "SetIoErr",
];

const ERROR_OBJECT_NOT_FOUND: u32 = 205;
const CONSOLE_FH: u32 = 0x3FFF_FF00 >> 2;

impl OsState {
    /// The file at host path `p`: one of `mem_files` (the path in any case, as the
    /// Amiga's names are), else the disk's.
    pub fn read_file(&self, p: &Path) -> std::io::Result<Vec<u8>> {
        let want = p.to_string_lossy();
        if let Some((_, d)) = self.mem_files.iter().find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(&want)) {
            return Ok(d.clone());
        }
        if self.no_host_files {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        std::fs::read(p)
    }

    /// Whether `p` is one of `mem_files`, or a directory above one (names in any case).
    fn mem_exists(&self, p: &Path) -> (bool, bool) {
        let want = p.to_string_lossy().to_ascii_lowercase();
        let mut dir = false;
        for k in self.mem_files.keys() {
            let k = k.to_string_lossy().to_ascii_lowercase();
            if k == want {
                return (true, false);
            }
            dir |= k.strip_prefix(want.trim_end_matches('/')).is_some_and(|r| r.starts_with('/'));
        }
        (dir, dir)
    }

    /// The host path of Amiga name `name` (relative names in `current`). As
    /// the Amiga's file systems ignore case, a path that does not exist as
    /// named is the one that does in another case, if any.
    pub fn host_path(&self, name: &str, current: Option<&Path>) -> PathBuf {
        if self.no_host_files {
            // (`mem_files` are found in any case by `read_file`; a `..` cannot leave them)
            return self.host_path_as_named(name, current);
        }
        any_case_path(self.host_path_as_named(name, current))
    }

    fn host_path_as_named(&self, name: &str, current: Option<&Path>) -> PathBuf {
        if let Some(rest) = name.strip_prefix("PROGDIR:").or_else(|| name.strip_prefix("progdir:")) {
            return self.progdir.join(rest);
        }
        if name.starts_with('/') {
            return PathBuf::from(name);
        }
        if let Some((vol, rest)) = name.split_once(':') {
            if vol.eq_ignore_ascii_case("Instruments") {
                // as UADE maps it: the module's instruments folder (IFF-SMUS's
                // instruments, Scott Johnston's samples), its names in any case as
                // on the Amiga's file systems (songs name "BigBow" for BIGBOW.instr)
                if self.no_host_files {
                    return self.module_dir.join("Instruments").join(rest);
                }
                let mut d = any_case(&self.module_dir, "Instruments");
                for part in rest.split('/').filter(|p| !p.is_empty()) {
                    d = any_case(&d, part);
                }
                return d;
            }
            if (vol.eq_ignore_ascii_case("Env") || vol.eq_ignore_ascii_case("EnvArc")) && !self.env_dir.as_os_str().is_empty() && !self.no_host_files {
                let mut d = self.env_dir.clone();
                for part in rest.split('/').filter(|p| !p.is_empty()) {
                    d = any_case(&d, part);
                }
                return d;
            }
            // unknown Amiga volume: try the remainder relative to PROGDIR
            log::debug!("mapping volume {vol}: of {name} to PROGDIR");
            return self.progdir.join(rest);
        }
        match current {
            Some(c) => c.join(name),
            None => PathBuf::from(name),
        }
    }
}

/// `dir/name`, or the entry of `dir` that is `name` in another case.
fn any_case(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if p.exists() {
        return p;
    }
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
        .unwrap_or(p)
}

/// `p` if it exists, else `p` with each component found in its directory in
/// another case (an exact match first), if all of them are; else `p`.
fn any_case_path(p: PathBuf) -> PathBuf {
    if p.exists() {
        return p;
    }
    let mut q = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::Normal(n) => {
                let dir = if q.as_os_str().is_empty() { Path::new(".") } else { &q };
                let found = any_case(dir, &n.to_string_lossy());
                q = if q.as_os_str().is_empty() { PathBuf::from(found.file_name().unwrap_or(n)) } else { found };
            }
            c => q.push(c),
        }
    }
    if q.exists() { q } else { p }
}

fn bptr_alloc(m: &mut Machine, size: u32) -> u32 {
    // BPTRs need longword-aligned memory; our heap is 8-byte aligned
    m.alloc(size) >> 2
}

fn dos_call(m: &mut Machine, lvo: i32) -> bool {
    let (d1, d2, d3) = (m.d(1), m.d(2), m.d(3));
    let cur = m.os.locks.get(&m.os.current_dir).cloned();
    match lvo {
        948 => {
            // PutStr(d1)
            let s = m.bus.read_cstr(d1);
            m.os.console.push_str(&s);
            m.set_d0(0);
            return true;
        }
        954 => {
            let s = m.bus.read_cstr(d1);
            m.os.console.push_str(&s);
            m.set_d0(s.len() as u32);
            return true;
        }
        870 => {
            // FilePart(d1)
            let s = m.bus.read_cstr(d1);
            let off = s.rfind(['/', ':']).map(|i| i + 1).unwrap_or(0);
            m.set_d0(d1 + off as u32);
            return true;
        }
        876 => {
            let s = m.bus.read_cstr(d1);
            let off = s.rfind('/').or_else(|| s.rfind(':').map(|i| i + 1)).unwrap_or(0);
            m.set_d0(d1 + off as u32);
            return true;
        }
        882 => {
            // AddPart(d1 dir, d2 file, d3 size)
            let mut dir = m.bus.read_cstr(d1);
            let f = m.bus.read_cstr(d2);
            if !dir.is_empty() && !dir.ends_with('/') && !dir.ends_with(':') {
                dir.push('/');
            }
            dir.push_str(&f);
            if dir.len() < d3 as usize {
                m.bus.write_cstr(d1, &dir);
                m.set_d0(1);
            } else {
                m.set_d0(0);
            }
            return true;
        }
        906 => {
            // GetVar -> not found
            m.set_d0(-1i32 as u32);
            m.os.io_err = ERROR_OBJECT_NOT_FOUND;
            return true;
        }
        _ => {}
    }
    let Some(name) = lvo_name(DOS_NAMES, lvo) else { return false };
    match name {
        "Open" => {
            let n = m.bus.read_cstr(d1);
            if n.starts_with("CON:") || n.starts_with("RAW:") || n == "*" || n.starts_with("NIL:") {
                m.set_d0(CONSOLE_FH);
                return true;
            }
            let p = m.os.host_path(&n, cur.as_deref());
            let f = if m.os.no_host_files {
                match d2 {
                    1006 => Err(std::io::ErrorKind::PermissionDenied.into()),
                    _ => m.os.read_file(&p).map(|d| Handle::Mem(std::io::Cursor::new(d))),
                }
            } else {
                match d2 {
                    1006 => File::create(&p), // MODE_NEWFILE
                    _ => File::open(&p),
                }
                .map(Handle::Disk)
            };
            match f {
                Ok(file) => {
                    let fh = bptr_alloc(m, 44);
                    log::debug!("Open(\"{n}\") -> {}", p.display());
                    m.os.files.insert(fh, OpenFile { file, path: p });
                    m.set_d0(fh);
                }
                Err(_) => {
                    log::debug!("Open(\"{n}\") failed ({})", p.display());
                    m.os.io_err = ERROR_OBJECT_NOT_FOUND;
                    m.set_d0(0);
                }
            }
        }
        "Close" => {
            if m.os.files.remove(&d1).is_some() {
                m.heap.free(d1 << 2);
            }
            m.set_d0(1);
        }
        "Read" | "FRead" => {
            let (buf, len) = if name == "FRead" { (d2, d3.wrapping_mul(m.d(4))) } else { (d2, d3) };
            // (no more than the machine's memory holds, whatever the guest asks for)
            let len = len.min(crate::bus::RAM_SIZE as u32);
            let r = match m.os.files.get_mut(&d1) {
                Some(f) => {
                    let mut v = vec![0u8; len as usize];
                    match f.file.read(&mut v) {
                        Ok(mut n) => {
                            // read() may return short counts; fill as much as possible
                            while n < v.len() {
                                match f.file.read(&mut v[n..]) {
                                    Ok(0) | Err(_) => break,
                                    Ok(k) => n += k,
                                }
                            }
                            m.bus.write_bytes(buf, &v[..n]);
                            n as u32
                        }
                        Err(_) => -1i32 as u32,
                    }
                }
                None => -1i32 as u32,
            };
            let r = if name == "FRead" && r != u32::MAX && m.d(3) != 0 { r / m.d(3) } else { r };
            m.set_d0(r);
        }
        "Write" | "FWrite" => {
            let len = if name == "FWrite" { d3.wrapping_mul(m.d(4)) } else { d3 };
            let len = len.min(crate::bus::RAM_SIZE as u32);
            let data: Vec<u8> = (0..len).map(|i| m.bus.r8(d2 + i)).collect();
            if d1 == CONSOLE_FH {
                m.os.console.push_str(&String::from_utf8_lossy(&data));
                m.set_d0(len);
            } else if let Some(f) = m.os.files.get_mut(&d1) {
                let ok = f.file.write_all(&data).is_ok();
                m.set_d0(if ok { len } else { -1i32 as u32 });
            } else {
                m.set_d0(-1i32 as u32);
            }
        }
        "FPuts" => {
            let s = m.bus.read_cstr(d2);
            m.os.console.push_str(&s);
            m.set_d0(0);
        }
        "FPutC" => {
            m.os.console.push(d2 as u8 as char);
            m.set_d0(d2);
        }
        "VFPrintf" | "VFWritef" => {
            let s = m.bus.read_cstr(d2);
            m.os.console.push_str(&s);
            m.set_d0(0);
        }
        "Input" | "Output" | "SelectInput" | "SelectOutput" => m.set_d0(CONSOLE_FH),
        "Flush" | "SetVBuf" | "IsInteractive" | "Delay" | "DateStamp" | "SetIoErr" => m.set_d0(0),
        "Seek" => {
            let r = match m.os.files.get_mut(&d1) {
                Some(f) => {
                    let old = f.file.stream_position().unwrap_or(0);
                    let to = match d3 as i32 {
                        -1 => SeekFrom::Start(d2 as u64),
                        0 => SeekFrom::Current(d2 as i32 as i64),
                        _ => SeekFrom::End(d2 as i32 as i64),
                    };
                    if f.file.seek(to).is_ok() { old as u32 } else { -1i32 as u32 }
                }
                None => -1i32 as u32,
            };
            m.set_d0(r);
        }
        "Lock" => {
            let n = m.bus.read_cstr(d1);
            let p = m.os.host_path(&n, cur.as_deref());
            let exists = if m.os.no_host_files { m.os.mem_exists(&p).0 } else { p.exists() };
            if exists {
                let l = bptr_alloc(m, 20);
                m.os.locks.insert(l, p);
                m.set_d0(l);
            } else {
                m.os.io_err = ERROR_OBJECT_NOT_FOUND;
                m.set_d0(0);
            }
        }
        "UnLock" => {
            if m.os.locks.remove(&d1).is_some() {
                m.heap.free(d1 << 2);
            }
        }
        "DupLock" | "ParentDir" | "DupLockFromFH" | "ParentOfFH" => {
            let src = if name.ends_with("FH") {
                m.os.files.get(&d1).map(|f| f.path.clone())
            } else {
                m.os.locks.get(&d1).cloned()
            };
            match src {
                Some(p) => {
                    let p = if name.starts_with("Parent") {
                        p.parent().map(|x| x.to_path_buf()).unwrap_or(p)
                    } else {
                        p
                    };
                    let l = bptr_alloc(m, 20);
                    m.os.locks.insert(l, p);
                    m.set_d0(l);
                }
                None => m.set_d0(0),
            }
        }
        "CurrentDir" => {
            let old = m.os.current_dir;
            m.os.current_dir = d1;
            m.set_d0(old);
        }
        "Examine" | "ExamineFH" => {
            let p = if name == "ExamineFH" {
                m.os.files.get(&d1).map(|f| f.path.clone())
            } else {
                m.os.locks.get(&d1).cloned()
            };
            match p {
                Some(p) => {
                    fill_fib(m, d2, &p);
                    m.set_d0(1);
                }
                None => m.set_d0(0),
            }
        }
        "ExNext" => {
            m.os.io_err = 232; // ERROR_NO_MORE_ENTRIES
            m.set_d0(0);
        }
        "NameFromLock" | "NameFromFH" => {
            let p = if name == "NameFromFH" {
                m.os.files.get(&d1).map(|f| f.path.clone())
            } else {
                m.os.locks.get(&d1).cloned()
            };
            match p {
                Some(p) => {
                    let s = p.to_string_lossy().to_string();
                    if s.len() < d3 as usize {
                        m.bus.write_cstr(d2, &s);
                        m.set_d0(1);
                    } else {
                        m.set_d0(0);
                    }
                }
                None => m.set_d0(0),
            }
        }
        "IoErr" => m.set_d0(m.os.io_err),
        "LoadSeg" => {
            let n = m.bus.read_cstr(d1);
            let p = m.os.host_path(&n, cur.as_deref());
            let d = if m.os.no_host_files { m.os.read_file(&p).ok() } else { std::fs::read(&p).ok() };
            let r = d.and_then(|d| crate::deli::load_seg(m, &d).ok());
            m.set_d0(r.map(|(seg, _)| seg).unwrap_or(0));
        }
        "UnLoadSeg" => m.set_d0(1),
        "DeleteFile" | "Rename" | "CreateDir" | "SetComment" | "SetProtection" | "Info" | "Execute"
        | "CreateProc" | "DeviceProc" => m.set_d0(0),
        "AllocDosObject" => {
            let p = m.alloc(if d1 == 2 { 260 } else { 256 });
            m.set_d0(p);
        }
        "FreeDosObject" => {
            m.heap.free(d2);
        }
        _ => return false,
    }
    true
}

fn fill_fib(m: &mut Machine, fib: u32, p: &Path) {
    let (is_dir, size) = if m.os.no_host_files {
        (m.os.mem_exists(p).1, m.os.read_file(p).map_or(0, |d| d.len() as u32))
    } else {
        let md = std::fs::metadata(p).ok();
        (md.as_ref().map(|x| x.is_dir()).unwrap_or(false), md.as_ref().map(|x| x.len()).unwrap_or(0) as u32)
    };
    let fname = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    m.bus.w32(fib + 4, if is_dir { 2 } else { -3i32 as u32 });
    let mut fnb = fname.into_bytes();
    fnb.truncate(106);
    m.bus.write_bytes(fib + 8, &fnb);
    m.bus.w8(fib + 8 + fnb.len() as u32, 0);
    m.bus.w32(fib + 120, if is_dir { 2 } else { -3i32 as u32 });
    m.bus.w32(fib + 124, size);
    m.bus.w32(fib + 128, size.div_ceil(512));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Advances the time by `clocks` colour clocks and runs Paula there.
    fn run_paula(m: &mut Machine, clocks: u64) {
        m.bus.clock += clocks;
        m.bus.paula.sync(m.bus.clock, &m.bus.ram);
    }
    use m68k::CpuType;

    #[test]
    fn host_path_ignores_case() {
        let dir = std::env::temp_dir().join(format!("amigaplayer-os-test-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        for f in ["SMP.Song", "smp.both", "SMP.BOTH", "Sub/Samples"] {
            std::fs::write(dir.join(f), f).unwrap();
        }
        let os = OsState::default();
        let at = |n: &str| os.host_path(n, Some(&dir));
        // Richard Joseph's rjp.song asks for SMP.song
        assert_eq!(at("SMP.song"), dir.join("SMP.Song"));
        assert_eq!(at("smp.SONG"), dir.join("SMP.Song"));
        // an exact match first
        assert_eq!(at("smp.both"), dir.join("smp.both"));
        assert_eq!(at("SMP.BOTH"), dir.join("SMP.BOTH"));
        // directories too, and a name found in no case stays as named
        assert_eq!(at("sub/SAMPLES"), dir.join("Sub/Samples"));
        assert_eq!(at("smp.none"), dir.join("smp.none"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hunk executable of one code hunk holding `code`, its longs at `relocs` relocated
    /// to the hunk itself.
    fn hunk_file(code: &[u8], relocs: &[u32]) -> Vec<u8> {
        let mut c = code.to_vec();
        c.resize(code.len().div_ceil(4) * 4, 0);
        let n = (c.len() / 4) as u32;
        let mut v = Vec::new();
        let mut l = |x: u32| v.extend_from_slice(&x.to_be_bytes());
        for x in [0x3F3, 0, 1, 0, 0, n, 0x3E9, n] {
            l(x);
        }
        v.extend_from_slice(&c);
        let mut l = |x: u32| v.extend_from_slice(&x.to_be_bytes());
        l(0x3EC);
        l(relocs.len() as u32);
        l(0);
        for &r in relocs {
            l(r);
        }
        l(0);
        l(0x3F2);
        v
    }

    /// A small RTF_AUTOINIT library of our own 68k code, "test.library" version 3: vectors
    /// Open (open count + 1, answers the base), Close (count - 1), Expunge, ExtFunc, then
    /// -30 (d0 * 2 + 1) and -36 (exec OpenLibrary("test.library", 0) from inside it). Its
    /// InitStruct table sets the node's type, name, version and flags; its init routine keeps
    /// the segment list at 34 of the base.
    fn test_library() -> Vec<u8> {
        let mut c: Vec<u8> = Vec::new();
        let mut relocs = Vec::new();
        let w = |c: &mut Vec<u8>, x: u16| c.extend_from_slice(&x.to_be_bytes());
        let l = |c: &mut Vec<u8>, x: u32| c.extend_from_slice(&x.to_be_bytes());
        w(&mut c, 0x70FF); // moveq #-1,d0
        w(&mut c, 0x4E75); // rts
        let rt = c.len() as u32;
        w(&mut c, 0x4AFC);
        relocs.push(c.len() as u32);
        l(&mut c, rt); // rt_MatchTag
        let endskip = c.len();
        relocs.push(c.len() as u32);
        l(&mut c, 0); // rt_EndSkip (patched)
        c.extend_from_slice(&[0x80, 3, 9, 0]); // RTF_AUTOINIT, version 3, NT_LIBRARY, pri 0
        let name_at = [c.len(), c.len() + 4, c.len() + 8];
        for _ in 0..3 {
            relocs.push(c.len() as u32);
            l(&mut c, 0); // rt_Name, rt_IdString, rt_Init (patched)
        }
        let name = c.len() as u32;
        c.extend_from_slice(b"test.library\0");
        if c.len() % 2 == 1 {
            c.push(0);
        }
        let init_tab = c.len() as u32;
        l(&mut c, 40); // dataSize
        let tab_ptrs = [c.len(), c.len() + 4, c.len() + 8];
        for _ in 0..3 {
            relocs.push(c.len() as u32);
            l(&mut c, 0); // vectors, structure, init (patched)
        }
        let vectors = c.len() as u32;
        w(&mut c, 0xFFFF);
        let offs_at = c.len();
        for _ in 0..6 {
            w(&mut c, 0); // patched
        }
        w(&mut c, 0xFFFF);
        let structure = c.len() as u32;
        c.extend_from_slice(&[0xA0, 8, 9, 0]); // INITBYTE LN_TYPE, NT_LIBRARY
        c.extend_from_slice(&[0x80, 10]); // INITLONG LN_NAME, name
        relocs.push(c.len() as u32);
        l(&mut c, name);
        c.extend_from_slice(&[0x90, 20, 0, 3]); // INITWORD LIB_VERSION, 3
        c.extend_from_slice(&[0xA0, 14, 6, 0]); // INITBYTE LIB_FLAGS, 6
        c.extend_from_slice(&[0, 0]);
        let init = c.len() as u32;
        for x in [0x2240, 0x2348, 0x0022, 0x4E75] {
            w(&mut c, x); // movea.l d0,a1; move.l a0,34(a1); rts
        }
        let open = c.len() as u32;
        for x in [0x526E, 0x0020, 0x200E, 0x4E75] {
            w(&mut c, x); // addq.w #1,32(a6); move.l a6,d0; rts
        }
        let close = c.len() as u32;
        for x in [0x536E, 0x0020, 0x7000, 0x4E75] {
            w(&mut c, x); // subq.w #1,32(a6); moveq #0,d0; rts
        }
        let nothing = c.len() as u32;
        for x in [0x7000, 0x4E75] {
            w(&mut c, x); // moveq #0,d0; rts
        }
        let f30 = c.len() as u32;
        for x in [0xD080, 0x5280, 0x4E75] {
            w(&mut c, x); // add.l d0,d0; addq.l #1,d0; rts
        }
        let f36 = c.len() as u32;
        let disp = (name as i32 - (f36 as i32 + 2)) as u16;
        // lea name(pc),a1; moveq #0,d0; movea.l 4.w,a6; jsr -552(a6); rts
        for x in [0x43FA, disp, 0x7000, 0x2C78, 0x0004, 0x4EAE, 0xFDD8, 0x4E75] {
            w(&mut c, x);
        }
        let end = c.len() as u32;
        let put = |c: &mut Vec<u8>, at: usize, v: u32| c[at..at + 4].copy_from_slice(&v.to_be_bytes());
        put(&mut c, endskip, end);
        put(&mut c, name_at[0], name);
        put(&mut c, name_at[1], name);
        put(&mut c, name_at[2], init_tab);
        put(&mut c, tab_ptrs[0], vectors);
        put(&mut c, tab_ptrs[1], structure);
        put(&mut c, tab_ptrs[2], init);
        for (i, f) in [open, close, nothing, nothing, f30, f36].into_iter().enumerate() {
            let o = (f - vectors) as u16;
            c[offs_at + i * 2..offs_at + i * 2 + 2].copy_from_slice(&o.to_be_bytes());
        }
        hunk_file(&c, &relocs)
    }

    #[test]
    fn disk_library_is_loaded_initialised_and_called() {
        let mut m = Machine::new(CpuType::M68000);
        m.os.no_host_files = true;
        m.os.module_dir = PathBuf::from("/songs/modules");
        m.os.mem_files.insert(PathBuf::from("/songs/libs/test.library"), test_library());
        let base = open_library_version(&mut m, "test.library", 2);
        assert_ne!(base, 0);
        // InitStruct and MakeLibrary filled the node; the init routine kept the segment list
        assert_eq!(m.bus.r8(base + 8), NT_LIBRARY);
        assert_eq!(m.bus.r8(base + 14), 6);
        let st = (0..40).map(|i| format!("{:02x}", m.bus.r8(base + i))).collect::<Vec<_>>().join(" "); assert_eq!(m.bus.r8(base + 14), 6, "{st}");
        assert_eq!(m.bus.r16(base + 16), 36); // 6 vectors of 6 bytes
        assert_eq!(m.bus.r16(base + 18), 40);
        assert_eq!(m.bus.r16(base + 20), 3);
        assert_ne!(m.bus.r32(base + 34), 0);
        // Open ran once
        assert_eq!(m.bus.r16(base + 32), 1);
        // a function of its own
        let mut r = [0u32; 16];
        r[0] = 20;
        r[14] = base;
        assert_eq!(m.call(base - 30, &r).unwrap()[0], 41);
        // the library opening a library (itself) goes through exec, and Open runs again
        assert_eq!(m.call(base - 36, &r).unwrap()[0], base);
        assert_eq!(m.bus.r16(base + 32), 2);
        // a later OpenLibrary by another path finds it loaded; one asking a newer version fails
        assert_eq!(open_library(&mut m, "LIBS:Test.Library"), base);
        assert_eq!(m.bus.r16(base + 32), 3);
        assert_eq!(open_library_version(&mut m, "test.library", 4), 0);
        // CloseLibrary calls its Close
        m.cpu.dar[9] = base;
        exec_call(&mut m, -(lvo_of("CloseLibrary")));
        assert_eq!(m.bus.r16(base + 32), 2);
    }

    /// The LVO of exec function `name` (negated).
    fn lvo_of(name: &str) -> i32 {
        -(EXEC_NAMES.iter().position(|&n| n == name).unwrap() as i32 * 6 + 30)
    }

    #[test]
    fn disk_library_search_path() {
        let lib = test_library();
        let dirs = |files: &[&str], want: &str| {
            let mut m = Machine::new(CpuType::M68000);
            crate::os::init(&mut m);
            m.os.no_host_files = true;
            m.os.module_dir = PathBuf::from("/a/modules");
            m.os.libs_dir = Some(PathBuf::from("/sys/libs"));
            for f in files {
                m.os.mem_files.insert(PathBuf::from(f), lib.clone());
            }
            let base = open_library(&mut m, want);
            let kind = m.os.libs.iter().find(|l| l.base == base).map(|l| l.kind);
            (base != 0, kind)
        };
        // the module's directory, its libs/, ../libs/ (with a path under it), LIBS:
        for f in ["/a/modules/test.library", "/a/modules/libs/test.library", "/a/libs/test.library", "/sys/libs/test.library"] {
            assert_eq!(dirs(&[f], "test.library"), (true, Some(LibKind::Disk)), "{f}");
        }
        assert_eq!(dirs(&["/a/libs/replayers/test.library"], "replayers/test.library"), (true, Some(LibKind::Disk)));
        // nothing found: the stub, as before
        assert_eq!(dirs(&[], "test.library"), (true, Some(LibKind::Stub)));
        // an emulated or system library is never loaded from disk, nor a decruncher
        let mut m = Machine::new(CpuType::M68000);
        m.os.no_host_files = true;
        m.os.module_dir = PathBuf::from("/a/modules");
        for n in ["dos.library", "reqtools.library", "xpkmaster.library"] {
            m.os.mem_files.insert(PathBuf::from("/a/modules").join(n), lib.clone());
        }
        let dos = open_library(&mut m, "dos.library");
        assert_eq!(m.os.libs.iter().find(|l| l.base == dos).unwrap().kind, LibKind::Dos);
        let req = open_library(&mut m, "reqtools.library");
        assert_eq!(m.os.libs.iter().find(|l| l.base == req).unwrap().kind, LibKind::Stub);
        assert_eq!(open_library(&mut m, "xpkmaster.library"), 0);
    }

    #[test]
    fn malformed_disk_library_falls_back_to_the_stub() {
        let good = test_library();
        // no RomTag (the match word broken), truncated, not an executable, a RomTag whose init
        // table points nowhere sensible
        let mut no_rt = good.clone();
        let p = no_rt.windows(2).position(|w| w == [0x4A, 0xFC]).unwrap();
        no_rt[p] = 0;
        let mut bad_tab = good.clone();
        // dataSize huge and the vector table endless: bounded, no panic
        let t = bad_tab.windows(4).position(|w| w == 40u32.to_be_bytes()).unwrap();
        bad_tab[t..t + 4].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        for (i, data) in [no_rt, good[..good.len() / 2].to_vec(), b"not a library".to_vec(), bad_tab].into_iter().enumerate() {
            let mut m = Machine::new(CpuType::M68000);
            crate::os::init(&mut m);
            m.os.no_host_files = true;
            m.os.module_dir = PathBuf::from("/m");
            m.os.mem_files.insert(PathBuf::from("/m/test.library"), data);
            let base = open_library(&mut m, "test.library");
            assert_ne!(base, 0, "case {i}");
        }
    }

    #[test]
    fn init_struct_commands() {
        let mut m = Machine::new(CpuType::M68000);
        let t = m.alloc(64);
        let mem = m.alloc(0x100);
        m.bus.w8(mem + 0x50, 0xEE); // cleared by the size given
        let table: &[u8] = &[
            0x21, 0x11, 0x22, // 2 bytes copied at 0
            0x00, // (even)
            0x51, 0x12, 0x34, // repeat word $1234 twice (at 2, 4)
            0x80, 0x10, 0xDE, 0xAD, 0xBE, 0xEF, // long at $10 (data even)
            0xE0, 0x00, 0x00, 0x40, 0x77, 0x00, // byte at 24-bit offset $40
            0x00,
        ];
        // (the repeat command at an odd address: its word aligned to the next even one)
        let table: Vec<u8> = {
            let mut v = table[..4].to_vec();
            v.extend_from_slice(&[0x51, 0x00, 0x12, 0x34]);
            v.extend_from_slice(&table[7..]);
            v
        };
        m.bus.write_bytes(t, &table);
        init_struct(&mut m, t, mem, 0x60);
        assert_eq!((m.bus.r8(mem), m.bus.r8(mem + 1)), (0x11, 0x22));
        assert_eq!((m.bus.r16(mem + 2), m.bus.r16(mem + 4)), (0x1234, 0x1234));
        assert_eq!(m.bus.r32(mem + 0x10), 0xDEAD_BEEF);
        assert_eq!(m.bus.r8(mem + 0x40), 0x77);
        assert_eq!(m.bus.r8(mem + 0x50), 0);
    }

    #[test]
    fn stonepacker_handle_is_not_zero() {
        let mut m = Machine::new(CpuType::M68000);
        let base = open_library(&mut m, "stonepacker.library");
        assert_ne!(base, 0);
        let lib = m.os.libs.iter().position(|l| l.base == base).unwrap();
        m.set_d0(0);
        lib_call(&mut m, lib, 30);
        let h = m.d(0);
        assert!(h != 0 && h != base);
        // another stub call of it still answers 0
        lib_call(&mut m, lib, 42);
        assert_eq!(m.d(0), 0);
        // and another library's -30 too
        let other = open_library(&mut m, "foo.library");
        let lib = m.os.libs.iter().position(|l| l.base == other).unwrap();
        lib_call(&mut m, lib, 30);
        assert_eq!(m.d(0), 0);
    }

    #[test]
    fn audio_device_write_reaches_paula() {
        let mut m = Machine::new(CpuType::M68000);
        let io = m.alloc(68);
        assert_eq!(open_device(&mut m, "audio.device", 0, io), 0);
        let port = m.alloc(34);
        let l = port + 20;
        m.bus.w32(l, l + 4);
        m.bus.w32(l + 8, l);
        m.bus.w32(io + 14, port); // mn_ReplyPort
        m.bus.w32(io + 24, 4); // io_Unit: channel 2
        m.bus.w16(io + 28, 3); // CMD_WRITE
        m.bus.w32(io + 34, 0x1000);
        m.bus.w32(io + 38, 200);
        m.bus.w16(io + 42, 428);
        m.bus.w16(io + 44, 64);
        m.bus.w8(io + 30, 0x10); // ADIOF_PERVOL
        m.bus.w16(io + 46, 2); // ioa_Cycles
        let code = m.alloc(16);
        // movea.l 20(a1),a6 ; jsr -30(a6) ; rts      (BeginIO through the device base)
        m.bus.write_bytes(code, &[0x2C, 0x69, 0x00, 0x14, 0x4E, 0xAE, 0xFF, 0xE2, 0x4E, 0x75]);
        let mut r = [0u32; 16];
        r[9] = io;
        m.call(code, &r).unwrap();
        assert_ne!(m.bus.paula.dmacon & 4, 0);
        // DMA started: the interrupt it raises at the next line starts the write,
        // with its period and volume
        run_paula(&mut m, crate::paula::LINE);
        audio_service(&mut m);
        let ch = &m.bus.paula.ch[2];
        assert_eq!((ch.lc, ch.len, ch.per, ch.vol), (0x1000, 100, 428, 64));
        // the request comes back once Paula has played it twice (100 words each)
        assert_eq!(get_msg(&mut m, port), 0);
        let mut words = 0;
        let done = loop {
            run_paula(&mut m, 428);
            audio_service(&mut m);
            words += 1;
            let n = get_msg(&mut m, port);
            if n != 0 || words > 1000 {
                break n;
            }
        };
        assert_eq!(done, io);
        assert!((399..=401).contains(&words), "replied after {words} bytes");
        assert_eq!(m.bus.r8(io + 8), 7); // NT_REPLYMSG
        assert_eq!(m.bus.paula.dmacon & 4, 0, "nothing follows: DMA off");
    }

    #[test]
    fn reply_msg_queues_in_order() {
        let mut m = Machine::new(CpuType::M68000);
        let port = m.alloc(34);
        let (a, b) = (m.alloc(20), m.alloc(20));
        // a port whose list was never initialised stays empty, as before
        put_msg(&mut m, port, a, 7);
        assert_eq!(get_msg(&mut m, port), 0);
        let l = port + 20;
        m.bus.w32(l, l + 4);
        m.bus.w32(l + 8, l);
        put_msg(&mut m, port, a, 7);
        put_msg(&mut m, port, b, 7);
        assert_eq!(get_msg(&mut m, port), a);
        assert_eq!(get_msg(&mut m, port), b);
        assert_eq!(get_msg(&mut m, port), 0);
    }

    fn port(m: &mut Machine) -> u32 {
        let p = m.alloc(34);
        let l = p + 20;
        m.bus.w32(l, l + 4);
        m.bus.w32(l + 8, l);
        p
    }

    /// An IOAudio for audio.device replying to `port`.
    fn ioa(m: &mut Machine, port: u32, cmd: u16, unit: u32, key: u16, pri: i8, flags: u8) -> u32 {
        let io = m.alloc(68);
        open_device(m, "audio.device", 0, io);
        m.bus.w32(io + 14, port);
        m.bus.w8(io + 9, pri as u8);
        m.bus.w32(io + 24, unit);
        m.bus.w16(io + 28, cmd);
        m.bus.w8(io + 30, flags);
        m.bus.w16(io + 32, key);
        io
    }

    fn allocate(m: &mut Machine, port: u32, mask: u8, pri: i8) -> (u32, u16) {
        let masks = m.alloc(2);
        m.bus.w8(masks, mask);
        let io = ioa(m, port, 32, 0, 0, pri, ADIOF_NOWAIT);
        m.bus.w32(io + 34, masks);
        m.bus.w32(io + 38, 1);
        begin_io(m, io);
        (io, m.bus.r16(io + 32))
    }

    fn write(m: &mut Machine, port: u32, unit: u32, key: u16) -> u32 {
        let io = ioa(m, port, 3, unit, key, 0, 0);
        m.bus.w32(io + 34, 0x1000);
        m.bus.w32(io + 38, 20);
        m.bus.w16(io + 42, 200);
        m.bus.w16(io + 46, 0); // forever
        m.bus.w8(io + 30, ADIOF_PERVOL);
        begin_io(m, io);
        io
    }

    fn drain(m: &mut Machine, port: u32) -> Vec<u32> {
        std::iter::from_fn(|| Some(get_msg(m, port)).filter(|&n| n != 0)).collect()
    }

    #[test]
    fn higher_precedence_steals_channels() {
        let mut m = Machine::new(CpuType::M68000);
        let (pa, pb) = (port(&mut m), port(&mut m));
        let (a, ka) = allocate(&mut m, pa, 1, 0);
        assert_eq!((m.bus.r8(a + 31), m.bus.r32(a + 24)), (0, 1));
        let w = write(&mut m, pa, 1, ka);
        let lock = ioa(&mut m, pa, 13, 1, ka, 0, 0);
        begin_io(&mut m, lock);
        drain(&mut m, pa);
        // equal precedence cannot take it
        let (b, _) = allocate(&mut m, pb, 1, 0);
        assert_eq!(m.bus.r8(b + 31), ADIOERR_ALLOCFAILED);
        // higher precedence does: the write and the lock come back as stolen
        let (c, kc) = allocate(&mut m, pb, 1, 10);
        assert_eq!(m.bus.r8(c + 31), 0);
        assert_ne!(kc, ka);
        assert_eq!(drain(&mut m, pa), vec![w, lock]);
        assert_eq!((m.bus.r8(w + 31), m.bus.r8(lock + 31)), (ADIOERR_CHANNELSTOLEN, ADIOERR_CHANNELSTOLEN));
        // the old key may no longer write there
        let w2 = write(&mut m, pa, 1, ka);
        assert_eq!(m.bus.r8(w2 + 31), ADIOERR_NOALLOCATION);
    }

    #[test]
    fn waitcycle_answers_at_the_end_of_a_pass_and_free_stops() {
        let mut m = Machine::new(CpuType::M68000);
        let p = port(&mut m);
        let (_, key) = allocate(&mut m, p, 2, 0);
        let w = write(&mut m, p, 2, key);
        drain(&mut m, p);
        // DMA start, with its interrupt at the next line: the write plays
        run_paula(&mut m, crate::paula::LINE);
        audio_service(&mut m);
        let wc = ioa(&mut m, p, 14, 2, key, 0, 0);
        begin_io(&mut m, wc);
        assert!(drain(&mut m, p).is_empty());
        for _ in 0..25 {
            run_paula(&mut m, 200);
            audio_service(&mut m);
        }
        // one pass is 20 bytes
        assert_eq!(drain(&mut m, p), vec![wc]);
        let free = ioa(&mut m, p, 9, 2, key, 0, 0);
        begin_io(&mut m, free);
        assert_eq!(drain(&mut m, p), vec![w, free]);
        assert_eq!(m.bus.r8(w + 31), IOERR_ABORTED);
        assert_eq!(m.bus.paula.dmacon & 2, 0);
        assert_eq!(m.os.audio[1].key, 0);
    }

}

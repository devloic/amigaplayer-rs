//! High-level emulation of the parts of AmigaOS that players use: exec
//! (memory, lists, interrupts, libraries, devices), dos (files mapped to the
//! host filesystem), utility (32-bit math, tags), cia resources and an
//! audio.device that plays CMD_WRITE on Paula. Everything else is a stub
//! library whose calls return 0 and are logged once.

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
}

pub struct Library {
    pub name: String,
    pub base: u32,
    pub kind: LibKind,
}

struct OpenFile {
    file: File,
    path: PathBuf,
}

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
    /// The current module's directory, where the `Instruments:` volume is.
    pub module_dir: PathBuf,
    /// Files held in memory, by host path, read before the disk: a host with no
    /// file system (the browser) hands a module's companions over this way.
    pub mem_files: HashMap<PathBuf, Vec<u8>>,
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

fn open_library(m: &mut Machine, name: &str) -> u32 {
    if let Some(b) = find_lib(m, name) {
        return b;
    }
    let lname = name.to_ascii_lowercase();
    // Decrunchers are refused so players fall back to plain data.
    if lname.contains("xpk") || lname.contains("powerpacker") || lname.contains("decrunch") {
        log::info!("OpenLibrary(\"{name}\") refused");
        return 0;
    }
    log::info!("OpenLibrary(\"{name}\") -> stub library");
    make_library(m, &lname, LibKind::Stub, 200, 0x100, 40)
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
        LibKind::Graphics | LibKind::Stub => false,
    };
    if !handled {
        if m.os.logged.insert((lib, lvo)) {
            log::warn!("unimplemented {} LVO -{}", m.os.libs[lib].name, lvo);
        }
        m.set_d0(0);
    }
}

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
        | "CloseLibrary" | "CloseDevice" | "CachePreDMA"
        | "CachePostDMA" | "SumLibrary" | "AddResource" | "AddLibrary" | "AddDevice"
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
            if !m.os.int_servers[n].contains(&a1) {
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
            let b = open_library(m, &n);
            m.set_d0(b);
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
        std::fs::read(p)
    }

    pub fn host_path(&self, name: &str, current: Option<&Path>) -> PathBuf {
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
                let mut d = any_case(&self.module_dir, "Instruments");
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
            let f = match d2 {
                1006 => File::create(&p), // MODE_NEWFILE
                _ => File::open(&p),
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
            let (buf, len) = if name == "FRead" { (d2, d3 * m.d(4)) } else { (d2, d3) };
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
            let len = if name == "FWrite" { d3 * m.d(4) } else { d3 };
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
            if p.exists() {
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
            let r = std::fs::read(&p).ok().and_then(|d| crate::deli::load_seg(m, &d).ok());
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
    let md = std::fs::metadata(p).ok();
    let is_dir = md.as_ref().map(|x| x.is_dir()).unwrap_or(false);
    let size = md.as_ref().map(|x| x.len()).unwrap_or(0) as u32;
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

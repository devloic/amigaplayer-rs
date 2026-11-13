//! The DeliTracker side of the player interface: loading DeliPlayers,
//! evaluating their tag arrays, the DeliTrackerGlobals structure and the
//! support functions players call through it (see DeliPlayer.i in the
//! original developer kit).

use crate::eagle;
use crate::machine::{Machine, Regs, TaskCtx, Trap};
use crate::notes::NoteVoices;
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const TAG_USER: u32 = 0x8000_0000;
pub const DTP_BASE: u32 = TAG_USER + 0x4454; // TAG_USER + "DT"

#[allow(dead_code)]
pub mod tag {
    use super::DTP_BASE as B;
    pub const CUSTOM_PLAYER: u32 = B + 1;
    pub const REQUEST_DT_VERSION: u32 = B + 2;
    pub const PLAYER_VERSION: u32 = B + 4;
    pub const PLAYER_NAME: u32 = B + 5;
    pub const CREATOR: u32 = B + 6;
    pub const CHECK1: u32 = B + 7;
    pub const CHECK2: u32 = B + 8;
    pub const EXT_LOAD: u32 = B + 9;
    pub const INTERRUPT: u32 = B + 10;
    pub const STOP: u32 = B + 11;
    pub const CONFIG: u32 = B + 12;
    pub const USER_CONFIG: u32 = B + 13;
    pub const SUBSONG_RANGE: u32 = B + 14;
    pub const INIT_PLAYER: u32 = B + 15;
    pub const END_PLAYER: u32 = B + 16;
    pub const INIT_SOUND: u32 = B + 17;
    pub const END_SOUND: u32 = B + 18;
    pub const START_INT: u32 = B + 19;
    pub const STOP_INT: u32 = B + 20;
    pub const VOLUME: u32 = B + 21;
    pub const BALANCE: u32 = B + 22;
    pub const FASTER: u32 = B + 23;
    pub const SLOWER: u32 = B + 24;
    pub const NEXT_PATT: u32 = B + 25;
    pub const PREV_PATT: u32 = B + 26;
    pub const NEXT_SONG: u32 = B + 27;
    pub const PREV_SONG: u32 = B + 28;
    pub const SUBSONG_TEST: u32 = B + 29;
    pub const NEW_SUBSONG_RANGE: u32 = B + 30;
    pub const DELI_BASE: u32 = B + 31;
    pub const FLAGS: u32 = B + 32;
    pub const CHECK_LEN: u32 = B + 33;
    pub const DESCRIPTION: u32 = B + 34;
    pub const DECRUNCH: u32 = B + 35;
    pub const CONVERT: u32 = B + 36;
    pub const NOTE_PLAYER: u32 = B + 37;
    pub const NOTE_STRUCT: u32 = B + 38;
    pub const NOTE_INFO: u32 = B + 39;
    pub const NOTE_SIGNAL: u32 = B + 40;
    pub const PROCESS: u32 = B + 41;
    pub const PRIORITY: u32 = B + 42;
    pub const STACK_SIZE: u32 = B + 43;
    pub const MSG_PORT: u32 = B + 44;
    pub const MODULE_NAME: u32 = B + 47;
    pub const FORMAT_NAME: u32 = B + 48;
    pub const AUTHOR_NAME: u32 = B + 49;
    pub const INIT_NOTE: u32 = B + 50;
}

// DeliTrackerGlobals offsets
pub mod dtg {
    pub const DOS_BASE: u32 = 0x04;
    pub const INTUITION_BASE: u32 = 0x08;
    pub const GFX_BASE: u32 = 0x0C;
    pub const GADTOOLS_BASE: u32 = 0x10;
    pub const DIR_ARRAY: u32 = 0x18;
    pub const FILE_ARRAY: u32 = 0x1C;
    pub const PATH_ARRAY: u32 = 0x20;
    pub const CHK_DATA: u32 = 0x24;
    pub const CHK_SIZE: u32 = 0x28;
    pub const SND_NUM: u32 = 0x2C;
    pub const SND_VOL: u32 = 0x2E;
    pub const SND_LBAL: u32 = 0x30;
    pub const SND_RBAL: u32 = 0x32;
    pub const TIMER: u32 = 0x36;
    pub const FUNCS: u32 = 0x38;
    /// The last field EaglePlayerGlobals shares.
    pub const NOTE_PLAYER: u32 = 0x74;
}

pub const DELI_FN_COUNT: u16 = 21;
/// Raster lines dtg_WaitAudioDMA waits.
const DMA_WAIT_LINES: u64 = 10;
const FN_NAMES: [&str; DELI_FN_COUNT as usize] = [
    "GetListData", "LoadFile", "CopyDir", "CopyFile", "CopyString", "AudioAlloc", "AudioFree",
    "StartInt", "StopInt", "SongEnd", "CutSuffix", "SetTimer", "WaitAudioDMA", "LockScreen",
    "UnlockScreen", "NotePlayer", "AllocListData", "FreeListData", "Reserved1", "Reserved2",
    "Reserved3",
];

/// Default CIA timer value: 709379 Hz / 50.
pub const DEFAULT_TIMER: u16 = 14187;

pub struct Player {
    pub name: String,
    pub version: u32,
    pub tags: HashMap<u32, u32>,
    pub configured: bool,
    /// An Eagleplayer: it gets the EaglePlayerGlobals instead of DeliTracker's.
    pub eagle: bool,
    /// What the player's functions get in A5.
    pub globals: u32,
    /// Address of the tag array.
    pub tag_list: u32,
}

impl Player {
    pub fn tag(&self, t: u32) -> Option<u32> {
        self.tags.get(&t).copied()
    }
    pub fn has(&self, t: u32) -> bool {
        self.tags.contains_key(&t)
    }
    pub fn is_player(&self) -> bool {
        self.check().is_some() || self.is_custom()
    }
    /// The module check, in score's order of preference.
    pub fn check(&self) -> Option<u32> {
        [tag::CHECK2, eagle::tag::CHECK5, eagle::tag::CHECK3, tag::CHECK1].into_iter().find_map(|t| self.tag(t))
    }
    /// Whether DeliTracker's timer calls DTP_Interrupt, rather than DTP_StartInt
    /// being called. Like score, an Eagleplayer with both gets StartInt.
    pub fn timer_driven(&self) -> bool {
        self.has(tag::INTERRUPT) && !(self.eagle && self.has(tag::START_INT))
    }
    pub fn is_custom(&self) -> bool {
        self.has(tag::CUSTOM_PLAYER) || self.tag(tag::FLAGS).unwrap_or(0) & 1 != 0
    }
}

#[derive(Default)]
pub struct DeliHost {
    pub globals: u32,
    pub path_buf: u32,
    pub dir_buf: u32,
    pub file_buf: u32,
    /// Files loaded for the current module (index 0 = the module itself).
    pub list: Vec<(u32, u32)>,
    pub module_dir: PathBuf,
    pub timer_running: bool,
    pub songend: bool,
    pub note_struct: u32,
    pub voices: NoteVoices,
    pub chk_buf: u32,
    pub eagle: eagle::EagleHost,
    /// Players that run as a process of their own (DTP_Process).
    pub procs: Vec<PlayerProc>,
    /// The DTP_NoteSignal of the NotePlayer genie playing the current `_note` player's
    /// notes (0: the built-in renderer does).
    pub note_signal: u32,
    /// The NoteStruct that genie reads, refilled from the player's (`notes::GenieNotes`).
    pub genie_notes: Option<crate::notes::GenieNotes>,
}

/// A `DTP_Process` player. DeliTracker starts its process code as a task, stores the
/// task's message port where `DTP_MsgPort` points, and sends it a DeliMessage for each
/// player function (DTMN_Function at 20, DTMN_Result at 24); the process calls the
/// function, stores the result and replies. The checks run directly, before there is a
/// process.
pub struct PlayerProc {
    entry: u32,
    /// DTP_Priority: which of several ready processes runs first.
    pri: i8,
    stack_size: u32,
    msg_port_ptr: u32,
    /// The functions that go to the process.
    fns: HashSet<u32>,
    task: Option<TaskCtx>,
    /// Started, and not running right now (`task` is taken while it runs).
    running: bool,
    port: u32,
    msg: u32,
    reply: u32,
}

impl PlayerProc {
    pub fn new(tags: &HashMap<u32, u32>) -> Option<Self> {
        let entry = *tags.get(&tag::PROCESS)?;
        let checks = [tag::CHECK1, tag::CHECK2, eagle::tag::CHECK3, eagle::tag::CHECK5];
        let fns = tags
            .iter()
            // NoteSignal is called directly, from the player's interrupt
            .filter(|(t, _)| !checks.contains(t) && ![tag::PROCESS, tag::CONFIG, tag::NOTE_SIGNAL].contains(t))
            .map(|(_, &v)| v)
            .collect();
        Some(PlayerProc {
            entry,
            pri: tags.get(&tag::PRIORITY).map_or(0, |&p| p as i8),
            stack_size: tags.get(&tag::STACK_SIZE).copied().unwrap_or(4096).max(0x4000),
            msg_port_ptr: tags.get(&tag::MSG_PORT).copied().unwrap_or(0),
            fns,
            task: None,
            running: false,
            port: 0,
            msg: 0,
            reply: 0,
        })
    }
}

fn new_port(m: &mut Machine, port: u32, task: u32, sigbit: u8) {
    m.bus.w8(port + 8, 4); // NT_MSGPORT
    m.bus.w8(port + 15, sigbit);
    m.bus.w32(port + 16, task);
    let l = port + 20;
    m.bus.w32(l, l + 4);
    m.bus.w32(l + 4, 0);
    m.bus.w32(l + 8, l);
}

/// Creates the player's process and runs it until it waits for its first message.
fn start_proc(m: &mut Machine, i: usize) -> Result<(), String> {
    // from the top of the heap, so that modules land where they would without processes
    let top = std::mem::replace(&mut m.alloc_top, true);
    let tcb = m.alloc(0x100);
    let (msg, reply) = (m.alloc(28), m.alloc(34));
    if tcb == 0 || msg == 0 || reply == 0 {
        return Err("out of guest memory for a player process".into());
    }
    m.bus.w8(tcb + 8, 13); // NT_PROCESS
    m.bus.w8(tcb + 9, m.deli.procs[i].pri as u8); // ln_Pri
    m.bus.w32(tcb + crate::os::TC_SIGALLOC, 0xFFFF); // the system signals
    m.bus.w32(tcb + 0xB8, 0xFFFF_FFFF); // pr_WindowPtr: no requesters
    let port = tcb + 92; // pr_MsgPort, signal SIGB_DOS
    new_port(m, port, tcb, 8);
    let main = m.os.process;
    new_port(m, reply, main, 16);
    let p = &m.deli.procs[i];
    let (entry, stack, ptr) = (p.entry, p.stack_size, p.msg_port_ptr);
    if ptr != 0 {
        m.bus.w32(ptr, port);
    }
    let t = m.new_task(entry, stack, tcb);
    m.alloc_top = top;
    let mut t = t.ok_or("out of guest memory for a process stack")?;
    let r = m.resume(&mut t);
    let p = &mut m.deli.procs[i];
    (p.port, p.msg, p.reply) = (port, msg, reply);
    p.task = Some(t);
    p.running = true;
    r
}

/// dos CreateNewProc for a player: a process of its own at `entry` (`name` 0 or a C string),
/// started at once and run until it first waits, as a child of higher priority would; after that
/// it runs, like the player processes, when it has received what it waits for. Returns the
/// Process (0 when guest memory ran out).
pub fn create_process(m: &mut Machine, entry: u32, stack_size: u32, pri: i8, name: u32) -> u32 {
    let top = std::mem::replace(&mut m.alloc_top, true);
    let tcb = m.alloc(0x100);
    if tcb == 0 {
        m.alloc_top = top;
        return 0;
    }
    m.bus.w8(tcb + 8, 13); // NT_PROCESS
    m.bus.w8(tcb + 9, pri as u8);
    m.bus.w32(tcb + 10, name);
    m.bus.w32(tcb + crate::os::TC_SIGALLOC, 0xFFFF);
    m.bus.w32(tcb + 0xB8, 0xFFFF_FFFF); // pr_WindowPtr: no requesters
    let port = tcb + 92; // pr_MsgPort
    new_port(m, port, tcb, 8);
    let stack_size = stack_size.max(0x4000);
    let t = m.new_task(entry, stack_size, tcb);
    m.alloc_top = top;
    let Some(mut t) = t else { return 0 };
    m.os.new_procs.insert(tcb);
    log::debug!("CreateNewProc: process ${tcb:06x} at ${entry:06x}, priority {pri}");
    m.deli.procs.push(PlayerProc {
        entry,
        pri,
        stack_size,
        msg_port_ptr: 0,
        fns: HashSet::new(),
        task: None,
        running: true,
        port,
        msg: 0,
        reply: 0,
    });
    let i = m.deli.procs.len() - 1;
    let r = m.resume(&mut t);
    m.deli.procs[i].task = Some(t);
    if let Err(e) = r {
        m.crashed = Some(e);
    }
    tcb
}

/// Calls a process player's function through a DeliMessage. Only D0 comes back.
fn proc_call(m: &mut Machine, i: usize, entry: u32) -> Result<Regs, String> {
    if m.deli.procs[i].task.is_none() && !m.deli.procs[i].running {
        start_proc(m, i)?;
    }
    let p = &m.deli.procs[i];
    let (port, msg, reply) = (p.port, p.msg, p.reply);
    m.bus.w32(msg + 14, reply); // mn_ReplyPort
    m.bus.w16(msg + 18, 28); // mn_Length
    m.bus.w32(msg + 20, entry);
    m.bus.w32(msg + 24, 0);
    crate::os::put_msg(m, port, msg, 5);
    log::trace!("DeliMessage ${entry:06x} to process {i}");
    for _ in 0..10_000 {
        run_proc(m, i)?;
        if crate::os::get_msg(m, reply) == msg {
            let mut r = [0; 16];
            r[0] = m.bus.r32(msg + 24);
            return Ok(r);
        }
        let Some(t) = &m.deli.procs[i].task else { return Err("player process ended".into()) };
        if t.done {
            return Err("player process ended".into());
        }
        if !crate::os::task_ready(m, t.tcb) {
            return Err(format!("player process did not answer a call to ${entry:06x}"));
        }
    }
    Err("player process keeps running without answering".into())
}

/// Runs a started process until it waits again.
fn run_proc(m: &mut Machine, i: usize) -> Result<(), String> {
    let Some(mut t) = m.deli.procs[i].task.take() else { return Ok(()) };
    let r = m.resume(&mut t);
    m.deli.procs[i].task = Some(t);
    r
}

/// Drops the processes the stopped module's player made with CreateNewProc (they are not run again).
pub fn end_new_procs(m: &mut Machine) {
    if m.os.new_procs.is_empty() {
        return;
    }
    let gone = std::mem::take(&mut m.os.new_procs);
    m.deli.procs.retain(|p| !p.task.as_ref().is_some_and(|t| gone.contains(&t.tcb)));
    m.os.delays.clear();
}

/// The engine's own context waits (dos Delay, or exec Wait for a signal that has not come) while
/// processes a player made with CreateNewProc have something to do: they run first, as they
/// would while the caller slept; a process asleep in Delay wakes, since time passes.
pub fn run_new_procs(m: &mut Machine) {
    for i in 0..m.deli.procs.len() {
        let Some(t) = &m.deli.procs[i].task else { continue };
        let tcb = t.tcb;
        if t.done || !m.os.new_procs.contains(&tcb) {
            continue;
        }
        if let Some(w) = m.os.delays.get_mut(&tcb) {
            *w = 0;
        } else if !crate::os::task_ready(m, tcb) {
            continue;
        }
        if let Err(e) = run_proc(m, i) {
            m.crashed = Some(e);
        }
    }
}

/// Runs the player processes that have received what they wait for (audio.device replies).
pub fn run_procs(m: &mut Machine) {
    // highest priority first, as exec would schedule them (SetTaskPri may change it)
    let mut order: Vec<usize> = (0..m.deli.procs.len()).filter(|&i| m.deli.procs[i].task.is_some()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(m.bus.r8(m.deli.procs[i].task.as_ref().map_or(0, |t| t.tcb) + 9) as i8));
    for i in order {
        let ready = match &m.deli.procs[i].task {
            Some(t) => !t.done && (crate::os::task_ready(m, t.tcb) || m.os.delays.get(&t.tcb).is_some_and(|&w| m.bus.clock >= w)),
            None => false,
        };
        if ready {
            if let Err(e) = run_proc(m, i) {
                m.crashed = Some(e);
            }
        }
    }
}

/// Loads a hunk executable into guest memory like dos LoadSeg(). Returns
/// (seglist BPTR, address of the first hunk's data).
pub fn load_seg(m: &mut Machine, data: &[u8]) -> Result<(u32, u32)> {
    let hf = crate::hunk::parse(data)?;
    let mut bases = Vec::new();
    for h in &hf.hunks {
        let blk = m.alloc(h.data.len() as u32 + 8);
        if blk == 0 {
            bail!("out of guest memory");
        }
        m.bus.w32(blk, h.data.len() as u32 + 8);
        m.bus.write_bytes(blk + 8, &h.data);
        bases.push(blk);
    }
    for (i, h) in hf.hunks.iter().enumerate() {
        let base = bases[i] + 8;
        for &(off, target) in &h.relocs {
            let Some(&tb) = bases.get(target as usize) else { bail!("bad reloc target") };
            let a = base + off;
            let v = m.bus.r32(a);
            m.bus.w32(a, v.wrapping_add(tb + 8));
        }
        let next = bases.get(i + 1).map(|&b| (b + 4) >> 2).unwrap_or(0);
        m.bus.w32(bases[i] + 4, next);
    }
    Ok(((bases[0] + 4) >> 2, bases[0] + 8))
}

/// `load_seg` for a player `players_native` holds a recompiled copy of: the sections go
/// to the addresses the copy was compiled for, and the machine runs its calls natively.
/// None when there is no copy for these exact bytes, or it is already loaded.
fn load_seg_native(m: &mut Machine, data: &[u8]) -> Option<(u32, u32)> {
    if !m.native_layout {
        return None;
    }
    let h = players_native::hash(data);
    let info = players_native::PLAYERS.iter().find(|p| p.hash == h)?;
    if m.native.iter().any(|n| std::ptr::eq(n.info, info)) {
        return None;
    }
    let hf = crate::hunk::parse(data).ok()?;
    if hf.hunks.len() != info.bases.len() {
        log::warn!("{}: {} hunks, recompiled with {}", info.file, hf.hunks.len(), info.bases.len());
        return None;
    }
    for (i, h) in hf.hunks.iter().enumerate() {
        let base = info.bases[i];
        m.bus.w32(base - 8, h.data.len() as u32 + 8);
        let next = info.bases.get(i + 1).map(|&b| (b - 4) >> 2).unwrap_or(0);
        m.bus.w32(base - 4, next);
        m.bus.write_bytes(base, &h.data);
    }
    for (i, h) in hf.hunks.iter().enumerate() {
        for &(off, target) in &h.relocs {
            let tb = *info.bases.get(target as usize)?;
            let a = info.bases[i] + off;
            let v = m.bus.r32(a);
            m.bus.w32(a, v.wrapping_add(tb));
        }
    }
    let lo = info.bases[0] - 8;
    let hi = info.bases[hf.hunks.len() - 1] + hf.hunks.last()?.data.len() as u32;
    m.add_native(info, lo, hi);
    log::debug!("{} loaded natively at ${lo:06x}", info.file);
    Some(((info.bases[0] - 4) >> 2, info.bases[0]))
}

pub fn init_globals(m: &mut Machine) {
    let g = m.alloc(0x200);
    m.deli.globals = g;
    m.deli.path_buf = m.alloc(1024);
    m.deli.dir_buf = m.alloc(1024);
    m.deli.file_buf = m.alloc(512);
    m.deli.chk_buf = 0;
    let lib = |m: &Machine, n: &str| m.os.libs.iter().find(|l| l.name == n).map(|l| l.base).unwrap_or(0);
    let (dos, int, gfx) = (lib(m, "dos.library"), lib(m, "intuition.library"), lib(m, "graphics.library"));
    m.bus.w32(g + dtg::DOS_BASE, dos);
    m.bus.w32(g + dtg::INTUITION_BASE, int);
    m.bus.w32(g + dtg::GFX_BASE, gfx);
    m.bus.w32(g + dtg::GADTOOLS_BASE, 0);
    m.bus.w32(g + dtg::DIR_ARRAY, m.deli.dir_buf);
    m.bus.w32(g + dtg::FILE_ARRAY, m.deli.file_buf);
    m.bus.w32(g + dtg::PATH_ARRAY, m.deli.path_buf);
    m.bus.w16(g + dtg::SND_VOL, 64);
    m.bus.w16(g + dtg::SND_LBAL, 64);
    m.bus.w16(g + dtg::SND_RBAL, 64);
    m.bus.w16(g + dtg::TIMER, DEFAULT_TIMER);
    for i in 0..DELI_FN_COUNT {
        let stub = m.new_stub(Trap::Deli(i));
        m.bus.w32(g + dtg::FUNCS + i as u32 * 4, stub);
    }
}

/// Loads a DeliPlayer/genie executable and parses its tag array.
pub fn load_player(m: &mut Machine, data: &[u8], file: &Path) -> Result<Player> {
    let (_seg, code) = match load_seg_native(m, data) {
        Some(r) => r,
        None => load_seg(m, data)?,
    };
    let id: Vec<u8> = (0..8).map(|i| m.bus.r8(code + 4 + i)).collect();
    if id != b"DELIRIUM" && id != b"EPPLAYER" {
        bail!("not a DeliTracker player");
    }
    let tag_list = m.bus.r32(code + 12);
    let tags = parse_tags(&m.bus, tag_list);
    let is_eagle = id == b"EPPLAYER" || eagle::has_ep_tags(&tags);
    let globals = if is_eagle { eagle::init_globals(m) } else { m.deli.globals };
    let s = |m: &Machine, t: u32| tags.get(&t).map(|&a| m.bus.read_cstr(a)).unwrap_or_default();
    let mut name = s(m, tag::PLAYER_NAME);
    if name.is_empty() {
        name = file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    }
    let player = Player {
        version: tags.get(&tag::PLAYER_VERSION).copied().unwrap_or(0),
        name,
        tags,
        configured: false,
        eagle: is_eagle,
        globals,
        tag_list,
    };
    for t in [tag::DELI_BASE, eagle::tag::EAGLE_BASE] {
        if let Some(db) = player.tag(t) {
            m.bus.w32(db, globals);
        }
    }
    Ok(player)
}

/// A tag array (TAG_DONE terminated, at most 200 entries) as a map.
pub fn parse_tags(bus: &crate::bus::Bus, list: u32) -> HashMap<u32, u32> {
    let mut tags = HashMap::new();
    let mut p = list;
    for _ in 0..200 {
        let t = bus.r32(p);
        if t == 0 {
            break;
        }
        tags.insert(t, bus.r32(p + 4));
        p += 8;
    }
    tags
}

/// Calls a player function with A5 = `globals`. Returns the registers.
pub fn call_fn(m: &mut Machine, globals: u32, entry: u32, regs: Option<Regs>) -> Result<Regs, String> {
    // a function tag left at 0 (unrelocated, e.g. Mark II's DTP_Volume, DTP_Balance and
    // EP_Voices) names no function: there is nothing to call (nor a process to send it to)
    if entry == 0 {
        return Ok(regs.unwrap_or([0; 16]));
    }
    // a process player's function goes to its process, unless that is what calls it
    if let Some(i) = m.deli.procs.iter().position(|p| p.fns.contains(&entry) && (p.task.is_some() || !p.running)) {
        return proc_call(m, i, entry);
    }
    let mut r = regs.unwrap_or([0; 16]);
    r[13] = globals;
    r[14] = m.os.exec_base;
    m.call(entry, &r)
}

pub fn timer_value(m: &Machine, globals: u32) -> u16 {
    let t = m.bus.r16(globals + dtg::TIMER);
    if t == 0 { DEFAULT_TIMER } else { t }
}

fn path_append(m: &mut Machine, s: &str) {
    let cur = m.bus.read_cstr(m.deli.path_buf);
    let mut n = cur + s;
    n.truncate(1000);
    m.bus.write_cstr(m.deli.path_buf, &n);
}

pub fn host_call(m: &mut Machine, f: u16) {
    let name = FN_NAMES.get(f as usize).copied().unwrap_or("?");
    log::trace!("dtg_{name}");
    match f {
        0 => {
            // GetListData(d0) -> a0 = addr, d0 = size
            let n = m.d(0) as usize;
            let (a, s) = m.deli.list.get(n).copied().unwrap_or((0, 0));
            m.cpu.dar[8] = a;
            // and the flags of a `move.l` of the size into d0, as UADE's score
            // returns: SoundFX 2.0's InitSound tests them (beq) after the call
            m.set_d0_flags(s);
        }
        1 => {
            // LoadFile(): load file named in path buffer
            let name = m.bus.read_cstr(m.deli.path_buf);
            let (p, data) = read_companion(m, &name);
            let r = match data {
                Ok(d) => {
                    let a = m.alloc(d.len() as u32 + 16);
                    if a == 0 {
                        1
                    } else {
                        m.bus.write_bytes(a, &d);
                        m.deli.list.push((a, d.len() as u32));
                        log::debug!("LoadFile {} ({} bytes)", p.display(), d.len());
                        0
                    }
                }
                Err(_) => {
                    log::info!("LoadFile: {} not found", p.display());
                    1
                }
            };
            m.set_d0(r);
        }
        2 => {
            let d = m.bus.read_cstr(m.deli.dir_buf);
            path_append(m, &d);
        }
        3 => {
            let f = m.bus.read_cstr(m.deli.file_buf);
            path_append(m, &f);
        }
        4 => {
            let s = m.bus.read_cstr(m.a(0));
            path_append(m, &s);
        }
        // AudioAlloc ok, with the flags of a `moveq #0,d0`: MusiclineEditor's
        // InitPlayer branches on them (beq) to go on loading the module
        5 => m.set_d0_flags(0),
        6 => {}
        7 => m.deli.timer_running = true,
        8 => m.deli.timer_running = false,
        9 => m.deli.songend = true,
        10 => {
            let s = m.bus.read_cstr(m.deli.path_buf);
            let l = s.to_ascii_lowercase();
            for suf in [".xpk", ".pp"] {
                if l.ends_with(suf) {
                    let n = &s[..s.len() - suf.len()];
                    m.bus.write_cstr(m.deli.path_buf, n);
                    break;
                }
            }
        }
        11 | 14 => {} // SetTimer is read back by the engine each tick
        13 => m.set_d0(0),
        15 if m.deli.note_signal != 0 => {
            // a NotePlayer genie plays the notes
            if let Some(gn) = &m.deli.genie_notes {
                gn.update(&mut m.bus, m.deli.note_struct);
            }
            let (f, g) = (m.deli.note_signal, m.deli.globals);
            if let Err(e) = call_fn(m, g, f, None) {
                m.crashed = Some(format!("NotePlayer genie: {e}"));
            }
        }
        15 => {
            let ns = m.deli.note_struct;
            if ns != 0 {
                let mut v = std::mem::take(&mut m.deli.voices);
                v.update_from_guest(&mut m.bus, ns);
                m.deli.voices = v;
            }
        }
        12 => wait_audio_dma(m),
        16 => {
            let a = m.alloc(m.d(0));
            if a != 0 {
                m.deli.list.push((a, m.d(0)));
            }
            m.set_d0(a);
        }
        17 => {
            let a = m.a(1);
            if a != 0 {
                m.deli.list.retain(|&(x, _)| x != a);
                m.heap.free(a);
            }
        }
        _ => m.set_d0(0),
    }
}

/// dtg_WaitAudioDMA: until the audio DMA has taken the new registers. As UADE's
/// (DeliTracker's is not known): until the beam has started 10 new lines.
pub fn wait_audio_dma(m: &mut Machine) {
    let line = crate::paula::LINE;
    m.bus.clock = (m.bus.clock / line + DMA_WAIT_LINES) * line;
}

/// Reads a file a player asks for by its Amiga name (relative names are in
/// the module's directory; case is ignored, as by `host_path`).
pub fn read_companion(m: &Machine, name: &str) -> (PathBuf, std::io::Result<Vec<u8>>) {
    let p = m.os.host_path(name, Some(&m.deli.module_dir));
    let d = m.os.read_file(&p);
    (p, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_array() {
        let mut b = crate::bus::Bus::new();
        for (i, v) in [tag::PLAYER_NAME, 0x100, eagle::tag::CHECK5, 0x200, 0, tag::CHECK2, 0x300].iter().enumerate() {
            b.w32(0x1000 + 4 * i as u32, *v);
        }
        let t = parse_tags(&b, 0x1000);
        assert_eq!(t.len(), 2);
        assert_eq!(t[&eagle::tag::CHECK5], 0x200);
        let p = Player { name: String::new(), version: 0, tags: t, configured: false, eagle: true, globals: 0, tag_list: 0x1000 };
        assert_eq!(p.check(), Some(0x200));
        assert!(p.is_player());
    }

    #[test]
    fn audio_alloc_sets_the_flags() {
        // MusiclineEditor's InitPlayer: jsr AudioAlloc ; beq go_on
        let mut m = Machine::new(m68k::CpuType::M68000);
        let stub = m.new_stub(Trap::Deli(5));
        let code = m.alloc(16);
        // moveq #1,d0 ; jsr (a0) ; seq d1 ; rts
        m.bus.write_bytes(code, &[0x70, 0x01, 0x4E, 0x90, 0x57, 0xC1, 0x4E, 0x75]);
        let mut r = [0u32; 16];
        r[8] = stub;
        let out = m.call(code, &r).unwrap();
        assert_eq!((out[0], out[1] & 0xFF), (0, 0xFF));
    }
}

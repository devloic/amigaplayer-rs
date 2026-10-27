//! The 68k machine: CPU, bus, guest heap, A-line trap stubs, and the
//! mechanism that lets host code call guest subroutines.
//!
//! Every host-implemented entry point (library vectors, DeliTracker support
//! functions, the call sentinel) is a stub `dc.w $Axxx ; rts` in guest
//! memory. Executing it surfaces an A-line trap; the low 12 bits select the
//! entry in `traps`.
//!
//! # Native players
//!
//! A player that `players_native` holds a recompiled copy of is loaded at the fixed
//! addresses it was compiled for (`NATIVE_LO..NATIVE_HI`, outside the heap). Calls into
//! it run the generated Rust through `run_native`, which interprets only what the
//! recompiler did not reach, with the 68000 interpreter the generated code was built
//! against. Everything else — players needing a 68020, custom modules, a player whose
//! code changed since it was loaded — runs on the 68020 interpreter as before.

use crate::bus::{Bus, RAM_SIZE, CPU_CYCLES};
use crate::deli::DeliHost;
use crate::heap::Heap;
use crate::os::OsState;
use m68k::{CpuCore, CpuType, StepResult};
use players_native::{DynBus, Info};

pub const SENTINEL_ADDR: u32 = 0x400;
const EXCEPTION_ADDR: u32 = 0x404;
const STUB_AREA: u32 = 0x408;
const STUB_AREA_END: u32 = 0x2000;
pub const HEAP_START: u32 = 0x2000;
const STACK_SIZE: u32 = 0x20000;
pub const STACK_TOP: u32 = RAM_SIZE as u32 - 0x10;
/// Max instructions for a single host->guest call (catches hung players).
const CALL_BUDGET: u64 = 200_000_000;
/// Where recompiled players live. Must match tools/native-players.py.
pub const NATIVE_LO: u32 = 0x80_0000;
pub const NATIVE_HI: u32 = STACK_TOP + 0x10 - STACK_SIZE;

#[derive(Clone, Copy, Debug)]
pub enum Trap {
    Sentinel,
    Exception,
    /// Library vector; the LVO is derived from the trapping PC.
    Lib(usize),
    /// DeliTracker support function (index into the dtg_ function table).
    Deli(u16),
    /// Eagleplayer support function (index into the ENPP_ jump table).
    Eagle(u16),
}

pub type Regs = [u32; 16];

/// A recompiled player loaded at its compiled addresses.
pub struct NativeSeg {
    pub lo: u32,
    pub hi: u32,
    pub info: &'static Info,
    /// The translated instructions' bytes as loaded and relocated (`info.code`). The
    /// generated code is only valid for these, so a call into a player that has since
    /// patched one is interpreted.
    original: Vec<u8>,
    pub patched: bool,
}

/// Instruction counts for calls into native players.
#[derive(Default, Clone, Copy, Debug)]
pub struct NativeStats {
    pub calls: u64,
    pub native: u64,
    pub interpreted: u64,
    /// Calls that went to the 68020 interpreter because the code had changed.
    pub patched_calls: u64,
}

enum TrapFlow {
    Continue,
    Returned,
    /// A task blocked in a host call (see [`Machine::yield_now`]): where it resumes.
    Yield(Option<u32>),
}

/// The saved state of a guest task (a player's `DTP_Process`) between runs.
#[derive(Clone, Debug)]
pub struct TaskCtx {
    pub regs: Regs,
    pub pc: u32,
    pub sr: u16,
    /// The entry point, which tells whose native code it runs.
    pub entry: u32,
    /// The stack pointer after the task's code returns through the sentinel.
    pub ret_sp: u32,
    /// Its Task structure.
    pub tcb: u32,
    /// The task's code returned: it is gone.
    pub done: bool,
}

pub struct Machine {
    pub cpu: CpuCore,
    pub bus: Bus,
    pub heap: Heap,
    traps: Vec<Trap>,
    next_stub: u32,
    depth: u32,
    pub os: OsState,
    pub deli: DeliHost,
    /// Set when a call ended abnormally; the engine stops the player.
    pub crashed: Option<String>,
    /// Recompiled players loaded into this machine.
    pub native: Vec<NativeSeg>,
    /// Whether recompiled players are loaded at their compiled addresses.
    pub native_layout: bool,
    /// Whether their calls run the generated code. `DELI_NATIVE=0` turns this off but
    /// keeps the layout, so that comparing the two runs tests the translation alone:
    /// players that read memory they never allocated (AHX-DeliPlayerII020 does) would
    /// otherwise differ just because different things lie around them.
    pub native_enabled: bool,
    pub native_stats: NativeStats,
    /// `alloc` takes memory from the top of the heap. Set while Eagleplayers
    /// load, so the memory DeliPlayers and modules get does not move.
    pub alloc_top: bool,
    /// `depth` of the task running now (0: the host's own context, which never blocks).
    task_depth: u32,
    /// Set by a host call that blocks the running task: where it resumes.
    yield_pc: Option<Option<u32>>,
    yielded: bool,
}

impl Machine {
    pub fn new(cpu_type: CpuType) -> Self {
        let mut cpu = CpuCore::new();
        cpu.set_cpu_type(cpu_type);
        let native_layout = !players_native::PLAYERS.is_empty();
        let native_enabled = native_layout && std::env::var("DELI_NATIVE").map_or(true, |v| v != "0");
        // The native region is only carved out of the heap when there is something to
        // put in it.
        let heap_end = if native_layout { NATIVE_LO } else { NATIVE_HI };
        let mut m = Machine {
            cpu,
            bus: Bus::new(),
            heap: Heap::new(HEAP_START, heap_end),
            traps: Vec::new(),
            next_stub: STUB_AREA,
            depth: 0,
            os: OsState::default(),
            deli: DeliHost::default(),
            crashed: None,
            native: Vec::new(),
            native_layout,
            native_enabled,
            native_stats: NativeStats::default(),
            alloc_top: false,
            task_depth: 0,
            yield_pc: None,
            yielded: false,
        };
        m.add_trap_at(SENTINEL_ADDR, Trap::Sentinel);
        m.add_trap_at(EXCEPTION_ADDR, Trap::Exception);
        // all exception vectors lead to the exception stub
        for v in 2..256u32 {
            m.bus.w32(v * 4, EXCEPTION_ADDR);
        }
        m.cpu.set_sr(0x2700);
        m.cpu.set_sp(STACK_TOP);
        m.cpu.pc = SENTINEL_ADDR;
        m.cpu.invalidate_prefetch();
        crate::os::init(&mut m);
        m
    }

    fn add_trap_at(&mut self, addr: u32, t: Trap) -> u16 {
        let id = self.traps.len() as u16;
        assert!(id < 0x1000, "out of trap ids");
        self.traps.push(t);
        self.bus.w16(addr, 0xA000 | id);
        self.bus.w16(addr + 2, 0x4E75); // rts
        id
    }

    /// Allocates a trap id without writing a stub (for library jump tables).
    pub fn new_trap_id(&mut self, t: Trap) -> u16 {
        let id = self.traps.len() as u16;
        assert!(id < 0x1000, "out of trap ids");
        self.traps.push(t);
        id
    }

    /// Writes a stub for a host function at `addr` (4 bytes, anywhere in RAM).
    pub fn stub_at(&mut self, addr: u32, t: Trap) {
        self.add_trap_at(addr, t);
    }

    /// Creates a stub routine for a host function and returns its address.
    pub fn new_stub(&mut self, t: Trap) -> u32 {
        let a = self.next_stub;
        assert!(a + 4 <= STUB_AREA_END, "stub area full");
        self.next_stub += 4;
        self.add_trap_at(a, t);
        a
    }

    pub fn alloc(&mut self, size: u32) -> u32 {
        let a = if self.alloc_top { self.heap.alloc_top(size) } else { self.heap.alloc(size) };
        match a {
            Some(a) => {
                let end = (a + size) as usize;
                self.bus.ram[a as usize..end].fill(0);
                a
            }
            None => 0,
        }
    }

    pub fn alloc_cstr(&mut self, s: &str) -> u32 {
        let a = self.alloc(s.len() as u32 + 1);
        if a != 0 {
            self.bus.write_cstr(a, s);
        }
        a
    }

    pub fn d(&self, n: usize) -> u32 {
        self.cpu.dar[n]
    }
    pub fn a(&self, n: usize) -> u32 {
        self.cpu.dar[8 + n]
    }
    pub fn set_d0(&mut self, v: u32) {
        self.cpu.dar[0] = v;
    }

    /// Whether the host call being handled may block: it was made by a task, not by
    /// code the host called.
    pub fn can_yield(&self) -> bool {
        self.task_depth != 0 && self.depth == self.task_depth
    }

    /// Blocks the running task in the current host call (only when [`Self::can_yield`]).
    /// It resumes at `pc`, or by making the same call again.
    pub fn yield_now(&mut self, pc: Option<u32>) {
        debug_assert!(self.can_yield());
        self.yield_pc = Some(pc);
    }

    /// A task that will start at `entry` on `stack_size` bytes of its own stack.
    pub fn new_task(&mut self, entry: u32, stack_size: u32, tcb: u32) -> Option<TaskCtx> {
        let stack = Some(self.alloc(stack_size)).filter(|&a| a != 0)?;
        let sp = (stack + stack_size - 4) & !3;
        self.bus.w32(sp, SENTINEL_ADDR);
        let mut regs = [0; 16];
        regs[15] = sp;
        Some(TaskCtx { regs, pc: entry, sr: 0x2000, entry, ret_sp: sp + 4, tcb, done: false })
    }

    /// Runs a task until it blocks or ends. The host context is restored afterwards.
    pub fn resume(&mut self, t: &mut TaskCtx) -> Result<(), String> {
        if t.done {
            return Ok(());
        }
        let (saved_dar, saved_pc, saved_sr) = (self.cpu.dar, self.cpu.pc, self.cpu.get_sr());
        let (saved_task, saved_cur) = (self.task_depth, self.os.current_task);
        self.cpu.dar = t.regs;
        self.cpu.set_sr(t.sr);
        self.cpu.pc = t.pc;
        self.cpu.invalidate_prefetch();
        self.depth += 1;
        self.task_depth = self.depth;
        self.os.current_task = t.tcb;
        let res = match self.native_seg(t.entry) {
            Some(i) => self.run_native(t.ret_sp, i),
            None => self.run_until_return(t.ret_sp),
        };
        self.depth -= 1;
        self.task_depth = saved_task;
        self.os.current_task = saved_cur;
        if std::mem::take(&mut self.yielded) {
            t.regs = self.cpu.dar;
            t.pc = self.cpu.pc;
            t.sr = self.cpu.get_sr();
        } else {
            t.done = true;
        }
        self.cpu.dar = saved_dar;
        self.cpu.set_sr(saved_sr);
        self.cpu.pc = saved_pc;
        self.cpu.invalidate_prefetch();
        res
    }

    /// Calls the guest subroutine at `entry` with the given registers (A7 is
    /// ignored) and returns the registers at its RTS. CPU state is restored
    /// afterwards so calls can nest from inside trap handlers.
    pub fn call(&mut self, entry: u32, regs: &Regs) -> Result<Regs, String> {
        if entry == 0 {
            return Err("call to NULL".into());
        }
        let saved_dar = self.cpu.dar;
        let saved_pc = self.cpu.pc;
        let saved_sr = self.cpu.get_sr();
        let sp = if self.depth == 0 { STACK_TOP } else { (saved_dar[15] - 0x100) & !3 };
        if sp < STACK_TOP - STACK_SIZE + 0x400 {
            return Err("guest stack overflow".into());
        }
        let sp = sp - 4;
        self.bus.w32(sp, SENTINEL_ADDR);
        self.cpu.dar = *regs;
        self.cpu.dar[15] = sp;
        self.cpu.set_sr(0x2000 | (saved_sr & 0x0700));
        self.cpu.pc = entry;
        self.cpu.invalidate_prefetch();
        self.depth += 1;
        let res = match self.native_seg(entry) {
            Some(i) => self.run_native(sp + 4, i),
            None => self.run_until_return(sp + 4),
        };
        self.depth -= 1;
        let out = self.cpu.dar;
        self.cpu.dar = saved_dar;
        self.cpu.set_sr(saved_sr);
        self.cpu.pc = saved_pc;
        self.cpu.invalidate_prefetch();
        res.map(|_| out)
    }

    fn run_until_return(&mut self, ret_sp: u32) -> Result<(), String> {
        let mut executed: u64 = 0;
        let mut cycles: u64 = 0;
        self.bus.rebase(cycles);
        loop {
            // the time: each instruction's 68000 cycles from the table the generated code
            // counts with, charged before it runs as there, so that interpreted and native
            // runs see the same clock (A-line stubs are host calls and take none)
            let pc = self.cpu.pc;
            if self.bus.r16(pc) & 0xF000 != 0xA000 {
                let ins = m68k_rc::decode::decode_model(pc, &mut |a| self.bus.r16(a), m68k_rc::Model::M68020);
                cycles += m68k_rc::cycles::cycles(&ins) as u64;
                m68k_rc::Bus::cycles(&mut self.bus, cycles);
            }
            let r = self.cpu.step(&mut self.bus);
            executed += 1;
            if executed > CALL_BUDGET {
                return Err(format!("guest code did not return (PC=${:06x})", self.cpu.pc));
            }
            match r {
                StepResult::Ok { .. } => {}
                StepResult::AlineTrap { opcode } => {
                    let trap_pc = self.cpu.ppc;
                    match self.trap(opcode, trap_pc, ret_sp)? {
                        TrapFlow::Returned => return Ok(()),
                        TrapFlow::Yield(pc) => {
                            self.cpu.pc = pc.unwrap_or(trap_pc);
                            self.cpu.invalidate_prefetch();
                            self.yielded = true;
                            return Ok(());
                        }
                        TrapFlow::Continue => {}
                    }
                    // resume after the A-line word (the stub's RTS)
                    self.cpu.pc = trap_pc + 2;
                    self.cpu.invalidate_prefetch();
                    self.bus.rebase(cycles);
                }
                StepResult::Stopped => {
                    return Err(format!("CPU stopped at ${:06x}", self.cpu.pc));
                }
                other => {
                    return Err(format!("unhandled {other:?} at PC=${:06x}", self.cpu.ppc));
                }
            }
        }
    }

    /// Handles the host side of an A-line stub. `self.cpu` must hold the guest state.
    fn trap(&mut self, opcode: u16, trap_pc: u32, ret_sp: u32) -> Result<TrapFlow, String> {
        let t = self.traps.get((opcode & 0xFFF) as usize).copied();
        match t {
            Some(Trap::Sentinel) => {
                if self.cpu.dar[15] == ret_sp {
                    return Ok(TrapFlow::Returned);
                }
                return Err(format!(
                    "returned through sentinel with unbalanced stack (SP=${:06x}, expected ${ret_sp:06x})",
                    self.cpu.dar[15]
                ));
            }
            Some(Trap::Exception) => {
                return Err(format!(
                    "CPU exception {:?} near PC=${:06x}",
                    self.cpu.last_exception_vector,
                    self.bus.r32(self.cpu.dar[15] + 2)
                ));
            }
            Some(Trap::Lib(lib)) => {
                let base = self.os.libs[lib].base;
                let lvo = base.wrapping_sub(trap_pc);
                crate::os::lib_call(self, lib, lvo as i32);
            }
            Some(Trap::Deli(f)) => crate::deli::host_call(self, f),
            Some(Trap::Eagle(f)) => crate::eagle::host_call(self, f),
            None => {
                return Err(format!("unknown A-line ${opcode:04x} at ${trap_pc:06x}"));
            }
        }
        if let Some(e) = self.crashed.take() {
            return Err(e);
        }
        if let Some(pc) = self.yield_pc.take() {
            return Ok(TrapFlow::Yield(pc));
        }
        Ok(TrapFlow::Continue)
    }

    /// The native player a call to `entry` should run in, if any.
    fn native_seg(&mut self, entry: u32) -> Option<usize> {
        if !self.native_enabled {
            return None;
        }
        let i = self.native.iter().position(|n| (n.lo..n.hi).contains(&entry))?;
        let n = &mut self.native[i];
        if !n.patched && !same_code(&self.bus.ram, n.info.code, &n.original) {
            n.patched = true;
            log::warn!("{} changed its own code; its calls are interpreted from now on", n.info.file);
        }
        if n.patched {
            self.native_stats.patched_calls += 1;
            return None;
        }
        Some(i)
    }

    /// Records a player loaded at its compiled addresses (`lo..hi` is its whole image).
    pub fn add_native(&mut self, info: &'static Info, lo: u32, hi: u32) {
        let original =
            info.code.iter().flat_map(|&(a, b)| self.bus.ram[a as usize..b as usize].iter().copied()).collect();
        self.native.push(NativeSeg { lo, hi, info, original, patched: false });
    }

    /// `run_until_return` for a call into a recompiled player: generated code where
    /// there is some, the 68000 interpreter elsewhere, host traps as usual.
    fn run_native(&mut self, ret_sp: u32, seg: usize) -> Result<(), String> {
        let lookup = self.native[seg].info.lookup;
        self.native_stats.calls += 1;
        let model = self.native[seg].info.cpu;
        let mut c = to_rc(&self.cpu);
        c.model = model;
        c.budget = CALL_BUDGET;
        self.bus.rebase(c.cycles);
        loop {
            let pc = c.pc;
            let op = self.bus.r16(pc);
            if op & 0xF000 == 0xA000 {
                // A host stub. The handlers work on `self.cpu`, and may call back into
                // the guest (which saves and restores it), so the state goes across
                // and comes back.
                from_rc(&c, &mut self.cpu);
                self.cpu.ppc = pc;
                let flow = self.trap(op, pc, ret_sp);
                self.native_stats.native += c.ticks - c.interp_count;
                self.native_stats.interpreted += c.interp_count;
                match flow? {
                    TrapFlow::Returned => return Ok(()),
                    TrapFlow::Yield(to) => {
                        self.cpu.pc = to.unwrap_or(pc);
                        self.cpu.invalidate_prefetch();
                        self.yielded = true;
                        return Ok(());
                    }
                    TrapFlow::Continue => {}
                }
                let budget = CALL_BUDGET.saturating_sub(c.ticks);
                c = to_rc(&self.cpu);
                c.model = model;
                c.budget = budget;
                c.pc = pc + 2;
                self.bus.rebase(c.cycles);
                continue;
            }
            if c.ticks >= c.budget {
                from_rc(&c, &mut self.cpu);
                return Err(format!("guest code did not return (PC=${pc:06x})"));
            }
            if c.stopped {
                from_rc(&c, &mut self.cpu);
                return Err(format!("CPU stopped at ${pc:06x}"));
            }
            match lookup(pc) {
                Some(f) => {
                    // SAFETY: the DynBus lives only for this call.
                    let mut b = unsafe { DynBus::new(&mut self.bus) };
                    f(&mut c, &mut b);
                }
                None => m68k_rc::runtime::interp_step(&mut c, &mut self.bus),
            }
        }
    }
}

fn same_code(ram: &[u8], code: &[(u32, u32)], original: &[u8]) -> bool {
    let mut off = 0;
    code.iter().all(|&(a, b)| {
        let n = (b - a) as usize;
        let same = ram[a as usize..b as usize] == original[off..off + n];
        off += n;
        same
    })
}

fn to_rc(cpu: &CpuCore) -> m68k_rc::Cpu {
    let mut c = m68k_rc::Cpu::default();
    c.d.copy_from_slice(&cpu.dar[..8]);
    c.a.copy_from_slice(&cpu.dar[8..]);
    c.pc = cpu.pc;
    let sr = cpu.get_sr();
    c.x = sr & 0x10 != 0;
    c.n = sr & 0x08 != 0;
    c.z = sr & 0x04 != 0;
    c.v = sr & 0x02 != 0;
    c.c = sr & 0x01 != 0;
    c.s = sr & 0x2000 != 0;
    c.t = sr & 0x8000 != 0;
    c.imask = ((sr >> 8) & 7) as u8;
    // The inactive stack pointer and the 68010+ control registers (MOVEC). The
    // reference core banks stacks as sp[0] = USP, sp[4] = ISP, sp[6] = MSP.
    c.other_sp = if c.s { cpu.sp[0] } else { cpu.sp[4] };
    c.msp = cpu.sp[6];
    c.vbr = cpu.vbr;
    c.sfc = cpu.sfc;
    c.dfc = cpu.dfc;
    c.cacr = cpu.cacr;
    c.caar = cpu.caar;
    c
}

fn from_rc(c: &m68k_rc::Cpu, cpu: &mut CpuCore) {
    cpu.dar[..8].copy_from_slice(&c.d);
    cpu.dar[8..].copy_from_slice(&c.a);
    cpu.pc = c.pc;
    let sr = (c.t as u16) << 15
        | (c.s as u16) << 13
        | (c.imask as u16 & 7) << 8
        | (c.x as u16) << 4
        | (c.n as u16) << 3
        | (c.z as u16) << 2
        | (c.v as u16) << 1
        | c.c as u16;
    // A7 is already the active stack pointer in `dar`: no stack banking.
    cpu.set_sr_noint_nosp(sr);
    if c.s {
        cpu.sp[0] = c.other_sp;
    } else {
        cpu.sp[4] = c.other_sp;
    }
    cpu.sp[6] = c.msp;
    cpu.vbr = c.vbr;
    cpu.sfc = c.sfc;
    cpu.dfc = c.dfc;
    cpu.cacr = c.cacr;
    cpu.caar = c.caar;
    cpu.invalidate_prefetch();
}

/// The bus as the recompiled code and its interpreter see it.
impl m68k_rc::Bus for Bus {
    #[inline(always)]
    fn read8(&mut self, a: u32) -> u8 {
        m68k::AddressBus::read_byte(self, a)
    }
    #[inline(always)]
    fn read16(&mut self, a: u32) -> u16 {
        m68k::AddressBus::read_word(self, a)
    }
    #[inline(always)]
    fn write8(&mut self, a: u32, v: u8) {
        m68k::AddressBus::write_byte(self, a, v)
    }
    #[inline(always)]
    fn write16(&mut self, a: u32, v: u16) {
        m68k::AddressBus::write_word(self, a, v)
    }
    /// Guest code runs on a 68000's time: the clock follows its cycles (see `rebase`).
    #[inline(always)]
    fn cycles(&mut self, total: u64) {
        let t = self.epoch.0 + total.saturating_sub(self.epoch.1) / CPU_CYCLES;
        if t > self.clock {
            self.clock = t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_returns_registers() {
        let mut m = Machine::new(CpuType::M68020);
        let code = m.alloc(16);
        // moveq #42,d0 ; add.l d1,d0 ; rts
        m.bus.write_bytes(code, &[0x70, 42, 0xD0, 0x81, 0x4E, 0x75]);
        let mut r = [0u32; 16];
        r[1] = 8;
        let out = m.call(code, &r).unwrap();
        assert_eq!(out[0], 50);
    }

    #[test]
    fn library_call_from_guest() {
        let mut m = Machine::new(CpuType::M68000);
        let code = m.alloc(32);
        // movea.l 4.w,a6 ; move.l #100,d0 ; moveq #0,d1 ; jsr -198(a6) ; rts
        m.bus.write_bytes(
            code,
            &[0x2C, 0x78, 0x00, 0x04, 0x20, 0x3C, 0, 0, 0, 100, 0x72, 0, 0x4E, 0xAE, 0xFF, 0x3A, 0x4E, 0x75],
        );
        let out = m.call(code, &[0; 16]).unwrap();
        assert!(out[0] >= HEAP_START && out[0] < STACK_TOP, "AllocMem returned {:x}", out[0]);
    }

    #[test]
    fn nested_call_from_trap() {
        // Cause() calls the interrupt code from inside a trap handler
        let mut m = Machine::new(CpuType::M68000);
        let int_code = m.alloc(8);
        // move.l #$1234,(a1) ; rts
        m.bus.write_bytes(int_code, &[0x22, 0xBC, 0, 0, 0x12, 0x34, 0x4E, 0x75]);
        let data = m.alloc(4);
        let is = m.alloc(22);
        m.bus.w32(is + 14, data);
        m.bus.w32(is + 18, int_code);
        let code = m.alloc(16);
        // movea.l 4.w,a6 ; jsr -180(a6) ; rts       (a1 = interrupt)
        m.bus.write_bytes(code, &[0x2C, 0x78, 0x00, 0x04, 0x4E, 0xAE, 0xFF, 0x4C, 0x4E, 0x75]);
        let mut r = [0u32; 16];
        r[9] = is;
        m.call(code, &r).unwrap();
        assert_eq!(m.bus.r32(data), 0x1234);
    }
}

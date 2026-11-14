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
const STUB_AREA_END: u32 = MEM_HEADER;
/// exec's one MemHeader (MemList at ExecBase + $142), right below the memory it describes, as
/// on the Amiga: the heap, public chip memory (as TypeOfMem answers).
pub const MEM_HEADER: u32 = 0x1FE0;
pub const HEAP_START: u32 = 0x2000;
const STACK_SIZE: u32 = 0x20000;
pub const STACK_TOP: u32 = LOW_RAM_END - 0x10;
/// The end of the RAM the layout above (heap, recompiled players, stack) was made for. The
/// bytes from there to `RAM_SIZE` are for the stack once `claim_native_region` has given the
/// region below it to the heap.
const LOW_RAM_END: u32 = 0x00B0_0000;
/// Max instructions for a single host->guest call (catches hung players). For interpreted
/// code the calls it nests (an interrupt handler run from inside a library call) count too.
pub const CALL_BUDGET: u64 = 200_000_000;
/// How deep host->guest calls may nest (a guest function that calls a host function that
/// calls the guest again, ...): each level is a frame on the host's stack.
const MAX_CALL_DEPTH: u32 = 64;
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

/// Slots of the memo of instruction cycle counts (`Machine::ins_cycles`), a power of two.
const CYCLE_MEMO_SLOTS: usize = 4096;
/// The most words an instruction the memo keeps may have been decoded from.
const CYCLE_MEMO_WORDS: usize = 12;

/// The cycle count `m68k_rc::cycles` gave for the instruction at `pc`, and the words its
/// decode read (from `pc` on): it holds for as long as those words are unchanged.
#[derive(Clone, Copy)]
struct CycleMemo {
    pc: u32,
    /// Words in `words`; 0: empty slot.
    n: u8,
    cycles: u32,
    words: [u16; CYCLE_MEMO_WORDS],
}

const EMPTY_MEMO: CycleMemo = CycleMemo { pc: 0, n: 0, cycles: 0, words: [0; CYCLE_MEMO_WORDS] };

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
    /// The guest stack's top (`STACK_TOP`, or the end of RAM once the native region is the heap's).
    stack_top: u32,
    /// `alloc` takes memory from the top of the heap. Set while Eagleplayers
    /// load, so the memory DeliPlayers and modules get does not move.
    pub alloc_top: bool,
    /// `depth` of the task running now (0: the host's own context, which never blocks).
    task_depth: u32,
    /// Set by a host call that blocks the running task: where it resumes.
    yield_pc: Option<Option<u32>>,
    yielded: bool,
    /// The instructions one host->guest call may run before it is given up as hung
    /// (`CALL_BUDGET` unless the host lowers it).
    pub call_budget: u64,
    /// Instructions interpreted since the outermost call began.
    spent: u64,
    /// Cycle counts of the instructions interpreted, by PC (see `ins_cycles`).
    cycle_memo: Box<[CycleMemo]>,
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
            stack_top: STACK_TOP,
            native_enabled,
            native_stats: NativeStats::default(),
            alloc_top: false,
            task_depth: 0,
            yield_pc: None,
            yielded: false,
            call_budget: CALL_BUDGET,
            spent: 0,
            cycle_memo: vec![EMPTY_MEMO; CYCLE_MEMO_SLOTS].into_boxed_slice(),
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
        m.mem_header(heap_end);
        m
    }

    /// The MemList of exec with its one MemHeader, `HEAP_START..end`: ln_Type NT_MEMORY,
    /// mh_Attributes MEMF_PUBLIC | MEMF_CHIP, mh_Lower, mh_Upper, mh_Free. Its MemChunks are
    /// not kept (mh_First 0): the heap answers the allocations.
    fn mem_header(&mut self, end: u32) {
        let (h, list) = (MEM_HEADER, self.os.exec_base + 0x142);
        self.bus.w32(list, h); // lh_Head
        self.bus.w32(list + 4, 0); // lh_Tail
        self.bus.w32(list + 8, h); // lh_TailPred
        self.bus.w8(list + 12, 10); // lh_Type NT_MEMORY
        self.bus.w32(h, list + 4);
        self.bus.w32(h + 4, list);
        self.bus.w8(h + 8, 10);
        self.bus.w16(h + 14, 3);
        self.bus.w32(h + 20, HEAP_START);
        self.bus.w32(h + 24, end);
        self.bus.w32(h + 28, self.heap.available());
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

    /// Gives the heap the memory the layout keeps out of it (the region the recompiled 68k
    /// players occupy, and the old stack) when a block of `need` bytes then fits and did not
    /// before; the stack moves to the bytes above (`RAM_SIZE`). Only when no recompiled player
    /// is loaded and no guest code runs; from then on 68k players are interpreted (the layout
    /// is gone). False, and nothing changed, otherwise.
    pub fn claim_native_region(&mut self, need: u32) -> bool {
        if self.stack_top != STACK_TOP || !self.native.is_empty() || self.depth != 0 {
            return false;
        }
        let from = if self.native_layout { NATIVE_LO } else { NATIVE_HI };
        if self.heap.largest() >= need || self.heap.largest_with_region(from, LOW_RAM_END) < need {
            return false;
        }
        self.native_layout = false;
        self.native_enabled = false;
        self.heap.add_region(from, LOW_RAM_END);
        self.bus.w32(MEM_HEADER + 24, LOW_RAM_END); // mh_Upper
        self.stack_top = RAM_SIZE as u32 - 0x10;
        true
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

    /// `set_d0` with the flags a `move.l` of the value into D0 leaves (X kept): for
    /// host functions whose callers test the condition codes after the call.
    pub fn set_d0_flags(&mut self, v: u32) {
        self.set_d0(v);
        let x = self.cpu.get_ccr() & 0x10;
        self.cpu.set_ccr(x | if v == 0 { 4 } else { 0 } | if v >> 31 != 0 { 8 } else { 0 });
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
        if self.depth == 0 {
            self.spent = 0;
        }
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
        if self.depth >= MAX_CALL_DEPTH {
            return Err("guest calls nested too deep".into());
        }
        if self.depth == 0 {
            self.spent = 0;
        }
        let saved_dar = self.cpu.dar;
        let saved_pc = self.cpu.pc;
        let saved_sr = self.cpu.get_sr();
        let sp = if self.depth == 0 { self.stack_top } else { (saved_dar[15] - 0x100) & !3 };
        if sp < self.stack_top - STACK_SIZE + 0x400 {
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
        if let Some(t) = &mut self.bus.trace {
            t.line(format_args!("C {entry:06x} depth={} clk={}", self.depth, self.bus.clock));
        }
        let res = match self.native_seg(entry) {
            Some(i) => self.run_native(sp + 4, i),
            None => self.run_until_return(sp + 4),
        };
        self.depth -= 1;
        if let Some(t) = &mut self.bus.trace {
            t.line(format_args!("E {entry:06x} depth={} clk={}", self.depth + 1, self.bus.clock));
            if self.depth == 0 {
                t.dump();
            }
        }
        let out = self.cpu.dar;
        self.cpu.dar = saved_dar;
        self.cpu.set_sr(saved_sr);
        self.cpu.pc = saved_pc;
        self.cpu.invalidate_prefetch();
        res.map(|_| out)
    }

    /// The 68000 cycles of the instruction at `pc` (whose first word is `w0`), from the
    /// table the generated code counts with. The decode behind it is memoised by PC with
    /// the words it read; a memo is used only while all of them are unchanged, so code
    /// that rewrites itself gets the count of what it runs now.
    #[inline]
    fn ins_cycles(&mut self, pc: u32, w0: u16) -> u32 {
        let slot = (pc >> 1) as usize & (CYCLE_MEMO_SLOTS - 1);
        let m = &self.cycle_memo[slot];
        if m.pc == pc && m.n != 0 && m.words[0] == w0 {
            let mut same = true;
            for k in 1..m.n as usize {
                if self.bus.r16(pc.wrapping_add(2 * k as u32)) != m.words[k] {
                    same = false;
                    break;
                }
            }
            if same {
                return m.cycles;
            }
        }
        let (mut words, mut n, mut sequential) = ([0u16; CYCLE_MEMO_WORDS], 0usize, true);
        let bus = &self.bus;
        let ins = m68k_rc::decode::decode_model(
            pc,
            &mut |a| {
                let w = bus.r16(a);
                if n < CYCLE_MEMO_WORDS && a == pc.wrapping_add(2 * n as u32) {
                    words[n] = w;
                    n += 1;
                } else {
                    sequential = false;
                }
                w
            },
            m68k_rc::Model::M68020,
        );
        let c = m68k_rc::cycles::cycles(&ins);
        if sequential && n != 0 {
            self.cycle_memo[slot] = CycleMemo { pc, n: n as u8, cycles: c, words };
        }
        c
    }

    fn run_until_return(&mut self, ret_sp: u32) -> Result<(), String> {
        let mut cycles: u64 = 0;
        self.bus.rebase(cycles);
        loop {
            // the time: each instruction's 68000 cycles from the table the generated code
            // counts with, charged before it runs as there, so that interpreted and native
            // runs see the same clock (A-line stubs are host calls and take none)
            let pc = self.cpu.pc;
            let w0 = self.bus.r16(pc);
            if w0 & 0xF000 != 0xA000 {
                let c = self.ins_cycles(pc, w0) as u64;
                cycles += c;
                if let Some(t) = &mut self.bus.trace {
                    t.ins(pc, c, self.bus.clock);
                }
                m68k_rc::Bus::cycles(&mut self.bus, cycles);
            }
            let r = self.cpu.step(&mut self.bus);
            self.spent += 1;
            if self.spent > self.call_budget {
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
        let clk0 = self.bus.clock;
        let r = self.trap_inner(t, opcode, trap_pc, ret_sp);
        if let Some(tr) = &mut self.bus.trace {
            tr.line(format_args!("T {t:?} pc={trap_pc:06x} clk={clk0} +{}", self.bus.clock - clk0));
        }
        r
    }

    fn trap_inner(&mut self, t: Option<Trap>, opcode: u16, trap_pc: u32, ret_sp: u32) -> Result<TrapFlow, String> {
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
        let t = self.epoch.0 + total.saturating_sub(self.epoch.1) / CPU_CYCLES.load(std::sync::atomic::Ordering::Relaxed);
        if t > self.clock {
            self.clock = t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claiming_the_native_region_makes_room_and_moves_the_stack() {
        let mut m = Machine::new(CpuType::M68000);
        let before = m.heap.largest();
        // fits already: nothing is claimed
        assert!(!m.claim_native_region(before));
        let need = before + 0x1000;
        assert!(m.claim_native_region(need));
        assert!(m.heap.largest() >= need);
        // once only; the stack is at the end of RAM now, and calls run on it
        assert!(!m.claim_native_region(need + 0x1000_0000));
        let code = m.alloc(16);
        m.bus.write_bytes(code, &[0x70, 42, 0xD0, 0x81, 0x4E, 0x75]);
        let mut r = [0u32; 16];
        r[1] = 8;
        assert_eq!(m.call(code, &r).unwrap()[0], 50);
        assert_eq!(m.stack_top, RAM_SIZE as u32 - 0x10);
    }

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

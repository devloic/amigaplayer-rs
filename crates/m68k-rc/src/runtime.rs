//! The dispatcher that ties generated code, HLE handlers and the fallback interpreter together.
//!
//! Generated functions have the signature `fn(&mut Cpu, &mut B)`: they start at `cpu.pc` (which
//! must be one of their entry points), run, and return with `cpu.pc` set to wherever control goes
//! next when it leaves them (return address after RTS, an unknown computed target, an exception
//! handler, ...). The dispatcher looks that address up and continues; addresses with no native code
//! are interpreted until execution reaches an address that has some.
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::interp;

pub type NativeFn<B> = fn(&mut Cpu, &mut B);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// `cpu.pc` reached the stop address.
    Stop,
    /// The CPU executed STOP.
    Stopped,
    /// `cpu.ticks` reached `cpu.budget` (or `cpu.cycles` reached `cpu.cycle_budget`).
    Budget,
}

/// One interpreted instruction, counted (`ticks`, `interp_count`, and its [`crate::cycles`],
/// charged before it runs, as generated code does).
#[inline]
pub fn interp_step<B: Bus>(cpu: &mut Cpu, bus: &mut B) {
    let ins = crate::decode::decode_model(cpu.pc, &mut |a| bus.fetch16(a), cpu.model);
    cpu.cycles += crate::cycles::cycles(&ins) as u64;
    bus.cycles(cpu.cycles);
    interp::exec(cpu, bus, &ins);
    cpu.ticks += 1;
    cpu.interp_count += 1;
}

/// Runs from `cpu.pc` until `stop` (not executed), STOP, or the budget (`ticks >= budget` or
/// `cycles >= cycle_budget`).
pub fn run<B: Bus>(cpu: &mut Cpu, bus: &mut B, lookup: &dyn Fn(u32) -> Option<NativeFn<B>>, stop: u32) -> Exit {
    loop {
        if cpu.pc == stop {
            return Exit::Stop;
        }
        if cpu.stopped {
            return Exit::Stopped;
        }
        if cpu.ticks >= cpu.budget || cpu.cycles >= cpu.cycle_budget {
            return Exit::Budget;
        }
        match lookup(cpu.pc) {
            Some(f) => f(cpu, bus),
            None => loop {
                interp_step(cpu, bus);
                if cpu.pc == stop || cpu.stopped || cpu.ticks >= cpu.budget || cpu.cycles >= cpu.cycle_budget || lookup(cpu.pc).is_some() {
                    break;
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// `run_with`: the same dispatcher with opt-in coverage recording, self-modifying-code protection,
// block stepping and a per-step hook (used by lockstep verification).

use crate::coverage::Coverage;
use crate::smc::{Admit, SmcContext, SmcHandle};

/// Why [`run_with`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunExit {
    Stop,
    Stopped,
    Budget,
    /// The `on_step` hook asked to stop.
    Hook,
}

impl RunExit {
    /// The equivalent [`Exit`] (None for [`RunExit::Hook`]).
    pub fn exit(self) -> Option<Exit> {
        match self {
            RunExit::Stop => Some(Exit::Stop),
            RunExit::Stopped => Some(Exit::Stopped),
            RunExit::Budget => Some(Exit::Budget),
            RunExit::Hook => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// A full-speed native call (may run any number of blocks and direct calls).
    Native,
    /// A block-stepped native call (exactly one basic block).
    Block,
    /// One interpreted instruction.
    Interp,
}

/// One dispatcher step, reported to `on_step` after it ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub kind: StepKind,
    /// `cpu.pc` when the step started.
    pub pc: u32,
    /// Instructions it executed (`cpu.ticks` difference).
    pub instructions: u64,
}

pub type StepHook<'a, B> = dyn FnMut(&mut Cpu, &mut B, &Step) -> bool + 'a;

/// Options of [`run_with`]; `RunOptions::default()` behaves exactly like [`run`].
pub struct RunOptions<'a, B> {
    /// Records where interpretation starts (see [`Coverage`]).
    pub coverage: Option<&'a mut Coverage>,
    /// Self-modifying code protection: the handle of the [`crate::smc::CodeGuard`] that `bus` is
    /// (or contains).
    pub smc: Option<SmcHandle>,
    /// Run all native code one basic block per call (every block becomes a dispatcher step).
    pub step_blocks: bool,
    /// Called after every step; returning false stops the run with [`RunExit::Hook`].
    pub on_step: Option<&'a mut StepHook<'a, B>>,
}

impl<'a, B> Default for RunOptions<'a, B> {
    fn default() -> Self {
        RunOptions { coverage: None, smc: None, step_blocks: false, on_step: None }
    }
}

/// Address of the `n`-th instruction (1-based) of the straight-line block starting at `pc`:
/// the instruction that left a block which executed `n` instructions.
fn nth_instruction<B: Bus>(bus: &mut B, model: crate::Model, pc: u32, n: u64) -> u32 {
    let mut p = pc;
    for _ in 1..n.max(1) {
        p = crate::decode::decode_model(p, &mut |a| bus.fetch16(a), model).next();
    }
    p
}

/// Runs from `cpu.pc` until `stop` (not executed), STOP, the budget, or the hook.
///
/// Differences from [`run`], all opt-in through `opts` except the last:
/// * coverage recording, SMC protection, block stepping, the step hook;
/// * a native function that returns at once without executing anything (it has no block at
///   `cpu.pc`, e.g. a wrong lookup entry) is not called again in a loop: the instruction there is
///   interpreted instead.
pub fn run_with<B: Bus>(cpu: &mut Cpu, bus: &mut B, lookup: &dyn Fn(u32) -> Option<NativeFn<B>>, stop: u32, opts: &mut RunOptions<B>) -> RunExit {
    if let Some(s) = &opts.smc {
        s.bind_lookup(&|pc| lookup(pc).map(|f| f as usize));
    }
    let exact = opts.coverage.as_ref().map_or(false, |c| c.exact_sources);
    // Source instruction of the transfer to `cpu.pc` (when the last step was a block).
    let mut src: Option<u32> = None;
    let mut interpreting = false;
    loop {
        if cpu.pc == stop {
            return RunExit::Stop;
        }
        if cpu.stopped {
            return RunExit::Stopped;
        }
        if cpu.ticks >= cpu.budget || cpu.cycles >= cpu.cycle_budget {
            return RunExit::Budget;
        }
        let pc = cpu.pc;
        let native = match lookup(pc) {
            Some(f) => match &opts.smc {
                None => Some((f, false)),
                Some(s) => match s.admit(f as usize) {
                    Admit::Fast => Some((f, false)),
                    Admit::Block => Some((f, true)),
                    Admit::Interpret => None,
                },
            },
            None => None,
        };
        if let Some((f, smc_block)) = native {
            let block = smc_block || opts.step_blocks || exact;
            let t0 = cpu.ticks;
            let ev0 = opts.smc.as_ref().map_or(0, |s| s.event_count());
            if let Some(s) = &opts.smc {
                s.set_context(if block { SmcContext::Block(pc) } else { SmcContext::Native(pc) });
            }
            if block {
                let b = cpu.budget;
                cpu.budget = b.min(t0 + 1);
                f(cpu, bus);
                cpu.budget = b;
            } else {
                f(cpu, bus);
            }
            if let Some(s) = &opts.smc {
                s.set_context(SmcContext::Outside);
            }
            let n = cpu.ticks - t0;
            if let Some(s) = &opts.smc {
                if block && n > 0 && s.event_count() != ev0 {
                    let last = nth_instruction(bus, cpu.model, pc, n);
                    let end = crate::decode::decode_model(last, &mut |a| bus.fetch16(a), cpu.model).next();
                    s.check_block(ev0, pc, end);
                }
            }
            if !(cpu.pc == pc && n == 0 && !cpu.stopped) {
                interpreting = false;
                src = if block && exact && n > 0 { Some(nth_instruction(bus, cpu.model, pc, n)) } else { None };
                if let Some(h) = opts.on_step.as_mut() {
                    let st = Step { kind: if block { StepKind::Block } else { StepKind::Native }, pc, instructions: n };
                    if !h(cpu, bus, &st) {
                        return RunExit::Hook;
                    }
                }
                continue;
            }
            // No progress: fall through and interpret.
        }
        if !interpreting {
            if let Some(c) = opts.coverage.as_mut() {
                c.record(pc, src);
            }
            interpreting = true;
        }
        if let Some(s) = &opts.smc {
            s.set_context(SmcContext::Interp(pc));
        }
        interp_step(cpu, bus);
        if let Some(s) = &opts.smc {
            s.set_context(SmcContext::Outside);
        }
        if let Some(h) = opts.on_step.as_mut() {
            if !h(cpu, bus, &Step { kind: StepKind::Interp, pc, instructions: 1 }) {
                return RunExit::Hook;
            }
        }
    }
}

//! 68000 core pieces shared by the static recompiler and its runtime.
//!
//! * [`decode`]: opcode words -> [`Instr`] (full 68000 ISA, all addressing modes).
//! * [`cpu`]: programmer-visible state ([`Cpu`]), SR/CCR, stacks and exception entry.
//! * [`alu`]: flag-exact instruction semantics, `#[inline(always)]`, called both by the interpreter
//!   and by generated code.
//! * [`interp`]: an interpreter over [`Instr`], the fallback for code the recompiler did not see.
//! * [`bus`]: the memory bus trait (big-endian, 24-bit addresses).
pub mod alu;
pub mod bus;
pub mod coverage;
pub mod cycles;
pub mod cpu;
pub mod decode;
pub mod disasm;
pub mod interp;
pub mod runtime;
pub mod smc;
pub mod state;
pub mod types;

pub use bus::{Bus, FlatMem};
pub use cpu::Cpu;
pub use types::*;

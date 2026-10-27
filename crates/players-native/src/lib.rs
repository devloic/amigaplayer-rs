//! 68k players recompiled to Rust.
//!
//! `tools/native-players.py` runs m68k-recomp over every player file and writes one
//! module per player into `gen/`, plus `gen/registry.rs` listing them. Each player is
//! compiled for fixed load addresses (`Info::bases`); the host must load the file's
//! sections exactly there for the native code to apply, and must check `Info::hash`
//! so that a different file of the same name is never run as native code.
//!
//! All generated functions are instantiated once, here, for [`DynBus`]. Leaving them
//! generic would move ~2 million lines of codegen into whichever crate names the bus
//! type, and rebuild them on every change to it.

pub use m68k::{Bus, Cpu, Model};

/// The host's bus behind a trait object.
///
/// A raw pointer rather than a reference: the generated functions are stored as plain
/// `fn` pointers, which cannot be generic over the borrow's lifetime.
pub struct DynBus(*mut (dyn Bus + 'static));

impl DynBus {
    /// # Safety
    /// The `DynBus` must not outlive the borrow of `b`.
    pub unsafe fn new(b: &mut (dyn Bus + 'static)) -> Self {
        DynBus(b)
    }
}

macro_rules! fwd {
    ($($name:ident($($arg:ident: $t:ty),*) $(-> $r:ty)?;)*) => {$(
        #[inline(always)]
        fn $name(&mut self, $($arg: $t),*) $(-> $r)? {
            // SAFETY: `new`'s contract — the pointee is borrowed for this DynBus's life.
            unsafe { (*self.0).$name($($arg),*) }
        }
    )*};
}

impl Bus for DynBus {
    fwd! {
        read8(a: u32) -> u8;
        read16(a: u32) -> u16;
        write8(a: u32, v: u8);
        write16(a: u32, v: u16);
        read32(a: u32) -> u32;
        write32(a: u32, v: u32);
        fetch16(a: u32) -> u16;
        cycles(total: u64);
    }
}

pub type Native = fn(&mut Cpu, &mut DynBus);

pub struct Info {
    /// File name the player was recompiled from.
    pub file: &'static str,
    /// FNV-1a 64 of the whole file.
    pub hash: u64,
    /// Load address of each hunk's data (its segment header sits 8 bytes before).
    pub bases: &'static [u32],
    /// Byte ranges of the instructions that were translated. The native code is valid
    /// only while these bytes are as loaded.
    pub code: &'static [(u32, u32)],
    /// Native code for an address inside this player, if any.
    pub lookup: fn(u32) -> Option<Native>,
    /// The CPU the player was recompiled for. The interpreter must run what the native code
    /// does not cover with the same model (`Cpu::model`).
    pub cpu: m68k::Model,
}

/// FNV-1a 64, as the generator computes it.
pub fn hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(has_gen)]
include!("../gen/registry.rs");

#[cfg(not(has_gen))]
pub static PLAYERS: &[Info] = &[];

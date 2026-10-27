//! Readable function names (`--names`, config `names = true`).
//!
//! Every function keeps its address in its name (`blit_copy_04a2c6`), so names stay unique and
//! stable and the maps and logs still line up. The name comes from, best first:
//!
//! 1. a name the annotations, maps or names files give (always wins; an edited `names.txt` fed
//!    back with `--names FILE` is such a file);
//! 2. the vector the function is installed in (`move.l #f,$6c.w` -> `irq_level3`), or an RTE
//!    (`irq_handler`);
//! 3. the custom-chip and CIA registers it reads and writes (absolute `$DFFxxx`/`$BFExxx`/
//!    `$BFDxxx`, and `d16(An)`/`(An)`/`(An)+` where An provably holds a known address: constant
//!    loads in the function, values passed by every static caller, values returned by callees,
//!    registers saved and restored around calls; code outside the crate is assumed to keep an
//!    address register that holds a hardware base),
//!    by the dominant group: `blit_*`, `copper_*`, `audio_*`, `sprite_*`, `display_*`,
//!    `palette_*`, `input_*`, `keyboard_*`, `disk_*`, `irq_*`, `dma_*`, `timer_*`, plus
//!    `wait_vblank` (a VPOSR/VHPOSR poll loop), `wait_blit` (a DMACONR poll loop), `random`
//!    (a beam-counter read plus shuffling) and `hw_init`/`hw_loop` (three or more groups, without
//!    or with a loop of more than 64 bytes);
//! 4. the OS calls it makes, directly or through small library stubs (`jsr -n(a6)` with a6 known
//!    to hold a library base: ExecBase from `4.w`, other bases from the `OpenLibrary` results
//!    stored in globals): `file_read`, `alloc_mem`, `gfx_*`, `ui_*`, ...;
//! 5. structure: a leaf that only does register arithmetic -> `math_*`;
//!
//! and otherwise stays `f_xxxxxx`. The analysis only reads the decoded code; the names change no
//! generated statement (the emitters write the same code under another identifier, plus a
//! `pub use name as f_xxxxxx` alias so that code naming the old identifier still builds).
use crate::{fn_name, Func, Image};
use m68k::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

// ---------------------------------------------------------------------------------------------
// Hardware registers

/// Register groups, in tie-break order (the first wins on equal counts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Group {
    Blitter,
    Disk,
    Copper,
    Audio,
    Sprite,
    Display,
    Palette,
    Keyboard,
    Input,
    Irq,
    Dma,
    Timer,
    Beam,
    Serial,
    Collision,
    #[default]
    Misc,
}

impl Group {
    pub fn word(self) -> &'static str {
        match self {
            Group::Blitter => "blitter",
            Group::Disk => "disk",
            Group::Copper => "copper",
            Group::Audio => "audio",
            Group::Sprite => "sprites",
            Group::Display => "display",
            Group::Palette => "palette",
            Group::Keyboard => "keyboard",
            Group::Input => "input",
            Group::Irq => "interrupts",
            Group::Dma => "DMA",
            Group::Timer => "CIA timers",
            Group::Beam => "beam counter",
            Group::Serial => "serial",
            Group::Collision => "collisions",
            Group::Misc => "misc",
        }
    }
}

const CUSTOM: &[(u32, &str)] = &[
    (0x000, "BLTDDAT"), (0x002, "DMACONR"), (0x004, "VPOSR"), (0x006, "VHPOSR"), (0x008, "DSKDATR"),
    (0x00A, "JOY0DAT"), (0x00C, "JOY1DAT"), (0x00E, "CLXDAT"), (0x010, "ADKCONR"), (0x012, "POT0DAT"),
    (0x014, "POT1DAT"), (0x016, "POTGOR"), (0x018, "SERDATR"), (0x01A, "DSKBYTR"), (0x01C, "INTENAR"),
    (0x01E, "INTREQR"), (0x020, "DSKPTH"), (0x022, "DSKPTL"), (0x024, "DSKLEN"), (0x026, "DSKDAT"),
    (0x028, "REFPTR"), (0x02A, "VPOSW"), (0x02C, "VHPOSW"), (0x02E, "COPCON"), (0x030, "SERDAT"),
    (0x032, "SERPER"), (0x034, "POTGO"), (0x036, "JOYTEST"), (0x038, "STREQU"), (0x03A, "STRVBL"),
    (0x03C, "STRHOR"), (0x03E, "STRLONG"), (0x040, "BLTCON0"), (0x042, "BLTCON1"), (0x044, "BLTAFWM"),
    (0x046, "BLTALWM"), (0x048, "BLTCPTH"), (0x04A, "BLTCPTL"), (0x04C, "BLTBPTH"), (0x04E, "BLTBPTL"),
    (0x050, "BLTAPTH"), (0x052, "BLTAPTL"), (0x054, "BLTDPTH"), (0x056, "BLTDPTL"), (0x058, "BLTSIZE"),
    (0x05A, "BLTCON0L"), (0x05C, "BLTSIZV"), (0x05E, "BLTSIZH"), (0x060, "BLTCMOD"), (0x062, "BLTBMOD"),
    (0x064, "BLTAMOD"), (0x066, "BLTDMOD"), (0x070, "BLTCDAT"), (0x072, "BLTBDAT"), (0x074, "BLTADAT"),
    (0x078, "SPRHDAT"), (0x07A, "BPLHDAT"), (0x07C, "DENISEID"), (0x07E, "DSKSYNC"), (0x080, "COP1LCH"),
    (0x082, "COP1LCL"), (0x084, "COP2LCH"), (0x086, "COP2LCL"), (0x088, "COPJMP1"), (0x08A, "COPJMP2"),
    (0x08C, "COPINS"), (0x08E, "DIWSTRT"), (0x090, "DIWSTOP"), (0x092, "DDFSTRT"), (0x094, "DDFSTOP"),
    (0x096, "DMACON"), (0x098, "CLXCON"), (0x09A, "INTENA"), (0x09C, "INTREQ"), (0x09E, "ADKCON"),
    (0x100, "BPLCON0"), (0x102, "BPLCON1"), (0x104, "BPLCON2"), (0x106, "BPLCON3"), (0x108, "BPL1MOD"),
    (0x10A, "BPL2MOD"), (0x10C, "BPLCON4"), (0x10E, "CLXCON2"), (0x1C0, "HTOTAL"), (0x1C2, "HSSTOP"),
    (0x1C4, "HBSTRT"), (0x1C6, "HBSTOP"), (0x1C8, "VTOTAL"), (0x1CA, "VSSTOP"), (0x1CC, "VBSTRT"),
    (0x1CE, "VBSTOP"), (0x1D0, "SPRHSTRT"), (0x1D2, "SPRHSTOP"), (0x1D4, "BPLHSTRT"), (0x1D6, "BPLHSTOP"),
    (0x1D8, "HHPOSW"), (0x1DA, "HHPOSR"), (0x1DC, "BEAMCON0"), (0x1DE, "HSSTRT"), (0x1E0, "VSSTRT"),
    (0x1E2, "HCENTER"), (0x1E4, "DIWHIGH"), (0x1FC, "FMODE"), (0x1FE, "NOOP"),
];

/// The HRM name of the custom register at word offset `off` (0..$1FE) from $DFF000.
pub fn custom_name(off: u32) -> String {
    let off = off & 0x1FE;
    match off {
        0x0A0..=0x0DF => {
            let r = ["LCH", "LCL", "LEN", "PER", "VOL", "DAT", "_0C", "_0E"][((off & 0xF) / 2) as usize];
            format!("AUD{}{}", (off - 0xA0) / 0x10, r)
        }
        0x0E0..=0x0FF => format!("BPL{}PT{}", (off - 0xE0) / 4 + 1, if off & 2 == 0 { "H" } else { "L" }),
        0x110..=0x11F => format!("BPL{}DAT", (off - 0x110) / 2 + 1),
        0x120..=0x13F => format!("SPR{}PT{}", (off - 0x120) / 4, if off & 2 == 0 { "H" } else { "L" }),
        0x140..=0x17F => format!("SPR{}{}", (off - 0x140) / 8, ["POS", "CTL", "DATA", "DATB"][((off & 7) / 2) as usize]),
        0x180..=0x1BF => format!("COLOR{:02}", (off - 0x180) / 2),
        _ => CUSTOM.iter().find(|(o, _)| *o == off).map(|(_, n)| n.to_string()).unwrap_or_else(|| format!("CUSTOM_{:03X}", off)),
    }
}

/// The group of a custom register.
pub fn custom_group(off: u32) -> Group {
    match off & 0x1FE {
        0x000 | 0x040..=0x076 => Group::Blitter,
        0x002 | 0x096 => Group::Dma,
        0x004 | 0x006 | 0x02A | 0x02C | 0x1D8 | 0x1DA => Group::Beam,
        0x008 | 0x01A | 0x020..=0x026 | 0x07E => Group::Disk,
        0x00A | 0x00C | 0x012..=0x016 | 0x034 | 0x036 => Group::Input,
        0x00E | 0x098 | 0x10E => Group::Collision,
        0x018 | 0x030 | 0x032 => Group::Serial,
        0x01C | 0x01E | 0x09A | 0x09C => Group::Irq,
        0x02E | 0x080..=0x08C => Group::Copper,
        0x08E..=0x094 | 0x0E0..=0x0FF | 0x100..=0x10C | 0x110..=0x11F | 0x07A | 0x1C0..=0x1D6 | 0x1DC..=0x1FC => Group::Display,
        0x0A0..=0x0DF => Group::Audio,
        0x120..=0x17F | 0x078 => Group::Sprite,
        0x180..=0x1BF => Group::Palette,
        _ => Group::Misc,
    }
}

const CIA_REGS: [&str; 16] = ["PRA", "PRB", "DDRA", "DDRB", "TALO", "TAHI", "TBLO", "TBHI", "TODLO", "TODMID", "TODHI", "REG_B", "SDR", "ICR", "CRA", "CRB"];

/// A hardware register at `addr`: (key address, name, group). CIAA: odd addresses of
/// $BFE001-$BFEF01; CIAB: even addresses of $BFD000-$BFDF00; custom: $DFF000-$DFF1FF (word
/// registers, a byte access names the word).
pub fn hw_reg(addr: u32) -> Option<(u32, String, Group)> {
    let addr = addr & 0x00FF_FFFF;
    if (0xDFF000..0xDFF200).contains(&addr) {
        let off = (addr - 0xDFF000) & !1;
        return Some((0xDFF000 + off, custom_name(off), custom_group(off)));
    }
    let r = ((addr >> 8) & 15) as usize;
    if (0xBFE001..=0xBFEF01).contains(&addr) && addr & 0xFF == 1 {
        let g = match r {
            0 => Group::Input,
            1..=3 => Group::Misc,
            4..=11 | 14 | 15 => Group::Timer,
            12 => Group::Keyboard,
            _ => Group::Irq,
        };
        return Some((addr, format!("CIAA{}", CIA_REGS[r]), g));
    }
    if (0xBFD000..=0xBFDF00).contains(&addr) && addr & 0xFF == 0 {
        let g = match r {
            1 => Group::Disk,
            0 | 2 | 3 | 12 => Group::Misc,
            13 => Group::Irq,
            _ => Group::Timer,
        };
        return Some((addr, format!("CIAB{}", CIA_REGS[r]), g));
    }
    None
}

/// True for addresses worth showing as a known base register value.
fn is_hw_base(v: u32) -> bool {
    let v = v & 0x00FF_FFFF;
    (0xDFF000..0xDFF200).contains(&v) || (0xBFD000..0xBFF000).contains(&v)
}

// ---------------------------------------------------------------------------------------------
// OS libraries (LVO names from amiga-rt)

use crate::layout as lay;

const LIBS: &[(&str, &[&str])] = &[
    ("exec", lay::EXEC_LVOS),
    ("dos", lay::DOS_LVOS),
    ("graphics", lay::GRAPHICS_LVOS),
    ("intuition", lay::INTUITION_LVOS),
    ("layers", lay::LAYERS_LVOS),
    ("mathffp", lay::MATHFFP_LVOS),
    ("icon", lay::ICON_LVOS),
    ("diskfont", lay::DISKFONT_LVOS),
];
const EXEC: u8 = 0;
const LVO_OLDOPENLIBRARY: u32 = 408;
const LVO_OPENLIBRARY: u32 = 552;

/// `lib.Function` of a library call.
pub fn lvo_label(lib: u8, lvo: u32) -> String {
    let (n, t) = LIBS[lib as usize];
    format!("{}.{}", n, lay::lvo_name(t, lvo))
}

/// The library named by the C string at `a` ("dos.library" -> dos).
fn lib_named(img: &Image, a: u32) -> Option<u8> {
    let mut s = String::new();
    for k in 0..40 {
        let w = img.word((a + k) & !1)?;
        let b = if (a + k) & 1 == 0 { (w >> 8) as u8 } else { w as u8 };
        if b == 0 {
            break;
        }
        s.push((b as char).to_ascii_lowercase());
    }
    let n = s.strip_suffix(".library")?;
    LIBS.iter().position(|(l, _)| *l == n).map(|i| i as u8)
}

/// "AllocMem" -> "alloc_mem", "LoadRGB4" -> "load_rgb4", "SPMul" -> "sp_mul".
pub fn snake(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut o = String::new();
    for (i, &ch) in c.iter().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            let p = c[i - 1];
            let next_lower = c.get(i + 1).map_or(false, |n| n.is_ascii_lowercase());
            // a single capital at the start stays with the next word: WBenchToFront -> wbench_to_front
            let lone = i == 1 && p.is_ascii_uppercase();
            if !lone && (p.is_ascii_lowercase() || p.is_ascii_digit() || (p.is_ascii_uppercase() && next_lower)) {
                o.push('_');
            }
        }
        o.push(ch.to_ascii_lowercase());
    }
    o.replace("un_load", "unload").replace("dr_md", "drmd")
}

/// The name an OS call suggests, and its rank (lower = more telling).
fn os_word(lib: u8, lvo: u32) -> (u32, String) {
    let (ln, t) = LIBS[lib as usize];
    let f = lay::lvo_name(t, lvo);
    let s = snake(f);
    let r = |rank: u32, n: &str| (rank, n.to_string());
    match (ln, f) {
        ("dos", "Write") => r(0, "file_write"),
        ("dos", "Read") => r(1, "file_read"),
        ("dos", "LoadSeg") => r(2, "load_seg"),
        ("dos", "Execute") => r(2, "dos_execute"),
        ("dos", "Seek") => r(3, "file_seek"),
        ("dos", "DeleteFile") => r(3, "file_delete"),
        ("dos", "Rename") => r(3, "file_rename"),
        ("dos", "CreateDir") => r(3, "dir_create"),
        ("dos", "Examine") | ("dos", "ExNext") => r(4, "dir_scan"),
        ("dos", "Open") => r(5, "file_open"),
        ("dos", "Lock") => r(6, "file_lock"),
        ("dos", "UnLock") => r(6, "file_unlock"),
        ("dos", "DupLock") => r(6, "file_dup_lock"),
        ("dos", "CurrentDir") => r(6, "dir_current"),
        ("dos", "ParentDir") => r(6, "dir_parent"),
        ("dos", "Close") => r(7, "file_close"),
        ("dos", "CreateProc") => r(3, "task_create"),
        ("dos", "Delay") => r(8, "dos_delay"),
        ("dos", "IoErr") => r(20, "file_error"),
        ("dos", _) => (9, format!("dos_{}", s)),
        ("exec", "AllocMem") | ("exec", "AllocAbs") | ("exec", "AllocEntry") | ("exec", "AllocVec") | ("exec", "Allocate") => r(2, "alloc_mem"),
        ("exec", "FreeMem") | ("exec", "FreeEntry") | ("exec", "FreeVec") | ("exec", "Deallocate") => r(3, "free_mem"),
        ("exec", "OpenLibrary") | ("exec", "OldOpenLibrary") => r(4, "open_library"),
        ("exec", "CloseLibrary") => r(6, "close_library"),
        ("exec", "OpenDevice") => r(3, "open_device"),
        ("exec", "CloseDevice") => r(5, "close_device"),
        ("exec", "DoIO") | ("exec", "SendIO") | ("exec", "WaitIO") | ("exec", "CheckIO") | ("exec", "AbortIO") => (2, format!("io_{}", s.trim_end_matches("_io"))),
        ("exec", "AddIntServer") | ("exec", "SetIntVector") => r(2, "irq_install"),
        ("exec", "RemIntServer") => r(3, "irq_remove"),
        // FindTask(0) mostly fetches the caller's own task: a weak hint
        ("exec", "FindTask") => r(14, "task_find"),
        ("exec", "Alert") => r(12, "exec_alert"),
        ("exec", "AddTask") | ("exec", "RemTask") | ("exec", "SetTaskPri") => (5, format!("task_{}", s.replace("_task", "").replace("task_", ""))),
        ("exec", "PutMsg") | ("exec", "GetMsg") | ("exec", "ReplyMsg") | ("exec", "WaitPort") => (4, format!("msg_{}", s.replace("_msg", ""))),
        ("exec", "Wait") | ("exec", "Signal") | ("exec", "SetSignal") | ("exec", "AllocSignal") | ("exec", "FreeSignal") => (6, format!("signal_{}", s.replace("_signal", ""))),
        ("exec", "CopyMem") | ("exec", "CopyMemQuick") => r(4, "copy_mem"),
        ("exec", "Forbid") | ("exec", "Permit") | ("exec", "Disable") | ("exec", "Enable") => (15, format!("exec_{}", s)),
        ("exec", _) => (8, format!("exec_{}", s)),
        ("graphics", _) => (5, format!("gfx_{}", s)),
        ("intuition", _) => (5, format!("ui_{}", s)),
        ("layers", _) => (6, format!("layers_{}", s)),
        ("mathffp", _) => (7, format!("math_ffp_{}", s.trim_start_matches("sp_"))),
        ("icon", _) => (6, format!("icon_{}", s)),
        ("diskfont", _) => (6, format!("font_{}", s)),
        _ => (9, s),
    }
}

// ---------------------------------------------------------------------------------------------
// Register values

/// A known register value: a constant, or the base of a library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Val {
    C(u32),
    Lib(u8),
}

/// Known values of d0-d7/a0-a7 (a7 is never tracked), the last constant pushed on the stack, and
/// the registers saved by `movem.l regs,-(a7)` / `move.l Rn,-(a7)` (restored by the matching pop).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct St {
    pub r: [Option<Val>; 16],
    pub push: Option<u32>,
    pub saved: Vec<(u16, [Option<Val>; 16])>,
}

impl St {
    fn meet(&self, o: &St) -> St {
        let mut s = St::default();
        for i in 0..16 {
            if self.r[i] == o.r[i] {
                s.r[i] = self.r[i];
            }
        }
        if self.push == o.push {
            s.push = self.push;
        }
        if self.saved.len() == o.saved.len() && self.saved.iter().zip(&o.saved).all(|(a, b)| a.0 == b.0) {
            s.saved = self.saved.iter().zip(&o.saved).map(|(a, b)| {
                let mut v = [None; 16];
                for i in 0..16 {
                    if a.1[i] == b.1[i] {
                        v[i] = a.1[i];
                    }
                }
                (a.0, v)
            }).collect();
        }
        s
    }
    /// Pops the save of exactly the registers `mask`: restores them (true), or forgets the saves.
    fn restore(&mut self, mask: u16) -> bool {
        match self.saved.last() {
            Some((m, _)) if *m == mask => {
                let (_, v) = self.saved.pop().unwrap();
                for i in 0..15 {
                    if mask & (1 << i) != 0 {
                        self.r[i] = v[i];
                    }
                }
                true
            }
            _ => {
                self.saved.clear();
                false
            }
        }
    }
    fn c(&self, i: usize) -> Option<u32> {
        match self.r[i] {
            Some(Val::C(v)) => Some(v),
            _ => None,
        }
    }
}

fn sext(sz: Sz, v: u32) -> u32 {
    sz.sext(v)
}

fn step(sz: Sz, r: u8) -> u32 {
    if sz == Sz::B && r == 7 { 2 } else { sz.bytes() }
}

/// Registers written by an instruction (bit i: d0-d7 = 0-7, a0-a7 = 8-15), side effects included.
pub fn written(ins: &Instr) -> u16 {
    written_as(ins, true)
}

/// [`written`]; `side`: include the (An)+/-(An) address updates.
fn written_as(ins: &Instr, with_side: bool) -> u16 {
    let side = |ea: &Ea| -> u16 {
        match *ea {
            Ea::Post(r) | Ea::Pre(r) if with_side => 1 << (8 + r),
            _ => 0,
        }
    };
    let reg = |ea: &Ea| -> u16 {
        match *ea {
            Ea::D(n) => 1 << n,
            Ea::A(n) => 1 << (8 + n),
            _ => side(ea),
        }
    };
    match ins.op {
        Op::Alu { op, src, dst, .. } => side(&src) | if op == AluOp::Cmp { side(&dst) } else { reg(&dst) },
        Op::AddA { src, an, .. } | Op::SubA { src, an, .. } | Op::MoveA { src, an, .. } => side(&src) | 1 << (8 + an),
        Op::CmpA { src, .. } | Op::Chk { src, .. } => side(&src),
        Op::Bit { op, dst, .. } => if op == BitOp::Tst { side(&dst) } else { reg(&dst) },
        Op::Move { src, dst, .. } => side(&src) | reg(&dst),
        Op::Negx { ea, .. } | Op::Clr { ea, .. } | Op::Neg { ea, .. } | Op::Not { ea, .. } | Op::Nbcd { ea } | Op::Tas { ea } | Op::Scc { ea, .. } | Op::MoveFromSr { ea } => reg(&ea),
        Op::Tst { ea, .. } | Op::MoveToCcr { ea } | Op::MoveToSr { ea } | Op::ShiftMem { ea, .. } => side(&ea),
        Op::Lea { an, .. } => 1 << (8 + an),
        Op::Swap { dn } | Op::Ext { dn, .. } | Op::Moveq { dn, .. } | Op::Dbcc { dn, .. } | Op::ShiftReg { dn, .. } => 1 << dn,
        Op::Divu { src, dn } | Op::Divs { src, dn } | Op::Mulu { src, dn } | Op::Muls { src, dn } => side(&src) | 1 << dn,
        // MOVEM's address update is not applied by the operand code: always a result.
        Op::Movem { to_mem, mask, ea, .. } => (match ea { Ea::Post(r) | Ea::Pre(r) => 1 << (8 + r), _ => 0 }) | if to_mem { 0 } else { mask },
        Op::Movep { dn, to_mem, .. } => if to_mem { 0 } else { 1 << dn },
        Op::Exg { rx, ry } => 1 << rx | 1 << ry,
        Op::Link { an, .. } | Op::Unlk { an } => 1 << (8 + an),
        Op::MoveUsp { to_usp: false, an } => 1 << (8 + an),
        Op::Movec { to_ctrl: false, reg, .. } => 1 << reg,
        Op::Abcd { rx, ry, mem } | Op::Sbcd { rx, ry, mem } | Op::Addx { rx, ry, mem, .. } | Op::Subx { rx, ry, mem, .. } => {
            if !mem { 1 << rx } else if with_side { 1 << (8 + rx) | 1 << (8 + ry) } else { 0 }
        }
        Op::Cmpm { ax, ay, .. } => if with_side { 1 << (8 + ax) | 1 << (8 + ay) } else { 0 },
        _ => 0,
    }
}

/// Registers a function saves and restores (`movem.l regs,-(a7)` ... `movem.l (a7)+,regs`,
/// `move.l Rn,-(a7)` ... `move.l (a7)+,Rn`, `link An`).
fn saved(f: &Func) -> u16 {
    let (mut push, mut pop) = (0u16, 0u16);
    for i in f.insns.values() {
        match i.op {
            Op::Movem { to_mem: true, mask, ea: Ea::Pre(7), .. } => push |= mask.reverse_bits(),
            Op::Movem { to_mem: false, mask, ea: Ea::Post(7), .. } => pop |= mask,
            Op::Move { sz: Sz::L, src: Ea::D(n), dst: Ea::Pre(7) } => push |= 1 << n,
            Op::Move { sz: Sz::L, src: Ea::A(n), dst: Ea::Pre(7) } => push |= 1 << (8 + n),
            Op::Move { sz: Sz::L, src: Ea::Post(7), dst: Ea::D(n) } => pop |= 1 << n,
            Op::MoveA { sz: Sz::L, src: Ea::Post(7), an } => pop |= 1 << (8 + an),
            Op::Link { an, .. } => {
                push |= 1 << (8 + an);
                pop |= 1 << (8 + an);
            }
            _ => {}
        }
    }
    push & pop
}

/// OS calling convention: d0/d1/a0/a1 are scratch.
const OS_SCRATCH: u16 = 0b0000_0011_0000_0011;

// ---------------------------------------------------------------------------------------------
// Per-function facts

/// Accesses to one hardware register.
#[derive(Clone, Debug, Default)]
pub struct Acc {
    pub name: String,
    pub group: Group,
    pub r: u32,
    pub w: u32,
    /// Read inside a small loop (a poll).
    pub polled: bool,
    /// Constants written.
    pub values: BTreeSet<u32>,
}

#[derive(Clone, Debug, Default)]
struct Facts {
    acc: BTreeMap<u32, Acc>,
    /// (library, LVO) -> calls (direct `jsr/jmp -n(a6)`).
    os: BTreeMap<(u8, u32), u32>,
    /// Library calls through small stubs.
    stub_os: BTreeMap<(u8, u32), u32>,
    unknown_lib: u32,
    /// Static calls: (target, state at the call).
    calls: Vec<(u32, St)>,
    /// Meet of the states at the returns.
    ret: Option<St>,
    /// Library bases stored at constant addresses.
    stores: Vec<(u32, Val)>,
    /// (vector address, handler) of `move.l #handler,vector` stores.
    vectors: Vec<(u32, u32)>,
    rte: bool,
    shuffle: u32,
    mul: u32,
    div: u32,
    shift: u32,
    arith: u32,
    /// Memory operands not relative to a7.
    mem: u32,
    /// Accesses at an unknown offset from a hardware base (`d8(An,Xn)`).
    idx_hw: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Summary {
    /// Registers the function or its known callees may change.
    clobber: u16,
    /// Registers code outside the crate (or computed calls) may change, through this function.
    unknown: u16,
    ret: Option<St>,
    /// The function is a small wrapper around this one library call.
    os_stub: Option<(u8, u32)>,
}

struct Cx<'a> {
    img: &'a Image,
    funcs: &'a BTreeMap<u32, Func>,
    sums: &'a BTreeMap<u32, Summary>,
    globals: &'a BTreeMap<u32, Val>,
    base_reg: Option<(u8, u32)>,
}

/// One function's analysis: register values to a fixed point, then the recording pass.
struct Fa<'a, 'b> {
    cx: &'a Cx<'b>,
    f: &'a Func,
    loops: Vec<(u32, u32)>,
    facts: Facts,
}

impl Fa<'_, '_> {
    fn addr(&self, ea: &Ea, sz: Sz, s: &St) -> Option<u32> {
        let idx = |x: Xn| -> Option<u32> {
            let v = s.c(x.reg as usize)?;
            Some(if x.long { v } else { v as u16 as i16 as i32 as u32 })
        };
        match *ea {
            Ea::Ind(r) | Ea::Post(r) => s.c(8 + r as usize),
            Ea::Pre(r) => s.c(8 + r as usize).map(|a| a.wrapping_sub(step(sz, r))),
            Ea::Disp(r, d) => s.c(8 + r as usize).map(|a| a.wrapping_add(d as i32 as u32)),
            Ea::Idx(r, d, x) => Some(s.c(8 + r as usize)?.wrapping_add(d as i32 as u32).wrapping_add(idx(x)?)),
            Ea::AbsW(a) | Ea::AbsL(a) | Ea::PcDisp(a) => Some(a),
            Ea::PcIdx(b, x) => Some(b.wrapping_add(idx(x)?)),
            _ => None,
        }
    }
    /// Value of a source operand (before its side effects).
    fn value(&self, ea: &Ea, sz: Sz, s: &St) -> Option<Val> {
        let m = |v: Option<Val>| match v {
            Some(Val::C(c)) => Some(Val::C(c & sz.mask())),
            Some(Val::Lib(l)) if sz == Sz::L => Some(Val::Lib(l)),
            _ => None,
        };
        match *ea {
            Ea::D(n) => m(s.r[n as usize]),
            Ea::A(n) => m(s.r[8 + n as usize]),
            Ea::Imm(v) => Some(Val::C(v & sz.mask())),
            _ if sz == Sz::L => {
                let a = self.addr(ea, sz, s)?;
                if a == 4 {
                    Some(Val::Lib(EXEC))
                } else {
                    self.cx.globals.get(&a).copied()
                }
            }
            _ => None,
        }
    }
    fn side(&self, ea: &Ea, sz: Sz, s: &mut St) {
        match *ea {
            Ea::Post(r) if r != 7 => {
                let i = 8 + r as usize;
                s.r[i] = s.c(i).map(|a| Val::C(a.wrapping_add(step(sz, r))));
            }
            Ea::Pre(r) if r != 7 => {
                let i = 8 + r as usize;
                s.r[i] = s.c(i).map(|a| Val::C(a.wrapping_sub(step(sz, r))));
            }
            _ => {}
        }
    }
    fn in_loop(&self, pc: u32) -> bool {
        self.loops.iter().any(|&(lo, hi)| lo <= pc && pc <= hi && hi - lo <= 48)
    }
    fn record(&mut self, pc: u32, a: u32, sz: Sz, rd: bool, wr: bool, val: Option<u32>) {
        let parts: Vec<(u32, Option<u32>)> = if sz == Sz::L { vec![(a, val.map(|v| v >> 16)), (a.wrapping_add(2), val.map(|v| v & 0xFFFF))] } else { vec![(a, if sz == Sz::W { val } else { None })] };
        // An H/L pointer pair written with one long access is one register (COP1LC, BLTAPT, ...).
        if sz == Sz::L {
            if let (Some((k, n, g)), Some((_, n2, _))) = (hw_reg(a), hw_reg(a.wrapping_add(2))) {
                if n.ends_with('H') && n2.ends_with('L') && n[..n.len() - 1] == n2[..n2.len() - 1] {
                    let polled = rd && self.in_loop(pc);
                    let e = self.facts.acc.entry(k).or_insert_with(|| Acc { name: n[..n.len() - 1].to_string(), group: g, ..Default::default() });
                    e.r += rd as u32;
                    e.w += wr as u32;
                    e.polled |= polled;
                    return;
                }
            }
        }
        for (a, v) in parts {
            if let Some((k, n, g)) = hw_reg(a) {
                let polled = rd && self.in_loop(pc);
                let e = self.facts.acc.entry(k).or_insert_with(|| Acc { name: n, group: g, ..Default::default() });
                e.r += rd as u32;
                e.w += wr as u32;
                e.polled |= polled;
                if wr {
                    if let Some(v) = v {
                        e.values.insert(v);
                    }
                }
            }
        }
    }
    /// A memory operand: records hardware accesses, then applies the (An)+/-(An) side effect.
    fn operand(&mut self, pc: u32, ea: &Ea, sz: Sz, rd: bool, wr: bool, val: Option<u32>, s: &mut St) {
        if ea.is_mem() {
            let base7 = matches!(*ea, Ea::Ind(7) | Ea::Post(7) | Ea::Pre(7) | Ea::Disp(7, _) | Ea::Idx(7, _, _));
            if !base7 {
                self.facts.mem += 1;
            }
            match self.addr(ea, sz, s) {
                Some(a) => self.record(pc, a, sz, rd, wr, val),
                None => {
                    // d8(An,Xn) from a hardware base: the register at the displacement, indexed
                    // (e.g. `lea $dff0a0,a0 ; move.w d1,6(a0,d0.w)`: AUD0PER[idx], audio).
                    if let Ea::Idx(r, d, _) = *ea {
                        if let Some(b) = s.c(8 + r as usize).filter(|&b| is_hw_base(b)) {
                            self.facts.idx_hw += 1;
                            if let Some((k, n, g)) = hw_reg(b.wrapping_add(d as i32 as u32)) {
                                let polled = rd && self.in_loop(pc);
                                let e = self.facts.acc.entry(k | 0x0100_0000).or_insert_with(|| Acc { name: format!("{}[idx]", n), group: g, ..Default::default() });
                                e.r += rd as u32;
                                e.w += wr as u32;
                                e.polled |= polled;
                            }
                        }
                    }
                }
            }
        }
        self.side(ea, sz, s);
    }
    /// After code the crate does not have (a call outside it, a computed call, a trap): every
    /// register in `mask` is unknown, except the small-data base and address registers that hold
    /// a hardware base (code that changes those would break its callers, which reuse them).
    fn forget(&self, s: &mut St, mask: u16) {
        for i in 0..16 {
            let keep = self.cx.base_reg.map_or(false, |(r, _)| 8 + r as usize == i) || (i >= 8 && s.c(i).map_or(false, is_hw_base));
            if mask & (1 << i) != 0 && !keep {
                s.r[i] = None;
            }
        }
        s.push = None;
    }
    /// After a static call of `t`.
    fn call(&mut self, t: u32, s: &mut St) {
        self.facts.calls.push((t, s.clone()));
        match self.cx.sums.get(&t) {
            Some(sm) => {
                let a1 = s.c(9);
                let push = s.push;
                for i in 0..15 {
                    if sm.clobber & (1 << i) != 0 {
                        s.r[i] = sm.ret.as_ref().and_then(|r| r.r[i]);
                    }
                }
                self.forget(s, sm.unknown);
                if let Some((l, lvo)) = sm.os_stub {
                    *self.facts.stub_os.entry((l, lvo)).or_default() += 1;
                    if l == EXEC && (lvo == LVO_OPENLIBRARY || lvo == LVO_OLDOPENLIBRARY) {
                        s.r[0] = a1.or(push).and_then(|a| lib_named(self.cx.img, a)).map(Val::Lib);
                    }
                }
                s.push = None;
            }
            None => self.forget(s, 0x7FFF),
        }
    }
    /// After a library call `-lvo(a6)`.
    fn os_call(&mut self, lib: Option<u8>, lvo: u32, s: &mut St) {
        let a1 = s.c(9);
        match lib {
            Some(l) => *self.facts.os.entry((l, lvo)).or_default() += 1,
            None => self.facts.unknown_lib += 1,
        }
        for i in 0..16 {
            if OS_SCRATCH & (1 << i) != 0 {
                s.r[i] = None;
            }
        }
        if lib == Some(EXEC) && (lvo == LVO_OPENLIBRARY || lvo == LVO_OLDOPENLIBRARY) {
            s.r[0] = a1.and_then(|a| lib_named(self.cx.img, a)).map(Val::Lib);
        }
        s.push = None;
    }
    fn ret(&mut self, s: &St) {
        self.facts.ret = Some(match &self.facts.ret {
            Some(r) => r.meet(s),
            None => s.clone(),
        });
    }
    fn lib_of(&self, s: &St) -> Option<u8> {
        match s.r[14] {
            Some(Val::Lib(l)) => Some(l),
            _ => None,
        }
    }

    /// Transfer function of one instruction; returns the successors inside the function.
    fn step(&mut self, ins: &Instr, s: &mut St) -> Vec<u32> {
        let pc = ins.pc;
        let next = ins.next();
        let wr = written(ins);
        match ins.op {
            Op::Mulu { .. } | Op::Muls { .. } => {
                self.facts.mul += 1;
                self.facts.shuffle += 1;
            }
            Op::Divu { .. } | Op::Divs { .. } => self.facts.div += 1,
            Op::ShiftReg { kind, .. } => {
                self.facts.shift += 1;
                if matches!(kind, ShiftKind::Ro | ShiftKind::Rox) {
                    self.facts.shuffle += 1;
                }
            }
            Op::Swap { .. } | Op::Alu { op: AluOp::Eor, .. } => self.facts.shuffle += 1,
            _ => {}
        }
        if matches!(ins.op, Op::Alu { op: AluOp::Add | AluOp::Sub | AluOp::And | AluOp::Or | AluOp::Eor, .. } | Op::Addx { .. } | Op::Subx { .. } | Op::Neg { .. } | Op::Not { .. } | Op::Ext { .. }
            | Op::Mulu { .. } | Op::Muls { .. } | Op::Divu { .. } | Op::Divs { .. } | Op::ShiftReg { .. } | Op::Swap { .. } | Op::AddA { .. } | Op::SubA { .. }) {
            self.facts.arith += 1;
        }
        // Operands (in execution order), then the register results.
        let mut res: Option<(usize, Option<Val>)> = None;
        match ins.op {
            Op::Alu { op, sz, src, dst } => {
                let a = self.value(&src, sz, s);
                let b = self.value(&dst, sz, s);
                self.operand(pc, &src, sz, true, false, None, s);
                self.operand(pc, &dst, sz, true, op != AluOp::Cmp, None, s);
                if let (Ea::D(n), true) = (dst, op != AluOp::Cmp) {
                    let v = match (a, b) {
                        (Some(Val::C(x)), Some(Val::C(y))) => Some(match op {
                            AluOp::Add => y.wrapping_add(x),
                            AluOp::Sub => y.wrapping_sub(x),
                            AluOp::And => y & x,
                            AluOp::Or => y | x,
                            _ => y ^ x,
                        }),
                        _ => None,
                    };
                    res = Some((n as usize, merge(sz, v.map(Val::C), s.r[n as usize])));
                }
            }
            Op::AddA { sz, src, an } | Op::SubA { sz, src, an } => {
                let a = self.value(&src, sz, s);
                self.operand(pc, &src, sz, true, false, None, s);
                let b = s.c(8 + an as usize);
                let v = match (a, b) {
                    (Some(Val::C(x)), Some(y)) => {
                        let x = sext(sz, x);
                        Some(Val::C(if matches!(ins.op, Op::AddA { .. }) { y.wrapping_add(x) } else { y.wrapping_sub(x) }))
                    }
                    _ => None,
                };
                res = Some((8 + an as usize, v));
            }
            Op::CmpA { sz, src, .. } => self.operand(pc, &src, sz, true, false, None, s),
            Op::Bit { op, dst, .. } => {
                let sz = if matches!(dst, Ea::D(_)) { Sz::L } else { Sz::B };
                self.operand(pc, &dst, sz, true, op != BitOp::Tst, None, s);
            }
            Op::Move { sz, src, dst } => {
                let v = self.value(&src, sz, s);
                self.operand(pc, &src, sz, true, false, None, s);
                let cv = match v {
                    Some(Val::C(c)) => Some(c),
                    _ => None,
                };
                if dst.is_mem() && sz == Sz::L {
                    if let (Some(a), Some(v)) = (self.addr(&dst, sz, s), v) {
                        match v {
                            Val::Lib(_) => self.facts.stores.push((a, v)),
                            Val::C(h) if (0x64..=0xBC).contains(&a) && self.cx.funcs.contains_key(&h) => self.facts.vectors.push((a, h)),
                            _ => {}
                        }
                    }
                }
                if let Ea::Pre(7) = dst {
                    self.operand(pc, &dst, sz, false, true, cv, s);
                    s.push = if sz == Sz::L { cv } else { None };
                    match (sz, src) {
                        (Sz::L, Ea::D(n)) => s.saved.push((1 << n, s.r)),
                        (Sz::L, Ea::A(n)) => s.saved.push((1 << (8 + n), s.r)),
                        _ => {}
                    }
                    return vec![next];
                }
                if let (Sz::L, Ea::Post(7), Ea::D(n)) = (sz, src, dst) {
                    if s.restore(1 << n) {
                        return vec![next];
                    }
                }
                self.operand(pc, &dst, sz, false, true, cv, s);
                if let Ea::D(n) = dst {
                    res = Some((n as usize, merge(sz, v, s.r[n as usize])));
                }
            }
            Op::MoveA { sz: Sz::L, src: Ea::Post(7), an } if s.saved.last().map_or(false, |(m, _)| *m == 1 << (8 + an)) => {
                s.restore(1 << (8 + an));
                return vec![next];
            }
            Op::MoveA { sz, src, an } => {
                let v = self.value(&src, sz, s);
                self.operand(pc, &src, sz, true, false, None, s);
                res = Some((8 + an as usize, match v {
                    Some(Val::C(c)) => Some(Val::C(sext(sz, c))),
                    other => other,
                }));
            }
            Op::Moveq { data, dn } => res = Some((dn as usize, Some(Val::C(data as i32 as u32)))),
            Op::Clr { sz, ea } => {
                self.operand(pc, &ea, sz, false, true, Some(0), s);
                if let Ea::D(n) = ea {
                    res = Some((n as usize, merge(sz, Some(Val::C(0)), s.r[n as usize])));
                }
            }
            Op::Negx { sz, ea } | Op::Neg { sz, ea } | Op::Not { sz, ea } => self.operand(pc, &ea, sz, true, true, None, s),
            Op::Nbcd { ea } | Op::Tas { ea } => self.operand(pc, &ea, Sz::B, true, true, None, s),
            Op::ShiftMem { ea, .. } => self.operand(pc, &ea, Sz::W, true, true, None, s),
            Op::Tst { sz, ea } => self.operand(pc, &ea, sz, true, false, None, s),
            Op::Scc { ea, .. } => self.operand(pc, &ea, Sz::B, false, true, None, s),
            Op::MoveFromSr { ea } => self.operand(pc, &ea, Sz::W, false, true, None, s),
            Op::MoveToCcr { ea } | Op::MoveToSr { ea } => self.operand(pc, &ea, Sz::W, true, false, None, s),
            Op::Chk { src, .. } | Op::Divu { src, .. } | Op::Divs { src, .. } | Op::Mulu { src, .. } | Op::Muls { src, .. } => self.operand(pc, &src, Sz::W, true, false, None, s),
            Op::Lea { src, an } => res = Some((8 + an as usize, self.addr(&src, Sz::L, s).map(Val::C))),
            Op::Pea { src } => {
                let a = self.addr(&src, Sz::L, s);
                s.push = a;
                return vec![next];
            }
            Op::Exg { rx, ry } => {
                s.r.swap(rx as usize, ry as usize);
                if rx == 15 || ry == 15 {
                    s.r[15] = None;
                }
                return vec![next];
            }
            Op::Cmpm { sz, ax, ay } => {
                self.operand(pc, &Ea::Post(ay), sz, true, false, None, s);
                self.operand(pc, &Ea::Post(ax), sz, true, false, None, s);
                return vec![next];
            }
            Op::Abcd { rx, ry, mem: true } | Op::Sbcd { rx, ry, mem: true } => {
                self.operand(pc, &Ea::Pre(ry), Sz::B, true, false, None, s);
                self.operand(pc, &Ea::Pre(rx), Sz::B, true, true, None, s);
                return vec![next];
            }
            Op::Addx { sz, rx, ry, mem: true } | Op::Subx { sz, rx, ry, mem: true } => {
                self.operand(pc, &Ea::Pre(ry), sz, true, false, None, s);
                self.operand(pc, &Ea::Pre(rx), sz, true, true, None, s);
                return vec![next];
            }
            Op::Movep { sz, an, disp, to_mem, .. } => {
                if let Some(b) = s.c(8 + an as usize) {
                    let a = b.wrapping_add(disp as i32 as u32);
                    for k in 0..sz.bytes() {
                        self.record(pc, a.wrapping_add(2 * k), Sz::B, !to_mem, to_mem, None);
                    }
                }
            }
            Op::Movem { to_mem: true, mask, ea: Ea::Pre(7), .. } => {
                s.saved.push((mask.reverse_bits(), s.r));
                return vec![next];
            }
            Op::Movem { to_mem: false, mask, ea: Ea::Post(7), .. } => {
                if !s.restore(mask) {
                    for i in 0..15 {
                        if mask & (1 << i) != 0 {
                            s.r[i] = None;
                        }
                    }
                }
                return vec![next];
            }
            Op::Movem { ea, .. } => {
                if !matches!(ea, Ea::Pre(7) | Ea::Post(7) | Ea::Ind(7) | Ea::Disp(7, _)) {
                    self.facts.mem += 1;
                }
            }
            Op::Rte => {
                self.facts.rte = true;
                self.ret(s);
                return vec![];
            }
            Op::Rts | Op::Rtr => {
                self.ret(s);
                return vec![];
            }
            Op::Bcc { target, .. } | Op::Dbcc { target, .. } => {
                for i in 0..16 {
                    if wr & (1 << i) != 0 {
                        s.r[i] = None;
                    }
                }
                return vec![target, next];
            }
            Op::Bra { target } => return vec![target],
            Op::Bsr { target } => {
                self.call(target, s);
                return vec![next];
            }
            Op::Jsr { ea } => {
                match ea {
                    Ea::Disp(6, d) if d < 0 => {
                        let l = self.lib_of(s);
                        self.os_call(l, (-(d as i32)) as u32, s);
                    }
                    _ => match self.addr(&ea, Sz::L, s) {
                        Some(t) if self.cx.funcs.contains_key(&t) => self.call(t, s),
                        _ => self.forget(s, 0x7FFF),
                    },
                }
                return vec![next];
            }
            Op::Jmp { ea } => {
                match ea {
                    Ea::Disp(6, d) if d < 0 => {
                        let l = self.lib_of(s);
                        self.os_call(l, (-(d as i32)) as u32, s);
                        self.ret(s);
                    }
                    Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) if self.f.insns.contains_key(&t) => return vec![t],
                    _ => match self.addr(&ea, Sz::L, s) {
                        Some(t) if self.cx.funcs.contains_key(&t) && !self.f.insns.contains_key(&t) => {
                            self.call(t, s);
                            self.ret(s);
                        }
                        _ => {
                            if let Some((_, ts)) = self.f.jump_tables.iter().find(|(a, _)| *a == pc) {
                                return ts.clone();
                            }
                        }
                    },
                }
                return vec![];
            }
            Op::Trap { .. } => {
                self.forget(s, 0x7FFF);
                return vec![next];
            }
            _ if crate::emit::is_terminator(&ins.op) => return vec![],
            _ => {}
        }
        // Results not computed above are unknown ((An)+/-(An) updates were applied by `operand`).
        let direct = written_as(ins, false);
        for i in 0..16 {
            if direct & (1 << i) != 0 {
                s.r[i] = None;
            }
        }
        if let Some((r, v)) = res {
            s.r[r] = v;
        }
        s.r[15] = None;
        vec![next]
    }
}

/// A partial-size write into a register with a known value.
fn merge(sz: Sz, v: Option<Val>, old: Option<Val>) -> Option<Val> {
    match (sz, v, old) {
        (Sz::L, v, _) => v,
        (_, Some(Val::C(x)), Some(Val::C(o))) => Some(Val::C((o & !sz.mask()) | (x & sz.mask()))),
        _ => None,
    }
}

/// Backward branches: (target, branch address) pairs.
fn loops(f: &Func) -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    for i in f.insns.values() {
        let t = match i.op {
            Op::Bcc { target, .. } | Op::Dbcc { target, .. } | Op::Bra { target } => target,
            Op::Jmp { ea: Ea::AbsW(t) | Ea::AbsL(t) | Ea::PcDisp(t) } => t,
            _ => continue,
        };
        if t <= i.pc {
            v.push((t, i.pc));
        }
    }
    v
}

fn analyze_fn(cx: &Cx, f: &Func, entry: &St) -> Facts {
    let mut fa = Fa { cx, f, loops: loops(f), facts: Facts::default() };
    let mut states: BTreeMap<u32, St> = BTreeMap::new();
    states.insert(f.entry, entry.clone());
    let mut work = vec![f.entry];
    while let Some(p) = work.pop() {
        let Some(ins) = f.insns.get(&p) else { continue };
        let mut s = states[&p].clone();
        for t in fa.step(ins, &mut s) {
            if !f.insns.contains_key(&t) {
                continue;
            }
            let changed = match states.get(&t) {
                None => {
                    states.insert(t, s.clone());
                    true
                }
                Some(old) => {
                    let m = old.meet(&s);
                    if m != *old {
                        states.insert(t, m);
                        true
                    } else {
                        false
                    }
                }
            };
            if changed {
                work.push(t);
            }
        }
    }
    // Recording pass over the final states.
    fa.facts = Facts::default();
    for (p, st) in &states {
        let mut s = st.clone();
        fa.step(&f.insns[p], &mut s);
    }
    fa.facts
}

// ---------------------------------------------------------------------------------------------
// Program analysis

/// Everything the naming rules look at, per function.
pub struct Analysis {
    facts: BTreeMap<u32, Facts>,
    entry: BTreeMap<u32, St>,
    /// Handler -> vector addresses it is installed in.
    vectors: BTreeMap<u32, BTreeSet<u32>>,
    callers: BTreeMap<u32, BTreeSet<u32>>,
}

fn base_state(base_reg: Option<(u8, u32)>) -> St {
    let mut s = St::default();
    if let Some((r, v)) = base_reg {
        s.r[8 + r as usize] = Some(Val::C(v));
    }
    s
}

/// Runs the analysis over all functions: callee summaries (registers clobbered, values returned),
/// entry values from the static callers, and library bases stored in globals, to a fixed point
/// (at most 8 rounds).
pub fn analyze(funcs: &BTreeMap<u32, Func>, img: &Image, base_reg: Option<(u8, u32)>) -> Analysis {
    // Transitive clobber sets.
    let mut sums: BTreeMap<u32, Summary> = BTreeMap::new();
    let mut own: BTreeMap<u32, (u16, u16)> = BTreeMap::new();
    for (&e, f) in funcs {
        let (mut w, mut u) = (0u16, 0u16);
        for i in f.insns.values() {
            w |= written(i);
            match i.op {
                Op::Jsr { ea: Ea::Disp(6, d) } | Op::Jmp { ea: Ea::Disp(6, d) } if d < 0 => w |= OS_SCRATCH,
                Op::Jsr { ea } if !matches!(ea, Ea::AbsW(_) | Ea::AbsL(_) | Ea::PcDisp(_) | Ea::Disp(4, _)) => u |= 0x7FFF,
                Op::Trap { .. } => u |= 0x7FFF,
                _ => {}
            }
        }
        own.insert(e, (w, u));
    }
    let save: BTreeMap<u32, u16> = funcs.iter().map(|(&e, f)| (e, saved(f))).collect();
    let mut clob: BTreeMap<u32, (u16, u16)> = own.clone();
    loop {
        let mut changed = false;
        for (&e, f) in funcs {
            let (mut c, mut u) = own[&e];
            for t in &f.callees {
                match clob.get(t) {
                    Some(&(cc, cu)) => {
                        c |= cc;
                        u |= cu;
                    }
                    None => u |= 0x7FFF,
                }
            }
            let v = ((c & !save[&e]) & 0x7FFF, (u & !save[&e]) & 0x7FFF);
            if v != clob[&e] {
                clob.insert(e, v);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for &e in funcs.keys() {
        sums.insert(e, Summary { clobber: clob[&e].0, unknown: clob[&e].1, ..Default::default() });
    }
    let mut globals: BTreeMap<u32, Val> = BTreeMap::new();
    let mut entry: BTreeMap<u32, St> = BTreeMap::new();
    let mut facts: BTreeMap<u32, Facts> = BTreeMap::new();
    let base = base_state(base_reg);
    for _round in 0..8 {
        let cx = Cx { img, funcs, sums: &sums, globals: &globals, base_reg };
        facts = funcs.iter().map(|(&e, f)| (e, analyze_fn(&cx, f, entry.get(&e).unwrap_or(&base)))).collect();
        // Globals: library bases stored at constant addresses (dropped when stores disagree).
        let mut g: BTreeMap<u32, Option<Val>> = BTreeMap::new();
        for fx in facts.values() {
            for &(a, v) in &fx.stores {
                let e = g.entry(a).or_insert(Some(v));
                if *e != Some(v) {
                    *e = None;
                }
            }
        }
        let new_globals: BTreeMap<u32, Val> = g.into_iter().filter_map(|(a, v)| Some((a, v?))).collect();
        // Entry states: the meet over every static call site.
        let mut new_entry: BTreeMap<u32, St> = BTreeMap::new();
        for fx in facts.values() {
            for (t, s) in &fx.calls {
                let mut s = s.clone();
                s.push = None;
                s.saved.clear();
                s.r[15] = None;
                let m = match new_entry.get(t) {
                    Some(o) => o.meet(&s),
                    None => s,
                };
                new_entry.insert(*t, m);
            }
        }
        let mut new_sums = sums.clone();
        for (&e, f) in funcs {
            let fx = &facts[&e];
            let sm = new_sums.get_mut(&e).unwrap();
            sm.ret = fx.ret.clone();
            let mut all: Vec<(u8, u32)> = fx.os.keys().copied().collect();
            all.extend(fx.stub_os.keys().copied());
            all.sort();
            all.dedup();
            sm.os_stub = if all.len() == 1 && f.insns.len() <= 12 && fx.calls.iter().all(|(t, _)| sums.get(t).map_or(false, |s| s.os_stub.is_some())) { Some(all[0]) } else { None };
        }
        let done = new_globals == globals && new_entry == entry && new_sums == sums;
        globals = new_globals;
        entry = new_entry;
        sums = new_sums;
        if done {
            break;
        }
    }
    let mut vectors: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for fx in facts.values() {
        for &(v, h) in &fx.vectors {
            vectors.entry(h).or_default().insert(v);
        }
    }
    let mut callers: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for (&e, f) in funcs {
        for &c in &f.callees {
            callers.entry(c).or_default().insert(e);
        }
    }
    Analysis { facts, entry, vectors, callers }
}

impl Analysis {
    /// Hardware accesses of a function, by key address.
    pub fn accesses(&self, e: u32) -> Option<&BTreeMap<u32, Acc>> {
        self.facts.get(&e).map(|f| &f.acc)
    }
    /// OS calls of a function, direct and through stubs: "lib.Function" -> count.
    pub fn os_calls(&self, e: u32) -> BTreeMap<String, u32> {
        let mut m = BTreeMap::new();
        if let Some(f) = self.facts.get(&e) {
            for (&(l, lvo), &n) in f.os.iter().chain(f.stub_os.iter()) {
                *m.entry(lvo_label(l, lvo)).or_default() += n;
            }
        }
        m
    }
    /// The known value of register `r` (0-7 d, 8-15 a) at entry, from the static callers.
    pub fn entry_value(&self, e: u32, r: usize) -> Option<Val> {
        self.entry.get(&e).and_then(|s| s.r[r])
    }
}

// ---------------------------------------------------------------------------------------------
// Naming rules

/// A derived name: the stem (without the address), where it comes from, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derived {
    pub stem: String,
    pub source: &'static str,
    pub reason: String,
}

fn d(stem: impl Into<String>, source: &'static str, reason: impl Into<String>) -> Option<Derived> {
    Some(Derived { stem: stem.into(), source, reason: reason.into() })
}

fn has<'a>(acc: &'a BTreeMap<u32, Acc>, name: &str) -> Option<&'a Acc> {
    acc.values().find(|a| a.name == name)
}

fn hex_list(v: &BTreeSet<u32>) -> String {
    v.iter().map(|x| format!("${:04x}", x)).collect::<Vec<_>>().join("/")
}

/// The blitter operation a BLTCON0/BLTCON1 pair describes.
pub fn blit_kind(con0: Option<u32>, con1: Option<u32>) -> (&'static str, String) {
    if let Some(c1) = con1 {
        if c1 & 1 != 0 {
            return ("line", format!("BLTCON1 = ${:04x}: line mode", c1));
        }
        if c1 & 0x18 != 0 {
            return ("area_fill", format!("BLTCON1 = ${:04x}: area fill", c1));
        }
    }
    let Some(c0) = con0 else { return ("", String::new()) };
    let (use_, lf) = ((c0 >> 8) & 0xF, c0 & 0xFF);
    let chans: String = [(8, 'A'), (4, 'B'), (2, 'C'), (1, 'D')].iter().filter(|(b, _)| use_ & b != 0).map(|(_, c)| *c).collect();
    let k = match (use_, lf) {
        (1, 0x00) => "clear",
        (1, 0xFF) => "fill",
        (1, _) => "fill",
        (9, 0xF0) | (5, 0xCC) | (3, 0xAA) => "copy",
        (9, 0x0F) | (5, 0x33) | (3, 0x55) => "copy_inverted",
        (0xF, 0xCA) | (0xF, 0xE2) | (0xB, 0xCA) | (0xF, 0xEA) | (0xB, 0xEA) | (0xF, 0xFA) | (0xB, 0xFA) => "masked",
        (0xB, 0x0A) | (0xF, 0x0A) | (0xB, 0x1A) => "erase",
        (_, _) if use_ & 1 == 0 => "test",
        _ => "",
    };
    (k, format!("BLTCON0 = ${:04x}: channels {}, minterm ${:02x}", c0, chans, lf))
}

/// The hardware-derived name of a function (None: it touches no named register).
pub fn hw_name(acc: &BTreeMap<u32, Acc>, shuffle: u32, big_loop: bool) -> Option<Derived> {
    if acc.is_empty() {
        return None;
    }
    let src = "hardware";
    let polled = |n: &str| has(acc, n).map_or(false, |a| a.polled);
    let read = |n: &str| has(acc, n).map_or(false, |a| a.r > 0);
    let written = |n: &str| has(acc, n).map_or(false, |a| a.w > 0);
    let disk_present = acc.values().any(|a| a.group == Group::Disk);
    let kb = read("CIAASDR");
    // Scores per group (DMACON values are spread on the channels they switch).
    let mut score: BTreeMap<Group, u32> = BTreeMap::new();
    let mut eff: BTreeMap<String, Group> = BTreeMap::new();
    for a in acc.values() {
        let mut g = a.group;
        if a.name == "CIAAPRA" && disk_present {
            g = Group::Disk;
        }
        if kb && matches!(a.name.as_str(), "CIAACRA" | "CIAAICR") {
            g = Group::Keyboard;
        }
        if a.name == "DMACON" && !a.values.is_empty() {
            let mut gs = BTreeSet::new();
            for v in &a.values {
                for (bit, g) in [(0x000F, Group::Audio), (0x0010, Group::Disk), (0x0020, Group::Sprite), (0x0040, Group::Blitter), (0x0080, Group::Copper), (0x0100, Group::Display)] {
                    if v & bit != 0 {
                        gs.insert(g);
                    }
                }
            }
            if gs.len() == 1 {
                g = *gs.iter().next().unwrap();
            }
        }
        *score.entry(g).or_default() += a.r + a.w;
        eff.insert(a.name.clone(), g);
    }
    let others = |ok: &[Group]| score.iter().filter(|(g, _)| !ok.contains(g) && **g != Group::Misc).map(|(_, n)| n).sum::<u32>();
    // Poll loops and the random number generator first.
    if (polled("VPOSR") || polled("VHPOSR")) && others(&[Group::Beam, Group::Irq]) == 0 {
        let r = if polled("VPOSR") { "VPOSR" } else { "VHPOSR" };
        return d("wait_vblank", src, format!("{} polled in a loop", r));
    }
    if polled("DMACONR") && score.get(&Group::Blitter).copied().unwrap_or(0) == 0 && others(&[Group::Dma]) == 0 {
        return d("wait_blit", src, "DMACONR polled in a loop (blitter busy)");
    }
    let beam_read = acc.values().any(|a| matches!(a.name.as_str(), "VPOSR" | "VHPOSR") && a.r > 0 && !a.polled);
    let timer_read = acc.values().any(|a| a.group == Group::Timer && a.r > 0 && !a.polled && a.name.contains("CIA") && (a.name.contains("TA") || a.name.contains("TB") || a.name.contains("TOD")));
    if (beam_read || timer_read) && shuffle > 0 && acc.len() <= 3 {
        return d("random", src, format!("{} read and shuffled (mul/eor/rotate/swap)", if beam_read { "beam counter" } else { "CIA timer" }));
    }
    let real: Vec<Group> = score.iter().filter(|(g, n)| **g != Group::Misc && **n > 0).map(|(g, _)| *g).collect();
    if real.len() >= 3 {
        return d(if big_loop { "hw_loop" } else { "hw_init" }, src, format!("{} register groups: {}", real.len(), real.iter().map(|g| g.word()).collect::<Vec<_>>().join(", ")));
    }
    let (&g, _) = score.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))?;
    let names = |gr: Group| acc.values().filter(|a| eff[&a.name] == gr).map(|a| a.name.clone()).collect::<Vec<_>>().join(", ");
    let one = |n: &str| has(acc, n).and_then(|a| if a.values.len() == 1 { a.values.iter().next().copied() } else { None });
    let stem: String;
    let why: String;
    match g {
        Group::Blitter => {
            let (k, kw) = blit_kind(one("BLTCON0"), one("BLTCON1"));
            if !written("BLTSIZE") && !written("BLTSIZV") {
                stem = "blit_setup".into();
                why = format!("blitter registers written, no BLTSIZE{}", if kw.is_empty() { String::new() } else { format!("; {}", kw) });
            } else if k.is_empty() {
                stem = "blit".into();
                why = format!("BLTSIZE written{}", if kw.is_empty() { String::new() } else { format!("; {}", kw) });
            } else {
                stem = format!("blit_{}", k);
                why = format!("BLTSIZE written; {}", kw);
            }
        }
        Group::Copper => {
            if written("COP1LC") || written("COP2LC") || written("COP1LCH") || written("COP2LCH") {
                stem = "copper_set_list".into();
                why = "copper list pointer written".into();
            } else if written("COPJMP1") || written("COPJMP2") {
                stem = "copper_restart".into();
                why = "COPJMPx strobed".into();
            } else {
                stem = "copper".into();
                why = names(g);
            }
        }
        Group::Audio => {
            let chans: BTreeSet<char> = acc.values().filter(|a| a.group == Group::Audio && a.name.starts_with("AUD") && !a.name.ends_with("[idx]")).filter_map(|a| a.name.chars().nth(3)).collect();
            let indexed = acc.values().any(|a| a.group == Group::Audio && a.name.ends_with("[idx]"));
            let regs: BTreeSet<String> = acc.values().filter(|a| a.group == Group::Audio && a.w > 0 && a.name.starts_with("AUD")).map(|a| a.name[4..].trim_end_matches("[idx]").to_string()).collect();
            let verb = if regs.iter().any(|r| r.starts_with("LC") || r == "LEN") {
                "play"
            } else if regs.iter().any(|r| r == "PER") {
                "update"
            } else if regs.iter().any(|r| r == "VOL") {
                "volume"
            } else if regs.is_empty() && one("DMACON").map_or(false, |v| v & 0x8000 == 0) {
                "stop"
            } else if regs.is_empty() {
                "start"
            } else {
                "data"
            };
            let ch = if chans.len() == 1 && !indexed { format!("_ch{}", chans.iter().next().unwrap()) } else { String::new() };
            stem = format!("audio_{}{}", verb, ch);
            why = format!("audio registers: {}", names(g));
        }
        Group::Sprite => {
            stem = if acc.values().any(|a| a.group == g && a.w > 0 && a.name.contains("PT")) {
                "sprite_set_pointers".into()
            } else if acc.values().any(|a| a.group == g && a.w > 0 && (a.name.ends_with("POS") || a.name.ends_with("CTL"))) {
                "sprite_move".into()
            } else {
                "sprite".into()
            };
            why = format!("sprite registers: {}", names(g));
        }
        Group::Display => {
            stem = if acc.values().any(|a| a.group == g && a.w > 0 && a.name.starts_with("BPL") && a.name.contains("PT")) {
                "display_set_bitplanes".into()
            } else if acc.values().filter(|a| a.group == g && a.w > 0).all(|a| a.name == "BPLCON1") && written("BPLCON1") {
                "display_scroll".into()
            } else {
                "display_setup".into()
            };
            why = format!("display registers: {}", names(g));
        }
        Group::Palette => {
            let n = acc.values().filter(|a| a.group == g).count();
            stem = "palette_set".into();
            why = format!("{} colour register{} written", n, if n == 1 { "" } else { "s" });
        }
        Group::Keyboard => {
            stem = "keyboard_read".into();
            why = format!("CIAA serial data: {}", names(g));
        }
        Group::Input => {
            let j0 = read("JOY0DAT");
            let j1 = read("JOY1DAT");
            stem = match (j0, j1) {
                (true, true) => "input_ports".into(),
                (true, false) => "input_mouse".into(),
                (false, true) => "input_joystick".into(),
                _ if read("CIAAPRA") => "input_fire".into(),
                _ if read("POTGOR") => "input_buttons".into(),
                _ => "input".into(),
            };
            why = format!("input registers: {}", names(g));
        }
        Group::Disk => {
            let dsk = has(acc, "DSKLEN");
            stem = if dsk.map_or(false, |a| a.values.iter().any(|v| v & 0x4000 != 0)) {
                "disk_write".into()
            } else if dsk.map_or(false, |a| a.w > 0) {
                "disk_read".into()
            } else if polled("DSKBYTR") || polled("INTREQR") {
                "disk_wait".into()
            } else if written("CIABPRB") {
                "disk_drive".into()
            } else {
                "disk".into()
            };
            why = format!("disk registers: {}", acc.values().filter(|a| a.group == Group::Disk || (a.name == "CIAAPRA")).map(|a| a.name.clone()).collect::<Vec<_>>().join(", "));
        }
        Group::Irq => {
            let ena = has(acc, "INTENA");
            stem = if written("INTENA") && written("INTREQ") {
                "irq_setup".into()
            } else if written("INTREQ") {
                "irq_ack".into()
            } else if let Some(a) = ena.filter(|a| a.w > 0) {
                if !a.values.is_empty() && a.values.iter().all(|v| v & 0x8000 == 0) { "irq_disable".into() } else if !a.values.is_empty() && a.values.iter().all(|v| v & 0x8000 != 0) { "irq_enable".into() } else { "irq_setup".into() }
            } else if polled("INTREQR") {
                "irq_wait".into()
            } else {
                "irq".into()
            };
            why = format!("interrupt registers: {}", names(g));
        }
        Group::Dma => {
            let a = has(acc, "DMACON").filter(|a| a.w > 0);
            stem = match a {
                Some(a) if !a.values.is_empty() && a.values.iter().all(|v| v & 0x8000 != 0) => "dma_enable".into(),
                Some(a) if !a.values.is_empty() && a.values.iter().all(|v| v & 0x8000 == 0) => "dma_disable".into(),
                Some(_) => "dma_set".into(),
                None => "dma_status".into(),
            };
            why = format!("DMA registers: {}{}", names(g), a.map(|a| if a.values.is_empty() { String::new() } else { format!(" = {}", hex_list(&a.values)) }).unwrap_or_default());
        }
        Group::Timer => {
            let w = acc.values().any(|a| a.group == g && a.w > 0);
            let p = acc.values().any(|a| a.group == g && a.polled);
            stem = if p { "timer_wait".into() } else if w { "timer_setup".into() } else { "timer_read".into() };
            why = format!("CIA timer registers: {}", names(g));
        }
        Group::Beam => {
            stem = if acc.values().any(|a| a.group == g && a.w > 0) { "beam_set".into() } else { "beam_read".into() };
            why = format!("beam counter: {}", names(g));
        }
        Group::Serial => {
            stem = "serial_io".into();
            why = names(g);
        }
        Group::Collision => {
            stem = if read("CLXDAT") { "collision_check".into() } else { "collision_setup".into() };
            why = names(g);
        }
        Group::Misc => {
            stem = "hw".into();
            why = acc.values().map(|a| a.name.clone()).collect::<Vec<_>>().join(", ");
        }
    }
    d(stem, src, why)
}

/// The OS-derived name of a function.
/// `insns`: the function's size; in a function of more than 40 instructions, only the telling
/// calls (read, write, LoadSeg, AllocMem, DoIO, ...: rank 2 or better) name it, not a FreeMem or
/// Close among other work.
pub fn os_name(calls: &BTreeMap<(u8, u32), u32>, insns: usize) -> Option<Derived> {
    let best = calls.iter().map(|(&(l, lvo), &n)| (os_word(l, lvo), n, l, lvo)).min_by(|a, b| a.0 .0.cmp(&b.0 .0).then(b.1.cmp(&a.1)).then((a.2, a.3).cmp(&(b.2, b.3))))?;
    if insns > 40 && best.0 .0 > 2 {
        return None;
    }
    let list = calls.iter().map(|(&(l, lvo), &n)| format!("{} x{}", lvo_label(l, lvo), n)).collect::<Vec<_>>().join(", ");
    d(best.0 .1, "OS calls", list)
}

/// Structure rules: interrupt handlers and arithmetic leaves.
fn structure_name(fx: &Facts, vectors: Option<&BTreeSet<u32>>) -> Option<Derived> {
    if let Some(vs) = vectors {
        let v = *vs.iter().next().unwrap();
        return match v {
            0x64..=0x7C => d(format!("irq_level{}", (v - 0x60) / 4), "structure", format!("installed in the level {} autovector ${:02x}", (v - 0x60) / 4, v)),
            _ => d(format!("trap{}_handler", (v - 0x80) / 4), "structure", format!("installed in the TRAP #{} vector ${:02x}", (v - 0x80) / 4, v)),
        };
    }
    if fx.rte {
        return d("irq_handler", "structure", "returns with RTE");
    }
    None
}

fn math_name(f: &Func, fx: &Facts) -> Option<Derived> {
    if !f.callees.is_empty() || f.dyn_calls > 0 || !f.jump_tables.is_empty() || f.dyn_jumps > 0 || fx.mem > 0 || fx.arith == 0 || fx.rte {
        return None;
    }
    if f.insns.values().any(|i| matches!(i.op, Op::Trap { .. } | Op::Stop { .. } | Op::Reset | Op::MoveToSr { .. } | Op::MoveUsp { .. } | Op::Movec { .. } | Op::Illegal | Op::LineA | Op::LineF)) {
        return None;
    }
    let (stem, what) = if fx.div > 0 {
        ("math_div", "divides")
    } else if fx.mul > 0 {
        ("math_mul", "multiplies")
    } else if fx.shift > 0 {
        ("math_shift", "shifts")
    } else {
        ("math", "register arithmetic")
    };
    d(stem, "structure", format!("leaf, no memory access: {}", what))
}

/// A name given by an annotation or a names file: the first word of the description, if it is
/// an identifier and reads as a name (has an underscore, is the whole description, or is
/// followed by ':' or '(').
pub fn given_name(desc: &str) -> Option<String> {
    let d = desc.trim();
    let end = d.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(d.len());
    let w = &d[..end];
    if w.is_empty() || w.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let rest = d[end..].trim_start();
    if w.contains('_') || rest.is_empty() || rest.starts_with(':') || rest.starts_with('(') {
        Some(w.to_string())
    } else {
        None
    }
}

/// `stem` + `_xxxxxx` (unless it already ends with the address), as a Rust identifier.
pub fn with_addr(stem: &str, e: u32) -> String {
    let mut s: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert_str(0, "f_");
    }
    let suf = format!("_{:06x}", e);
    if s.ends_with(&suf) || s == fn_name(e) {
        s
    } else {
        s + &suf
    }
}

// ---------------------------------------------------------------------------------------------
// Result

/// The names of a crate's functions and their doc lines.
#[derive(Clone, Debug, Default)]
pub struct Naming {
    /// Entry -> Rust function name (every function of the crate).
    pub names: BTreeMap<u32, String>,
    /// Entry -> (source, reason) of the name.
    pub reasons: BTreeMap<u32, (String, String)>,
    /// Entry -> doc lines (without `///`).
    pub docs: BTreeMap<u32, Vec<String>>,
    /// The `names.txt` text.
    pub file: String,
}

impl Naming {
    pub fn name(&self, e: u32) -> String {
        self.names.get(&e).cloned().unwrap_or_else(|| fn_name(e))
    }
    /// The doc lines of a function.
    pub fn write_doc(&self, o: &mut String, e: u32) {
        for l in self.docs.get(&e).into_iter().flatten() {
            let _ = writeln!(o, "/// {}", l);
        }
    }
    /// `pub use name as f_xxxxxx;` after a renamed function, so that code naming the address
    /// identifier still builds.
    pub fn write_alias(&self, o: &mut String, e: u32) {
        let n = self.name(e);
        if n != fn_name(e) {
            let _ = writeln!(o, "#[doc(hidden)]\npub use self::{} as {};\n", n, fn_name(e));
        }
    }
    /// Number of functions with a name other than `f_xxxxxx`.
    pub fn derived(&self) -> usize {
        self.names.iter().filter(|(e, n)| **n != fn_name(**e)).count()
    }
    /// Count per source ("annotation", "hardware", ...).
    pub fn by_source(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for (s, _) in self.reasons.values() {
            *m.entry(s.clone()).or_default() += 1;
        }
        m
    }
}

/// What [`derive`] takes besides the code.
pub struct Given<'a> {
    /// Plain identifier names from the annotation sets (map names, RR notes).
    pub aliases: &'a BTreeMap<u32, String>,
    /// Entry -> description from names files, the config's `[names]` and the maps; the first
    /// word is used when it reads as a name (see [`given_name`]).
    pub descriptions: &'a BTreeMap<u32, String>,
    /// Address as written in `names.txt` (e.g. `0:2a6c6`).
    pub addr: &'a dyn Fn(u32) -> String,
    /// Crate name, for the `names.txt` header.
    pub crate_name: &'a str,
}

fn list_names(v: &BTreeSet<u32>, names: &BTreeMap<u32, String>, max: usize) -> String {
    let mut s: Vec<String> = v.iter().take(max).map(|a| names.get(a).cloned().unwrap_or_else(|| format!("${:06x}", a))).collect();
    if v.len() > max {
        s.push(format!("+{} more", v.len() - max));
    }
    s.join(", ")
}

/// Names every function of `funcs`.
pub fn derive(funcs: &BTreeMap<u32, Func>, img: &Image, base_reg: Option<(u8, u32)>, given: &Given) -> Naming {
    let an = analyze(funcs, img, base_reg);
    let mut nm = Naming::default();
    for (&e, f) in funcs {
        let fx = &an.facts[&e];
        let user = given.aliases.get(&e).cloned().or_else(|| given.descriptions.get(&e).and_then(|d| given_name(d)));
        let mut os = fx.os.clone();
        for (k, n) in &fx.stub_os {
            *os.entry(*k).or_default() += n;
        }
        let auto = structure_name(fx, an.vectors.get(&e)).or_else(|| hw_name(&fx.acc, fx.shuffle, loops(f).iter().any(|(lo, hi)| hi - lo > 64))).or_else(|| os_name(&os, f.insns.len())).or_else(|| math_name(f, fx));
        let der = match (&user, auto) {
            (Some(u), Some(a)) if with_addr(u, e) != with_addr(&a.stem, e) => d(u.clone(), "annotation", format!("given by the annotations or a names file; derived: {} ({}: {})", a.stem, a.source, a.reason)),
            (Some(u), _) => d(u.clone(), "annotation", "given by the annotations or a names file"),
            (None, a) => a,
        };
        let (name, src, why) = match der {
            Some(x) => (with_addr(&x.stem, e), x.source.to_string(), x.reason),
            None => (fn_name(e), "none".to_string(), "no rule matched".to_string()),
        };
        nm.names.insert(e, name);
        nm.reasons.insert(e, (src, why));
    }
    // Doc lines and names.txt.
    let mut file = String::new();
    let _ = writeln!(file, "# Function names of {} (m68k-recomp --names): <address> <name>  # <source>: <reason>", given.crate_name);
    let _ = writeln!(file, "# Edit a name and pass this file back with `--names FILE` (or `names_files` in the config):");
    let _ = writeln!(file, "# a name from a names file always wins over a derived one; `_<address>` is appended if missing.");
    for (&e, f) in funcs {
        let fx = &an.facts[&e];
        let (src, why) = &nm.reasons[&e];
        let mut doc = Vec::new();
        let name = &nm.names[&e];
        if src != "none" {
            doc.push(format!("Name: `{}` ({}: {}).", name, src, why));
        }
        if !fx.acc.is_empty() {
            let regs: Vec<String> = fx.acc.values().take(24).map(|a| {
                let mut s = a.name.clone();
                if a.r > 0 {
                    let _ = write!(s, " r{}", a.r);
                }
                if a.w > 0 {
                    let _ = write!(s, " w{}", a.w);
                }
                if a.polled {
                    s.push_str(" (polled)");
                }
                if !a.values.is_empty() && a.values.len() <= 3 {
                    let _ = write!(s, " = {}", hex_list(&a.values));
                }
                s
            }).collect();
            let more = if fx.acc.len() > 24 { format!(", +{} more", fx.acc.len() - 24) } else { String::new() };
            doc.push(format!("Hardware: {}{}.", regs.join(", "), more));
        }
        let osc = an.os_calls(e);
        if !osc.is_empty() {
            doc.push(format!("OS calls: {}.", osc.iter().map(|(k, n)| format!("{} x{}", k, n)).collect::<Vec<_>>().join(", ")));
        }
        let ent: Vec<String> = (0..15).filter_map(|r| {
            let rn = if r < 8 { format!("d{}", r) } else { format!("a{}", r - 8) };
            if base_reg.map_or(false, |(b, _)| 8 + b as usize == r) {
                return None;
            }
            match an.entry_value(e, r)? {
                Val::C(v) if is_hw_base(v) => Some(format!("{} = ${:06x}", rn, v)),
                Val::Lib(l) => Some(format!("{} = {}.library base", rn, LIBS[l as usize].0)),
                _ => None,
            }
        }).collect();
        if !ent.is_empty() {
            let n = an.callers.get(&e).map_or(0, |c| c.len());
            doc.push(format!("At entry (from every static call): {} ({} caller{}).", ent.join(", "), n, if n == 1 { "" } else { "s" }));
        }
        if let Some(c) = an.callers.get(&e) {
            doc.push(format!("Callers: {}.", list_names(c, &nm.names, 12)));
        }
        if !f.callees.is_empty() {
            doc.push(format!("Callees: {}.", list_names(&f.callees, &nm.names, 12)));
        }
        nm.docs.insert(e, doc);
        let _ = writeln!(file, "{} {}  # {}: {}", (given.addr)(e), name, src, why);
    }
    nm.file = file;
    nm
}

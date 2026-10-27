//! Compiler profiles: what the front end may assume about code produced by a given compiler.
//!
//! A profile answers four questions:
//!
//! * **Base register.** Does the program keep a constant small-data base in an address register,
//!   and where does it point (relative to the first data section)?
//! * **Data-section jump stubs.** Are calls made through `jmp abs.l` stubs in the data section,
//!   reached as `jsr d16(base)`? (Manx/Aztec C large-code model.)
//! * **Jump-table idioms.** Which `switch` / dispatch sequences ending in a computed `jmp`/`jsr`
//!   may be resolved statically.
//! * **Startup code.** How to find the base register's value in the program's own startup code.
//!
//! Profiles only ever add *proven* edges: a table is accepted only with a dominating unsigned
//! bound check on the (unscaled) index and entries that decode as code. Anything else stays a
//! computed transfer and runs through the dispatcher.
use crate::Image;
use m68k::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Profile {
    /// Hand-written assembler or an unknown compiler: no base register, generic table idioms.
    #[default]
    Generic,
    /// Manx/Aztec C: a4 = first data section + $7FFE, `jsr d16(a4)` into `jmp abs.l` stubs in the
    /// data section, `move.w tbl(pc,Dn.w),Dn ; jmp base(pc,Dn.w)` switches.
    Manx,
    /// SAS/C (Lattice) small-data model: a4 = `_LinkerDB` loaded by the startup code
    /// (`lea _LinkerDB,a4`); no data-section stubs; generic table idioms. Minimal, unverified on
    /// real SAS/C binaries (none available here).
    Sasc,
    /// `link a6` C compilers with absolute globals and amiga.lib-style stubs (Defender of the
    /// Crown's music modules, Sinbad; probably Green Hills C, UNSURE): no base register, no data
    /// stubs; the generic idioms plus this compiler's two `switch` forms:
    /// * dense: `[subi.l #lo,Dn ; blt dflt ;] cmpi.l #N,Dn ; bge dflt ; asl.l #k,Dn ;
    ///   jmp tbl(pc,Dn.l)` into N `bra` slots of 2^k bytes (a *signed* bound);
    /// * sparse: `moveq #8N,Dk ; L: subi.l #8,Dk ; bmi dflt ; cmp.l val(pc,Dk.l),Dn ; bne L ;
    ///   jmp tbl(pc,Dk.l)` over N `{long value, bra.w}` entries.
    GreenHills,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Profile> {
        Some(match s.to_ascii_lowercase().as_str() {
            "generic" | "asm" | "none" => Profile::Generic,
            "manx" | "aztec" => Profile::Manx,
            "sasc" | "sas" | "lattice" => Profile::Sasc,
            "greenhills" | "ghs" | "a6c" => Profile::GreenHills,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Profile::Generic => "generic",
            Profile::Manx => "manx",
            Profile::Sasc => "sasc",
            Profile::GreenHills => "greenhills",
        }
    }
    /// Base register and its default offset from the start of the first data section.
    pub fn base_reg(self) -> Option<(u8, u32)> {
        match self {
            Profile::Generic | Profile::GreenHills => None,
            Profile::Manx | Profile::Sasc => Some((4, 0x7FFE)),
        }
    }
    /// `jsr d16(base)` may target `jmp abs.l` stubs in the data section.
    pub fn data_stubs(self) -> bool {
        self == Profile::Manx
    }
    /// The value the startup code loads into the base register: a `lea abs.l,An` or
    /// `move.l #imm,An` within the first instructions from `entry`, following `bra`/`jmp abs` and
    /// looking one level into `bsr`/`jsr abs` callees (Manx: `jmp .begin ; .begin: bsr __geta4`,
    /// `__geta4: lea __H1_org+$7FFE,a4 ; rts`; SAS/C: `lea _LinkerDB,a4` inline in c.o).
    pub fn startup_base(self, img: &Image, entry: u32) -> Option<u32> {
        let (reg, _) = self.base_reg()?;
        let mut p = entry;
        for _ in 0..8 {
            match scan_base(img, reg, p, 48, true) {
                Step::Found(v) => return Some(v),
                Step::Call(t, ret) => match scan_base(img, reg, t, 8, false) {
                    Step::Found(v) => return Some(v),
                    _ => p = ret, // not a base-loading helper: continue after the call
                },
                Step::Fail => return None,
            }
        }
        None
    }
    /// Statically resolved targets of the computed transfer `jmp` (a `jmp` or `jsr`), given the
    /// instructions that fall through to it (`back`, oldest first).
    pub fn jump_table(self, img: &Image, back: &[Instr], jmp: &Instr) -> Option<Vec<u32>> {
        match self {
            Profile::Manx => manx_jump_table(img, back, jmp),
            Profile::Generic | Profile::Sasc => generic_jump_table(img, back, jmp, false),
            Profile::GreenHills => sparse_switch(img, back, jmp).or_else(|| generic_jump_table(img, back, jmp, true)),
        }
    }
}

enum Step {
    Found(u32),
    /// A call to `target` returning to `ret`.
    Call(u32, u32),
    Fail,
}

/// Straight-line scan (through `bra`/`jmp abs`) for the instruction loading base register `reg`.
fn scan_base(img: &Image, reg: u8, mut p: u32, n: usize, stop_at_calls: bool) -> Step {
    for _ in 0..n {
        let ins = match img.decode(p) {
            Some(i) => i,
            None => return Step::Fail,
        };
        match ins.op {
            Op::Lea { src: Ea::AbsL(x), an } if an == reg => return Step::Found(x),
            Op::MoveA { sz: Sz::L, src: Ea::Imm(x), an } if an == reg => return Step::Found(x),
            // Anything else that writes the base register: give up.
            Op::Lea { an, .. } | Op::MoveA { an, .. } if an == reg => return Step::Fail,
            Op::Bra { target } | Op::Jmp { ea: Ea::AbsL(target) | Ea::PcDisp(target) } => {
                p = target;
                continue;
            }
            Op::Bsr { target } | Op::Jsr { ea: Ea::AbsL(target) | Ea::PcDisp(target) } if stop_at_calls => {
                return Step::Call(target, ins.next());
            }
            _ if crate::emit::is_terminator(&ins.op) => return Step::Fail,
            _ => {}
        }
        p = ins.next();
    }
    Step::Fail
}

/// The Manx/Aztec C switch idiom ending at the computed jump `jmp`:
/// `cmp.l #N,Dn ; bcc default ; asl.l #1,Dn ; move.w tbl(pc,Dn.w),Dn ; jmp base(pc,Dn.w)`.
/// (Kept exactly as it was before profiles existed, so Manx output does not change.)
fn manx_jump_table(img: &Image, prev: &[Instr], jmp: &Instr) -> Option<Vec<u32>> {
    let (base, x) = match jmp.op {
        Op::Jmp { ea: Ea::PcIdx(base, x) } => (base, x),
        _ => return None,
    };
    let n = prev.len();
    if n < 4 {
        return None;
    }
    let tbl = match prev[n - 1].op {
        Op::Move { sz: Sz::W, src: Ea::PcIdx(t, x2), dst: Ea::D(r) } if x2 == x && r == x.reg => t,
        _ => return None,
    };
    let count = prev[..n - 1].iter().rev().take(4).find_map(|i| match i.op {
        Op::Alu { op: AluOp::Cmp, sz: Sz::L | Sz::W, src: Ea::Imm(c), dst: Ea::D(r) } if r == x.reg => Some(c),
        _ => None,
    })?;
    if count > 1024 {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..count {
        let w = img.word(tbl + 2 * i)? as i16 as i32 as u32;
        out.push(base.wrapping_add(w));
    }
    Some(out)
}

/// True unless `op` provably leaves data register `r` unchanged (conservative).
fn may_write_d(op: &Op, r: u8) -> bool {
    let wr = |ea: &Ea| matches!(ea, Ea::D(x) if *x == r);
    match op {
        Op::Lea { .. } | Op::Pea { .. } | Op::MoveA { .. } | Op::AddA { .. } | Op::SubA { .. } | Op::CmpA { .. } => false,
        Op::Tst { .. } | Op::Nop | Op::Bcc { .. } => false,
        Op::Alu { op: AluOp::Cmp, .. } => false,
        Op::Alu { dst, .. } => wr(dst),
        Op::Move { dst, .. } => wr(dst),
        Op::Moveq { dn, .. } => *dn == r,
        Op::Clr { ea, .. } | Op::Neg { ea, .. } | Op::Not { ea, .. } => wr(ea),
        _ => true,
    }
}

/// Walks back over `back` from its end, looking for the unsigned bound on data register `r`:
/// returns (number of valid unscaled index values, scale applied after the check).
/// Accepted: `cmp.{w,l} #N,Dr ; bhi default` (N+1 values) or `bcc/bhs default` (N values), then
/// only `add Dr,Dr` / `lsl|asl #k,Dr` on Dr and instructions that leave Dr alone.
/// `idx_long`: the index register is used as a long; a `cmp.w` bound then says nothing.
/// `signed`: also accept the signed upper bound `bge default` (N values) / `bgt` (N+1). The lower
/// end is not proven then (a negative index jumps before the table): only entries 0..N become
/// edges, and any other run-time target still goes through the dispatcher.
fn index_bound(back: &[Instr], r: u8, idx_long: bool, signed: bool) -> Option<(u32, u32)> {
    let mut scale = 1u32;
    let mut i = back.len();
    while i > 0 {
        i -= 1;
        let ins = &back[i];
        match ins.op {
            Op::Alu { op: AluOp::Add, sz: Sz::W | Sz::L, src: Ea::D(a), dst: Ea::D(b) } if a == r && b == r => scale *= 2,
            Op::ShiftReg { kind: ShiftKind::As | ShiftKind::Ls, left: true, sz: Sz::W | Sz::L, count: ShiftCount::Imm(k), dn } if dn == r => {
                scale = scale.checked_shl(k as u32)?;
            }
            Op::Bcc { cond, .. } => {
                if i == 0 {
                    return None;
                }
                let (n, sz) = match back[i - 1].op {
                    Op::Alu { op: AluOp::Cmp, sz: sz @ (Sz::W | Sz::L), src: Ea::Imm(n), dst: Ea::D(x) } if x == r => (n, sz),
                    _ => return None,
                };
                if sz == Sz::W && idx_long {
                    return None;
                }
                let count = match cond {
                    Cond::Hi => n.checked_add(1)?,
                    Cond::Cc => n,
                    Cond::Gt if signed && (n as i32) >= 0 => n.checked_add(1)?,
                    Cond::Ge if signed && (n as i32) >= 0 => n,
                    _ => return None,
                };
                if count == 0 || count > 1024 || scale > 16 {
                    return None;
                }
                return Some((count, scale));
            }
            ref op if may_write_d(op, r) => return None,
            _ => {}
        }
    }
    None
}

/// Generic idioms (hand-written assembler and most compilers):
///
/// 1. `move.w tbl(pc,Dk.w),Dn ; jmp base(pc,Dn.w)`: word offsets from `base` (Manx, GCC, SAS/C
///    `switch`, and `jmp tbl(pc,Dn.w)` with `base == tbl`);
/// 2. `jmp tbl(pc,Dk.w)` straight into a table of `bra`/`jmp` instructions, stride = index scale;
/// 3. `move.l tbl(pc,Dk.w),An ; jmp|jsr (An)` (or `lea tbl,Am ; move.l d(Am,Dk.w),An`): absolute
///    pointers; with relocation information (hunk files) every entry must be relocated.
///
/// Each needs the bound check described in [`index_bound`] on the unscaled index register.
fn generic_jump_table(img: &Image, back: &[Instr], jmp: &Instr, signed: bool) -> Option<Vec<u32>> {
    let n = back.len();
    let ea = match jmp.op {
        Op::Jmp { ea } | Op::Jsr { ea } => ea,
        _ => return None,
    };
    let is_jsr = matches!(jmp.op, Op::Jsr { .. });
    match ea {
        Ea::PcIdx(base, xj) if !is_jsr => {
            // Form 1: word offsets loaded into the jump's index register.
            if n >= 1 {
                if let Op::Move { sz: Sz::W, src: Ea::PcIdx(tbl, xs), dst: Ea::D(r) } = back[n - 1].op {
                    if r == xj.reg && !xj.long && xs.reg < 8 {
                        let (count, scale) = index_bound(&back[..n - 1], xs.reg, xs.long, signed)?;
                        if scale != 2 {
                            return None;
                        }
                        let mut out = Vec::new();
                        for i in 0..count {
                            let w = img.word(tbl + 2 * i)? as i16 as i32 as u32;
                            let t = base.wrapping_add(w);
                            if t & 1 != 0 || !img.is_code(t) {
                                return None;
                            }
                            out.push(t);
                        }
                        return Some(out);
                    }
                    return None;
                }
            }
            // Form 2: a table of branches.
            if xj.reg >= 8 {
                return None;
            }
            let (count, stride) = index_bound(back, xj.reg, xj.long, signed)?;
            if stride < 2 {
                return None;
            }
            let mut out = Vec::new();
            for i in 0..count {
                let t = base.wrapping_add(stride * i);
                let ins = img.decode(t)?;
                let ok = matches!(ins.op, Op::Bra { .. } | Op::Jmp { ea: Ea::AbsW(_) | Ea::AbsL(_) | Ea::PcDisp(_) }) && ins.len <= stride;
                if !ok || !img.is_code(t) {
                    return None;
                }
                out.push(t);
            }
            Some(out)
        }
        Ea::Ind(an) => {
            // Form 3: absolute pointers.
            if n < 1 {
                return None;
            }
            let (tbl, xs, rest) = match back[n - 1].op {
                Op::MoveA { sz: Sz::L, src: Ea::PcIdx(t, xs), an: a } if a == an => (t, xs, n - 1),
                Op::MoveA { sz: Sz::L, src: Ea::Idx(m, d, xs), an: a } if a == an => {
                    // `lea tbl,Am` somewhere before, with Am untouched in between.
                    let mut found = None;
                    for k in (0..n - 1).rev() {
                        match back[k].op {
                            Op::Lea { src: Ea::AbsL(t) | Ea::PcDisp(t), an: am } if am == m => {
                                found = Some(t.wrapping_add(d as i32 as u32));
                                break;
                            }
                            Op::Lea { an: am, .. } | Op::MoveA { an: am, .. } | Op::AddA { an: am, .. } | Op::SubA { an: am, .. } if am == m => return None,
                            Op::Lea { .. } | Op::MoveA { .. } | Op::AddA { .. } | Op::SubA { .. } | Op::CmpA { .. } => {}
                            Op::Alu { dst: Ea::D(_), .. } | Op::Moveq { .. } | Op::ShiftReg { .. } | Op::Bcc { .. } | Op::Tst { .. } | Op::Ext { .. } => {}
                            Op::Move { dst: Ea::D(_), src, .. } if !matches!(src, Ea::Post(r) | Ea::Pre(r) if r == m) => {}
                            _ => return None,
                        }
                    }
                    (found?, xs, n - 1)
                }
                _ => return None,
            };
            if xs.reg >= 8 {
                return None;
            }
            let (count, scale) = index_bound(&back[..rest], xs.reg, xs.long, signed)?;
            if scale != 4 {
                return None;
            }
            let mut out = Vec::new();
            for i in 0..count {
                let at = tbl + 4 * i;
                if img.has_relocs && !img.relocs.contains(&at) {
                    return None;
                }
                let t = img.long(at)?;
                if !crate::plausible_code(img, t) {
                    return None;
                }
                out.push(t);
            }
            Some(out)
        }
        _ => None,
    }
}

/// The Green Hills-style sparse `switch` (see [`Profile::GreenHills`]): a backwards search over
/// `{long value, bra.w}` entries with the counter `Dk` stepping by the entry size from `8N`.
/// Every entry's branch is an edge.
fn sparse_switch(img: &Image, back: &[Instr], jmp: &Instr) -> Option<Vec<u32>> {
    let (tbl, xj) = match jmp.op {
        Op::Jmp { ea: Ea::PcIdx(t, x) } if x.reg < 8 && x.long => (t, x),
        _ => return None,
    };
    let k = xj.reg;
    let n = back.len();
    if n < 5 {
        return None;
    }
    // ... cmp.l val(pc,Dk.l),Dn ; bne loop ; jmp tbl(pc,Dk.l)
    let loop_at = match back[n - 1].op {
        Op::Bcc { cond: Cond::Ne, target } => target,
        _ => return None,
    };
    match back[n - 2].op {
        Op::Alu { op: AluOp::Cmp, sz: Sz::L, src: Ea::PcIdx(_, xs), dst: Ea::D(_) } if xs.reg == k && xs.long => {}
        _ => return None,
    }
    // loop: subi.l #S,Dk ; bmi|blt default
    if !matches!(back[n - 3].op, Op::Bcc { cond: Cond::Mi | Cond::Lt, .. }) {
        return None;
    }
    let step = match back[n - 4].op {
        Op::Alu { op: AluOp::Sub, sz: Sz::L, src: Ea::Imm(s), dst: Ea::D(r) } if r == k && back[n - 4].pc == loop_at => s,
        _ => return None,
    };
    // initial count: moveq/move.l #8N,Dk falling into the loop
    let start = match back[n - 5].op {
        Op::Moveq { data, dn } if dn == k && data > 0 => data as u32,
        Op::Move { sz: Sz::L, src: Ea::Imm(v), dst: Ea::D(r) } if r == k => v,
        _ => return None,
    };
    if step < 6 || step > 16 || start % step != 0 || start / step > 512 {
        return None;
    }
    let mut out = Vec::new();
    for i in 0..start / step {
        let t = tbl.wrapping_add(step * i);
        let ins = img.decode(t)?;
        if !matches!(ins.op, Op::Bra { .. } | Op::Jmp { ea: Ea::AbsL(_) | Ea::PcDisp(_) }) || !img.is_code(t) {
            return None;
        }
        out.push(t);
    }
    Some(out)
}

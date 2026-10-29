//! A small test-only encoder of 68000 instructions from their descriptions (sizes,
//! registers, addresses), so that the tests of the replayers that read code carry no
//! literal instruction words. Every form is checked by decoding it with `m68kdec`.

const ABS_L: u16 = 7 << 3 | 1;
const IMM: u16 = 7 << 3 | 4;
/// Size field of the move group: byte 1, long 2.
const MB: u16 = 1;
const ML: u16 = 2;

fn l(v: u32) -> [u16; 2] {
    [(v >> 16) as u16, v as u16]
}

/// `andi.b #imm,abs.l`.
pub fn andi_b_abs(imm: u8, addr: u32) -> Vec<u16> {
    [&[1 << 9 | ABS_L, imm as u16][..], &l(addr)].concat()
}
/// `bne.s` with a displacement.
pub fn bne_s(disp: i8) -> Vec<u16> {
    vec![6 << 12 | 6 << 8 | disp as u8 as u16]
}
pub fn rts() -> Vec<u16> {
    vec![0x4E << 8 | 0x75]
}
pub fn nop() -> Vec<u16> {
    vec![0x4E << 8 | 0x71]
}
/// `moveq #imm,dn`.
pub fn moveq(n: u16, imm: u8) -> Vec<u16> {
    vec![7 << 12 | n << 9 | imm as u16]
}
/// `move.b abs.l,dn`.
pub fn move_b_abs_d(n: u16, addr: u32) -> Vec<u16> {
    [&[MB << 12 | n << 9 | ABS_L][..], &l(addr)].concat()
}
/// `tst.b abs.l`.
pub fn tst_b_abs(addr: u32) -> Vec<u16> {
    [&[0x4A << 8 | ABS_L][..], &l(addr)].concat()
}
/// `move.b dm,dn`.
pub fn move_b_dd(m: u16, n: u16) -> Vec<u16> {
    vec![MB << 12 | n << 9 | m]
}
/// `asl.b #count,dn`.
pub fn asl_b_imm(count: u16, n: u16) -> Vec<u16> {
    vec![0xE << 12 | count << 9 | 1 << 8 | n]
}
/// `lsr.b #count,dn`.
pub fn lsr_b_imm(count: u16, n: u16) -> Vec<u16> {
    vec![0xE << 12 | count << 9 | 1 << 3 | n]
}
/// `cmpi.b #imm,dn`.
pub fn cmpi_b_d(imm: u8, n: u16) -> Vec<u16> {
    vec![6 << 9 | n, imm as u16]
}
/// `cmp.b #imm,dn`.
pub fn cmp_b_imm_d(imm: u8, n: u16) -> Vec<u16> {
    vec![0xB << 12 | n << 9 | IMM, imm as u16]
}
/// `movea.l #imm,an`.
pub fn movea_l_imm(n: u16, imm: u32) -> Vec<u16> {
    [&[ML << 12 | n << 9 | 1 << 6 | IMM][..], &l(imm)].concat()
}
/// `lea d16(pc),an` (the displacement counts from the extension word).
pub fn lea_pc(n: u16, disp: i16) -> Vec<u16> {
    vec![0x4 << 12 | n << 9 | 7 << 6 | 7 << 3 | 2, disp as u16]
}
/// `lea abs.l,an`.
pub fn lea_abs(n: u16, addr: u32) -> Vec<u16> {
    [&[0x4 << 12 | n << 9 | 7 << 6 | ABS_L][..], &l(addr)].concat()
}
/// `move.l (d8,an,d0.w),abs.l`.
pub fn move_l_idx_abs(n: u16, d8: u8, addr: u32) -> Vec<u16> {
    [
        &[ML << 12 | 1 << 9 | 7 << 6 | 6 << 3 | n, d8 as u16][..],
        &l(addr),
    ]
    .concat()
}
/// `move.b #imm,abs.l`.
pub fn move_b_imm_abs(imm: u8, addr: u32) -> Vec<u16> {
    [
        &[MB << 12 | 1 << 9 | 7 << 6 | IMM, imm as u16][..],
        &l(addr),
    ]
    .concat()
}
pub const BRA: u16 = 0;
pub const BSR: u16 = 1;
pub const BNE: u16 = 6;
pub const BEQ: u16 = 7;
pub const BCS: u16 = 5;
/// Sizes of the plain forms: byte, word, long.
pub const B: u16 = 0;
pub const W: u16 = 1;
pub const L: u16 = 2;

fn mv_size(s: u16) -> u16 {
    [MB, 3, ML][s as usize]
}
/// `bcc.w` (cond: `BRA`, `BSR`, `BNE`, `BEQ`, ...) with a displacement.
pub fn bcc_w(cond: u16, disp: i16) -> Vec<u16> {
    vec![6 << 12 | cond << 8, disp as u16]
}
/// `bra.s`.
pub fn bra_s(disp: i8) -> Vec<u16> {
    vec![6 << 12 | disp as u8 as u16]
}
/// `beq.s`.
pub fn beq_s(disp: i8) -> Vec<u16> {
    vec![6 << 12 | 7 << 8 | disp as u8 as u16]
}
/// `tst.<size> abs.l`.
pub fn tst_abs(size: u16, addr: u32) -> Vec<u16> {
    [&[0x4A << 8 | size << 6 | ABS_L][..], &l(addr)].concat()
}
/// `tst.w dn`.
pub fn tst_w_d(n: u16) -> Vec<u16> {
    vec![0x4A << 8 | 1 << 6 | n]
}
/// `cmpi.w #imm,dn`.
pub fn cmpi_w_d(imm: u16, n: u16) -> Vec<u16> {
    vec![6 << 9 | 1 << 6 | n, imm]
}
/// `subi.b #imm,abs.l`.
pub fn subi_b_abs(imm: u8, addr: u32) -> Vec<u16> {
    [&[1 << 10 | ABS_L, imm as u16][..], &l(addr)].concat()
}
/// `subi.b #imm,dn`.
pub fn subi_b_d(imm: u8, n: u16) -> Vec<u16> {
    vec![1 << 10 | n, imm as u16]
}
/// `movea.l abs.l,an`.
pub fn movea_l_abs(n: u16, addr: u32) -> Vec<u16> {
    [&[ML << 12 | n << 9 | 1 << 6 | ABS_L][..], &l(addr)].concat()
}
/// `move.b (an)+,dm`.
pub fn move_b_post_d(n: u16, m: u16) -> Vec<u16> {
    vec![MB << 12 | m << 9 | 3 << 3 | n]
}
/// `move.b dn,abs.l`.
pub fn move_b_d_abs(n: u16, addr: u32) -> Vec<u16> {
    [&[MB << 12 | 1 << 9 | 7 << 6 | n][..], &l(addr)].concat()
}
/// `asl.w #count,dn`.
pub fn asl_w_imm(count: u16, n: u16) -> Vec<u16> {
    vec![0xE << 12 | count << 9 | 1 << 8 | 1 << 6 | n]
}
/// `adda.l dn,am`.
pub fn adda_l_d(n: u16, m: u16) -> Vec<u16> {
    vec![0xD << 12 | m << 9 | 7 << 6 | n]
}
/// `jmp (an)`.
pub fn jmp_ind(n: u16) -> Vec<u16> {
    vec![0x4E << 8 | 3 << 6 | 2 << 3 | n]
}
/// `move.l an,abs.l`.
pub fn move_l_a_abs(n: u16, addr: u32) -> Vec<u16> {
    [&[ML << 12 | 1 << 9 | 7 << 6 | 1 << 3 | n][..], &l(addr)].concat()
}
/// `move.l #imm,abs.l`.
pub fn move_l_imm_abs(imm: u32, addr: u32) -> Vec<u16> {
    [&[ML << 12 | 1 << 9 | 7 << 6 | IMM][..], &l(imm), &l(addr)].concat()
}
/// `move.w #imm,abs.l`.
pub fn move_w_imm_abs(imm: u16, addr: u32) -> Vec<u16> {
    [&[3 << 12 | 1 << 9 | 7 << 6 | IMM, imm][..], &l(addr)].concat()
}
/// `move.w #imm,d16(an)`.
pub fn move_w_imm_disp(imm: u16, n: u16, disp: i16) -> Vec<u16> {
    vec![3 << 12 | n << 9 | 5 << 6 | IMM, imm, disp as u16]
}
/// `move.w dn,abs.l`.
pub fn move_w_d_abs(n: u16, addr: u32) -> Vec<u16> {
    [&[3 << 12 | 1 << 9 | 7 << 6 | n][..], &l(addr)].concat()
}
/// `add.w dn,dm`.
pub fn add_w_dd(n: u16, m: u16) -> Vec<u16> {
    vec![0xD << 12 | m << 9 | 1 << 6 | n]
}
/// `move.w 0(an,dm.w),dk`.
pub fn move_w_idx_d(n: u16, m: u16, k: u16) -> Vec<u16> {
    vec![3 << 12 | k << 9 | 6 << 3 | n, m << 12]
}
/// `movem.l d0-d7/a0-a6,-(sp)`.
pub fn movem_l_save_all() -> Vec<u16> {
    vec![0x48 << 8 | 0x80 | 1 << 6 | 4 << 3 | 7, !1]
}
/// `move.w #imm,dn`.
pub fn move_w_imm_d(imm: u16, n: u16) -> Vec<u16> {
    vec![3 << 12 | n << 9 | IMM, imm]
}
/// `lea d16(pc),an` placed at `at`, for the target `target`.
pub fn lea_pc_at(n: u16, at: usize, target: usize) -> Vec<u16> {
    vec![0x4 << 12 | n << 9 | 7 << 6 | 7 << 3 | 2, (target as i64 - (at as i64 + 2)) as i16 as u16]
}
/// `move.b (an,dm.w),d16(ar)`.
pub fn move_b_idx_disp(an: u16, dm: u16, ar: u16, disp: i16) -> Vec<u16> {
    vec![MB << 12 | ar << 9 | 5 << 6 | 6 << 3 | an, dm << 12, disp as u16]
}
/// `move.l (an,dm.w),d16(ar)`.
pub fn move_l_idx_disp(an: u16, dm: u16, ar: u16, disp: i16) -> Vec<u16> {
    vec![ML << 12 | ar << 9 | 5 << 6 | 6 << 3 | an, dm << 12, disp as u16]
}
/// `mulu.w #imm,dn`.
pub fn mulu_imm_d(imm: u16, n: u16) -> Vec<u16> {
    vec![0xC << 12 | n << 9 | 3 << 6 | IMM, imm]
}
/// `adda.l d16(an),am`.
pub fn adda_l_disp(an: u16, disp: i16, am: u16) -> Vec<u16> {
    vec![0xD << 12 | am << 9 | 7 << 6 | 5 << 3 | an, disp as u16]
}
/// `move.w d16(an),dn`.
pub fn move_w_disp_d(an: u16, disp: i16, n: u16) -> Vec<u16> {
    vec![3 << 12 | n << 9 | 5 << 3 | an, disp as u16]
}
/// `move.l d16(an),dn`.
pub fn move_l_disp_d(an: u16, disp: i16, n: u16) -> Vec<u16> {
    vec![ML << 12 | n << 9 | 5 << 3 | an, disp as u16]
}
/// `move.l d16(pc),dn` placed at `at`, for the target `target`.
pub fn move_l_pc_d(n: u16, at: usize, target: usize) -> Vec<u16> {
    vec![ML << 12 | n << 9 | 7 << 3 | 2, (target as i64 - (at as i64 + 2)) as i16 as u16]
}
/// `cmpi.b #imm,(an)`.
pub fn cmpi_b_ind(imm: u8, n: u16) -> Vec<u16> {
    vec![6 << 9 | 2 << 3 | n, imm as u16]
}
/// `move.b d8(pc,dm.w),d16(ar)` placed at `at`, for the table `target`.
pub fn move_b_pcidx_disp(dm: u16, at: usize, target: usize, ar: u16, disp: i16) -> Vec<u16> {
    vec![MB << 12 | ar << 9 | 5 << 6 | 7 << 3 | 3, dm << 12 | ((target as i64 - (at as i64 + 2)) as i8 as u8 as u16), disp as u16]
}
/// `add.w d8(pc,dm.w),dn` placed at `at`, for the table `target`.
pub fn add_w_pcidx_d(dm: u16, at: usize, target: usize, n: u16) -> Vec<u16> {
    vec![0xD << 12 | n << 9 | 1 << 6 | 7 << 3 | 3, dm << 12 | ((target as i64 - (at as i64 + 2)) as i8 as u8 as u16)]
}
/// `cmpi.w #imm,(an)`.
pub fn cmpi_w_ind(imm: u16, n: u16) -> Vec<u16> {
    vec![6 << 9 | 1 << 6 | 2 << 3 | n, imm]
}
/// `tst.b dn`.
pub fn tst_b_d(n: u16) -> Vec<u16> {
    vec![0x4A << 8 | n]
}
/// `move.b #imm,dn`.
pub fn move_b_imm_d(imm: u8, n: u16) -> Vec<u16> {
    vec![MB << 12 | n << 9 | IMM, imm as u16]
}
/// `btst #bit,d16(an)`.
pub fn btst_imm_disp(bit: u8, n: u16, disp: i16) -> Vec<u16> {
    vec![4 << 9 | 5 << 3 | n, bit as u16, disp as u16]
}
/// `lsr.w #count,dn`.
pub fn lsr_w_imm(count: u16, n: u16) -> Vec<u16> {
    vec![0xE << 12 | count << 9 | 1 << 6 | 1 << 3 | n]
}
/// `addi.b #imm,dn`.
pub fn addi_b_d(imm: u8, n: u16) -> Vec<u16> {
    vec![3 << 9 | n, imm as u16]
}
/// `move.l #imm,d16(an)`.
pub fn move_l_imm_disp(imm: u32, n: u16, disp: i16) -> Vec<u16> {
    [&[ML << 12 | n << 9 | 5 << 6 | IMM][..], &l(imm), &[disp as u16]].concat()
}
/// `rte`.
pub fn rte() -> Vec<u16> {
    vec![0x4E << 8 | 0x73]
}
/// `move.<size> #imm,abs.l` for word and long are `move_w_imm_abs`, `move_l_imm_abs`.
pub fn size_code(s: u16) -> u16 {
    mv_size(s)
}

/// An operand of the general forms below.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    D(u16),
    A(u16),
    /// `(an)`.
    Ind(u16),
    /// `(an)+`.
    Post(u16),
    /// `d16(an)`.
    Disp(u16, i16),
    /// `d16(pc)`, given the target address (negative for a place before the module).
    Pc(i64),
    /// `abs.l`.
    Abs(u32),
    Imm(u32),
}

fn ea_bits(o: Op) -> u16 {
    match o {
        Op::D(n) => n,
        Op::A(n) => 1 << 3 | n,
        Op::Ind(n) => 2 << 3 | n,
        Op::Post(n) => 3 << 3 | n,
        Op::Disp(n, _) => 5 << 3 | n,
        Op::Pc(_) => 7 << 3 | 2,
        Op::Abs(_) => ABS_L,
        Op::Imm(_) => IMM,
    }
}

/// The extension words of an operand whose first extension word is at `at`.
fn ea_ext(o: Op, at: usize, size: u16) -> Vec<u16> {
    match o {
        Op::Disp(_, d) => vec![d as u16],
        Op::Pc(t) => vec![(t - at as i64) as i16 as u16],
        Op::Abs(a) => l(a).to_vec(),
        Op::Imm(v) if size == L => l(v).to_vec(),
        Op::Imm(v) => vec![v as u16],
        _ => vec![],
    }
}

/// The instruction at `at` with the operation word `op` and the operands that carry
/// extension words, in order.
fn build(at: usize, op: u16, ops: &[(Op, u16)]) -> Vec<u16> {
    let mut out = vec![op];
    for &(o, s) in ops {
        let e = ea_ext(o, at + 2 * out.len(), s);
        out.extend(e);
    }
    out
}

/// `move.<size> src,dst` (`movea` when `dst` is an address register), placed at `at`.
pub fn mv(size: u16, src: Op, dst: Op, at: usize) -> Vec<u16> {
    let d = ea_bits(dst);
    build(at, mv_size(size) << 12 | (d & 7) << 9 | (d >> 3) << 6 | ea_bits(src), &[(src, size), (dst, size)])
}
/// `clr.<size> dst`.
pub fn clr(size: u16, dst: Op, at: usize) -> Vec<u16> {
    build(at, 0x42 << 8 | size << 6 | ea_bits(dst), &[(dst, size)])
}
/// `addq` (`add == true`) or `subq` `#n,dst`.
pub fn quick(add: bool, n: u16, size: u16, dst: Op, at: usize) -> Vec<u16> {
    build(at, 5 << 12 | (n & 7) << 9 | (!add as u16) << 8 | size << 6 | ea_bits(dst), &[(dst, size)])
}
/// `scc dst` (condition 0 is `st`).
pub fn scc(cond: u16, dst: Op, at: usize) -> Vec<u16> {
    build(at, 5 << 12 | cond << 8 | 3 << 6 | ea_bits(dst), &[(dst, B)])
}
/// The immediate forms: `code` 0 ori, 1 andi, 2 subi, 3 addi, 5 eori, 6 cmpi.
pub fn imm_op(code: u16, size: u16, imm: u32, dst: Op, at: usize) -> Vec<u16> {
    build(at, code << 9 | size << 6 | ea_bits(dst), &[(Op::Imm(imm), size), (dst, size)])
}
/// `<line> src,dn` with `line` 0x8 or, 0x9 sub, 0xB cmp, 0xC and, 0xD add.
pub fn alu(line: u16, size: u16, src: Op, n: u16, at: usize) -> Vec<u16> {
    build(at, line << 12 | n << 9 | size << 6 | ea_bits(src), &[(src, size)])
}
/// `adda.<size> src,an` (word or long).
pub fn adda(size: u16, src: Op, an: u16, at: usize) -> Vec<u16> {
    build(at, 0xD << 12 | an << 9 | (if size == W { 3 } else { 7 }) << 6 | ea_bits(src), &[(src, size)])
}
/// `lea src,an`.
pub fn lea(src: Op, an: u16, at: usize) -> Vec<u16> {
    build(at, 0x4 << 12 | an << 9 | 7 << 6 | ea_bits(src), &[(src, L)])
}
/// `bset` (`set == true`) or `bclr dn,dst`.
pub fn bit_dyn(set: bool, dn: u16, dst: Op, at: usize) -> Vec<u16> {
    build(at, dn << 9 | 1 << 8 | (if set { 3 } else { 2 }) << 6 | ea_bits(dst), &[(dst, B)])
}
/// `dbra dn,<target>` placed at `at`.
pub fn dbra(dn: u16, at: usize, target: usize) -> Vec<u16> {
    vec![5 << 12 | 1 << 8 | 3 << 6 | 1 << 3 | dn, (target as i64 - (at as i64 + 2)) as i16 as u16]
}
/// `rol.w #count,dn`.
pub fn rol_w_imm(count: u16, n: u16) -> Vec<u16> {
    vec![0xE << 12 | count << 9 | 1 << 8 | 1 << 6 | 3 << 3 | n]
}
/// `not.b dn`.
pub fn not_b(n: u16) -> Vec<u16> {
    vec![0x46 << 8 | n]
}

#[cfg(test)]
mod tests {
    use super::super::m68kdec::{Ea, K, decode};
    use super::*;

    fn dec(ws: Vec<u16>) -> super::super::m68kdec::Insn {
        let b: Vec<u8> = ws.iter().flat_map(|w| w.to_be_bytes()).collect();
        let n = b.len();
        let i = decode(&b, 0).unwrap();
        assert_eq!(i.len, n);
        i
    }

    #[test]
    fn encoded_forms_decode_to_the_intended_instructions() {
        assert_eq!(dec(rts()).k, K::Rts);
        assert_eq!(dec(nop()).k, K::Nop);
        let i = dec(moveq(3, 0xFF));
        assert_eq!((i.k, i.data, i.dst), (K::Moveq, -1, Ea::D(3)));
        let i = dec(bcc_w(BNE, 20));
        assert_eq!((i.k, i.cond, i.target), (K::Bcc, 6, 22));
        let i = dec(bcc_w(BEQ, -4));
        assert_eq!((i.k, i.cond), (K::Bcc, 7));
        assert_eq!(dec(bcc_w(BRA, 8)).k, K::Bra);
        assert_eq!(dec(bcc_w(BSR, 8)).k, K::Bsr);
        assert_eq!(dec(bra_s(-4)).k, K::Bra);
        assert_eq!(dec(bne_s(10)).k, K::Bcc);
        assert_eq!(dec(beq_s(2)).cond, 7);
        let i = dec(tst_abs(W, 0x1234));
        assert_eq!(
            (i.k, i.size, i.dst),
            (
                K::Tst,
                2,
                Ea::AbsL {
                    addr: 0x1234,
                    pos: 2
                }
            )
        );
        let i = dec(tst_w_d(0));
        assert_eq!((i.k, i.size, i.dst), (K::Tst, 2, Ea::D(0)));
        let i = dec(cmpi_w_d(5, 0));
        assert_eq!((i.k, i.size, i.dst), (K::Imm(6), 2, Ea::D(0)));
        let i = dec(cmpi_b_d(0xFE, 0));
        assert_eq!(
            (i.k, i.size, i.src),
            (K::Imm(6), 1, Ea::Imm { val: 0xFE, pos: 3 })
        );
        let i = dec(subi_b_abs(1, 0x40));
        assert_eq!(
            (i.k, i.size, i.dst),
            (K::Imm(2), 1, Ea::AbsL { addr: 0x40, pos: 4 })
        );
        let i = dec(subi_b_d(5, 1));
        assert_eq!((i.k, i.size, i.dst), (K::Imm(2), 1, Ea::D(1)));
        let i = dec(move_b_imm_abs(6, 0x40));
        assert_eq!(
            (i.k, i.size, i.src, i.dst),
            (
                K::Move,
                1,
                Ea::Imm { val: 6, pos: 3 },
                Ea::AbsL { addr: 0x40, pos: 4 }
            )
        );
        let i = dec(movea_l_abs(1, 0x40));
        assert_eq!((i.k, i.size, i.dst), (K::Movea, 4, Ea::A(1)));
        let i = dec(move_b_post_d(1, 1));
        assert_eq!(
            (i.k, i.size, i.src, i.dst),
            (K::Move, 1, Ea::Post(1), Ea::D(1))
        );
        let i = dec(move_b_d_abs(0, 0x40));
        assert_eq!((i.src, i.dst), (Ea::D(0), Ea::AbsL { addr: 0x40, pos: 2 }));
        let i = dec(move_b_abs_d(0, 0x40));
        assert_eq!((i.src, i.dst), (Ea::AbsL { addr: 0x40, pos: 2 }, Ea::D(0)));
        let i = dec(lea_abs(2, 0x700));
        assert_eq!(
            (i.k, i.src, i.dst),
            (
                K::Lea,
                Ea::AbsL {
                    addr: 0x700,
                    pos: 2
                },
                Ea::A(2)
            )
        );
        let i = dec(lea_pc(4, 0x10));
        assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::Pc(0x12), Ea::A(4)));
        let i = dec(lea_pc(0, -6));
        assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::Pc(0usize.wrapping_sub(4)), Ea::A(0)));
        let i = dec(asl_w_imm(4, 1));
        assert_eq!((i.k, i.size, i.data, i.dst), (K::Shift, 2, 4, Ea::D(1)));
        assert_eq!(be16_of(&asl_w_imm(4, 1)) & 0x100, 0x100);
        let i = dec(adda_l_d(0, 2));
        assert_eq!(
            (i.k, i.size, i.src, i.dst),
            (K::Adda, 4, Ea::D(0), Ea::A(2))
        );
        let i = dec(jmp_ind(2));
        assert_eq!((i.k, i.dst), (K::Jmp, Ea::Ind(2)));
        let i = dec(move_l_a_abs(1, 0x44));
        assert_eq!(
            (i.k, i.size, i.src, i.dst),
            (K::Move, 4, Ea::A(1), Ea::AbsL { addr: 0x44, pos: 2 })
        );
        let i = dec(move_l_imm_abs(0xC5A, 0xDFF0AA));
        assert_eq!(
            (i.k, i.size, i.src, i.dst),
            (
                K::Move,
                4,
                Ea::Imm { val: 0xC5A, pos: 2 },
                Ea::AbsL {
                    addr: 0xDFF0AA,
                    pos: 6
                }
            )
        );
        let i = dec(move_w_imm_abs(16, 0xDFF0A4));
        assert_eq!(
            (i.k, i.size, i.src),
            (K::Move, 2, Ea::Imm { val: 16, pos: 2 })
        );
        let i = dec(move_w_d_abs(1, 0xDFF0A6));
        assert_eq!(
            (i.size, i.src, i.dst),
            (
                2,
                Ea::D(1),
                Ea::AbsL {
                    addr: 0xDFF0A6,
                    pos: 2
                }
            )
        );
        let i = dec(add_w_dd(1, 1));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Add, 2, Ea::D(1), Ea::D(1)));
        let i = dec(move_w_idx_d(2, 1, 1));
        assert_eq!((i.k, i.size, i.dst), (K::Move, 2, Ea::D(1)));
        assert!(matches!(i.src, Ea::Idx(2, 0, _)));
        let i = dec(andi_b_abs(1, 0x100));
        assert_eq!((i.k, i.size), (K::Imm(1), 1));
        let i = dec(move_l_idx_abs(4, 8, 0x200));
        assert_eq!((i.k, i.size), (K::Move, 4));
        let i = dec(movea_l_imm(3, 0x1000));
        assert_eq!((i.k, i.dst), (K::Movea, Ea::A(3)));
        let i = dec(lea_pc(2, -2));
        assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::Pc(0), Ea::A(2)));
        let i = dec(cmpi_w_ind(1, 0));
        assert_eq!((i.k, i.size, i.dst), (K::Imm(6), 2, Ea::Ind(0)));
        assert!(matches!(i.src, Ea::Imm { val: 1, .. }));
        let i = dec(tst_b_d(1));
        assert_eq!((i.k, i.size, i.dst), (K::Tst, 1, Ea::D(1)));
        let i = dec(move_b_imm_d(1, 1));
        assert_eq!((i.k, i.size, i.dst), (K::Move, 1, Ea::D(1)));
        assert!(matches!(i.src, Ea::Imm { val: 1, .. }));
        let i = dec(btst_imm_disp(6, 6, 28));
        assert_eq!((i.k, i.dst), (K::Bit(0), Ea::Disp(6, 28)));
        assert!(matches!(i.src, Ea::Imm { val: 6, .. }));
        let i = dec(lsr_w_imm(7, 0));
        assert_eq!((i.k, i.size, i.data, i.left, i.shk, i.dst), (K::Shift, 2, 7, false, 1, Ea::D(0)));
        let i = dec(addi_b_d(12, 0));
        assert_eq!((i.k, i.size, i.dst), (K::Imm(3), 1, Ea::D(0)));
        assert!(matches!(i.src, Ea::Imm { val: 12, .. }));
        let i = dec(move_l_imm_disp(0x1234, 4, 160));
        assert_eq!((i.k, i.size, i.dst), (K::Move, 4, Ea::Disp(4, 160)));
        assert!(matches!(i.src, Ea::Imm { val: 0x1234, .. }));
        assert_eq!(dec(rte()).k, K::Rte);
        assert_eq!(dec(lsr_b_imm(1, 2)).k, K::Shift);
        assert_eq!(dec(cmp_b_imm_d(3, 2)).k, K::Cmp);
        assert_eq!(dec(tst_b_abs(0x10)).k, K::Tst);
        assert_eq!(dec(move_b_dd(0, 2)).k, K::Move);
        assert_eq!(dec(asl_b_imm(1, 2)).k, K::Shift);
    }

    #[test]
    fn forms_of_the_ashley_hogg_scans_decode_to_the_intended_instructions() {
        let i = dec(movem_l_save_all());
        assert_eq!((i.k, i.dst), (K::Movem, Ea::Pre(7)));
        assert_eq!(movem_l_save_all()[1], !1);
        let i = dec(move_w_imm_d(0, 0));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 2, Ea::Imm { val: 0, pos: 2 }, Ea::D(0)));
        let i = dec(lea_pc_at(5, 0, 100));
        assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::Pc(100), Ea::A(5)));
        let i = dec(move_b_idx_disp(0, 0, 4, 47));
        assert_eq!((i.k, i.size, i.dst), (K::Move, 1, Ea::Disp(4, 47)));
        assert!(matches!(i.src, Ea::Idx(0, 0, _)));
        let i = dec(move_l_idx_disp(3, 1, 0, 54));
        assert_eq!((i.k, i.size, i.dst), (K::Move, 4, Ea::Disp(0, 54)));
        assert!(matches!(i.src, Ea::Idx(3, 0, _)));
        let i = dec(mulu_imm_d(44, 1));
        assert_eq!((i.k, i.src, i.dst), (K::Mulu, Ea::Imm { val: 44, pos: 2 }, Ea::D(1)));
        let i = dec(adda_l_disp(0, 32, 3));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Adda, 4, Ea::Disp(0, 32), Ea::A(3)));
        let i = dec(move_w_disp_d(0, 6, 6));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 2, Ea::Disp(0, 6), Ea::D(6)));
        let i = dec(move_l_disp_d(0, 58, 0));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 4, Ea::Disp(0, 58), Ea::D(0)));
        let i = dec(move_l_pc_d(6, 0, 200));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 4, Ea::Pc(200), Ea::D(6)));
        let i = dec(cmpi_b_ind(130, 2));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Imm(6), 1, Ea::Imm { val: 130, pos: 3 }, Ea::Ind(2)));
        let i = dec(move_b_pcidx_disp(1, 0, 40, 1, 17));
        assert_eq!((i.k, i.size, i.dst), (K::Move, 1, Ea::Disp(1, 17)));
        assert!(matches!(i.src, Ea::PcIdx(40, _)));
        let i = dec(add_w_pcidx_d(1, 0, 60, 4));
        assert_eq!((i.k, i.size, i.dst), (K::Add, 2, Ea::D(4)));
        assert!(matches!(i.src, Ea::PcIdx(60, _)));
    }

    #[test]
    fn the_desire_opening_forms_round_trip() {
        let i = dec(move_w_imm_disp(255, 3, 158));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 2, Ea::Imm { val: 255, pos: 2 }, Ea::Disp(3, 158)));
        let i = dec(lea_abs(2, 0xDFF000));
        assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::AbsL { addr: 0xDFF000, pos: 2 }, Ea::A(2)));
        let i = dec(movea_l_imm(5, 0xDFF000));
        assert_eq!((i.k, i.size, i.src, i.dst), (K::Movea, 4, Ea::Imm { val: 0xDFF000, pos: 2 }, Ea::A(5)));
    }

    /// Decodes an instruction placed at `at`.
    fn dec_at(ws: Vec<u16>, at: usize) -> super::super::m68kdec::Insn {
        let mut b = vec![0u8; at];
        b.extend(ws.iter().flat_map(|w| w.to_be_bytes()));
        let n = b.len() - at;
        let i = decode(&b, at).unwrap();
        assert_eq!(i.len, n);
        i
    }

    #[test]
    fn general_forms_decode_to_the_intended_instructions() {
        use Op::*;
        for at in [0usize, 10, 300] {
            let d = |ws: Vec<u16>| dec_at(ws, at);
            let i = d(mv(B, Pc(40), Disp(4, 100), at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 1, Ea::Pc(40), Ea::Disp(4, 100)));
            let i = d(mv(L, Imm(0xDFF0A6), A(2), at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Movea, 4, Ea::Imm { val: 0xDFF0A6, pos: at + 2 }, Ea::A(2)));
            let i = d(mv(W, Ind(1), D(0), at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Move, 2, Ea::Ind(1), Ea::D(0)));
            let i = d(mv(L, D(0), Post(1), at));
            assert_eq!((i.size, i.src, i.dst), (4, Ea::D(0), Ea::Post(1)));
            let i = d(mv(B, Imm(15), Disp(4, -3), at));
            assert_eq!((i.size, i.src, i.dst), (1, Ea::Imm { val: 15, pos: at + 3 }, Ea::Disp(4, -3)));
            let i = d(mv(W, Disp(0, 100), Abs(0xDFF096), at));
            assert_eq!((i.size, i.src, i.dst), (2, Ea::Disp(0, 100), Ea::AbsL { addr: 0xDFF096, pos: at + 4 }));
            let i = d(clr(B, Disp(4, 8), at));
            assert_eq!((i.k, i.size, i.dst), (K::Clr, 1, Ea::Disp(4, 8)));
            let i = d(clr(W, Ind(0), at));
            assert_eq!((i.k, i.size, i.dst), (K::Clr, 2, Ea::Ind(0)));
            let i = d(quick(true, 1, B, Disp(4, 2), at));
            assert_eq!((i.k, i.size, i.data, i.dst), (K::Addq, 1, 1, Ea::Disp(4, 2)));
            let i = d(quick(false, 8, L, A(2), at));
            assert_eq!((i.k, i.size, i.data, i.dst), (K::Subq, 4, 8, Ea::A(2)));
            let i = d(scc(0, Disp(4, 2), at));
            assert_eq!((i.k, i.cond, i.size, i.dst), (K::Scc, 0, 1, Ea::Disp(4, 2)));
            let i = d(imm_op(1, B, 15, Disp(4, 10), at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Imm(1), 1, Ea::Imm { val: 15, pos: at + 3 }, Ea::Disp(4, 10)));
            let i = d(imm_op(6, L, 0x464F524D, Post(0), at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Imm(6), 4, Ea::Imm { val: 0x464F524D, pos: at + 2 }, Ea::Post(0)));
            let i = d(imm_op(5, W, 65535, D(1), at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Imm(5), 2, Ea::Imm { val: 65535, pos: at + 2 }, Ea::D(1)));
            let i = d(imm_op(1, L, 4095, D(6), at));
            assert_eq!((i.k, i.size, i.dst), (K::Imm(1), 4, Ea::D(6)));
            assert_eq!(d(imm_op(3, B, 64, D(0), at)).k, K::Imm(3));
            assert_eq!(d(imm_op(6, B, 5, Disp(4, 6), at)).k, K::Imm(6));
            let i = d(alu(0xB, B, D(0), 1, at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Cmp, 1, Ea::D(0), Ea::D(1)));
            let i = d(alu(0xD, W, Pc(60), 0, at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Add, 2, Ea::Pc(60), Ea::D(0)));
            let i = d(alu(0xD, L, Ind(6), 6, at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Add, 4, Ea::Ind(6), Ea::D(6)));
            let i = d(alu(0x9, W, Pc(60), 0, at));
            assert_eq!((i.k, i.size), (K::Sub, 2));
            assert_eq!(d(alu(0xB, B, Imm(125), 0, at)).k, K::Cmp);
            let i = d(adda(W, Post(0), 5, at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Adda, 2, Ea::Post(0), Ea::A(5)));
            let i = d(adda(L, D(4), 1, at));
            assert_eq!((i.k, i.size, i.src, i.dst), (K::Adda, 4, Ea::D(4), Ea::A(1)));
            let i = d(lea(Pc(-6), 4, at));
            assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::Pc((-6i64) as usize), Ea::A(4)));
            let i = d(lea(Abs(0xDFF0A4), 5, at));
            assert_eq!((i.k, i.src, i.dst), (K::Lea, Ea::AbsL { addr: 0xDFF0A4, pos: at + 2 }, Ea::A(5)));
            let i = d(bit_dyn(true, 1, Disp(4, 9), at));
            assert_eq!((i.k, i.src, i.dst), (K::Bit(3), Ea::D(1), Ea::Disp(4, 9)));
            assert_eq!(d(bit_dyn(false, 2, Disp(4, 9), at)).k, K::Bit(2));
            let i = d(dbra(1, at, at + 20));
            assert_eq!((i.k, i.dst, i.target), (K::Dbcc, Ea::D(1), at + 20));
            let i = d(rol_w_imm(1, 6));
            assert_eq!((i.k, i.size, i.left, i.shk, i.data), (K::Shift, 2, true, 3, 1));
            assert_eq!(d(not_b(1)).len, 2);
        }
    }

    fn be16_of(ws: &[u16]) -> u16 {
        ws[0]
    }
}

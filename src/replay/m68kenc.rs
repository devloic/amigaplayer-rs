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
/// `move.<size> #imm,abs.l` for word and long are `move_w_imm_abs`, `move_l_imm_abs`.
pub fn size_code(s: u16) -> u16 {
    mv_size(s)
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

    fn be16_of(ws: &[u16]) -> u16 {
        ws[0]
    }
}

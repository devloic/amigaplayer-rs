// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/Integrator6581.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::filter_model_config::Dither;
use super::filter_model_config6581::FilterModelConfig6581;

// Find output voltage in inverting integrator SID op-amp circuits, using a
// single fixpoint iteration step.
//
// A circuit diagram of a MOS 6581 integrator is shown below.
//
//                   +---C---+
//                   |       |
//     vi --o--Rw--o-o--[A>--o-- vo
//          |      | vx
//          +--Rs--+
//
// From Kirchoff's current law it follows that
//
//     IRw + IRs + ICr = 0
//
// Using the formula for current through a capacitor, i = C*dv/dt, we get
//
//     IRw + IRs + C*(vc - vc0)/dt = 0
//     dt/C*(IRw + IRs) + vc - vc0 = 0
//     vc = vc0 - n*(IRw(vi,vx) + IRs(vi,vx))
//
// which may be rewritten as the following iterative fixpoint function:
//
//     vc = vc0 - n*(IRw(vi,g(vc)) + IRs(vi,g(vc)))
//
// To accurately calculate the currents through Rs and Rw, we need to use
// transistor models. Rs has a gate voltage of Vdd = 12V, and can be
// assumed to always be in triode mode. For Rw, the situation is rather
// more complex, as it turns out that this transistor will operate in
// both subthreshold, triode, and saturation modes.
//
// The Shichman-Hodges transistor model routinely used in textbooks may
// be written as follows:
//
//     Ids = 0                          , Vgst < 0               (subthreshold mode)
//     Ids = K*W/L*(2*Vgst - Vds)*Vds   , Vgst >= 0, Vds < Vgst  (triode mode)
//     Ids = K*W/L*Vgst^2               , Vgst >= 0, Vds >= Vgst (saturation mode)
//
// where
//     K   = u*Cox/2 (transconductance coefficient)
//     W/L = ratio between substrate width and length
//     Vgst = Vg - Vs - Vt (overdrive voltage)
//
// This transistor model is also called the quadratic model.
//
// Note that the equation for the triode mode can be reformulated as
// independent terms depending on Vgs and Vgd, respectively, by the
// following substitution:
//
//     Vds = Vgst - (Vgst - Vds) = Vgst - Vgdt
//
//     Ids = K*W/L*(2*Vgst - Vds)*Vds
//         = K*W/L*(2*Vgst - (Vgst - Vgdt)*(Vgst - Vgdt)
//         = K*W/L*(Vgst + Vgdt)*(Vgst - Vgdt)
//         = K*W/L*(Vgst^2 - Vgdt^2)
//
// This turns out to be a general equation which covers both the triode
// and saturation modes (where the second term is 0 in saturation mode).
// The equation is also symmetrical, i.e. it can calculate negative
// currents without any change of parameters (since the terms for drain
// and source are identical except for the sign).
//
// FIXME: Subthreshold as function of Vgs, Vgd.
//
//     Ids = I0*W/L*e^(Vgst/(Ut/k))   , Vgst < 0               (subthreshold mode)
//
// where
//     I0 = (2 * uCox * Ut^2) / k
//
// The remaining problem with the textbook model is that the transition
// from subthreshold to triode/saturation is not continuous.
//
// Realizing that the subthreshold and triode/saturation modes may both
// be defined by independent (and equal) terms of Vgs and Vds,
// respectively, the corresponding terms can be blended into (equal)
// continuous functions suitable for table lookup.
//
// The EKV model (Enz, Krummenacher and Vittoz) essentially performs this
// blending using an elegant mathematical formulation:
//
//     Ids = Is * (if - ir)
//     Is = ((2 * u*Cox * Ut^2)/k) * W/L
//     if = ln^2(1 + e^((k*(Vg - Vt) - Vs)/(2*Ut))
//     ir = ln^2(1 + e^((k*(Vg - Vt) - Vd)/(2*Ut))
//
// For our purposes, the EKV model preserves two important properties
// discussed above:
//
// - It consists of two independent terms, which can be represented by
//   the same lookup table.
// - It is symmetrical, i.e. it calculates current in both directions,
//   facilitating a branch-free implementation.
//
// Rw in the circuit diagram above is a VCR (voltage controlled resistor),
// as shown in the circuit diagram below.
//
//
//                        Vdd
//                           |
//              Vdd         _|_
//                 |    +---+ +---- Vw
//                _|_   |
//             +--+ +---o Vg
//             |      __|__
//             |      -----  Rw
//             |      |   |
//     vi -----o------+   +-------- vo
//
//
// In order to calculalate the current through the VCR, its gate voltage
// must be determined.
//
// Assuming triode mode and applying Kirchoff's current law, we get the
// following equation for Vg:
//
//     u*Cox/2*W/L*((nVddt - Vg)^2 - (nVddt - vi)^2 + (nVddt - Vg)^2 - (nVddt - Vw)^2) = 0
//     2*(nVddt - Vg)^2 - (nVddt - vi)^2 - (nVddt - Vw)^2 = 0
//     (nVddt - Vg) = sqrt(((nVddt - vi)^2 + (nVddt - Vw)^2)/2)
//
//     Vg = nVddt - sqrt(((nVddt - vi)^2 + (nVddt - Vw)^2)/2)

pub struct Integrator6581 {
    pub vx: i32,
    pub vc: i32,

    n_vddt_vw_2: u32,

    n_vddt: u16,
    n_vt: u16,
    n_vmin: u16,
}

impl Integrator6581 {
    pub fn new(fmc: &FilterModelConfig6581, dither: &mut Dither) -> Self {
        let base = fmc.base();

        Integrator6581 {
            vx: 0,
            vc: 0,
            n_vddt_vw_2: 0,
            n_vddt: base.get_normalized_value(base.vddt, dither),
            n_vt: base.get_normalized_value(base.vth, dither),
            n_vmin: base.get_nv_min(),
        }
    }

    pub fn set_vw(&mut self, vw: u16) {
        let d = i32::from(self.n_vddt) - i32::from(vw);
        self.n_vddt_vw_2 = (d.wrapping_mul(d) >> 1) as u32;
    }

    pub fn restart(&mut self) {
        self.vx = 16384;
        self.vc = 16384;
    }

    /// `n_snake` is `fmc.get_normalized_current_factor(13, fmc.get_wl_snake())`,
    /// which only changes with the filter range and the old caps setting
    #[inline(always)]
    pub fn solve(&mut self, vi: i32, fmc: &FilterModelConfig6581, n_snake: u16) -> i32 {
        // "Snake" voltages for triode mode calculation
        let vgst = (i32::from(self.n_vddt).wrapping_sub(self.vx)) as u32;
        let vgdt = (i32::from(self.n_vddt).wrapping_sub(vi)) as u32;

        let vgst_2 = vgst.wrapping_mul(vgst);
        let vgdt_2 = vgdt.wrapping_mul(vgdt);

        // "Snake" current, scaled by (1/m)*2^13*m*2^16*m*2^16*2^-15 = m*2^30
        let n_i_snake = i32::from(n_snake).wrapping_mul((vgst_2.wrapping_sub(vgdt_2) as i32) >> 15);

        // VCR gate voltage.		// Scaled by m*2^16
        // Vg = Vddt - sqrt(((Vddt - vW)^2 + Vgdt^2)/2)
        let n_vg = i32::from(fmc.get_vcr_n_vg(self.n_vddt_vw_2.wrapping_add(vgdt_2 >> 1) >> 16));
        let k_vgt = (n_vg - i32::from(self.n_vt)) - i32::from(self.n_vmin);

        // VCR voltages for EKV model table lookup
        let k_vgt_vs = (k_vgt.wrapping_sub(self.vx)) - i32::from(i16::MIN);
        let k_vgt_vd = (k_vgt.wrapping_sub(vi)) - i32::from(i16::MIN);

        // VCR current, scaled by m*2^15*2^15 = m*2^30
        let i_f = u32::from(fmc.get_vcr_n_ids_term(k_vgt_vs)) << 15;
        let ir = u32::from(fmc.get_vcr_n_ids_term(k_vgt_vd)) << 15;
        let n_i_vcr = i_f.wrapping_sub(ir) as i32;

        // Change in capacitor charge
        self.vc = self.vc.wrapping_add(n_i_snake.wrapping_add(n_i_vcr));

        // vx = g(vc)
        let tmp = (self.vc >> 15) - i32::from(i16::MIN);
        self.vx = i32::from(fmc.base().get_opamp_rev(tmp));

        // Return vo
        self.vx - (self.vc >> 14)
    }
}

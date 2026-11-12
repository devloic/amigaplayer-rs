// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/OpAmp.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

use super::spline::{Point, Spline};

// Opamp input -> output voltage conversion

// Find output voltage in inverting gain and inverting summer SID op-amp
// circuits, using a combination of Newton-Raphson and bisection.
//
//               +---R2--+
//               |       |
//     vi ---R1--o--[A>--o-- vo
//               vx
//
// From Kirchoff's current law it follows that
//
//     IR1f + IR2r = 0
//
// Substituting the triode mode transistor model K*W/L*(Vgst^2 - Vgdt^2)
// for the currents, we get:
//
//     n*((Vddt - vx)^2 - (Vddt - vi)^2) + (Vddt - vx)^2 - (Vddt - vo)^2 = 0
//
// where n is the ratio between R1 and R2.
//
// Our root function f can thus be written as:
//
//     f = (n + 1)*(Vddt - vx)^2 - n*(Vddt - vi)^2 - (Vddt - vo)^2 = 0
//
// Using substitution constants
//
//     a = n + 1
//     b = Vddt
//     c = n*(Vddt - vi)^2
//
// the equations for the root function and its derivative can be written as:
//
//     f = a*(b - vx)^2 - c - (b - vo)^2
//     df = 2*((b - vo)*dvo - a*(b - vx))

const EPSILON: f64 = 1e-8;

pub struct OpAmp {
    /// Current root position
    x: f64,

    vddt: f64,
    v_min: f64,
    v_max: f64,

    opamp: Spline,
}

impl OpAmp {
    pub fn new(opamp_voltages: &[Point], vddt: f64, v_min: f64, v_max: f64) -> Self {
        OpAmp {
            x: 0.0,
            vddt,
            v_min,
            v_max,
            opamp: Spline::new(opamp_voltages),
        }
    }

    /// Reset root position
    pub fn reset(&mut self) {
        self.x = self.v_min;
    }

    /// Solve the opamp equation for input vi in loading context n
    pub fn solve(&mut self, n: f64, vi: f64) -> f64 {
        // Start off with an estimate of x and a root bracket [ak, bk].
        // f is decreasing, so that f(ak) > 0 and f(bk) < 0
        let mut ak = self.v_min;
        let mut bk = self.v_max;

        let a = n + 1.0;
        let b = self.vddt;
        let b_vi = if b > vi { b - vi } else { 0.0 };
        let c = n * (b_vi * b_vi);

        loop {
            let xk = self.x;

            // Calculate f and df
            let out = self.opamp.evaluate(self.x);
            let vo = out.x;
            let dvo = out.y;

            let b_vx = if b > self.x { b - self.x } else { 0.0 };
            let b_vo = if b > vo { b - vo } else { 0.0 };

            // f = a*(b - vx)^2 - c - (b - vo)^2
            let f = a * (b_vx * b_vx) - c - (b_vo * b_vo);

            // df = 2*((b - vo)*dvo - a*(b - vx))
            let df = 2.0 * (b_vo * dvo - a * b_vx);

            // Newton-Raphson step: xk1 = xk - f(xk)/f'(xk)
            self.x -= f / df;

            if (self.x - xk).abs() < EPSILON {
                return self.opamp.evaluate(self.x).x;
            }

            // Narrow down root bracket
            if f < 0.0 {
                bk = xk;
            } else {
                ak = xk;
            }

            if self.x <= ak || self.x >= bk {
                // Bisection step (ala Dekker's method)
                self.x = (ak + bk) * 0.5;
            }
        }
    }
}

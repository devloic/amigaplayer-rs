// Port of NostalgicPlayer's C# port of reSIDfp (libsidplayfp's SID emulation, by
// Dag Lem, Antti Lankila and Leandro Nini): Source/Ports/LibReSidFp/Spline.cs,
// git e183943. Licence: GNU General Public License version 2 (LICENSE-LibReSidFp);
// the upstream file carries only NostalgicPlayer's header:
//   "This source, or parts thereof, may be used in any software as long the
//    license of NostalgicPlayer is keep. See the LICENSE file for more
//    information."

// Fritsch-Carlson monotone cubic spline interpolation.
//
// Based on the implementation from the [Monotone cubic interpolation] wikipedia page.
//
// [Monotone cubic interpolation]: https://en.wikipedia.org/wiki/Monotone_cubic_interpolation

#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

#[derive(Clone, Copy, Default)]
struct Param {
    x1: f64,
    x2: f64,
    a: f64,
    b: f64,
    c: f64,
    d: f64,
}

pub struct Spline {
    /// Interpolation parameters
    params: Vec<Param>,

    /// Last used parameters, cached for speed up
    c: usize,
}

impl Spline {
    /// `input` needs at least two points with increasing x
    pub fn new(input: &[Point]) -> Self {
        let mut params = vec![Param::default(); input.len()];
        let coeff_length = input.len() - 1;

        let mut dxs = vec![0.0; coeff_length];
        let mut ms = vec![0.0; coeff_length];

        // Get consecutive differences and slopes
        for i in 0..coeff_length {
            let dx = input[i + 1].x - input[i].x;
            let dy = input[i + 1].y - input[i].y;

            dxs[i] = dx;
            ms[i] = dy / dx;
        }

        // Get degree-1 coefficients
        params[0].c = ms[0];

        for i in 1..coeff_length {
            let m = ms[i - 1];
            let m_next = ms[i];

            if m * m_next <= 0.0 {
                params[i].c = 0.0;
            } else {
                let dx = dxs[i - 1];
                let dx_next = dxs[i];
                let common = dx + dx_next;
                params[i].c = 3.0 * common / ((common + dx_next) / m + (common + dx) / m_next);
            }
        }

        params[coeff_length].c = ms[coeff_length - 1];

        // Get degree-2 and degree-3 coefficients
        for i in 0..coeff_length {
            params[i].x1 = input[i].x;
            params[i].x2 = input[i + 1].x;
            params[i].d = input[i].y;

            let c1 = params[i].c;
            let m = ms[i];
            let inv_dx = 1.0 / dxs[i];
            let common = c1 + params[i + 1].c - m - m;

            params[i].b = (m - c1 - common) * inv_dx;
            params[i].a = common * inv_dx * inv_dx;
        }

        // Fix the upper range, because we interpolate outside original bounds if necessary
        params[coeff_length - 1].x2 = f64::MAX;

        Spline { params, c: 0 }
    }

    /// Evaluate y and its derivative at given point x
    pub fn evaluate(&mut self, x: f64) -> Point {
        let cur = self.params[self.c];
        if x < cur.x1 || x > cur.x2 {
            for (i, param) in self.params.iter().enumerate() {
                if x <= param.x2 {
                    self.c = i;
                    break;
                }
            }
        }

        let c = &self.params[self.c];

        // Interpolate
        let diff = x - c.x1;

        Point {
            // y = a*x^3 + b*x^2 + c*x + d
            x: ((c.a * diff + c.b) * diff + c.c) * diff + c.d,

            // dy = 3*a*x^2 + 2*b*x + c
            y: (3.0 * c.a * diff + 2.0 * c.b) * diff + c.c,
        }
    }
}

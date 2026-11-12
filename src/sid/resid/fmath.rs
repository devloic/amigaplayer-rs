// Not a port: helpers replacing the .NET Math.Exp/Log/Log10/Sin/Tanh/Pow calls
// of NostalgicPlayer's C# port of reSIDfp (Source/Ports/LibReSidFp/, git e183943;
// libsidplayfp's SID emulation by Dag Lem, Antti Lankila and Leandro Nini).
// Licence: GNU General Public License version 2 (LICENSE-LibReSidFp).
//
// The std transcendental functions call the platform's libm, whose last-bit
// results differ between x86-64, aarch64 and wasm32. Everything here is built
// from + - * / and plain casts only (all IEEE-754 exact operations), so the
// result is bit-identical everywhere. Accuracy is about 1e-15 relative.

use std::f64::consts::{FRAC_2_PI, FRAC_PI_4, LOG2_E as INV_LN2, LOG10_E, SQRT_2 as SQRT2, TAU as TWO_PI};

const LN2_HI: f64 = 6.931_471_803_691_238_164_90e-1;
const LN2_LO: f64 = 1.908_214_929_270_587_700_02e-10;

// Cody-Waite split of pi/2 (fdlibm)
const PIO2_1: f64 = 1.570_796_326_734_125_614_17;
const PIO2_2: f64 = 6.077_100_506_303_965_976_60e-11;
const PIO2_3: f64 = 2.022_266_248_711_166_455_80e-21;
const PIO2_3T: f64 = 8.478_427_660_368_899_569_97e-32;

/// 2^k for k in the normal range.
#[inline]
fn pow2(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

/// e^x
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.782_712_893_384 {
        return f64::INFINITY;
    }
    if x < -745.2 {
        return 0.0;
    }

    // x = k*ln2 + r, |r| <= ln2/2
    let k = (x * INV_LN2 + if x < 0.0 { -0.5 } else { 0.5 }) as i32;
    let kf = k as f64;
    let r = (x - kf * LN2_HI) - kf * LN2_LO;

    // Taylor series of e^r (|r| < 0.3466), Horner form
    let mut s = 1.0;
    for n in (1..=14).rev() {
        s = 1.0 + s * r / n as f64;
    }

    // Two exact power of two factors keep both inside the normal range
    let k1 = k / 2;
    s * pow2(k1) * pow2(k - k1)
}

/// e^x - 1, accurate for small x
pub fn expm1(x: f64) -> f64 {
    if x.abs() < 0.5 {
        let mut s = 1.0;
        for n in (2..=18).rev() {
            s = 1.0 + s * x / n as f64;
        }
        x * s
    } else {
        exp(x) - 1.0
    }
}

/// Natural logarithm
pub fn ln(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    if x == f64::INFINITY {
        return x;
    }

    let mut bits = x.to_bits();
    let mut e = ((bits >> 52) & 0x7ff) as i32;
    if e == 0 {
        // Subnormal: scale by 2^54 and redo
        bits = (x * 18_014_398_509_481_984.0).to_bits();
        e = ((bits >> 52) & 0x7ff) as i32 - 54;
    }
    e -= 1023;

    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | (0x3ff << 52));
    if m > SQRT2 {
        m *= 0.5;
        e += 1;
    }

    // ln(m) = 2*atanh(f), f = (m - 1)/(m + 1), |f| <= 0.1716
    let f = (m - 1.0) / (m + 1.0);
    let s = f * f;
    let mut acc = 1.0 / 25.0;
    for k in (0..12).rev() {
        acc = 1.0 / (2 * k + 1) as f64 + s * acc;
    }
    let lnm = 2.0 * f * acc;

    let ef = e as f64;
    ef * LN2_HI + (ef * LN2_LO + lnm)
}

/// Base 10 logarithm
pub fn log10(x: f64) -> f64 {
    ln(x) * LOG10_E
}

/// sin(r) and cos(r) kernels for |r| <= pi/4
fn sin_kernel(r: f64) -> f64 {
    let r2 = r * r;
    let mut s = 1.0;
    for n in (1..=13).rev() {
        let d = (2 * n * (2 * n + 1)) as f64;
        s = 1.0 - s * r2 / d;
    }
    r * s
}

fn cos_kernel(r: f64) -> f64 {
    let r2 = r * r;
    let mut c = 1.0;
    for n in (1..=13).rev() {
        let d = ((2 * n - 1) * (2 * n)) as f64;
        c = 1.0 - c * r2 / d;
    }
    c
}

/// Sine
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }

    let mut x = x;
    if x.abs() > 1.0e9 {
        // Far outside anything reSIDfp needs; crude but still deterministic
        x %= TWO_PI;
    }

    if x.abs() <= FRAC_PI_4 {
        return sin_kernel(x);
    }

    let kf = (x * FRAC_2_PI).round();
    let r = (((x - kf * PIO2_1) - kf * PIO2_2) - kf * PIO2_3) - kf * PIO2_3T;

    match (kf as i64) & 3 {
        0 => sin_kernel(r),
        1 => cos_kernel(r),
        2 => -sin_kernel(r),
        _ => -cos_kernel(r),
    }
}

/// Hyperbolic tangent
pub fn tanh(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    let ax = x.abs();
    let r = if ax > 20.0 {
        1.0
    } else {
        let t = expm1(2.0 * ax);
        t / (t + 2.0)
    };
    if x < 0.0 { -r } else { r }
}

/// x^n for an integer n (square and multiply)
pub fn powi(x: f64, n: i32) -> f64 {
    let mut e = n.unsigned_abs();
    let mut base = x;
    let mut r = 1.0;
    while e > 0 {
        if e & 1 != 0 {
            r *= base;
        }
        base *= base;
        e >>= 1;
    }
    if n < 0 { 1.0 / r } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(a: f64, b: f64) -> f64 {
        if a == b {
            0.0
        } else {
            ((a - b) / b).abs()
        }
    }

    #[test]
    fn exp_matches_std() {
        let mut x = -700.0;
        while x < 700.0 {
            assert!(rel(exp(x), x.exp()) < 1e-13, "exp({x})");
            x += 0.37;
        }
        for i in -2000..2000 {
            let x = i as f64 * 0.001;
            assert!(rel(exp(x), x.exp()) < 1e-14, "exp({x})");
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(exp(1000.0), f64::INFINITY);
        assert_eq!(exp(-1000.0), 0.0);
    }

    #[test]
    fn expm1_matches_std() {
        for i in -3000..3000 {
            let x = i as f64 * 0.0013 + 1e-9;
            assert!(rel(expm1(x), x.exp_m1()) < 1e-13, "expm1({x})");
        }
    }

    #[test]
    fn ln_log10_match_std() {
        let mut x = 1e-300;
        while x < 1e300 {
            assert!(rel(ln(x), x.ln()) < 1e-13, "ln({x})");
            x *= 1.7;
        }
        for i in 1..5000 {
            let x = i as f64 * 0.01;
            if (x - 1.0).abs() > 1e-9 {
                assert!(rel(ln(x), x.ln()) < 1e-13, "ln({x})");
                assert!(rel(log10(x), x.log10()) < 1e-13, "log10({x})");
            }
        }
        assert!(rel(log10(1.0 / 65536.0), (1.0f64 / 65536.0).log10()) < 1e-14);
        assert_eq!(ln(1.0), 0.0);
        assert!(ln(-1.0).is_nan());
        assert_eq!(ln(0.0), f64::NEG_INFINITY);
    }

    #[test]
    fn sin_matches_std() {
        let mut x = -300.0;
        while x < 300.0 {
            let (a, b) = (sin(x), x.sin());
            assert!((a - b).abs() < 1e-12, "sin({x}) {a} {b}");
            if b.abs() > 1e-3 {
                assert!(rel(a, b) < 1e-12, "sin({x})");
            }
            x += 0.0137;
        }
        assert_eq!(sin(0.0), 0.0);
        assert!(sin(f64::INFINITY).is_nan());
    }

    #[test]
    fn tanh_matches_std() {
        for i in -4000..4000 {
            let x = i as f64 * 0.0071 + 1e-7;
            assert!(rel(tanh(x), x.tanh()) < 1e-13, "tanh({x})");
        }
        assert_eq!(tanh(50.0), 1.0);
        assert_eq!(tanh(-50.0), -1.0);
    }

    #[test]
    fn powi_matches_std() {
        let mut x = 0.0001;
        while x < 60.0 {
            for n in -12..=12 {
                assert!(rel(powi(x, n), x.powi(n)) < 1e-13, "powi({x},{n})");
            }
            x *= 1.31;
        }
        assert_eq!(powi(2.0, 10), 1024.0);
        assert_eq!(powi(5.0, 0), 1.0);
    }
}

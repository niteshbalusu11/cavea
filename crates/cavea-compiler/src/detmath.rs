//! Trigonometry that gives the same bits on every target.
//!
//! Platform `sin` and `cos` may differ in the last bit between native targets and
//! wasm32, and compiled charts are content-hashed. These functions use only
//! IEEE 754 `+ − × ÷`, which are correctly rounded everywhere (Rust never fuses
//! them into FMA), so every target computes the same result. The code is
//! fdlibm's, as used by musl.
//!
//! ====================================================
//! Copyright (C) 1993 by Sun Microsystems, Inc. All rights reserved.
//!
//! Developed at SunPro, a Sun Microsystems, Inc. business.
//! Permission to use, copy, modify, and distribute this
//! software is freely granted, provided that this notice
//! is preserved.
//! ====================================================

/// Degrees to radians.
pub fn radians(degrees: f64) -> f64 {
    degrees * (core::f64::consts::PI / 180.0)
}

/// `(sin x, cos x)` to within an ulp, for `|x| < 2^20 × π/2`; beyond that the
/// result is still the same on every target, but not accurate.
///
/// This is fdlibm's `sin` and `cos` with the medium-size argument reduction of
/// `__rem_pio2`, which is all the expansion needs: arc angles are within ±π and
/// skew within ±π/2.
pub fn sin_cos(x: f64) -> (f64, f64) {
    const TOINT: f64 = 1.5 / f64::EPSILON;
    const INV_PIO2: f64 = f64::from_bits(0x3fe45f306dc9c883); // 6.36619772367581382433e-1
    // π/2 as PIO2_1 + PIO2_2 + PIO2_3 + PIO2_3T. The first three have their low
    // bits zero, so `n × PIO2_k` is exact for |n| < 2^20.
    const PIO2_1: f64 = f64::from_bits(0x3ff921fb54400000); // 1.57079632673412561417
    const PIO2_1T: f64 = f64::from_bits(0x3dd0b4611a626331); // 6.07710050650619224932e-11
    const PIO2_2: f64 = f64::from_bits(0x3dd0b4611a600000); // 6.07710050630396597660e-11
    const PIO2_2T: f64 = f64::from_bits(0x3ba3198a2e037073); // 2.02226624879595063154e-21
    const PIO2_3: f64 = f64::from_bits(0x3ba3198a2e000000); // 2.02226624871116645580e-21
    const PIO2_3T: f64 = f64::from_bits(0x397b839a252049c1); // 8.47842766036889956997e-32
    let exponent = |v: f64| (v.to_bits() >> 52) & 0x7ff;

    // Nearest integer to x × 2/π, ties to even, without a libm call.
    let n = (x * INV_PIO2 + TOINT) - TOINT;
    let mut r = x - n * PIO2_1;
    let mut w = n * PIO2_1T; // good to 85 bits
    let mut hi = r - w;
    if exponent(x).saturating_sub(exponent(hi)) > 16 {
        // x is near a multiple of π/2: cancellation, so reduce again.
        let t = r;
        w = n * PIO2_2;
        r = t - w;
        w = n * PIO2_2T - ((t - r) - w); // good to 118 bits
        hi = r - w;
        if exponent(x).saturating_sub(exponent(hi)) > 49 {
            let t = r;
            w = n * PIO2_3;
            r = t - w;
            w = n * PIO2_3T - ((t - r) - w); // good to 151 bits
            hi = r - w;
        }
    }
    let lo = (r - hi) - w;
    let (s, c) = (k_sin(hi, lo), k_cos(hi, lo));
    match (n as i64) & 3 {
        0 => (s, c),
        1 => (c, -s),
        2 => (-s, -c),
        _ => (-c, s),
    }
}

/// sin(x + y) for |x| ≤ π/4, y the tail of x.
fn k_sin(x: f64, y: f64) -> f64 {
    const S1: f64 = f64::from_bits(0xbfc5555555555549); // -1.66666666666666324348e-1
    const S2: f64 = f64::from_bits(0x3f8111111110f8a6); // 8.33333333332248946124e-3
    const S3: f64 = f64::from_bits(0xbf2a01a019c161d5); // -1.98412698298579493134e-4
    const S4: f64 = f64::from_bits(0x3ec71de357b1fe7d); // 2.75573137070700676789e-6
    const S5: f64 = f64::from_bits(0xbe5ae5e68a2b9ceb); // -2.50507602534068634195e-8
    const S6: f64 = f64::from_bits(0x3de5d93a5acfd57c); // 1.58969099521155010221e-10
    let z = x * x;
    let w = z * z;
    let r = S2 + z * (S3 + z * S4) + z * w * (S5 + z * S6);
    let v = z * x;
    x - ((z * (0.5 * y - v * r) - y) - v * S1)
}

/// cos(x + y) for |x| ≤ π/4, y the tail of x.
fn k_cos(x: f64, y: f64) -> f64 {
    const C1: f64 = f64::from_bits(0x3fa555555555554c); // 4.16666666666666019037e-2
    const C2: f64 = f64::from_bits(0xbf56c16c16c15177); // -1.38888888888741095749e-3
    const C3: f64 = f64::from_bits(0x3efa01a019cb1590); // 2.48015872894767294178e-5
    const C4: f64 = f64::from_bits(0xbe927e4f809c52ad); // -2.75573143513906633035e-7
    const C5: f64 = f64::from_bits(0x3e21ee9ebdb4b1c4); // 2.08757232129817482790e-9
    const C6: f64 = f64::from_bits(0xbda8fae9be8838d4); // -1.13596475577881948265e-11
    let z = x * x;
    let w = z * z;
    let r = z * (C1 + z * (C2 + z * C3)) + w * w * (C4 + z * (C5 + z * C6));
    let hz = 0.5 * z;
    let w = 1.0 - hz;
    w + (((1.0 - w) - hz) + (z * r - x * y))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fdlibm promises under one ulp. The expected values are the correctly
    /// rounded results, computed with `bc -l` at 70 digits.
    #[test]
    fn within_an_ulp_of_known_values() {
        let cases = [
            (0.5, 0x3fdeaee8744b05f0, 0x3fec1528065b7d50),
            (1.0, 0x3feaed548f090cee, 0x3fe14a280fb5068c),
            (3.0, 0x3fc210386db6d55b, 0xbfefae04be85e5d2),
            (-2.5, 0xbfe326af0dcfcab1, 0xbfe9a2f7ef858b7d),
            (1e-8, 0x3e45798ee2308c3a, 0x3ff0000000000000),
            // Near multiples of π/2 the reduction must not lose the small result.
            (
                core::f64::consts::FRAC_PI_2,
                0x3ff0000000000000,
                0x3c91a62633145c07,
            ),
            (
                core::f64::consts::PI,
                0x3ca1a62633145c07,
                0xbff0000000000000,
            ),
            (
                -core::f64::consts::FRAC_PI_4,
                0xbfe6a09e667f3bcc,
                0x3fe6a09e667f3bcd,
            ),
            (1.0471975511965976, 0x3febb67ae8584caa, 0x3fe0000000000001),
            (1000.0, 0x3fea75cc150a206b, 0x3fe1ff026793f1bb),
        ];
        let ulps = |got: f64, want: u64| {
            let want = f64::from_bits(want);
            (got - want).abs() / (f64::from_bits(want.abs().to_bits() + 1) - want.abs())
        };
        for (x, sin, cos) in cases {
            let (s, c) = sin_cos(x);
            assert!(ulps(s, sin) <= 1.0, "sin {x}: {s:e}");
            assert!(ulps(c, cos) <= 1.0, "cos {x}: {c:e}");
        }
    }
}

//! Trigonometry that gives the same bits on every target.
//!
//! Platform `sin` and `cos` may differ in the last bit between native targets and
//! wasm32, and compiled charts are content-hashed. These functions use only
//! IEEE 754 `+ − × ÷` and `round`, which are exact or correctly rounded
//! everywhere (Rust never fuses them into FMA), so every target computes the same
//! result. The kernels are the fdlibm ones, as used by musl.
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

/// `(sin x, cos x)`, within an ulp or two of the true value for `|x| ≤ 2^19`.
pub fn sin_cos(x: f64) -> (f64, f64) {
    const INV_PIO2: f64 = f64::from_bits(0x3fe45f306dc9c883); // 6.36619772367581382433e-1
    // π/2 split so that `n × PIO2_1` is exact for |n| < 2^20.
    const PIO2_1: f64 = f64::from_bits(0x3ff921fb54400000); // 1.57079632673412561417
    const PIO2_1T: f64 = f64::from_bits(0x3dd0b4611a626331); // 6.07710050650619224932e-11

    let n = (x * INV_PIO2).round();
    let r = x - n * PIO2_1;
    let w = n * PIO2_1T;
    let hi = r - w;
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

    #[test]
    fn matches_std_closely() {
        let mut x = -20.0;
        while x < 20.0 {
            let (s, c) = sin_cos(x);
            assert!((s - x.sin()).abs() <= 4.0 * f64::EPSILON, "sin {x}");
            assert!((c - x.cos()).abs() <= 4.0 * f64::EPSILON, "cos {x}");
            x += 0.000_731;
        }
    }
}

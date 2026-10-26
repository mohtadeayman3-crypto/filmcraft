//! ACES (Academy Color Encoding System) colour science: primaries, white point, chromatic adaptation,
//! normalised primary matrices, and transfer curves (ACEScg, ACEScct, ACEScc).
//!
//! Specifications and standards:
//! - SMPTE ST 2065-1:2012: Academy Color Encoding Specification (ACES2065-1 / AP0).
//! - AMPAS S-2014-004: ACEScg — A Working Space for CGI Render and Compositing (AP1).
//! - AMPAS S-2014-003: ACEScc — A Logarithmic Encoding of ACES Data for Use within Color Grading Systems.
//! - AMPAS S-2016-001: ACEScct — A Quasi-Logarithmic Encoding of ACES Data for Use within Color Grading Systems.
//! - K. M. Lam, "Metamerism and Colour Constancy", PhD thesis, University of Bradford (1985); ICC.1 Annex E.

use crate::spaces::{self, Mat3, inverse, mul, mul_vec, npm};

/// ACES white point chromaticity coordinates (x, y) ≈ D60.
/// Source: SMPTE ST 2065-1:2012; AMPAS S-2014-004.
pub const ACES_WHITE: [f64; 2] = [0.32168, 0.33767];

/// AP0 chromaticity coordinates: R, G, B in (x, y).
/// Source: SMPTE ST 2065-1:2012.
pub const AP0_PRIMARIES: [[f64; 2]; 3] = [
    [0.7347, 0.2653],
    [0.0000, 1.0000],
    [0.0001, -0.0770],
];

/// AP1 chromaticity coordinates: R, G, B in (x, y).
/// Source: AMPAS S-2014-004 (ACEScg).
pub const AP1_PRIMARIES: [[f64; 2]; 3] = [
    [0.713, 0.293],
    [0.165, 0.830],
    [0.128, 0.044],
];

/// Bradford cone response matrix (M_B).
/// Source: K. M. Lam (1985); reproduced in ICC.1 Annex E.
pub const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// ACEScct constants.
/// Source: AMPAS S-2016-001.
pub const ACESCCT_X_BRK: f64 = 0.0078125;
pub const ACESCCT_Y_BRK: f64 = 0.155251141552511;
pub const ACESCCT_A: f64 = 10.5402377416545;
pub const ACESCCT_B: f64 = 0.0729055341958355;

/// Half-float maximum value (65504.0), clamped by ACEScc and ACEScct decode.
pub const HALF_MAX: f64 = 65504.0;

/// Compute the Bradford chromatic adaptation transform (CAT) matrix from `src_white` to `dst_white`.
///
/// Returns the 3×3 matrix such that `XYZ_dst = CAT · XYZ_src`.
/// When white points match within 1e-9, returns the identity matrix.
pub fn bradford_cat(src_white: [f64; 2], dst_white: [f64; 2]) -> Mat3 {
    if (src_white[0] - dst_white[0]).abs() < 1e-9 && (src_white[1] - dst_white[1]).abs() < 1e-9 {
        return spaces::IDENTITY;
    }
    let to_xyz = |c: [f64; 2]| [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]];
    let ws = mul_vec(&BRADFORD, to_xyz(src_white));
    let wd = mul_vec(&BRADFORD, to_xyz(dst_white));
    let diag: Mat3 = [
        [wd[0] / ws[0], 0.0, 0.0],
        [0.0, wd[1] / ws[1], 0.0],
        [0.0, 0.0, wd[2] / ws[2]],
    ];
    let inv_b = inverse(&BRADFORD);
    mul(&inv_b, &mul(&diag, &BRADFORD))
}

/// Derive the AP0 → CIE XYZ normalised primary matrix from published chromaticities and white point.
/// Source formula: SMPTE RP 177.
pub fn ap0_to_xyz() -> Mat3 {
    npm(AP0_PRIMARIES, ACES_WHITE)
}

/// Derive the AP1 (ACEScg) → CIE XYZ normalised primary matrix from published chromaticities and white point.
/// Source formula: SMPTE RP 177.
pub fn ap1_to_xyz() -> Mat3 {
    npm(AP1_PRIMARIES, ACES_WHITE)
}

/// Derive the AP0 (ACES2065-1) → AP1 (ACEScg) conversion matrix.
/// Computed as: `NPM_AP1⁻¹ · NPM_AP0`.
pub fn ap0_to_ap1() -> Mat3 {
    mul(&inverse(&ap1_to_xyz()), &ap0_to_xyz())
}

/// Derive the AP1 (ACEScg) → AP0 (ACES2065-1) conversion matrix.
/// Computed as: `NPM_AP0⁻¹ · NPM_AP1`.
pub fn ap1_to_ap0() -> Mat3 {
    mul(&inverse(&ap0_to_xyz()), &ap1_to_xyz())
}

/// AP1 (ACEScg) luminance weights: the Y row of [`ap1_to_xyz`].
pub fn ap1_luma() -> [f64; 3] {
    ap1_to_xyz()[1]
}

/// ACEScg forward curve (linear scene-referred light, identity).
#[inline]
pub fn acescg_encode(lin: f64) -> f64 {
    lin
}

/// ACEScg inverse curve (linear scene-referred light, identity).
#[inline]
pub fn acescg_decode(cg: f64) -> f64 {
    cg
}

/// ACEScct forward curve: encodes scene-linear ACEScg light to logarithmic ACEScct code values.
/// Source: AMPAS S-2016-001.
#[inline]
pub fn acescct_encode(lin: f64) -> f64 {
    if lin <= ACESCCT_X_BRK {
        ACESCCT_A * lin + ACESCCT_B
    } else {
        (lin.log2() + 9.72) / 17.52
    }
}

/// ACEScct inverse curve: decodes logarithmic ACEScct code values to scene-linear ACEScg light.
/// Source: AMPAS S-2016-001.
#[inline]
pub fn acescct_decode(cct: f64) -> f64 {
    if cct <= ACESCCT_Y_BRK {
        (cct - ACESCCT_B) / ACESCCT_A
    } else if cct < (HALF_MAX.log2() + 9.72) / 17.52 {
        2.0f64.powf(cct * 17.52 - 9.72)
    } else {
        HALF_MAX
    }
}

/// ACEScc forward curve: encodes scene-linear ACEScg light to pure-logarithmic ACEScc code values.
/// Source: AMPAS S-2014-003.
#[inline]
pub fn acescc_encode(lin: f64) -> f64 {
    if lin <= 0.0 {
        (-16.0 + 9.72) / 17.52
    } else if lin < 2.0f64.powi(-15) {
        ((2.0f64.powi(-16) + lin * 0.5).log2() + 9.72) / 17.52
    } else {
        (lin.log2() + 9.72) / 17.52
    }
}

/// ACEScc inverse curve: decodes logarithmic ACEScc code values to scene-linear ACEScg light.
/// Source: AMPAS S-2014-003.
#[inline]
pub fn acescc_decode(cc: f64) -> f64 {
    if cc < (9.72 - 15.0) / 17.52 {
        (2.0f64.powf(cc * 17.52 - 9.72) - 2.0f64.powi(-16)) * 2.0
    } else if cc < (HALF_MAX.log2() + 9.72) / 17.52 {
        2.0f64.powf(cc * 17.52 - 9.72)
    } else {
        HALF_MAX
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Published AP0 → XYZ NPM from SMPTE ST 2065-1:2012 / AMPAS S-2014-004.
    pub const AMPAS_AP0_TO_XYZ: Mat3 = [
        [0.9525523959, 0.0000000000, 0.0000936786],
        [0.3439664498, 0.7281660966, -0.0721325464],
        [0.0000000000, 0.0000000000, 1.0088251844],
    ];

    /// Published AP1 → XYZ NPM from AMPAS S-2014-004.
    pub const AMPAS_AP1_TO_XYZ: Mat3 = [
        [0.6624541811, 0.1340042065, 0.1561876870],
        [0.2722287168, 0.6740817658, 0.0536895174],
        [-0.0055746495, 0.0040607335, 1.0103391003],
    ];

    /// Published AP0 → AP1 matrix from AMPAS S-2014-004.
    pub const AMPAS_AP0_TO_AP1: Mat3 = [
        [1.4514393161, -0.2365107469, -0.2149285693],
        [-0.0765537734, 1.1762296998, -0.0996759264],
        [0.0083161484, -0.0060324498, 0.9977163014],
    ];

    /// Published AP1 → AP0 matrix from AMPAS S-2014-004.
    pub const AMPAS_AP1_TO_AP0: Mat3 = [
        [0.6954522414, 0.1406786965, 0.1638690622],
        [0.0447945634, 0.8596711185, 0.0955343182],
        [-0.0055258826, 0.0040252103, 1.0015006723],
    ];

    fn mat_max_diff(a: &Mat3, b: &Mat3) -> f64 {
        let mut d = 0.0f64;
        for i in 0..3 {
            for j in 0..3 {
                d = d.max((a[i][j] - b[i][j]).abs());
            }
        }
        d
    }

    #[test]
    fn matrix_cross_check_against_published_ampas() {
        let ap0_xyz = ap0_to_xyz();
        let diff_ap0_xyz = mat_max_diff(&ap0_xyz, &AMPAS_AP0_TO_XYZ);
        println!("AP0 -> XYZ max difference from published: {:.10e}", diff_ap0_xyz);
        assert!(
            diff_ap0_xyz < 1e-6,
            "AP0 -> XYZ derivation differs from AMPAS published matrix: {:.10e}",
            diff_ap0_xyz
        );

        let ap1_xyz = ap1_to_xyz();
        let diff_ap1_xyz = mat_max_diff(&ap1_xyz, &AMPAS_AP1_TO_XYZ);
        println!("AP1 -> XYZ max difference from published: {:.10e}", diff_ap1_xyz);
        assert!(
            diff_ap1_xyz < 1e-6,
            "AP1 -> XYZ derivation differs from AMPAS published matrix: {:.10e}",
            diff_ap1_xyz
        );

        let ap0_ap1 = ap0_to_ap1();
        let diff_ap0_ap1 = mat_max_diff(&ap0_ap1, &AMPAS_AP0_TO_AP1);
        println!("AP0 -> AP1 max difference from published: {:.10e}", diff_ap0_ap1);
        assert!(
            diff_ap0_ap1 < 1e-6,
            "AP0 -> AP1 derivation differs from AMPAS published matrix: {:.10e}",
            diff_ap0_ap1
        );

        let ap1_ap0 = ap1_to_ap0();
        let diff_ap1_ap0 = mat_max_diff(&ap1_ap0, &AMPAS_AP1_TO_AP0);
        println!("AP1 -> AP0 max difference from published: {:.10e}", diff_ap1_ap0);
        assert!(
            diff_ap1_ap0 < 1e-6,
            "AP1 -> AP0 derivation differs from AMPAS published matrix: {:.10e}",
            diff_ap1_ap0
        );
    }

    #[test]
    fn matrix_inverses_roundtrip() {
        let ap0_ap1 = ap0_to_ap1();
        let ap1_ap0 = ap1_to_ap0();
        let roundtrip = mul(&ap0_ap1, &ap1_ap0);
        assert!(mat_max_diff(&roundtrip, &spaces::IDENTITY) < 1e-12);

        let ap0_xyz = ap0_to_xyz();
        let xyz_ap0 = inverse(&ap0_xyz);
        assert!(mat_max_diff(&mul(&ap0_xyz, &xyz_ap0), &spaces::IDENTITY) < 1e-12);

        let ap1_xyz = ap1_to_xyz();
        let xyz_ap1 = inverse(&ap1_xyz);
        assert!(mat_max_diff(&mul(&ap1_xyz, &xyz_ap1), &spaces::IDENTITY) < 1e-12);
    }

    #[test]
    fn acescct_known_constants_and_continuity() {
        // Continuity at break-point
        let lin_brk = ACESCCT_X_BRK;
        let cct_from_lin = ACESCCT_A * lin_brk + ACESCCT_B;
        let cct_from_log = (lin_brk.log2() + 9.72) / 17.52;
        assert!(
            (cct_from_lin - cct_from_log).abs() < 1e-12,
            "ACEScct discontinuity at X_BRK: {} vs {}",
            cct_from_lin,
            cct_from_log
        );
        assert!(
            (cct_from_lin - ACESCCT_Y_BRK).abs() < 1e-12,
            "ACEScct Y_BRK mismatch: {} vs {}",
            cct_from_lin,
            ACESCCT_Y_BRK
        );

        // Derivative continuity (C1 smoothness) at X_BRK:
        // d/dlin (A * lin + B) = A
        // d/dlin ((log2(lin) + 9.72)/17.52) = 1 / (lin * ln(2) * 17.52)
        let log_deriv = 1.0 / (lin_brk * 2.0f64.ln() * 17.52);
        assert!(
            (ACESCCT_A - log_deriv).abs() < 1e-6,
            "ACEScct derivative mismatch at X_BRK: {} vs {}",
            ACESCCT_A,
            log_deriv
        );

        // 18% mid-grey
        let mid_grey_cct = acescct_encode(0.18);
        assert!(
            (mid_grey_cct - 0.4135884).abs() < 1e-6,
            "ACEScct 18% grey: expected ~0.4135884, got {}",
            mid_grey_cct
        );

        // 1.0 linear
        let one_cct = acescct_encode(1.0);
        let expected_one = 9.72 / 17.52; // log2(1) = 0
        assert!(
            (one_cct - expected_one).abs() < 1e-12,
            "ACEScct 1.0: expected {}, got {}",
            expected_one,
            one_cct
        );
        assert!((one_cct - 0.5547945).abs() < 1e-6);
    }

    #[test]
    fn acescc_known_constants() {
        // 18% mid-grey matches ACEScct
        let mid_grey_cc = acescc_encode(0.18);
        assert!((mid_grey_cc - 0.4135884).abs() < 1e-6);

        // 1.0 matches ACEScct
        let one_cc = acescc_encode(1.0);
        assert!((one_cc - 9.72 / 17.52).abs() < 1e-12);

        // Sub-zero clamp
        let min_cc = acescc_encode(-1.0);
        let expected_min = (-16.0 + 9.72) / 17.52;
        assert!((min_cc - expected_min).abs() < 1e-12);
        assert!((min_cc - (-0.3584475)).abs() < 1e-6);
    }

    #[test]
    fn acescct_roundtrip() {
        // Dense geometric grid from 1e-6 to 65504
        let mut v = 1e-6;
        while v <= HALF_MAX {
            let enc = acescct_encode(v);
            let dec = acescct_decode(enc);
            let err = (dec - v).abs() / v;
            assert!(
                err < 1e-10,
                "ACEScct roundtrip failed at {}: enc={}, dec={}, rel_err={}",
                v,
                enc,
                dec,
                err
            );
            v *= 1.15;
        }

        // Linear toe roundtrip down to -0.005
        for &v in &[-0.005, -0.001, 0.0, 0.001, 0.005, ACESCCT_X_BRK] {
            let enc = acescct_encode(v);
            let dec = acescct_decode(enc);
            assert!(
                (dec - v).abs() < 1e-12,
                "ACEScct linear toe roundtrip failed at {}: enc={}, dec={}",
                v,
                enc,
                dec
            );
        }
    }

    #[test]
    fn acescc_roundtrip() {
        // Values from 2^-14 to 65504
        let mut v = 2.0f64.powi(-14);
        while v <= HALF_MAX {
            let enc = acescc_encode(v);
            let dec = acescc_decode(enc);
            let err = (dec - v).abs() / v;
            assert!(
                err < 1e-10,
                "ACEScc roundtrip failed at {}: enc={}, dec={}, rel_err={}",
                v,
                enc,
                dec,
                err
            );
            v *= 1.15;
        }

        // Sub-2^-15 region
        for &v in &[2.0f64.powi(-16), 2.0f64.powf(-15.5)] {
            let enc = acescc_encode(v);
            let dec = acescc_decode(enc);
            let err = (dec - v).abs() / v;
            assert!(err < 1e-10, "ACEScc toe roundtrip failed at {}", v);
        }
    }

    #[test]
    fn bradford_cat_properties() {
        // Identity when source and destination whites match
        let d65 = spaces::D65;
        assert!(mat_max_diff(&bradford_cat(d65, d65), &spaces::IDENTITY) < 1e-12);
        assert!(mat_max_diff(&bradford_cat(ACES_WHITE, ACES_WHITE), &spaces::IDENTITY) < 1e-12);

        // Inverting white points inverts the matrix
        let fwd = bradford_cat(d65, ACES_WHITE);
        let rev = bradford_cat(ACES_WHITE, d65);
        let roundtrip = mul(&fwd, &rev);
        assert!(mat_max_diff(&roundtrip, &spaces::IDENTITY) < 1e-12);
    }
}

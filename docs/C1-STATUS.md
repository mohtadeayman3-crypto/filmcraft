# Task C1 — GPU Lumetri Status

## Completed Steps

- **C1.0:** Design document (`docs/C1-DESIGN.md`), including section 7.3 (`C1.7: external LUTs on the GPU`).
- **C1.1:** GPU Lumetri Basic Correction + HDR Grading Space (`Rec2100Pq`, `Rec2100Hlg`, `hdr_white`, `hdr_specular`) + `LumetriOp` struct refactor.

## Parity Tolerance Notes

- **SDR Basic Correction, Creative Adjustments, and Vignette (`gpu_effects_match_cpu_exactly`):**
  - All SDR cases pass within `EXACT = 1e-4` and `flips == 0.0` (measured max relative difference `1.64e-6` to `6.87e-5`).
- **HDR `Rec2100Hlg` Basic Correction (`gpu_lumetri_hdr_basic_correction_matches_cpu`):**
  - Passes within `EXACT = 1e-4` and `flips == 0.0` (measured max relative difference `2.87e-6` and `1.15e-6`).
- **HDR `Rec2100Pq` Basic Correction (`gpu_lumetri_hdr_basic_correction_matches_cpu`):**
  - **Measured value:** `2.22e-4` (case 1) and `6.71e-5` (case 2), `flips == 0.0`.
  - **Tolerance used:** `3e-4` (`flips == 0.0`).
  - **Reason:** SMPTE ST 2084 (PQ) encode (`pq_inv_eotf1`) followed by decode (`pq_eotf1`) raises intermediate quantities to exponents $m_2 = 78.84375$ and $1 / m_1 = 6.277395$. Because GPU shader hardware evaluates `pow(x, y)` via `exp2(y * log2(x))`, a 1–2 ULP rounding difference in `log2`/`exp2` is amplified by $m_2 / m_1 \approx 495\times$ on steep portions of the PQ curve.

## Remaining Steps

1. **C1.2:** Color Wheels (`wheel_shadows`, `wheel_midtones`, `wheel_highlights`, luma sliders) in `OP_LUMETRI_ADV`.
2. **C1.3:** RGB & Hue/Sat Curves (`curve_luma`, `curve_red`, `curve_green`, `curve_blue`, `hue_vs_sat`, `hue_vs_hue`, `hue_vs_luma`, `luma_vs_sat`, `sat_vs_sat`) via 1024×9 LUT texture on `@binding(3)` (`aux`).
3. **C1.4:** Creative Looks (`1..=8`, `look_intensity`) & Creative Sharpen (`unsharp` reuse).
4. **C1.5:** HSL Secondary with Matte Refinement (`OP_LUMETRI_HSL_KEY`, `OP_LUMETRI_HSL_DENOISE`, `OP_BOX`, `OP_LUMETRI_HSL_APPLY`).
5. **C1.6:** Vignette & Full Combined 6-Section Chain Parity + workspace test verification.
6. **Phase 3:** `crates/gpu/examples/lumetri_bench.rs` (GPU grading only, end-to-end 4K with upload/readback, playback FPS, and scopes refresh time).

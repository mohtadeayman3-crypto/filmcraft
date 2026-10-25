# Task C1 — GPU Lumetri Status

## Completed Steps

- **C1.0 (`4120bead`):** Design document (`docs/C1-DESIGN.md`), including section 7.3 (`C1.7: external LUTs on the GPU`).
- **C1.1 (`8ed7cdc1`):** GPU Lumetri Basic Correction + HDR Grading Space (`Rec2100Pq`, `Rec2100Hlg`, `hdr_white`, `hdr_specular`) + `LumetriOp` struct refactor.
- **C1.2 (`987b4ec0`):** GPU Lumetri Color Wheels (`wheel_shadows`, `wheel_midtones`, `wheel_highlights`, luma sliders) in `OP_LUMETRI_ADV`.
- **C1.3 (`bccae632`):** GPU Lumetri RGB & Hue/Sat Curves (`curve_luma`, `curve_red`, `curve_green`, `curve_blue`, `hue_vs_sat`, `hue_vs_hue`, `hue_vs_luma`, `luma_vs_sat`, `sat_vs_sat`) via a `1024×9` `Rgba32Float` LUT texture bound to `@binding(3)` (`aux`).
- **C1.4 (`4479d17e`):** GPU Lumetri Creative Looks (`1..=8` procedural looks + `look_intensity`) & Creative Sharpen (`OP_BOX` + `OP_UNSHARP` reuse).
- **C1.5 (`9f29a7e8`):** GPU Lumetri HSL Secondary with Matte Refinement (`OP_LUMETRI_HSL_KEY`, `OP_LUMETRI_HSL_DENOISE` 2D median filter, `OP_BOX` separable blur, `OP_LUMETRI_HSL_APPLY` with `hsl_show_mask` modes `0..=3`).
- **C1.6:** GPU Lumetri Vignette extremes (`+100` circle, `-100` oval, `0` feather, positive amount), full 6-section Lumetri chain parity tests (`SDR` and `HDR`), and Phase 3 extended 4K GPU benchmark (`crates/gpu/examples/lumetri_bench.rs`, keeping `crates/render/examples/lumetri_bench.rs` untouched).

## Parity Tolerance Notes

- **SDR Basic Correction, Color Wheels, Curves, Creative Looks (`1..=8`), Creative Sharpen, HSL Secondary (Key, Denoise, Blur, Correction, Mask Preview `1..=3`), Vignette, and Combined 6-Section Chain (`gpu_effects_match_cpu_exactly`):**
  - All SDR single-section and combined 6-section cases pass within `EXACT = 1e-4` (`max <= 1e-4`, `flips == 0.0`).
  - Measured max relative differences across all SDR Lumetri parity cases: `0.0` (exact bitwise match on HSL mask preview) to `4.29e-5` (full 6-section chain with sharpen, denoise, and blur).
- **HDR `Rec2100Hlg` Basic Correction & Full Chain (`gpu_lumetri_hdr_basic_correction_matches_cpu`):**
  - Passes within `EXACT = 1e-4` (`flips == 0.0`, measured max relative difference `1.15e-6` to `1.12e-5`).
- **HDR `Rec2100Pq` Basic Correction & Full Chain (`gpu_lumetri_hdr_basic_correction_matches_cpu`):**
  - **Measured value:** `2.22e-4` (Basic Correction case 1), `6.71e-5` (Basic Correction case 2), and `1.44e-4` (Full 6-section HDR PQ chain), `flips == 0.0`.
  - **Tolerance used:** `3e-4` (`flips == 0.0`).
  - **Reason:** SMPTE ST 2084 (PQ) encode (`pq_inv_eotf1`) followed by decode (`pq_eotf1`) raises intermediate quantities to exponents $m_2 = 78.84375$ and $1 / m_1 = 6.277395$. Because GPU shader hardware evaluates `pow(x, y)` via `exp2(y * log2(x))`, a 1–2 ULP rounding difference in `log2`/`exp2` is amplified by $m_2 / m_1 \approx 495\times$ on steep portions of the PQ curve.

## Phase 3 Benchmark Results (`3840×2160`, NVIDIA GeForce RTX 5060 Ti, Vulkan, 20 Rayon Threads)

Command: `cargo run --release -p filmcraft-gpu --example lumetri_bench`

### (a) GPU Grading Time Only (`3840×2160`, source cached in VRAM, no CPU readback)

| Workload | Compute Passes | Median (ms) | Mean (ms) | Min / Max (ms) | Throughput (FPS) | Speedup vs CPU (`294.64 ms`) |
|---|---:|---:|---:|---:|---:|---:|
| Ungraded 4K composite + sRGB resolve (baseline) | 0 | **0.57 ms** | 0.58 ms | 0.55 / 0.62 ms | 1752.8 fps | — |
| GPU Lumetri (all 6 sections, per-pixel, `sharpen=0`) | 3 | **8.03 ms** | 8.41 ms | 7.92 / 12.96 ms | **124.6 fps** | **36.7×** |
| GPU Lumetri (all 6 sections, `sharpen=20`) + composite + resolve | 10 | **19.63 ms** | 19.74 ms | 19.10 / 20.60 ms | **50.9 fps** | **15.0×** |
| Net GPU Lumetri `FxStage` compute passes (`sharpen=20`) | 10 | **19.06 ms** | — | — | **52.5 fps** | **15.5×** |
| GPU Lumetri (6 sections + `sharpen=20` + HSL Denoise `10` & Blur `10`) | 18 | **38.53 ms** | 38.55 ms | 37.18 / 40.55 ms | **26.0 fps** | **7.6×** |
| CPU Reference (Rayon 20 threads, `3840×2160` f32, `sharpen=20`) | — | **294.64 ms** | 304.81 ms | 281.87 / 330.93 ms | **3.4 fps** | 1.0× |

### (b) End-to-End 4K Frame Time Including Upload and Readback (uncached frame)

| Path | Median (ms) | Mean (ms) | Min / Max (ms) | Throughput (FPS) | Notes |
|---|---:|---:|---:|---:|---|
| `GpuCompositor::effect_image` (`RgbaF32` → `f16` + 66 MB upload + GPU grade + 132.7 MB `Rgba32Float` readback) | **68.09 ms** | 68.00 ms | 61.64 / 75.37 ms | **14.7 fps** | Upload + 132.7 MB f32 staging readback + CPU copy takes **~48.5 ms (71%)**; GPU grading is **19.6 ms (29%)**. |
| `prepare` + `render_export_prepared` (`RgbaF32` → `RgbaF32` export path) | **50.99 ms** | 50.79 ms | 44.58 / 60.21 ms | **19.6 fps** | Uses `Rgba16Float` (66.4 MB) accumulator readback. |
| ├─ CPU `prepare` (`3840×2160` `f32 → f16` conversion) | **4.04 ms** | 4.06 ms | 3.69 / 4.43 ms | 247.8 fps | Runs on background worker thread before GPU upload. |
| └─ Upload (66.4 MB) + GPU grade + `Rgba16Float` readback & `f16 → f32` convert | **47.15 ms** | 46.73 ms | 40.60 / 55.94 ms | 21.2 fps | Upload + readback + `f16 → f32` takes **~27.5 ms (54% of total)**; GPU grading is **19.6 ms (38%)**. |
| `prepare` + `render_export_prepared` (`4K YUV8 4:2:0` → `RgbaF32` export path) | **43.33 ms** | 42.66 ms | 39.20 / 46.17 ms | **23.1 fps** | 12.4 MB YUV upload + GPU grade (`21.3 ms`) + 66.4 MB `Rgba16Float` readback & `f16 → f32` convert (`22.1 ms`, **51%**). **6.8× faster** than CPU grading alone. |

### (c) Playback FPS of Playing a 4K Clip in the App with the Grade Applied (no CPU readback)

| Playback Stream (`3840×2160` uncached frames → `GpuCompositor` display texture) | Median (ms) | Mean (ms) | Min / Max (ms) | Playback FPS |
|---|---:|---:|---:|---:|
| **4K YUV 4:2:0 8-bit + full 6-section Lumetri (`sharpen=20`)** | **21.27 ms** | 22.38 ms | 20.34 / 29.86 ms | **47.0 fps** |
| **4K YUV 4:2:0 8-bit + 6-section Lumetri + HSL Denoise (`10`) & Blur (`10`)** | **39.15 ms** | 39.15 ms | 38.66 / 39.77 ms | **25.5 fps** |
| **4K `RgbaF32` + full 6-section Lumetri (worker `prepare` + GPU composite)** | **29.92 ms** | 30.02 ms | 28.92 / 31.06 ms | **33.4 fps** |
| └─ UI thread portion only (66.4 MB `f16` upload + GPU grade + resolve) | **24.55 ms** | 24.78 ms | 23.63 / 26.18 ms | **40.7 fps** |

### (d) Follow-Up Item: Scopes Refresh Time on a Graded 4K Clip (`Target::Sequence` CPU Path)

| Scopes Refresh Path | Resolution | Median (ms) | Mean (ms) | Min / Max (ms) | Refresh Rate (Hz) |
|---|---|---:|---:|---:|---:|
| **UI Scopes panel SDR refresh (`Target::Sequence`, `scale = 0.25` + 4 scopes)** | `960×540` | **35.38 ms** | 35.68 ms | 32.54 / 40.01 ms | **28.3 Hz** |
| ├─ CPU `render_sequence` (`scale = 0.25` CPU Lumetri + `over_black_rgba8`) | `960×540` | **23.84 ms** | 23.94 ms | 21.97 / 25.47 ms | 41.9 Hz |
| └─ `Signal::from_rgba8` + compute & paint 4 scopes (Waveform, Parade, Vectorscope, Histogram) | `480×270` | **11.23 ms** | 11.74 ms | 10.29 / 15.13 ms | 89.0 Hz |
| **UI Scopes panel HDR Rec. 2100 refresh (`scale = 0.125`, `working_output` + 4 scopes)** | `480×270` | **23.69 ms** | 23.85 ms | 23.10 / 24.73 ms | **42.2 Hz** |
| **Engine `scopes.read` default (`scale = 0.5` CPU render + 4 scopes)** | `1920×1080` | **89.59 ms** | 91.66 ms | 84.40 / 104.82 ms | **11.2 Hz** |

- **Follow-up observation:** Two-thirds (**23.84 ms / 35.38 ms = 67%**) of a UI scopes refresh on a graded 4K clip is spent re-rendering the `960×540` frame on the CPU through `Target::Sequence`. Feeding the scopes from a downscaled GPU output readback (or a `480×270` GPU render) would eliminate the redundant CPU Lumetri pass and cut SDR scopes refresh time from **35.4 ms** to **~12 ms** (>80 Hz).

# Task C1 — GPU Lumetri Status

## Completed Steps

- **C1.0 (`4120bead`):** Design document (`docs/C1-DESIGN.md`), including section 7.3 (`C1.7: external LUTs on the GPU`).
- **C1.1 (`8ed7cdc1`):** GPU Lumetri Basic Correction + HDR Grading Space (`Rec2100Pq`, `Rec2100Hlg`, `hdr_white`, `hdr_specular`) + `LumetriOp` struct refactor.
- **C1.2 (`987b4ec0`):** GPU Lumetri Color Wheels (`wheel_shadows`, `wheel_midtones`, `wheel_highlights`, luma sliders) in `OP_LUMETRI_ADV`.
- **C1.3 (`bccae632`):** GPU Lumetri RGB & Hue/Sat Curves (`curve_luma`, `curve_red`, `curve_green`, `curve_blue`, `hue_vs_sat`, `hue_vs_hue`, `hue_vs_luma`, `luma_vs_sat`, `sat_vs_sat`) via a `1024×9` `Rgba32Float` LUT texture bound to `@binding(3)` (`aux`).
- **C1.4 (`4479d17e`):** GPU Lumetri Creative Looks (`1..=8` procedural looks + `look_intensity`) & Creative Sharpen (`OP_BOX` + `OP_UNSHARP` reuse).
- **C1.5 (`9f29a7e8`):** GPU Lumetri HSL Secondary with Matte Refinement (`OP_LUMETRI_HSL_KEY`, `OP_LUMETRI_HSL_DENOISE` 2D median filter, `OP_BOX` separable blur, `OP_LUMETRI_HSL_APPLY` with `hsl_show_mask` modes `0..=3`).
- **C1.6 (`0ce7779c`):** GPU Lumetri Vignette extremes (`+100` circle, `-100` oval, `0` feather, positive amount), full 6-section Lumetri chain parity tests (`SDR` and `HDR`), and Phase 3 extended 4K GPU benchmark (`crates/gpu/examples/lumetri_bench.rs`, keeping `crates/render/examples/lumetri_bench.rs` untouched).
- **C1.9 (`acb7f49e`):** GPU effects (`LayerFx`) for CPU-decoded log/camera clips (such as Sony `S-Log3 / S-Gamut3.Cine`) in `crates/render/src/plan.rs`.
- **C1.8 (`c871ebff`):** Real-footage 4K Sony ZV-E10 S-Log3 (`C1102.MP4`) CPU vs GPU A/B check (`max = 1/255`, `mean = 0.0169/255`, `95.02%` bitwise identical, visually identical) and live desktop app GPU playback verification (`25.0 fps` clip at `60.17 fps` UI repaint, `0` dropped frames). See [`docs/c1-ab/README.md`](c1-ab/README.md).
- **C1.7:** GPU Lumetri external `.cube`/`.3dl` and built-in LUTs (`input_lut` and `look_lut`) via `OP_LUMETRI_LUT` (`36u`) with tetrahedral 3D interpolation and optional 1D shaper table packed into `@binding(3)` (`aux`).

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
- **C1.7 External `.cube` / `.3dl` and Built-in LUTs (`gpu_lumetri_external_cube_and_3dl_luts_match_cpu`, `gpu_effects_match_cpu_exactly`):**
  - All SDR (`Rec709`) and `Rec2100Hlg` cases (including a `.cube` with both a 32-point 1D shaper over non-unit `DOMAIN_MIN`/`DOMAIN_MAX` `[-0.05, 1.15]` and a $17^3$ 3D cube, a 12-bit `.3dl` LUT, and built-in $33^3$ LUTs `builtin:slog3-sgamut3cine-to-rec709` + `builtin:look-teal-orange`) pass within `EXACT = 1e-4` (`flips == 0.0`), and `Rec2100Pq` passes within `3e-4` (`flips == 0.0`).
  - When `look_lut` is followed by Color Wheels or Curves (`OP_LUMETRI_ADV`), `OP_LUMETRI_LUT` sets `skip_decode = 1` and `OP_LUMETRI_ADV` sets `skip_encode = 1` (`u.p0.w = 1.0`), avoiding an intermediate `gs_decode` + `gs_encode` round-trip and preserving unclamped grading-space values between the Look LUT and Wheels/Curves exactly like `effects::lumetri_advanced`.

## Phase 3 Benchmark Results (`3840×2160`, NVIDIA GeForce RTX 5060 Ti, Vulkan, 20 Rayon Threads)

Command: `cargo run --release -p filmcraft-gpu --example lumetri_bench`

### (a) GPU Grading Time Only (`3840×2160`, source cached in VRAM, no CPU readback)

| Workload | Compute Passes | Median (ms) | Mean (ms) | Min / Max (ms) | Throughput (FPS) | Speedup vs CPU (`294.64 ms`) |
|---|---:|---:|---:|---:|---:|---:|
| Ungraded 4K composite + sRGB resolve (baseline) | 0 | **0.57 ms** | 0.58 ms | 0.55 / 0.62 ms | 1752.8 fps | — |
| GPU Lumetri (all 6 sections, per-pixel, `sharpen=0`) | 3 | **8.03 ms** | 8.41 ms | 7.92 / 12.96 ms | **124.6 fps** | **36.7×** |
| GPU Lumetri (all 6 sections, `sharpen=20`) + composite + resolve | 10 | **19.63 ms** | 19.74 ms | 19.10 / 20.60 ms | **50.9 fps** | **15.0×** |
| Net GPU Lumetri `FxStage` compute passes (`sharpen=20`) | 10 | **19.06 ms** | — | — | **52.5 fps** | **15.5×** |
| GPU Lumetri (6 sections + `sharpen=20` + Input $33^3$ LUT + Look $33^3$ LUT) | 12 | **24.55 ms** | 24.55 ms | 24.07 / 25.12 ms | **40.7 fps** | **13.5×** (vs `331.80 ms` CPU+LUTs) |
| GPU Lumetri (6 sections + `sharpen=20` + HSL Denoise `10` & Blur `10`) | 18 | **38.53 ms** | 38.55 ms | 37.18 / 40.55 ms | **26.0 fps** | **7.6×** |
| CPU Reference (Rayon 20 threads, `3840×2160` f32, `sharpen=20`) | — | **294.64 ms** | 304.81 ms | 281.87 / 330.93 ms | **3.4 fps** | 1.0× |
| CPU Reference + Input $33^3$ LUT + Look $33^3$ LUT (Rayon 20 threads, `3840×2160` f32) | — | **331.80 ms** | 331.15 ms | 322.21 / 338.99 ms | **3.0 fps** | 1.0× |

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

## C1.9: GPU Effects (`LayerFx`) for Log / Camera Clips (`crates/render/src/plan.rs`)

### What kept log clips on the CPU path before
In `crates/render/src/plan.rs` (`push_item`), a clip entered the GPU effect stage only when `plain && !chain.is_empty()`, where:
```rust
let plain =
    (input.is_some() || !crate::colorman::needs_management(pipe, cs, &frame))
    && crate::interpolation_blend(item, t, src.info().frame_rate()).is_none();
```
When a clip's input colour space (`cs`) is a camera log curve (such as `ColorSpace::SLog3SGamut3Cine`), `colorman::needs_management(pipe, cs, &frame)` is `true` and `colorman::gpu_input(...)` returns `None` (because the GPU input transform shader currently handles SDR, linear, PQ, and HLG, while camera log curves are decoded by `colorman::decode` on the CPU until Task C2). Because `plain` was `false`, `push_item` skipped the GPU effect stage entirely and fell through to `crate::item_layer(...)`, running **both** the log decode **and** the entire Lumetri effect chain on the CPU (`fx: None`).

### New behaviour in `C1.9`
We split the condition in `crates/render/src/plan.rs` (`push_item`) into:
- `no_blend = crate::interpolation_blend(item, t, src.info().frame_rate()).is_none()` (no Frame Blending / Optical Flow in-between frame).
- `plain = (input.is_some() || !crate::colorman::needs_management(pipe, cs, &frame)) && no_blend`.
- When `no_blend && !chain.is_empty()` and all effects in `chain` evaluate to `FxOp::gpu_ok`:
  - **Non-log / GPU-managed clips (`plain == true`):** Unchanged (`(frame, n as u32, input)`).
  - **Log / CPU-decoded clips (`plain == false`, `no_blend == true`):** `crate::colorman::decode(project, item.item, &frame, n, pipe)` first converts the log frame to linear working space on the CPU (matching `base_layer` / `media_image` on the CPU reference path), wraps the linear `Image` in `cpu_frame(img)` (`PixelData::RgbaF32`) with `decimation = 1` and `input = None`, and attaches `fx: Some(Arc::new(LayerFx { size: (lw as u32, lh as u32), decimation: 1, ops }))` so that the effects (including the 6-section Lumetri chain) execute on the GPU.
  - **Speed-changed / frame-blended clips (`no_blend == false`):** Unchanged; they fall through to `crate::item_layer(...)` on the CPU.

### Parity & Test Notes for `C1.9`
- **`tests::log_clip_schedules_gpu_fx_while_frame_blending_stays_on_cpu` (`crates/render/src/tests.rs`):** Verifies that (1) a non-log clip keeps its raw source frame and `decimation`, (2) an `SLog3SGamut3Cine` clip decodes its input transform to linear `RgbaF32` before scheduling `LayerFx` on the GPU and matches `render_sequence` bit-for-bit on `execute_cpu`, and (3) a frame-blended speed-changed clip stays on the CPU (`fx.is_none()`).
- **`fx_tests::gpu_log_clip_full_lumetri_chain_matches_cpu` (`crates/gpu/src/fx_tests.rs`):**
  - Because `GpuCompositor` uploads `PixelData::RgbaF32` into an `Rgba16Float` (`f16`, 10-bit mantissa, half-ULP $2^{-11} \approx 4.88 \times 10^{-4}$) source texture before the `Rgba32Float` compute stage, comparing the GPU working image against the unquantized `f32` CPU decode has a measured max relative difference of `2.52e-4` (`<= 3e-4`, `flips == 0.0`), while comparing against the `f16`-quantized source matches within `EXACT = 1e-4`. On the final 8-bit composited output, `max <= 1` and `mean < 0.2`.
- **Vulkan parallel test runner note:** Running `cargo test -p filmcraft-gpu --lib` with the default 20-thread test runner hung after 28 tests due to concurrent Vulkan `wgpu::Device` creation across 20 threads; running with `-- --test-threads=1` completes all 43 `filmcraft-gpu` unit tests cleanly in `10.59 s`.

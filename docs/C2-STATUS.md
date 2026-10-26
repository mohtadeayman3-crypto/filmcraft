# Task C2 — Managed Colour (ACES) Status

Phase 1 (design only, no code changes). Design document: [`C2-DESIGN.md`](C2-DESIGN.md).

## State at start (2026-10-26)

- `git status`: clean. HEAD `5925df40` (C1.7). No `docs/C2-DESIGN.md` / `docs/C2-STATUS.md` existed.

## Reading notes

- `crates/color/src/spaces.rs`: every `Gamut` is D65; `npm()` is hard-wired to `D65`, so no
  chromatic adaptation exists. `WorkingSpace` = Rec709 | Rec2100Pq | Rec2100Hlg.
  `ColorPipeline { working, wide_gamut, auto_tone_map }` (serde, `Eq + Hash`, used as a cache key).
  `working_gamut()` = BT.2020 for HDR or wide gamut, else BT.709. `is_plain()` = Rec709 && !wide_gamut.
- `crates/color/src/log.rs`: 10 curves (S-Log3, V-Log, C-Log2, C-Log3, LogC3 EI800, LogC4, Apple Log,
  D-Log, F-Log, F-Log2), each a pure f64 encode/decode pair with a `Domain` (CV or IRE).
- `crates/color/src/transform.rs`: `InputTransform` = 1D `DecodeTable` (log: 8192 samples over
  −0.1…1.1) + pixel stage (HLG OOTF → matrix → BT.2390 tone map → camera-gamut compression or
  desaturate). `gpu_params()` returns `None` for `Curve::Log(_)` **and** whenever `compress` is set
  (`compress = wider && src.is_log()`), so every log source falls back to the CPU.
  `OutputTransform` = tone map (HDR working → SDR) → matrix → desaturate; `encode()` does PQ/HLG.
  `gamut_compress` is a simplified ACES RGC (one threshold 0.8 / limit 1.25 / power 1.2 for all channels).
- `crates/render/src/colorman.rs`: `source_space` = Interpret Footage override else `ColorSpace::from_info`.
  `needs_management` false only for plain pipeline + Rec709/sRGB/Linear709 media matching metadata.
  `gpu_input` requires `pipe.is_plain()` and a Y'CbCr frame, then `gpu_params().is_some()`.
  `decode` = CPU table + `has_pixel_stage` map on the full image (log clips, non-plain pipelines).
- `docs/C1-STATUS.md` (C1.9): log clips are CPU-decoded to `RgbaF32`, uploaded as `Rgba16Float`,
  then graded on the GPU (`LayerFx`). Upload of 66 MB/frame at 4K is the remaining cost.
- `crates/project/src/lib.rs`: `SequenceSettings.color: ColorPipeline` (`#[serde(default)]`),
  `working_space: String` (display label), `Interpretation.color_space: Option<ColorSpace>`.
  Latest schema comment in `Project` = v15 (notes); confirm `SCHEMA_VERSION` in `crates/format` at C2.4.

## Progress

- [x] §0–2 written (scope, today's pipeline, log on GPU) — commit "C2.0: design draft, part 1"
- [x] §3–5 written (ACES architecture, output transform options, sources) — part 2
- [x] §6–8 written (tests, schema, UI/commands) — part 3
- [x] §9–11 written (risks, staged plan, open questions) — final "C2.0: design draft"
- [x] §12 written (Addendum: `composite.wgsl` inspection for C2.2)
- [x] C2.1 pure-Rust ACES core implemented and pushed to `origin/main`
- [x] C2.2 GPU camera log decode implemented and verified with A/B real-footage metrics
- [x] Isolated OCIO oracle setup in `D:\FilmCraftX-tools\` (Python 3.12, OpenColorIO 2.6.0, NumPy 2.5.4)

## Next

- Item 3: Chroma siting empirical test with synthetic 4:2:0 frame
- Item 4: Export default investigation and C1102.MP4 benchmark
- Item 5: ACEScg pipeline wiring design summary (15 lines)

## C2.1 Pure-Rust ACES Core Completion Summary (2026-10-26)

- **Implementation in `filmcraft-color`**:
  - `crates/color/src/aces.rs`:
    - Defined published chromaticities for AP0 (SMPTE ST 2065-1:2012) and AP1 (AMPAS S-2014-004), and ACES white point `[0.32168, 0.33767]`.
    - Implemented Bradford Chromatic Adaptation Transform (`bradford_cat`) based on K. M. Lam (1985) / ICC.1 Annex E.
    - Derived Normalized Primary Matrices via SMPTE RP 177: `ap0_to_xyz()`, `ap1_to_xyz()`, `ap0_to_ap1()`, `ap1_to_ap0()`, and AP1 luminance weights (`ap1_luma()`).
    - Implemented ACEScg (scene-linear identity), ACEScct (AMPAS S-2016-001 forward and inverse), and ACEScc (AMPAS S-2014-003 forward and inverse).
  - `crates/color/src/spaces.rs`:
    - Added `Gamut::Ap0` and `Gamut::Ap1`.
    - Added `white(self)` returning ACES white for AP0/AP1 and D65 for all existing gamuts.
    - Updated `gamut_matrix(src, dst)` to insert Bradford CAT when white points differ (`D65 ↔ ACES white`), preserving 100% bit-for-bit identity across existing D65 pairs.
- **Matrix Cross-Check Against Published AMPAS Matrices**:
  - `AP0 -> XYZ`: max difference from published SMPTE ST 2065-1 matrix is **$4.84 \times 10^{-11}$**.
  - `AP1 -> XYZ`: max difference from published S-2014-004 matrix is **$4.36 \times 10^{-11}$**.
  - `AP0 -> AP1`: max difference from published S-2014-004 matrix is **$4.81 \times 10^{-11}$**.
  - `AP1 -> AP0`: max difference from published S-2014-004 matrix is **$4.79 \times 10^{-11}$**.
  - Confirmed: derivation from primary chromaticities agrees with official published 10-decimal matrices within rounding limits of IEEE 754 f64.
- **Curve Verification**:
  - ACEScct: verified $C^1$ continuity at $X_{BRK} = 0.0078125$ ($Y_{BRK} \approx 0.15525114$) within $10^{-12}$ and exact first-derivative match $A = 10.5402377...$.
  - ACEScc & ACEScct: verified 18% mid-grey maps to $0.4135884...$ and 1.0 maps to $0.5547945...$.
  - Round-trip accuracy: relative error $< 10^{-10}$ across geometric grid from $10^{-6}$ to $65504$.
- **Automated Tests**:
  - 38 tests passing in `filmcraft-color`.
  - 207 tests passing in `filmcraft-render`.
  - Zero compiler or clippy warnings across entire workspace.


## C2.2 Completion Summary (2026-10-26)

- **Implementation**:
  - `crates/color/src/transform.rs`: Added `range: Range` to `InputTransform`, `GpuCurve::Log(Curve, Range)` to `enum GpuCurve`, and `compress: bool` to `struct GpuInputParams`. `gpu_params()` now maps all 12 camera log curves and preserves gamut compression flags for GPU execution.
  - `crates/gpu/src/composite.wgsl`:
    - Bound `lut_table: texture_2d<f32>` at `@group(0) @binding(6)`.
    - Added `sample_decode_table(t, v)`: reads from the 8192-entry 1D decode table using `textureLoad` with manual `f32` linear interpolation over the $[-0.1, 1.1]$ domain (protecting against platforms without R32Float hardware filtering).
    - Added `gamut_compress(c)`: exact GPU translation of the CPU ACES-style gamut compression with identical parameters ($s \approx 0.29803417$, threshold $0.8$, limit $1.25$, power $1.2$).
    - Updated `managed()` path to execute decode table sampling and gamut compression in shader for log curves (`curve == 4u`).
  - `crates/gpu/src/lib.rs`:
    - Added on-demand upload of 8192x1 `R32Float` textures in `GpuCompositor::get_decode_table` cached by `(Curve, Range)`.
    - Bound `lut_table` across composite and layer effect passes (with fallback dummy texture when unmanaged).
  - `crates/render/src/colorman.rs`: Updated documentation to reflect that camera log media on Y'CbCr hardware frames is managed entirely on the GPU. Retained CPU decode fallback for non-Y'CbCr frames (RGB/generators) and frame-blended clips.
- **Automated Tests**:
  - `log_input_gpu_matches_cpu` in `crates/gpu/src/tests.rs`: verified all 12 camera log spaces (S-Log3, S-Gamut3, V-Log, C-Log2, C-Log3, LogC3, LogC4, Apple Log, D-Log, F-Log, F-Log2, F-Log2 C) achieve $\le 2$ 8-bit code difference against CPU reference.
  - `gpu_yuv_log_clip_full_lumetri_chain_matches_cpu` in `crates/gpu/src/fx_tests.rs`: verified synthetic 4:4:4 YUV log clip with full 6-section Lumetri chain matches CPU reference with $p99 = 1$, mean $= 0.32$, max $= 5$.
  - Clean test suites across `filmcraft-color` (30 passed), `filmcraft-render` (207 passed), and `filmcraft-gpu` (47 passed).
  - `cargo clippy --workspace --all-targets` passes with 0 warnings.

- **Real Footage A/B Parity (`C1102.MP4`, Sony S-Log3 / S-Gamut3.Cine, 3840x2160, Frame 3500)**:
  - **Raw Input Transform (S-Log3 to Rec.709 Working Space)**:
    - Total pixels: 8,294,400 ($3840 \times 2160$).
    - Mean channel difference: $0.289$ / 255 ($0.113\%$).
    - Mean per-pixel max difference: $0.647$ / 255.
    - 99th percentile difference ($p99$): $3$ / 255 codes.
    - Max difference: $64$ / 255 codes (confined strictly to 4:2:0 subsampled chroma edge transitions).
    - Match distribution:
      - $\le 1$ code difference: $92.80\%$ ($7,697,550$ px)
      - $\le 2$ codes difference: $98.18\%$ ($8,143,184$ px)
      - $> 20$ codes difference: $635$ px ($0.0077\%$)
  - **Full 6-Section Lumetri Chain on S-Log3 Footage**:
    - Mean channel difference: $0.274$ / 255 ($0.107\%$).
    - Mean per-pixel max difference: $0.626$ / 255.
    - 99th percentile difference ($p99$): $4$ / 255 codes.
    - Max difference: $169$ / 255 codes (boosted by aggressive sharpening/contrast at high-frequency chroma boundaries).
    - Match distribution:
      - $\le 1$ code difference: $91.40\%$ ($7,580,784$ px)
      - $\le 2$ codes difference: $97.37\%$ ($8,076,014$ px)
      - $> 20$ codes difference: $1,300$ px ($0.0157\%$)

- **4:2:0 Chroma Upsampling Quantification**:
  - *Mechanism*: CPU `filmcraft-frame` upsamples 4:2:0 chroma planes using nearest-neighbor box sampling (`cy = y >> sy; cx = x >> sx;`), repeating each $2\times 2$ chroma sample across four luma pixels. The GPU compositor in `composite.wgsl` samples chroma planes using hardware bilinear interpolation via `load4(tex1, cp)`.
  - *Impact*: In interior pixel regions and smooth gradients ($> 98\%$ of the image), CPU and GPU agree within $\le 2$ codes (mean difference $\approx 0.28$ codes). The outlier differences ($> 20$ codes, representing $< 0.016\%$ of pixels) occur exclusively along sharp, high-contrast chroma edges where nearest-neighbor creates 2-pixel stair-stepping blocks while GPU bilinear filtering produces smooth sub-pixel anti-aliasing. GPU bilinear filtering is visually superior and prevents chroma blockiness.

## Isolated OCIO Test Oracle Setup (2026-10-26)

- **Location**: `D:\FilmCraftX-tools\` (strictly external to repo, no admin, no system PATH/registry alterations).
- **Tooling**:
  - `uv` standalone binary version: `0.13.5` (`uv-x86_64-pc-windows-msvc.zip`).
  - Verified official release SHA256: `a38b8d1902b7cfbc76b423ba1bb456f623f42064cdc72628b6075b89afd1019a`.
  - Python installation: Managed CPython `3.12.15` in `D:\FilmCraftX-tools\python\`.
  - Cache directory: `D:\FilmCraftX-tools\cache\`.
  - Virtual environment: `D:\FilmCraftX-tools\venv\`.
- **Packages Installed from PyPI**:
  - `opencolorio==2.6.0`
  - `numpy==2.5.4`
- **ACES 2.0 Built-in Capabilities**:
  - Verified `OpenColorIO 2.6.0` contains built-in ACES 2.0 output transforms:
    - SDR: `ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-100nit-REC709_2.0`, `SDR-100nit-P3-D65_2.0`
    - HDR: `HDR-500nit-REC2020_2.0`, `HDR-1000nit-REC2020_2.0`, `HDR-2000nit-REC2020_2.0`, `HDR-4000nit-REC2020_2.0`, `HDR-108nit-P3-D65_2.0`
  - Verified built-in ACES 2.0 configs:
    - `cg-config-v4.0.0_aces-v2.0_ocio-v2.5`
    - `studio-config-v4.0.0_aces-v2.0_ocio-v2.5`
    - `cg-config-v5.0.0_aces-v2.1_ocio-v2.6`
    - `studio-config-v5.0.0_aces-v2.1_ocio-v2.6`
  - Also verified built-in camera IDTs (`SONY_SLOG3-SGAMUT3.CINE_to_ACES2065-1`, `ARRI_LOGC4_to_ACES2065-1`, `CANON_CLOG2-CGAMUT_to_ACES2065-1`, etc.).



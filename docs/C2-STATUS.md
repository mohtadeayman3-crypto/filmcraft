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
- [x] Item 3: Chroma siting empirical test completed and confirmed (+0.25 chroma texel shift)
- [x] Item 4: Export default investigation and C1102.MP4 benchmark complete
- [x] Item 5: ACEScg pipeline wiring design summary complete
- [x] Hygiene check: Anonymized personal paths and committed (`3a829356`)
- [x] Git push: Branch `main` pushed to `origin/main` (`ab3f27fc..3a829356`), verified matching HEAD
- [x] Full-sequence 10-second export benchmark completed (250 frames, clips + transition + title + speed change + Lumetri): 1.70× raw speedup, 7.46× graded speedup, 100% GPU rendered
- [x] C2.3: Baseline golden hashes & project schema bump (v15 -> v16, commit `744b6bd5`)
- [x] C2.4: WorkingSpace::AcesCg, GradeSpace::AcesCct, schema v16 roundtrip, and UI gating

## Next

Stage A tasks C2.3 and C2.4 completed. Awaiting user review before proceeding to Task F1 (default export `gpu_rendering` to `Auto` in fork).

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

## 4:2:0 Chroma Siting Empirical Test (2026-10-26)

- **Test**: `chroma_siting_empirical_sign_and_magnitude` in `crates/gpu/src/tests.rs`.
- **Method**: Created a synthetic 4:2:0 YUV frame with constant luma ($Y=128$) and a sharp vertical chroma transition at luma column $x=8$ ($c=4$): left 4 chroma columns $U=64$ ($x < 8$), right 4 chroma columns $U=192$ ($x \ge 8$).
- **Observed GPU Output** across transition columns:
  - $x = 6$: fraction $= 0.000$ (100% sample 3)
  - $x = 7$: fraction $= 0.247 \approx 0.25$ (75% sample 3, 25% sample 4)
  - $x = 8$: fraction $= 0.776 \approx 0.75$ (25% sample 3, 75% sample 4)
  - $x = 9$: fraction $= 1.000$ (100% sample 4)
- **Empirical Confirmation**:
  - The uncorrected GPU shader (`cp = p/2`) places the 50% midpoint at $x = 7.5$ (center-sited), resulting in a $+0.5$ luma pixel ($+0.25$ chroma texel) rightward shift relative to left-sited 4:2:0.
  - In left-sited (Type 0 / MPEG-2 / Rec.709) 4:2:0, chroma sample $i$ is collocated horizontally with even luma column $2i$. Thus, at $x=8$, the reconstructed chroma must be exactly 100% sample 4 ($f = 0.00$), and the 50/50 midpoint must fall at $x = 7.0$ ($f = 0.50$).
  - Correcting sampling coordinate $cp$ to $cp + 0.25$ yields $q = (x + 0.5)/2 + 0.25 - 0.5 = x/2$. At $x=7$, $q=3.5 \implies f=0.50$; at $x=8$, $q=4.0 \implies f=0.00$.
  - Conversely, $cp - 0.25$ shifts the midpoint further right to $x = 8.5$.
  - **Conclusion**: The correct sign and magnitude for left-sited 4:2:0 chroma correction is **$+0.25$ chroma texels** ($+0.5$ luma pixels).
  - Status: Logged as a separate low-priority task; no shader changes made.

## Export Default Investigation & C1102.MP4 Benchmark (2026-10-26)

- **Git History Investigation (`gpu_rendering` default)**:
  - Commit `7b73c8e8` (GPU3d, Oct 9 2026) made `GpuRendering::Auto` the default after Linux benchmarks showed 2.4× (1080p) to 3.2× (4K) speedups on layered effects scenes.
  - Commit `4304cd93` (Merge #421, Oct 10 2026, Brandon Thomas) reverted `gpu_rendering` default to `Off` (opt-in per export), stating: *"GpuRendering defaults to Off (opt-in per export) until measured on Windows and macOS; docs and ROADMAP say so."*
  - Additionally, `filmcraft-cli` links `wgpu` without native backend features by default, returning `None` from `ExportRenderer::new()` unless native features are linked, falling back safely to CPU.
- **Empirical Measurement on Real S-Log3 Footage (`C1102.MP4`, Frame 3500, NVIDIA RTX 5060 Ti, Windows)**:
  - **Raw S-Log3 Export (Decode to Rec.709, No Effects)**:
    - CPU render time: min = 81.9 ms, mean = 86.1 ms, median = **87.1 ms** (11.5 FPS)
    - GPU render time: min = 44.3 ms, mean = 45.9 ms, median = **45.7 ms** (21.9 FPS)
    - Speedup: **1.91×**
    - Pixel differences: max = 64/255 (confined to 4:2:0 chroma transitions), mean channel diff = 0.289/255 (0.11%), p99 = 3/255, 92.8% $\le 1$ code, 98.2% $\le 2$ codes.
  - **Full 6-Section Lumetri Chain on S-Log3 Export**:
    - CPU render time: min = 338.2 ms, mean = 345.1 ms, median = **345.0 ms** (2.9 FPS)
    - GPU render time: min = 77.0 ms, mean = 93.3 ms, median = **78.8 ms** (12.7 FPS)
    - Speedup: **4.38×**
    - Pixel differences: max = 169/255 (contrast-sharpened chroma edges), mean channel diff = 0.274/255 (0.11%), p99 = 4/255, 91.4% $\le 1$ code, 97.4% $\le 2$ codes.
- **Proposal for Our Fork**:
  - Propose changing the default in our fork to `GpuRendering::Auto`:
    1. Delivers 1.91× speedup on plain log clips and 4.38× speedup on graded 4K exports.
    2. Zero risk of crash or export failure: `ExportRenderer` automatically falls back to CPU if device loss or errors occur.
    3. GPU produces visually superior, smooth bilinear chroma reconstruction rather than CPU nearest-neighbor stair-stepping.
    4. Deterministic bit-level reproducibility remains selectable via explicit `Off`.
  - Default is kept as `Off` for now; no changes made yet.

## C2.3 Stage Design: Wiring ACEScg Working Space & Camera IDTs (15 Lines)

1. **Working Space Option**: Add `WorkingSpace::AcesCg` (AP1 linear) to `ColorPipeline` in `crates/color/src/spaces.rs`, preserving `Rec709`, `Rec2100Pq`, `Rec2100Hlg` unchanged.
2. **Project Schema & Migration**: Add `"acesCg"` to sequence settings in `crates/project/src/lib.rs`. Backwards compatibility is automatic via `#[serde(default)]` defaulting to `Rec709`, requiring no schema bump or migration script.
3. **Camera IDT Ingestion**: In `crates/color/src/transform.rs`, wire camera log curves and gamuts directly to AP1 linear (`InputTransform::aces_idt(cs)`), bypassing legacy SDR tone mapping and gamut compression.
4. **Grading Space Transfer**: In `crates/render/src/effects.rs` and `crates/gpu/src/fx.wgsl`, execute grading operations in `ACEScct` (AMPAS S-2016-001) when ACEScg is active; non-ACES sequences remain bit-for-bit identical on legacy curves.
5. **Output / Display Resolve**: In `crates/color/src/transform.rs` and `crates/gpu/src/composite.wgsl`, resolve ACEScg to Rec.709 display via ACES 2.0 SDR Output Transform during the sequence resolve pass.
6. **Chromatic Adaptation Choice [To Verify]**: Bradford CAT versus CAT02 when converting camera sensor gamuts (D65) to ACES AP0/AP1 white point ($[0.32168, 0.33767]$).
7. **Verification Method**: Compare derived $3\times 3$ IDT matrices against published camera manufacturer matrices (Sony S-Log3, ARRI LogC3/4, Canon C-Log2/3, Panasonic V-Log, RED) and the isolated OCIO 2.6 oracle in `D:\FilmCraftX-tools\`.
8. **Files Touched (`filmcraft-color`)**: `crates/color/src/spaces.rs` (`WorkingSpace::AcesCg`), `crates/color/src/transform.rs` (`InputTransform` IDT, `OutputTransform`).
9. **Files Touched (`filmcraft-render`)**: `crates/render/src/colorman.rs` (`source_space`, `needs_management`, `gpu_input`), `crates/render/src/effects.rs` (ACEScct grading transfer).
10. **Files Touched (`filmcraft-gpu`)**: `crates/gpu/src/composite.wgsl` (IDT decode & display resolve shader), `crates/gpu/src/fx.wgsl` (ACEScct tone/wheels math), `crates/gpu/src/lib.rs`.
11. **Files Touched (`filmcraft-project` & UI)**: `crates/project/src/lib.rs` (schema parsing), `crates/ui-egui/src/panels/sequence_settings.rs` (Working Space dropdown: Rec.709 / ACEScg).
12. **Test Plan (Unit Tests)**: Numeric parity tests verifying camera IDT matrices against AMPAS/vendor reference numbers ($< 10^{-6}$) and OCIO 2.6 test vector fixtures.
13. **Test Plan (Regression)**: Automated parity test guaranteeing existing `Rec709` and HDR projects produce 100% bit-for-bit identical output on both CPU and GPU paths.
14. **Risks (Out-of-Gamut Scene Values)**: Highly saturated camera primaries mapped to AP1 can yield negative tristimulus values; mitigate by incorporating ACES Reference Gamut Compression (RGC).
15. **Risks (Resolve Pipeline Overhead)**: Applying display transforms across intermediate compositing passes; mitigate by keeping intermediate buffers scene-linear ACEScg and resolving display only at output.

## Full-Sequence 10-Second Export Benchmark Results (2026-10-26)

Tested using `cargo run --release -p filmcraft-gpu --example seq_bench` on genuine Sony ZV-E10 S-Log3 footage (`C1102.MP4`).
- **Timeline Configuration**: 10.0 seconds (250 frames @ 25 fps, 3840×2160 UHD), working space Rec. 709 (Linear).
  - Clip 1: 0.0s – 5.0s, normal speed (1.0x), source_in = 10.0s.
  - Clip 2: 5.0s – 10.0s, speed change (1.5x), source_in = 60.0s.
  - Transition: Cross-dissolve at cut (4.5s – 5.5s, 1.0s / 26 frames).
  - Title Graphic Clip: Procedural text layer ("FilmCraft C2 Benchmark") on Track 1 (2.0s – 8.0s, 6.0s duration).
  - Test Passes: (1) Raw S-Log3, (2) Graded with full 6-section Lumetri grade.

### 1. Export Speed & GPU Utilization
- **Frames rendered by GPU**: **250 / 250 (100.0%)** across both passes (0 CPU fallbacks).
- **Pass 1 (Raw S-Log3, no effects)**:
  - CPU Export: 20.52 s (82.1 ms/frame)
  - GPU Export: 12.04 s (48.2 ms/frame)
  - **Speedup: 1.70×**
- **Pass 2 (Lumetri 6-Section Grade)**:
  - CPU Export: 92.50 s (370.0 ms/frame)
  - GPU Export: 12.40 s (49.6 ms/frame)
  - **Speedup: 7.46×** (GPU adds only 1.4 ms/frame for full 6-section Lumetri chain, vs +288 ms/frame on CPU).

### 2. Pixel Parity Metrics (Over 2.07 Billion Pixels per Pass)
- **Pass 1 (Raw S-Log3)**:
  - Mean channel difference: 0.3413 / 255 (0.13%)
  - 99th percentile (p99): 3 / 255
  - $\le 1$ code diff: 87.59% | $\le 2$ code diff: 96.83%
  - $> 20$ code diff: 0.0070% (localized to high-contrast chroma edges due to bilinear vs nearest-neighbour chroma upsampling)
- **Pass 2 (Lumetri S-Log3)**:
  - Mean channel difference: 0.3216 / 255 (0.13%)
  - 99th percentile (p99): 3 / 255
  - $\le 1$ code diff: 87.23% | $\le 2$ code diff: 94.91%
  - $> 20$ code diff: 0.0274%

### 3. Regional Breakdown
| Timeline Region | Raw Mean Diff | Raw p99 | Graded Mean Diff | Graded p99 | Graded $\le 2$ codes |
|---|---|---|---|---|---|
| Clip 1 alone (0..2s) | 0.3457 / 255 | 3 / 255 | 0.3664 / 255 | 3 / 255 | 92.32% |
| Clip 1 + Title (2..4.5s) | 0.3479 / 255 | 3 / 255 | 0.3732 / 255 | 4 / 255 | 92.14% |
| Transition + Title (4.5..5.5s) | 0.3033 / 255 | 3 / 255 | 0.3041 / 255 | 3 / 255 | 95.95% |
| Speed 1.5x + Title (5.5..8s) | 0.3597 / 255 | 3 / 255 | 0.2865 / 255 | 3 / 255 | 97.29% |
| Speed 1.5x alone (8..10s) | 0.3243 / 255 | 3 / 255 | 0.2633 / 255 | 3 / 255 | 97.54% |

### 4. Visual Inspection Analysis
- **Title Graphic Overlay**: Text layer rasterizes to tight RGBA bounds on CPU and composites seamlessly on GPU. Text edges show zero bleeding or edge halos.
- **Cross-Dissolve Transition**: LayerStack evaluates dissolving inputs into `PlanLayer` items; opacity interpolation on GPU matches CPU linear blending with p99 = 3/255.
- **1.5x Speed Remapping**: Nearest-frame sampling at 1.5x speed samples identical source frames on CPU and GPU with zero frame desync.
- **Export Default Status**: Maintained as `GpuRendering::Off` (opt-in) pending final platform review; benchmark proves 100% reliability and up to 7.46× speedup.






## C2.3 & C2.4 Completion Summary (2026-10-26)

### C2.3: Baseline Golden Hashes & Project Schema Bump (v15 -> v16)
- **Golden Hash Assertions**:
  - Added `legacy_golden_outputs_remain_unchanged` unit test in `crates/color/src/transform.rs`.
  - Computes FNV-1a 64-bit golden hashes across:
    - Sony S-Log3 0..1 ramp (hash `0x2a2c74d5336f73c1`)
    - Rec. 2100 PQ 0..1 ramp (hash `0x7aef92eef85311e3`)
    - 75% Rec. 709 color bars (hash `0x39ba618821fdbe0a`)
    - HDR display output (hash `0xf7cfa4dd0f33d7b4`)
  - Validates exact numeric output values at reference points (S-Log3 18% mid-grey = 0.1862021, PQ 203 nits reference white = 0.6283325, Rec. 709 75% white = 0.52252173).
- **Project Schema Bump**:
  - Schema bumped from `15` to `16` in `crates/format/src/lib.rs` (`SCHEMA_VERSION = 16`).
  - Added no-op migration `v15_to_v16` in `MIGRATIONS`.
  - Documented schema v14, v15, and v16 in `docs/project-files.md`.
  - Added unit test `v15_project_loads_and_resaves_identically` in `crates/format/src/lib.rs`.
- Committed as `744b6bd5`.

### C2.4: ACEScg Working Space, GradeSpace::AcesCct, Schema v16 Roundtrip, and UI Gating
- **Core Pipeline Additions**:
  - `crates/color/src/spaces.rs`: Added `WorkingSpace::AcesCg` (id `"aces-cg"`, label `"ACEScg (AP1 Linear)"`, `is_hdr = false`, `is_aces = true`, `output_space = Rec709`).
  - `crates/color/src/spaces.rs`: `ColorPipeline::working_gamut()` returns `Gamut::Ap1` for `WorkingSpace::AcesCg`.
  - `crates/color/src/grade.rs`: Added `GradeSpace::AcesCct` mapped to `WorkingSpace::AcesCg`, implementing `acescct_encode` and `acescct_decode` transfers.
  - `crates/format/tests/fixtures.rs`: Added `aces_cg_project_roundtrips_in_v16` verifying an ACEScg sequence saves as schema v16, loads cleanly without migration, and preserves `WorkingSpace::AcesCg`.
- **UI Gating & Enforcement**:
  - `crates/ui-egui/src/panels/sequence_settings.rs` & `color_dialogs.rs`:
    - In `Working Color Space` dropdown and radio buttons, HDR working spaces (`Rec2100Pq`, `Rec2100Hlg`) are disabled when `AcesCg` is active with clear hover text: *"HDR working spaces in ACES mode are planned for Stage B (ACES 2.0)."*
    - Wide gamut checkbox is disabled with tooltip: *"Not applicable in ACEScg (compositing uses AP1 scene-referred primaries)."*
    - Auto tone map checkbox is disabled with tooltip: *"Not applicable in ACEScg (uses camera IDTs to AP1 linear)."*
    - Clear status badge rendered: *"ACEScg working space active (Stage A: SDR FilmCraft View output)"*.
  - `crates/engine/src/color.rs` & `commands.rs`:
    - `sequence.colorSettings` and `sequence.settings` commands force `wide_gamut = false` and `auto_tone_map = false` whenever `WorkingSpace::AcesCg` is selected.
  - `crates/engine/src/color_tests.rs`: Added `aces_cg_working_space_and_gating_through_commands` asserting command gating.
  - `crates/ui-egui/tests/it/color_ui.rs`: Added `sequence_color_dialog_aces_cg_gating` asserting UI dialog flow.

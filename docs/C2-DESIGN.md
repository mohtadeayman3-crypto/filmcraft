# Task C2 Design — Managed Colour (ACES)

**Status:** Draft (Phase 1 — design only, awaiting review)  
**Scope:** Task C2 from [`FILMCRAFT_COLOR_UPGRADE_PLAN.md`](../FILMCRAFT_COLOR_UPGRADE_PLAN.md) §4  
**Progress notes:** [`C2-STATUS.md`](C2-STATUS.md)

---

## 0. Scope and ground rules

C2 adds a managed colour pipeline next to the existing one:

- **Working space ACEScg** (AP1 primaries, scene-linear), **grading space ACEScct**, **AP0
  (ACES2065-1)** as interchange;
- **input transforms** from every camera log curve FilmCraft already decodes into ACES;
- **output transforms** for SDR Rec.709 (100 cd/m²) and HDR PQ / HLG;
- a **Color Space Transform** effect and a project setting **Colour management: Rec.709/HDR
  (legacy) | ACES**;
- moving the **camera-log input transforms to the GPU** (benefits the legacy pipeline too).

Rules this design follows (AGENTS.md, CLAUDE.md):

1. **Pure Rust, clean room.** Formulas and constants come from public AMPAS / SMPTE / ITU /
   camera-vendor documents. Every constant used is listed with its source in §5. Values I am not
   certain of are marked **(to verify)** and must be checked against the cited document before
   they enter code; nothing is invented.
2. **Never crash.** All transforms are total functions on `f32`/`f64` (NaN/∞/negative inputs give
   finite outputs); settings from project files and commands are validated; no
   `unwrap`/`expect`/`panic`.
3. **Legacy stays bit-identical.** A project in the legacy mode renders exactly as today; every
   existing colour test keeps passing unmodified.
4. **Layering.** All maths in `filmcraft-color` (L0, no dependencies); renderer glue in
   `filmcraft-render`; shaders in `filmcraft-gpu`; settings in `filmcraft-project`; commands in
   `filmcraft-engine`; UI in `filmcraft-ui-egui`.
5. **One task id per commit** (`C2.1: …`), `cargo xtask ci` green.

---

## 1. How the colour pipeline is structured today

### 1.1 Types (`crates/color/src/spaces.rs`)

| Type | Role |
|---|---|
| `Gamut` | 11 RGB primary sets (BT.709, BT.2020, P3-D65, S-Gamut3(.Cine), V-Gamut, Cinema Gamut, AWG3, AWG4, D-Gamut, F-Gamut C). **All D65**; `npm()` is called with the constant `D65`, so `gamut_matrix(a, b) = XYZ→b · a→XYZ` with **no chromatic adaptation**. |
| `Curve` | `Sdr` (decoded with the sRGB curve), `Srgb`, `Linear`, `Pq`, `Hlg`, `Log(LogCurve)`. |
| `ColorSpace` | 19 source spaces offered in Interpret Footage (`id()` strings are the stable project/command ids). |
| `WorkingSpace` | `Rec709` (default) / `Rec2100Pq` / `Rec2100Hlg`; `output_space()` is the export encoding. |
| `ColorPipeline` | `{ working, wide_gamut, auto_tone_map }`, stored in `SequenceSettings.color`; `Eq + Hash` (used as a cache key in `colorman`). `working_gamut()` = BT.2020 for HDR or wide gamut, else BT.709; `is_plain()` = Rec709 without wide gamut. |

### 1.2 Working units (`crates/color/src/transform.rs`)

Linear light with **1.0 = reference white**: SDR white for SDR media, **203 cd/m²** (BT.2408) for
HDR. Log media decodes to scene-linear reflectance (18 % grey = 0.18) and is placed directly into
those units (0.18 → 0.18).

### 1.3 Input: source → working (`InputTransform`)

```text
signal (decoder-normalised, file range)
  → per-channel DecodeTable (sRGB 4096 / PQ / HLG inverse OETF / log: 8192 samples over −0.1…1.1)
  → HLG OOTF (BT.2100, Lw 1000, γ 1.2, BT.2020 luma) ............ HLG only
  → 3×3 gamut matrix into the working primaries ................. when gamuts differ
  → BT.2390 EETF on max(R,G,B), as a ratio ...................... HDR/log source into SDR working + Auto Tone Map
  → camera-gamut compression (simplified ACES RGC) ............... log source, gamut wider than working
    or luminance-preserving desaturation ......................... display-referred wide source
```

Tone-mapping source peak: PQ MaxCLL/mastering peak (default 1000), HLG 1000, log = curve peak ×
203 clamped to 203…4000 cd/m² (`LOG_PEAK_NITS`).

### 1.4 Output: working → monitor / export (`OutputTransform`)

`display(pipe)` targets SDR BT.709 linear (sRGB-encoded at resolve); `new(pipe, target)` targets
Rec709 / Rec2100Pq / Rec2100Hlg for export. Stages: BT.2390 tone map (HDR working → SDR target,
1000 → 203) → matrix → `desaturate_into_gamut`. `encode()` applies the PQ curve, or the HLG
inverse OOTF + OETF.

### 1.5 Grading space (`crates/color/src/grade.rs`, per C1)

Lumetri works on a 0…1 signal chosen from the working space: sRGB curve (Rec.709), normalised PQ
(Rec.2100 PQ, white `W`), HLG with BT.2100 inverse OOTF (Rec.2100 HLG). The GPU mirrors this with
`gs_encode`/`gs_decode` modes `0 = Sdr, 1 = Pq, 2 = Hlg` (C1.1).

### 1.6 Renderer glue (`crates/render/src/colorman.rs`)

| Function | Today |
|---|---|
| `source_space(project, item, frame)` | Interpret Footage override (`media.interpret.color_space`, following subclips), else `ColorSpace::from_info(&frame.color)` (stream metadata). Log is **never** detected from metadata; it always comes from the override. |
| `needs_management(pipe, cs, frame)` | `false` only for a plain pipeline and Rec709 / sRGB / Linear709 media whose override matches its metadata (the original fast path: transfer-only decode). |
| `gpu_input(project, item, pipe, cs, frame)` | `Some(transform)` only when the pipeline is plain, the frame is Y'CbCr, and `InputTransform::gpu_params()` is `Some`. |
| `decode(project, item, frame, n, pipe)` | CPU path: decode with the table (or none for float frames), then `InputTransform::apply` on every pixel. |
| `to_display(img, pipe)` | CPU working → monitor for non-plain pipelines. |

### 1.7 Where each case runs

| Sequence / media | Decode + input transform | Effects (C1) | Display transform |
|---|---|---|---|
| Rec.709 plain, Rec.709/sRGB media | GPU (fast path) | GPU | identity |
| Rec.709 plain, PQ/HLG/BT.2020/P3 Y'CbCr | GPU (`fs_fx_source` + `GpuInputParams`, #965) | GPU | identity |
| Rec.709 plain, **camera log** | **CPU** (`colorman::decode`) → `RgbaF32` → `Rgba16Float` upload (C1.9) | GPU | identity |
| HDR working or wide gamut (non-plain) | CPU (`gpu_input` requires `is_plain`) | GPU when possible | CPU `to_display` |

---

## 2. Camera log on the GPU

### 2.1 Why `gpu_input` returns `None` for camera log (the reason C1.9 decodes on the CPU)

Four independent reasons, all in `InputTransform::gpu_params()` / `colorman::gpu_input`:

1. **No shader curve.** `GpuCurve` has only `Sdr | Linear | Pq | Hlg`; `gpu_params()` returns
   `None` at `Curve::Log(_) => return None`. The shader evaluates SDR/PQ/HLG analytically; nobody
   wrote the ten log decoders (and their CV/IRE domain step, `signal_from_normalized`) in WGSL.
2. **No shader gamut compression.** For every log source the camera gamut is wider than BT.709,
   so `compress = wider && src.is_log()` is `true`, and `gpu_params()` returns `None` whenever
   `self.compress` is set. `GpuInputParams` has `desaturate` but no `compress` stage.
3. **Non-plain pipelines are excluded wholesale.** `gpu_input` returns `None` unless
   `pipe.is_plain()`, because the working → monitor transform (`to_display`) only exists on the CPU;
   so log media in an HDR or wide-gamut sequence would stay on the CPU even with 1 and 2 fixed.
4. **RGB frames are excluded.** Only `Yuv8`/`Yuv16` frames qualify (log stills/EXR are rare;
   acceptable to leave on the CPU).

C1.9 therefore decodes the log frame on the CPU (`colorman::decode`), uploads 66 MB of
`Rgba16Float` per 4K frame and then grades on the GPU. The A/B in C1.8 showed the upload + decode
is now the dominant cost of a graded log clip.

### 2.2 Design: decode table on the GPU (recommended), analytic curves later

**Option A — sample the existing `DecodeTable` on the GPU (recommended for C2.2).**
The CPU already decodes log through `DecodeTable::for_curve` (8192 `f32` samples over −0.1…1.1
in the decoder-normalised domain, so the CV/IRE domain step and the file range are already baked
in). Upload the same `values` to a `8192 × 1` `R32Float` texture and sample it with the CPU's
exact arithmetic (`p = clamp((v − lo)/(hi − lo), 0, 1)·(n−1)`, `i = min(floor(p), n−2)`,
`lerp(values[i], values[i+1], p − i)`), via `textureLoad` (no hardware filtering), like the C1.3
curve rows.

- Parity: identical table, identical lerp in `f32` → expected `≤ 1e-6` relative before the
  pixel stage, i.e. well inside `EXACT = 1e-4`.
- One code path for all ten curves (and any future curve), no `pow`-amplified ULP drift (the PQ
  lesson from C1: `3e-4`).
- Cost: one 32 KB texture per (curve, range), cached by key like `colorman::input_transform`.
- The table must reach the source shader (`fs_fx_source` in `composite.wgsl`) and the plain
  composite path; this needs one new texture binding in that bind group (exact binding slot to be
  confirmed when C2.2 starts; I have not read `composite.wgsl` in this phase).

**Option B — analytic WGSL decoders.** All ten curves fit three families:
`x = (base^((y − d)/c) − b)/a` above a cut with a linear toe (S-Log3, V-Log, LogC3, LogC4 with
base 2, D-Log, F-Log, F-Log2), Apple Log (base-2 log with a square-root toe and a floor), Canon
Log 2/3 (sign-symmetric log, C-Log3 with a linear middle). Exact, no texture, but GPU `pow` =
`exp2(y·log2 x)` brings ULP differences amplified by the slope (≈ 10^(1/c)); parity tolerance
would likely be ~`1e-4…3e-4`. Kept as a later optimisation if the texture fetch ever shows up in
profiles.

**Gamut compression in WGSL.** `gamut_compress` is pure ALU (max, divide, `pow` with power 1.2):
port it 1:1 as `gc_compress(c)` and add `compress: bool` (later: per-channel thresholds/limits,
§3.6) to `GpuInputParams`. Achromatic guard `ach <= 1e-9` and the `max(0)` must be mirrored.

**Tone mapping.** Already supported by `GpuInputParams.tone` (BT.2390 analytic in the shader);
log sources only add their source peak.

**Resulting changes (C2.2):**

- `GpuCurve::Table` (values + lo/hi) and `GpuInputParams.compress`; `gpu_params()` returns `Some`
  for log sources;
- `colorman::gpu_input` returns the transform for log Y'CbCr frames on a plain pipeline;
  `plan.rs` keeps the raw `Yuv8`/`Yuv16` frame (12–25 MB upload instead of 66 MB) and the C1.9
  CPU-decode branch remains only for frame-blended clips and RGB frames;
- parity tests: every `ColorSpace::is_log()` × {Limited, Full} × {8-bit, 10-bit}, GPU vs
  `InputTransform::convert`.

### 2.3 Lifting the `is_plain()` restriction (C2.3)

To put non-plain pipelines (HDR working, wide gamut, and later ACES) on the GPU, the display
transform must run on the GPU too: a resolve-stage pass `accum (working linear) → OutputTransform
→ sRGB/encode` with the same stages as `OutputTransform::apply` (BT.2390 analytic, matrix,
desaturate). Export reads the accumulator before this pass and applies `OutputTransform::encode`
on the GPU as well (or keeps the CPU encode at first). Once that exists, `gpu_input`'s
`is_plain()` condition is replaced by "the pipeline has a GPU display transform".

---

## 3. Where ACES fits

### 3.1 Target pipeline (ACES mode)

```text
camera log / HDR / SDR source signal
  → decode curve (existing DecodeTable; GPU per §2.2)
  → [HLG OOTF | inverse output transform for display-referred media, §3.5]
  → 3×3: source gamut (D65) → CAT D65→ACES white → AP1  ............ "input transform" (IDT), §3.4
  → optional Reference Gamut Compression (in AP1), §3.6
  ═ ACEScg working space (AP1, scene-linear, 0.18 = mid grey) ═══════  compositing, blending, effects
      Lumetri / grading ops: encode ACEScg → ACEScct, grade, decode ACEScct → ACEScg, §3.3
  → output transform (ACES 2.0 recommended, §4): ACEScg → AP0 → OT(peak, limiting primaries, EOTF)
  → display encode: sRGB/BT.1886 (monitor, Rec.709 export) | PQ | HLG
```

AP0 (ACES2065-1) is the **interchange** space only: EXR import/export ("ACES2065-1" Interpret
Footage choice and an export option) and the input to the 2.0 output transform. FilmCraft never
composites in AP0 (negative-free but very wide; grading operators behave badly there).

### 3.2 Working space: ACEScg

- Primaries AP1, white point ACES white (≈ D60), linear (AMPAS S-2014-004).
- Units: scene-linear, **0.18 = 18 % grey**, no reference-white normalisation. Camera log media
  decoded today to "0.18 = 0.18" therefore needs only a matrix — no scaling.
- Storage: compositor textures are `Rgba32Float` (FX stage) and `Rgba16Float` (accumulator).
  ACEScg values above 1 and slightly negative values (out-of-AP1 camera colours before RGC) fit in
  `f16` (max 65 504). Precision of `f16` around 0.18 is ~1e-4, same as today.
- Luma weights for operators that need luminance (Lumetri, keyers, desaturate): the Y row of the
  AP1 → XYZ NPM (§5.1). Lumetri currently hard-codes `luma709`; in ACES mode it gets the AP1
  weights through its `GradeSpace` (legacy stays `luma709` — parity rule 3).

### 3.3 Grading space: ACEScct

- ACEScct (AMPAS S-2016-001): log2-based encoding of ACEScg with a linear toe; formula and
  constants in §5.2. ACEScc (S-2014-003) is offered as a secondary choice (pure log, no toe) —
  optional, low cost, both are in §5.2.
- Implementation: a new `GradeSpace` mode `3 = Acescct` (CPU `grade.rs` and WGSL `gs_encode` /
  `gs_decode`). Encoding range is about −0.36…1.47 for 0…65 504, so Lumetri must treat it like its
  HDR modes (no clamping to 0…1, `within_unit` scaling above 1) — values above 1 are legitimate.
- Lumetri controls tuned for an sRGB-encoded signal behave differently on an ACEScct signal (mid
  grey sits at 0.4136 instead of 0.4613 on the sRGB curve; contrast pivot, shadows/highlights
  weights). The design keeps the existing control maths and documents the different feel;
  retuning is a C3 (node grading: log wheels, contrast + pivot) concern. Open question Q4.

### 3.4 Input transforms (IDTs) for the existing camera curves

An IDT here is **curve decode (exists) + one 3×3 matrix** (camera gamut → AP1). The matrices are
*derived*, not copied:

`M_idt = NPM_AP1⁻¹ · CAT(D65 → W_ACES) · NPM_camera` (camera NPM from the vendor chromaticities
already in `Gamut::primaries()`, SMPTE RP 177; CAT = Bradford, §5.4).

Code change: `Gamut` gets a `white()` (D65 for all existing gamuts, ACES white for `Ap0`/`Ap1`),
`npm()` uses it, and `gamut_matrix` inserts the CAT only when the whites differ. All existing
gamut pairs are D65↔D65, so legacy matrices are **bit-identical** (a test pins them).

| Source (`ColorSpace`) | Curve (exists) | Gamut (exists) | IDT matrix |
|---|---|---|---|
| S-Log3 / S-Gamut3.Cine, S-Log3 / S-Gamut3 | `SLog3` | `SGamut3Cine`, `SGamut3` | derived |
| V-Log / V-Gamut | `VLog` | `VGamut` | derived |
| C-Log2, C-Log3 / Cinema Gamut | `CLog2`, `CLog3` | `CinemaGamut` | derived |
| LogC3 / AWG3, LogC4 / AWG4 | `LogC3`, `LogC4` | `Awg3`, `Awg4` | derived |
| Apple Log | `AppleLog` | `Bt2020` | derived |
| D-Log / D-Gamut | `DLog` | `DGamut` | derived |
| F-Log, F-Log2 / F-Gamut; F-Log2 C / F-Gamut C | `FLog`, `FLog2` | `Bt2020`, `FGamutC` | derived |

Vendors and the Academy also publish IDT matrices for some of these cameras. Those published
matrices are used **only as test references** (§6.2); where a vendor matrix differs from our
derivation by more than rounding (e.g. because the vendor used another CAT or a spectrally
optimised matrix), the difference is documented in the crate README and the user decides whether
to adopt the vendor matrix (open question Q3). I do not quote vendor IDT numbers in this document
because I cannot verify them from memory — **(to verify)** at C2.5 from the vendors' documents.

Exposure: camera log decodes to scene-linear reflectance already normalised so 18 % grey = 0.18
(the vendor curves are defined that way), which is exactly ACES' scene-linear convention, so no
exposure scale is added. (ARRI LogC3 is EI-dependent; FilmCraft implements EI 800 only — unchanged.)

### 3.5 Display-referred media in an ACES project

Rec.709/sRGB clips, titles, graphics, generators, colour mattes and PQ/HLG deliverables are
display-referred. Two choices, selectable per sequence (default first):

1. **Inverse output transform** (recommended default): decode → display linear → inverse of the
   sequence's output transform → ACEScg. A Rec.709 title then comes out of the forward output
   transform unchanged (within the OT's invertible range), which is what users expect for
   graphics. Requires an invertible OT — a strong argument for ACES 2.0 (§4).
2. **Linear + matrix** ("scene-linear interpretation"): decode, BT.709 → AP1 matrix; SDR white
   1.0 lands at ACEScg 1.0, so it looks dimmer/compressed after the OT. Cheap, and the only option
   until the inverse OT exists (C2.5 uses it as a stand-in).

PQ / HLG sources use the inverse of the matching HDR output transform (PQ 1000 / HLG 1000) or the
linear option (PQ in BT.2408 working units → scene-linear by `0.18 / (BT.2408 grey ≈ 26 cd/m² /
203)` — **(to verify)** the scaling choice at C2.9; not used by default).

### 3.6 Reference Gamut Compression

Today's `gamut_compress` is described as "after the ACES Reference Gamut Compression" but uses one
threshold/limit for all channels and runs in the *working* gamut (BT.709). In ACES mode the
**ACES 1.3 Reference Gamut Compression** runs in AP1 with its published per-channel parameters
(§5.5, values **to verify**), as an optional sequence setting (default on, as in common ACES
practice for camera media). Legacy mode keeps the current function untouched.

### 3.7 New colour spaces (Interpret Footage / Color Space Transform)

`Gamut::Ap0`, `Gamut::Ap1`; `ColorSpace::Aces2065_1` (AP0 linear, id `aces2065-1`),
`ColorSpace::AcesCg` (`acescg`), `ColorSpace::AcesCct` (`acescct`), `ColorSpace::AcesCc`
(`acescc`); `Curve::AcesCct`, `Curve::AcesCc`. `from_info` never returns them (no stream metadata
signals ACES); they come from the override or from EXR headers (`chromaticities` attribute —
matching EXR AP0/AP1 chromaticities is a C2 follow-up, not in scope).

---

## 4. Output transform: ACES 1.x RRT + ODT versus ACES 2.0

### 4.1 The options

| | **A. ACES 1.x (1.3) RRT + ODT** | **B. ACES 2.0 Output Transform** | **C. FilmCraft display transform from ACEScg** (non-ACES) |
|---|---|---|---|
| What it is | Fixed Reference Rendering Transform (glow, red modifier, global desaturation, segmented-spline tone scale) followed by a per-display ODT (second spline, dim-surround compensation, display gamut clip). HDR ODTs use the 1.1+ "single stage tone scale" (SSTS). | One parametric transform per display: tone scale parameterised by peak luminance, chroma compression and display gamut compression in a CAM-based (JMh, Hellwig 2022) space, white/limiting-primaries handling. | AP1 → BT.709/BT.2020 matrix + the existing BT.2390 EETF (max(RGB) ratio) + `desaturate_into_gamut`. |
| Specification | Defined by AMPAS reference CTL (aces-dev 1.3); no stand-alone prose formula document for the RRT. | AMPAS ACES 2.0 technical documentation (docs.acescentral.com) + reference CTL (aces-output, 2024–2025). | Ours (BT.2390 is public ITU-R text). |
| Look | The "classic ACES look"; known hue skews (blue → purple, saturated reds/yellows clip abruptly), SDR and HDR renderings do not match well. | Designed to fix those: hue-preserving, smoother gamut approach, SDR/HDR consistent from one parameter set. | Neutral, hue-preserving, not ACES-compliant; looks like today's FilmCraft. |
| Invertibility | Poor (ODT clipping, RRT not cleanly invertible) → weak for graphics (§3.5). | Designed to be invertible within the display gamut (SDR up to its peak) → inverse OT for graphics works. | Invertible below the source peak. |
| Industry state | Still widely used; frozen. | Current release; shipping in OCIO 2.4+ configs and major graders (to verify exact versions); interchange with other ACES 2.0 tools. | — |
| Complexity / GPU cost | Moderate: two splines, a few matrices, per-pixel ALU; easy on GPU. | High: per-pixel CAM forward/inverse, cusp/gamut-boundary tables (hue-indexed, precomputed per display), iterative gamut-boundary search in the reference (can be tabled). Feasible on GPU with precomputed tables. | Trivial (exists). |
| Constants to transcribe | Many spline coefficients from CTL. | Many CAM and tone-scale parameters; tables computed at init from the documented algorithm. | None new. |

### 4.2 Recommendation

**Implement ACES 2.0 as the ACES output transform (option B), and ship option C first as a
fallback/preview view.** Do not implement ACES 1.x unless a user needs to match a 1.x pipeline
(possible later task, open question Q2).

Reasons:

1. **Invertibility** is required for the default handling of graphics and Rec.709 media (§3.5);
   1.x makes titles look wrong in an ACES timeline.
2. **One parametric transform** covers SDR 100 cd/m², PQ 1000 (and later 2000/4000) and HLG from
   the same code, matching FilmCraft's existing peak-driven design (`HDR_PEAK_NITS`,
   `src_peak_nits`), instead of one ODT per display.
3. **Hue behaviour** is better and consistent between SDR and HDR deliveries.
4. It is the current Academy release; building on the frozen 1.x would be legacy on day one.
5. Option C costs nothing (it is today's tone/gamut mapping fed from ACEScg) and lets C2.4–C2.5
   land the whole ACES working/grading pipeline before the large OT work, with the UI labelling it
   honestly ("FilmCraft view", not "ACES").

Implementation strategy for B (C2.6–C2.8):

- **CPU reference first**, `f64`, in `filmcraft-color::aces2`, written from the AMPAS 2.0
  technical documentation. Per-display initialisation (peak, limiting primaries, encoding
  primaries, white) precomputes the tone-scale parameters and the hue-indexed cusp / gamut-boundary
  tables once, cached like `display_transform`.
- **GPU**: an analytic WGSL port of the per-pixel part reading the precomputed tables from a small
  texture, compared against the CPU reference (`f32`). Interim fallback if parity is hard: bake the
  CPU reference into a 65³ 3D LUT over an ACEScct-shaped domain and evaluate it with the existing
  tetrahedral GPU LUT code (C1.7) — documented tolerance ~1e-3, used only until the analytic path
  passes `1e-4`.
- **Oracle**: the AMPAS reference CTL (via `ctlrender`) or OpenColorIO's ACES 2.0 built-in
  transforms as an **external test oracle only**, generating reference values once into a small
  committed test table (numbers, not media), exactly as ffmpeg is used today. Installing either
  tool needs your approval (open question Q1); nothing is linked or shipped.

> [!IMPORTANT]
> **Clean-room question (Q1).** AGENTS.md §2 forbids reading the *source* of reference
> implementations of a *format*; for ACES the Academy's published CTL is itself the normative
> definition of the RRT/ODT (1.x) and accompanies the 2.0 technical documentation. This design
> assumes we implement from the AMPAS **documentation** (prose + equations) and use CTL/OCIO only
> as black-box oracles. If the 2.0 documentation turns out to be insufficient for a constant or a
> step, we need your decision whether reading the AMPAS CTL (Academy licence, permissive with
> attribution — **to verify**) is allowed, recorded in an ADR.

---

## 5. Formulas, constants and their sources

Status column: **pub** = value as published, reproduced from the cited document and to be
re-checked digit by digit when coded (tests pin it); **derived** = computed in code from published
inputs; **to verify** = I am not certain of the exact value; it must be taken from the cited
document at implementation time and is not to be used from this file.

### 5.1 Primaries, white points, matrices

| Item | Value | Source | Status |
|---|---|---|---|
| ACES white point (x, y) | 0.32168, 0.33767 | SMPTE ST 2065-1 (ACES); AMPAS S-2014-004 | pub |
| AP0 primaries R, G, B (x, y) | (0.7347, 0.2653), (0.0000, 1.0000), (0.0001, −0.0770) | SMPTE ST 2065-1 | pub |
| AP1 primaries R, G, B (x, y) | (0.713, 0.293), (0.165, 0.830), (0.128, 0.044) | AMPAS S-2014-004 (ACEScg) | pub |
| AP0 → XYZ | [0.9525523959, 0, 0.0000936786; 0.3439664498, 0.7281660966, −0.0721325464; 0, 0, 1.0088251844] | ST 2065-1 / S-2014-004 (published NPM) | pub — test: `npm(AP0, W_ACES)` matches to 1e-9 |
| AP1 → XYZ | [0.6624541811, 0.1340042065, 0.1561876870; 0.2722287168, 0.6740817658, 0.0536895174; −0.0055746495, 0.0040607335, 1.0103391003] | S-2014-004 | pub — test as above |
| AP0 → AP1 | [1.4514393161, −0.2365107469, −0.2149285693; −0.0765537734, 1.1762296998, −0.0996759264; 0.0083161484, −0.0060324498, 0.9977163014] | S-2014-004 | pub — test: `NPM_AP1⁻¹·NPM_AP0` |
| AP1 → AP0 | [0.6954522414, 0.1406786965, 0.1638690622; 0.0447945634, 0.8596711185, 0.0955343182; −0.0055258826, 0.0040252103, 1.0015006723] | S-2014-004 | pub — test as above |
| AP1 luma weights | 0.2722287168, 0.6740817658, 0.0536895174 (Y row of AP1 → XYZ) | derived from S-2014-004 | derived |
| NPM from chromaticities | `M = [X_r X_g X_b; …]·diag(S)`, `S = [X_r…]⁻¹·W_XYZ` | SMPTE RP 177 (existing `npm()`) | exists |
| Camera gamut chromaticities | as in `crates/color/README.md` table | vendor documents listed there | exists |

The code **derives** every matrix from chromaticities (as `npm()` does today) and the tests compare
against the published matrices above; the published numbers are never the source of truth in code.

### 5.2 ACEScct and ACEScc

ACEScct (AMPAS **S-2016-001**), `lin` = ACEScg value:

```text
encode:  lin ≤ X_BRK : cct = A·lin + B
         lin > X_BRK : cct = (log2(lin) + 9.72) / 17.52
decode:  cct ≤ Y_BRK : lin = (cct − B) / A
         cct < (log2(65504) + 9.72)/17.52 : lin = 2^(cct·17.52 − 9.72)
         else : lin = 65504
X_BRK = 0.0078125   Y_BRK = 0.155251141552511
A = 10.5402377416545   B = 0.0729055341958355
```

ACEScc (AMPAS **S-2014-003**):

```text
encode:  lin ≤ 0      : (log2(2^−16) + 9.72) / 17.52
         lin < 2^−15  : (log2(2^−16 + lin·0.5) + 9.72) / 17.52
         else         : (log2(lin) + 9.72) / 17.52
decode:  cc < (9.72 − 15)/17.52           : (2^(cc·17.52 − 9.72) − 2^−16)·2
         cc < (log2(65504) + 9.72)/17.52   : 2^(cc·17.52 − 9.72)
         else : 65504
```

| Constant | Status |
|---|---|
| 9.72, 17.52 (shared log2 offset/scale) | pub (S-2014-003, S-2016-001) |
| X_BRK, Y_BRK, A, B | pub (S-2016-001); test: continuity of both pieces at X_BRK within 1e-12 |
| 65504 (half-float max clamp) | pub |
| Reference points: 0.18 → 0.4135884 (both curves), 1.0 → 0.5547945, ACEScc minimum −0.3584475 | derived from the formulas above (to be computed by the test, not copied) |

### 5.3 Camera curves and gamuts

Unchanged: formulas and constants as documented in `crates/color/README.md` (§ Camera log curves,
gamut table), with their vendor sources. C2 adds no camera constants.

### 5.4 Chromatic adaptation (D65 ↔ ACES white)

Bradford (von Kries in a sharpened cone space): `CAT = M_B⁻¹ · diag(ρ_d/ρ_s, γ_d/γ_s, β_d/β_s) · M_B`,
with `(ρ, γ, β) = M_B · XYZ_white`.

| Item | Value | Source | Status |
|---|---|---|---|
| `M_B` | [0.8951, 0.2664, −0.1614; −0.7502, 1.7135, 0.0367; 0.0389, −0.0685, 1.0296] | K. M. Lam, *Metamerism and Colour Constancy*, PhD thesis, University of Bradford (1985); reproduced in ICC.1 (ICC profile specification) Annex E | pub |
| D65 (x, y) | 0.3127, 0.3290 | ITU-R BT.709-6 (existing `D65`) | exists |
| Choice of Bradford for D65 ↔ ACES white | — | ACES 1.x display ODTs use a Bradford D60→D65 adaptation; whether ACES 2.0 uses Bradford, CAT02 or none (2.0 handles white differently) | **to verify** against the ACES 2.0 documentation at C2.6 |

### 5.5 ACES 1.3 Reference Gamut Compression (optional stage in AP1)

Per-channel distance `d = (ach − c)/|ach|`, `ach = max(R,G,B)`; distances above the threshold are
compressed with a parametrised power curve so that `limit` maps to 1.0 (same form as today's
`gamut_compress`, with per-channel parameters for the cyan (R), magenta (G) and yellow (B)
distances).

| Parameter | Value | Source | Status |
|---|---|---|---|
| Thresholds C / M / Y | 0.815 / 0.803 / 0.880 | AMPAS, *ACES Reference Gamut Compression* (ACES 1.3, 2021) documentation | **to verify** |
| Limits C / M / Y | 1.147 / 1.264 / 1.312 | same | **to verify** |
| Power | 1.2 | same | **to verify** |
| Working space of the operator | ACEScg (AP1) | same | pub (stated design of the RGC) |

### 5.6 ACES 2.0 Output Transform

Source: AMPAS ACES 2.0 Output Transform technical documentation (docs.acescentral.com, 2024–2025)
and the accompanying reference implementation used **only as an oracle** (Q1).

| Component | Parameters to transcribe at C2.6 | Status |
|---|---|---|
| Tone scale (peak-luminance parameterised, includes mid-grey anchoring and flare/toe) | all constants and the formula for deriving them from peak luminance | **to verify** — not reproduced here |
| CAM: Hellwig 2022 JMh (forward/inverse), adapting luminance, background, surround, compression exponents | all model constants and the AP0→CAM matrices | **to verify** |
| Chroma compression (in-gamut), gamut compression to the limiting primaries, cusp / boundary tables (hue resolution) | all constants, table size, smoothing | **to verify** |
| Display encodings: Rec.709/BT.1886 100 cd/m² (SDR), Rec.2100 PQ 1000 cd/m² (limiting P3-D65 or Rec.2020), Rec.2100 HLG 1000 cd/m² | preset list and their parameters | **to verify** |
| PQ EOTF / inverse | SMPTE ST 2084 (existing `pq_eotf`) | exists |
| HLG OETF / OOTF | ARIB STD-B67, ITU-R BT.2100 (existing) | exists |

### 5.7 Fallback view (option C)

AP1 → BT.709/BT.2020 matrices (derived, §5.1 + §5.4), ITU-R BT.2390 EETF (existing
`bt2390_eetf`), `desaturate_into_gamut` (existing). No new constants.

---

## 6. Test plan

All tests are synthetic (no media committed). GPU tests run with `--test-threads=1` on Vulkan (C1
note).

### 6.1 Legacy must not move

- Every existing test in `filmcraft-color`, `filmcraft-render` (`colorman`, `lumetri_hdr_tests`)
  and `filmcraft-gpu` (`fx_tests`) passes **unmodified**.
- New pin test: `gamut_matrix(a, b)` for all existing D65 pairs equals the pre-C2 result bit for
  bit (computed by the old D65-only formula inside the test).
- `ColorPipeline::REC709` and every pre-C2 project deserialises to the legacy mode; a render of a
  synthetic legacy sequence (SDR, PQ, HLG, wide gamut, S-Log3 clip) is byte-identical before/after
  each C2 commit (8-bit output hash).

### 6.2 Reference values (`filmcraft-color`, CPU, f64)

| Test | Reference | Tolerance |
|---|---|---|
| `npm(AP0, W_ACES)`, `npm(AP1, W_ACES)` | published NPMs (§5.1) | 1e-9 per element (published to 10 decimals) |
| AP0 ↔ AP1 | published matrices (§5.1) | 1e-9 |
| AP1 luma | Y row | 1e-10 |
| Bradford CAT D65→ACES white | maps D65 XYZ to ACES-white XYZ exactly | 1e-12; and white (1,1,1) in any D65 gamut → (1,1,1) in AP1 within 1e-9 |
| ACEScct | continuity at X_BRK/Y_BRK; 0.18 → `(log2 0.18 + 9.72)/17.52`; 0 → B | 1e-12 |
| ACEScc | three-piece continuity; min value | 1e-12 |
| Camera IDTs | 18 % grey code value of every log curve → ACEScg (0.18, 0.18, 0.18) | 1e-6 |
| Camera IDTs vs published vendor/Academy IDT matrices | values copied from the vendor documents at C2.5 (**to verify**), each with a citation in the test | 1e-4 (rounding); larger differences are documented, not hidden |
| RGC | in-gamut colours unchanged; `limit` distance → boundary; monotone | exact / 1e-6 |
| ACES 2.0 OT | oracle table (Q1) of ~200 ACES2065-1 triples → display code values per preset (SDR 709, PQ 1000, HLG 1000), including greys, primaries, out-of-gamut camera colours, 0 and very large values | 1e-4 display-linear (SDR), 1e-4 PQ code value |
| ACES 2.0 OT properties | grey axis stays neutral; monotone along greys; peak input saturates at the display peak; output finite for NaN/±∞/negative inputs | exact / finite |

### 6.3 Round trips

- `AcesCct::decode(encode(x)) = x` for x in [−0.01, 65504] (log-spaced), rel 1e-12; same for ACEScc
  (above its floor).
- Source → ACEScg → same source space (`InputTransform` then the inverse via Color Space
  Transform) for every `ColorSpace` × {Limited, Full}: signal error ≤ 2e-4 (as
  `channel_roundtrips_every_curve`), skipping Apple Log below its floor.
- OT ∘ inverse OT on display code values 0…1 for SDR 709 and 0…PQ(1000) for PQ: ≤ 1e-4 inside the
  display gamut (documents where 2.0 is not exactly invertible).
- Rec.709 title through inverse-OT input → ACEScg → OT (§3.5 option 1): 8-bit output identical to
  the input for ≥ 99.9 % of a synthetic test chart, max 1 code value.

### 6.4 GPU / CPU parity (`crates/gpu/src/fx_tests.rs` style)

Metric as C1: `d = |cpu − gpu| / (1 + |cpu|)`, `flips` for hue ties.

| Case | Tolerance |
|---|---|
| Log decode via table texture (all 10 curves × 2 ranges × 8/10-bit) + matrix + gamut compression (legacy) | `EXACT = 1e-4`, flips 0 |
| Same into ACEScg (IDT + RGC) | `1e-4` |
| Lumetri full chain in `GradeSpace::Acescct` | `1e-4` (log2/exp2 ULPs; slope ≤ 17.52·ln2 ≈ 12, far below PQ's ≈ 495) |
| GPU display transform for non-plain legacy pipelines (C2.3) | `1e-4` SDR, `3e-4` PQ (C1 precedent) |
| ACES 2.0 OT analytic WGSL vs CPU f32 | target `1e-4`; interim 3D-LUT path documented at `1e-3` |
| End-to-end 8-bit composite (S-Log3 clip, ACES SDR) | `max ≤ 1`, `mean < 0.2` (C1.9 precedent) |

### 6.5 Hostile inputs (never-crash §0.8)

- Every new transform under `catch_unwind` with NaN, ±∞, −1e30, 1e30, subnormals: no panic, finite
  output.
- Project files: unknown colour-management mode, unknown output preset, peak nits 0 / negative /
  NaN / 1e9, missing fields → clear error or documented default, never a panic. Fuzz the colour
  settings JSON with mutations (truncation, type flips).
- Commands: `sequence.colorManagement` with bad ids/values → error message listing valid values.

### 6.6 Performance (benchmark, C2.2 and C2.8)

Extend `crates/gpu/examples/lumetri_bench.rs`: 4K S-Log3 Y'CbCr clip, (a) legacy C1.9 path,
(b) GPU log decode (expect upload 66 MB → 12–25 MB and the CPU decode gone), (c) ACES mode with
OT. Real-footage A/B on the ZV-E10 S-Log3 clip as in C1.8 (local media, not committed).

---

## 7. Project-file schema and migration

### 7.1 Model changes (`filmcraft-color`, serialised through `filmcraft-project`)

```rust
/// Sequence Settings ▸ Color ▸ Colour management.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColorManagement { #[default] Legacy, Aces }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcesSettings {
    pub grading: AcesGrading,            // Acescct (default) | Acescc
    pub output: AcesOutput,              // Sdr709 (default) | Pq { peak_nits: u32 } | Hlg { peak_nits: u32 }
    pub view: AcesView,                  // Aces2 (default once C2.8 lands) | FilmCraft (option C)
    pub gamut_compression: bool,         // ACES RGC, default true
    pub display_referred: DisplayReferred, // InverseOutput (default) | Linear
}

pub struct ColorPipeline {
    pub working: WorkingSpace,
    pub wide_gamut: bool,
    pub auto_tone_map: bool,
    #[serde(default, skip_serializing_if = "is_legacy")]
    pub management: ColorManagement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aces: Option<AcesSettings>,
}
```

- Peaks are integer cd/m² (`u32`) so `ColorPipeline` stays `Eq + Hash` (it is a cache key);
  validated to 100…10 000 on load and in commands.
- In ACES mode `working`/`wide_gamut`/`auto_tone_map` are ignored by the renderer but kept, so
  switching back to legacy restores the previous settings. `SequenceSettings::working_space`
  (display label) becomes "ACEScg".
- `ColorSpace` gains `Aces2065_1`, `AcesCg`, `AcesCct`, `AcesCc` (Interpret Footage). Their ids
  are new strings; existing ids never change.
- Color Space Transform effect instances are ordinary `EffectInstance`s (new effect id, §8.3).

### 7.2 Schema bump and migration

- An older build would fail to parse a new `ColorSpace` variant or silently drop `management` /
  `aces` on save, so this is a **schema bump with a no-op migration step** (docs/project-files.md
  rule): `v{N} → v{N+1}` where N is the current `filmcraft_format::SCHEMA_VERSION` (the model
  mentions v15 for project notes; confirm in `crates/format` at C2.4).
- Old files: missing fields → `management = Legacy`, `aces = None` → identical rendering.
- Older builds opening a C2 file get the existing "saved by a newer version" refusal.
- Tests: hand-made fixture of the previous version in `crates/format/tests/fixtures/` loads with
  `Legacy`; a C2 project round-trips; `docs/project-files.md` schema history gets a row
  ("C2.4 | ACES colour management, ACES colour spaces; no-op step").

---

## 8. UI, commands and agent surface

### 8.1 Commands (`crates/engine/src/color.rs`; everything is a command)

| Id | Params | Notes |
|---|---|---|
| `sequence.colorManagement` | `{sequence?, mode: legacy\|aces, output?: sdr709\|pq\|hlg, peakNits?, grading?: acescct\|acescc, view?: aces2\|filmcraft, gamutCompression?, displayReferred?: inverseOutput\|linear}` | one undo step; returns the effective pipeline |
| `sequence.colorInfo` | `{sequence?}` | working, grading, output, view, per-clip input spaces (for agents) |
| `media.interpret` (existing) | `colorSpace` accepts the new ACES ids | |
| `effect.add` (existing) | `colorSpaceTransform` | §8.3 |
| `export.*` (existing) | `outputTransform?` override (e.g. SDR 709 deliverable from a PQ sequence) | C2.7 |

### 8.2 UI (`crates/ui-egui`)

- **Sequence Settings ▸ Color**: "Colour management" dropdown (Rec.709 / HDR | ACES). In ACES
  mode: Output (SDR Rec.709 100 nits / PQ / HLG + peak), Grading space (ACEScct / ACEScc),
  View (ACES 2.0 / FilmCraft), "Gamut compression", "Display-referred media" (Inverse output
  transform / Linear). Legacy controls are hidden, not lost.
- **Interpret Footage ▸ Color Management**: ACES spaces added to the list.
- **Lumetri panel**: grading-space badge ("ACEScct") in the header; HDR White / HDR Range controls
  hidden in ACES mode (the OT defines the display peak).
- **Program monitor**: small view indicator ("ACES 2.0 · Rec.709 100") and a tooltip.
- **Scopes**: default to the output-transformed signal; option "ACEScct (grading signal)".
- **Export**: shows the sequence output transform; HDR exports keep writing PQ/HLG metadata.
- Automation ids: `seqSettings.color.management.<legacy|aces>`, `seqSettings.color.aces.output.<id>`,
  `…aces.peak`, `…aces.grading.<id>`, `…aces.view.<id>`, `…aces.gamutCompression`,
  `…aces.displayReferred.<id>`, `lumetri.gradingSpace`, `monitor.colorView`; UI state stays serde.
- Wording: "ACES" is used descriptively; no ACES logo and no claim of Academy logo-program
  certification.

### 8.3 Color Space Transform effect

Effect id `colorSpaceTransform` (Effects ▸ Color Correction): params `inputSpace`, `outputSpace`
(any `ColorSpace` id incl. ACES), `toneMap` (none / BT.2390 / ACES OT preset), `gamutMap`
(none / desaturate / RGC), `forward` (bool, inverse when off). CPU via `InputTransform`/
`OutputTransform` building blocks, GPU as an `FxOp` (decode-table texture + matrix + optional
stages) with parity tests; documented in `build_effects()` agent docs.

---

## 9. Risks and mitigations

| # | Risk | Mitigation |
|---|---|---|
| R1 | **Clean-room / licence**: the ACES 2.0 documentation may not contain every constant; the normative detail lives in AMPAS CTL. | Implement from documentation; CTL/OCIO only as black-box oracles; if a gap remains, ask (Q1) and record an ADR before reading any reference source. |
| R2 | **ACES 2.0 complexity and GPU cost** (CAM per pixel, gamut-boundary search). | Precompute per-display tables on the CPU; analytic WGSL reading a small table texture; 3D-LUT fallback with documented tolerance; benchmark in C2.8 against the 4K real-time target. |
| R3 | **Constants transcribed wrong** (RGC, OT, vendor IDTs). | Everything "to verify" is checked against the cited document at implementation time; tests pin values with citations; derived matrices are cross-checked against published ones. |
| R4 | **Legacy regressions** (white-point refactor, `GradeSpace` new mode, schema). | Bit-identical pin tests (§6.1); legacy is the default; new paths are additive. |
| R5 | **Lumetri feel in ACEScct** (controls tuned for sRGB signal; `luma709` hard-coded). | Document; AP1 luma in ACES mode only; real retuning and log-wheel controls in C3. |
| R6 | **Display-referred media looks wrong** until the inverse OT exists. | Linear option as stand-in (C2.5) clearly labelled; inverse OT in C2.9 becomes the default. |
| R7 | **CAT choice mismatch** with other ACES tools (Bradford vs CAT02 vs vendor matrices) → small colour differences in interchange. | Make the CAT a parameter internally; document; compare against vendor IDTs (§6.2); Q3. |
| R8 | **f16 accumulator** precision/range for ACEScg (negatives, highlights). | f16 range suffices (65 504); RGC removes most negatives; parity tests include out-of-gamut colours; switch accumulator to `Rgba32Float` only if measured necessary. |
| R9 | **LUTs** (Lumetri input/look LUTs) assume Rec.709 or camera-log input, not ACEScg/ACEScct. | In ACES mode, LUT stages receive the ACEScct signal; document; per-LUT "input space" option is a follow-up (C6 looks). Built-in camera → Rec.709 LUTs are not offered in ACES mode (the IDT does that job). |
| R10 | **Scopes / colour pickers / keyers** read working values that are now scene-linear AP1. | Scopes default to the output-transformed signal; keyers keep working on their existing signal via the grading space; audit list in C2.11. |
| R11 | **Performance of the CPU fallback** (no GPU, scopes `Target::Sequence`) with an extra OT per pixel. | CPU OT uses precomputed tables + Rayon; scopes at reduced size (C1 measurements). |
| R12 | **"ACES" naming** (Academy trademark / logo programme). | Descriptive use only, no logo, no certification claim. |
| R13 | **Quota/time**: C2 is large. | Staged plan below; every stage is independently useful and green. |

---

## 10. Staged plan

Each stage is one or a few commits with its own tests, `cargo xtask ci` green, docs updated
(crate README, `docs/C2-STATUS.md`).

| Stage | Content | Depends on | User-visible |
|---|---|---|---|
| **C2.1** | `Gamut::white()`, Bradford CAT in `gamut_matrix` (only when whites differ), `Gamut::Ap0/Ap1`, ACEScct / ACEScc curves, ACES `ColorSpace`s (CPU), §6.2 reference tests, legacy pin tests. | — | no (library) |
| **C2.2** | **Camera log on the GPU**: decode-table texture in the source shader, gamut compression in WGSL, `gpu_params()` for log, `gpu_input` for log Y'CbCr; plan keeps raw YUV; parity + benchmark. | — | yes: faster log playback in legacy |
| **C2.3** | GPU display/output transform for non-plain legacy pipelines (HDR working, wide gamut); drop the `is_plain()` gate in `gpu_input`. | C2.2 | yes: HDR/wide-gamut sequences on GPU |
| **C2.4** | Schema: `ColorManagement`, `AcesSettings`, schema bump + fixture test, `sequence.colorManagement` / `sequence.colorInfo` commands, docs/project-files.md. | C2.1 | commands only |
| **C2.5** | ACES working pipeline: IDTs into ACEScg (CPU + GPU), RGC (verified constants), `GradeSpace::Acescct` (CPU + WGSL), option-C view (FilmCraft display transform from ACEScg), display-referred media "linear" stand-in. | C2.1–C2.4 | yes: ACES mode (FilmCraft view) |
| **C2.6** | ACES 2.0 Output Transform CPU reference (f64): SDR Rec.709 100 nit; oracle table (after Q1); property tests. | C2.5, Q1 | yes: "ACES 2.0" view (CPU) |
| **C2.7** | OT presets PQ 1000 and HLG 1000; export encoding and export override; HDR metadata. | C2.6 | yes: ACES HDR delivery |
| **C2.8** | OT on the GPU (analytic + tables; LUT fallback if needed); parity; 4K benchmark. | C2.3, C2.6 | yes: real-time ACES |
| **C2.9** | Inverse OT; default handling of display-referred media and graphics in ACES mode; round-trip tests. | C2.6 | yes: titles look right |
| **C2.10** | Color Space Transform effect (CPU + GPU, agent docs, parity). | C2.5 (OT presets after C2.7) | yes |
| **C2.11** | UI: Sequence Settings, Interpret Footage, Lumetri badge, monitor indicator, scopes option, export panel; automation ids; screenshots via the control channel; scopes/keyers audit. | C2.4–C2.9 | yes |
| **C2.12** (optional) | ACES 1.3 RRT/ODT for matching legacy ACES pipelines; AP0 EXR export; EXR chromaticities detection. | Q2 | optional |

C2.2 and C2.3 do not depend on ACES and can start right after approval (they also remove the
C1.9 CPU decode cost).

## 11. Open questions for approval

- **Q1 — Clean room and oracle.** May we (a) install `ctlrender` (AMPAS CTL) or OpenColorIO tools
  locally **as external test oracles only** to produce reference tables, and (b) if the 2.0
  documentation is insufficient, read the AMPAS reference CTL under an ADR? (Nothing would be
  installed without your go-ahead.)
- **Q2 — Output transform.** Approve ACES 2.0 as the ACES output transform with the FilmCraft
  view (option C) first, and ACES 1.x only as optional C2.12?
- **Q3 — IDT matrices.** Derived matrices (chromaticities + Bradford) as the default, vendor-
  published matrices only as test references — or adopt vendor matrices where they differ?
- **Q4 — Lumetri in ACEScct.** Keep the current control maths on the ACEScct signal (with AP1
  luma) and defer retuning to C3?
- **Q5 — Order.** Start with C2.2 + C2.3 (GPU log / GPU display transform, immediate speed win),
  then C2.1/C2.4/C2.5 — or ACES-first (C2.1 → C2.5)?
- **Q6 — Defaults in ACES mode.** RGC on, display-referred media via inverse OT (once C2.9
  lands), output SDR Rec.709 100 nit — OK?


---

## 12. Addendum: Inspection of `composite.wgsl` and Compositor Bindings (C2.2 Findings)

Inspecting `crates/gpu/src/composite.wgsl` and `crates/gpu/src/lib.rs` confirms the exact GPU pipeline structure for C2.2:

1. **Pipeline & Bind Group Layout:**
   - Both direct layer compositing (`layer_pipeline`, `blend_pipeline`) and the effect source pass (`fs_fx_source` in `fx.rs`) share `composite.wgsl`'s vertex and fragment logic.
   - `layer_bgl` currently defines:
     - `@binding(0)`: Uniform buffer `U` (52 `f32`s: 28 layer floats + 24 colour management `q0..q5` floats).
     - `@binding(1)`: `tex0` (Y or RGBA).
     - `@binding(2)`: `tex1` (U / Cb).
     - `@binding(3)`: `tex2` (V / Cr).
     - `@binding(4)`: `backdrop` (only present in `blend_bgl`; omitted in `layer_bgl`).
     - `@binding(5)`: `tex3` (alpha plane, or `dummy`).
   - `@binding(6)` can be added cleanly to both `layer_bgl` and `blend_bgl`:
     `@group(0) @binding(6) var lut_table: texture_2d<f32>;`
     For SDR / linear / PQ / HLG layers that do not need a decode table, slot 6 binds `self.dummy` (which already exists as a 1×1 texture). For log layers, slot 6 binds the cached 8192×1 `R32Float` `DecodeTable` texture.

2. **Log Signal Domain & Clamping in `managed()`:**
   - In `composite.wgsl`, line 128 currently clamps signal inputs: `let v = clamp(rgb, vec3(0.0), vec3(1.0));`.
   - Log media uses `DecodeTable` with range `lo = -0.1`, `hi = 1.1` (8192 samples). For log curves, clamping must be extended to `clamp(rgb, vec3(-0.1), vec3(1.1))` (or inside `sample_decode_table`) to preserve sub-blacks and super-whites up to 1.1.

3. **Gamut Compression in WGSL:**
   - `InputTransform::apply` applies `gamut_compress` after the gamut matrix and tone mapping.
   - In `composite.wgsl`, `gamut_compress` will be added to `managed()`:
     - `s = (1.25 - 0.8) / pow(pow((1.25 - 0.8) / (1.0 - 0.8), 1.2) - 1.0, 1.0 / 1.2) ≈ 0.29803417`.
     - Distance `d = (ach - v) / ach` with threshold 0.8 and power 1.2.
     - Activated via uniform flag (e.g. in `q0` or `q5`).

4. **Integration with `plan.rs` (`C1.9` CPU decode bypass):**
   - In `crates/render/src/plan.rs`, `plain` is defined as `(input.is_some() || !needs_management(pipe, cs, &frame)) && no_blend`.
   - When `gpu_input` returns `Some(input)` for log clips, `plain` becomes `true`.
   - The clip retains its raw `Yuv8` / `Yuv16` `frame` and passes `input` to `PlanLayer`.
   - When effects are present (`!chain.is_empty()`), `fs_fx_source` draws the YUV frame directly into `FxStage` `views[0]` via the GPU decode table, completely eliminating the CPU decode and the 66 MB `f16` CPU upload per 4K frame!

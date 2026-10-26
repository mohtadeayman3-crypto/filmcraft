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

# Task C1 Design — GPU Lumetri Color Grading Pipeline

**Status:** Draft (Phase 1 — Design Only, awaiting review)  
**Scope:** Task C1 (`C1.1`–`C1.6`) from [`FILMCRAFT_COLOR_UPGRADE_PLAN.md`](../FILMCRAFT_COLOR_UPGRADE_PLAN.md)

---

## 1. How a Frame Travels Today (Decode, CPU, GPU, and Readbacks)

### 1.1 End-to-End Frame Lifecycle

```text
Media Container (MP4 / MOV / MKV / MXF / DNG / Raw)
  │
  ├─► Demuxer + GOP-aware Seek (`crates/codecs`)
  │     ├─► Hardware Decode (`crates/platform`: Windows MF D3D11 DXVA / macOS VideoToolbox / Linux VA-API)
  │     │     └─► [GPU → CPU Readback #1] NV12/P010 D3D11 texture → staging texture → planar `Yuv8` / `Yuv16`
  │     └─► Software Decode (`h264`, `hevc`, `prores`, `dnx`, `av1`, `vp9`, `mpeg2v`, `apv`, `dng`)
  │
  ▼
`VideoFrame` in `MediaPool` Byte-Budgeted LRU Frame Cache (`crates/media`)
  │   (Planar `Yuv8`, `Yuv16`, `Rgba8`, or linear premultiplied `RgbaF32`)
  │
  ├─► PATH A: Program Monitor GPU Path (`Target::SequencePlan` in `crates/ui-egui/src/frames.rs`)
  │     │
  │     ├─► 1. `render::plan::plan_frame` (runs on background frame worker thread)
  │     │     • Checks `whole_frame_on_cpu(project, seq, t)`:
  │     │       - Today returns `true` if `seq.settings.color.working.is_hdr()` (PQ/HLG),
  │     │         or if an adjustment layer or CPU-only transition is visible.
  │     │     • Per visible clip, checks `gpu_chain`: all enabled standard effects are in `GPU_EFFECTS`,
  │     │       have no effect masks, clip has no opacity masks, and no CPU-only color management.
  │     │     • Calls `FxOp::eval(e, &cx, lw, lh).filter(FxOp::gpu_ok)` for each effect:
  │     │       - **Fast Sub-path (GPU Effects):** All effects return `Some(op)` with `op.gpu_ok() == true`.
  │     │         Emits `PlanLayer` holding `Arc<VideoFrame>` + `fx: Some(Arc<LayerFx>)`.
  │     │       - **Slow Sub-path (CPU Fallback — current full Lumetri):** Any `op.gpu_ok() == false`.
  │     │         Worker calls `render::item_layer` on the CPU: converts `VideoFrame` to linear `f32`
  │     │         `Image` (`3840×2160×16B = 132.7 MB`), runs `effects::apply` on CPU (~284 ms at 4K),
  │     │         wraps in `VideoFrame::rgba_f32`.
  │     │
  │     ├─► 2. `filmcraft_gpu::prepare(&plan)` (runs on background frame worker thread)
  │     │     • 8-bit `Yuv8` and `Rgba8` frames require no preparation (`None`).
  │     │     • 16-bit `Yuv16` frames are shifted to 16-bit `u16` code units.
  │     │     • CPU-fallback `RgbaF32` frames are converted from `f32` to IEEE `f16` (`u16` bytes, 66.4 MB at 4K).
  │     │
  │     └─► 3. `FilmcraftApp::gpu_present` → `GpuCompositor::composite_prepared` (runs on UI thread)
  │           • Uploads source planes via `queue.write_texture` (cached in `self.cache` by `Arc::as_ptr` address).
  │           • For each `PlanLayer` with `fx: Some(LayerFx)`:
  │             - `FxStage::job` & `FxStage::record`:
  │               1) Render pass `fs_fx_source` (`composite.wgsl`) decodes YUV/RGBA + `InputTransform`
  │                  into pooled `Rgba32Float` working texture `views[0]` (with integer box decimation).
  │               2) Compute passes (`fx_px` @ 16×16 or `fx_run` @ 64×1 in `fx.wgsl`) ping-pong between
  │                  pooled `Rgba32Float` working textures (`views[0]`, `views[1]`, and `views[2]` when
  │                  `combine` is needed for Unsharp Mask).
  │           • Draws `FxJob::result` quad into `Rgba16Float` accumulator texture (`self.accum`) with
  │             Motion `Affine` matrix, opacity, and all 27 blend modes.
  │           • Resolves `self.accum` (`Rgba16Float`) over black into `self.output` (`Rgba8Unorm`, sRGB encoded).
  │           • Registers `self.output` directly with `egui_wgpu::Renderer` (`register_native_texture`).
  │             **Zero GPU→CPU readback on the Program Monitor display path.**
  │
  ├─► PATH B: Export GPU Path (`ExportRenderer::render` in `crates/gpu/src/export_renderer.rs`)
  │     • Runs `plan_frame` → `prepare` → `composite_prepared` on a shared headless `wgpu::Device`.
  │     • [GPU → CPU Readback #2] Calls `GpuCompositor::read_accumulator()`:
  │       copies `self.accum` (`Rgba16Float`) to `accum_readback_buf` (`MAP_READ`), waits via `poll_wait`,
  │       and converts `f16 → f32` on Rayon into a linear `render::Image` for the video encoder.
  │
  └─► PATH C: Pure CPU Render Path (`Target::Sequence` / `render::render_sequence`)
        • Used by video scopes (`panels/scopes.rs` at `size: 250`), eyedropper picking (`panels/monitor.rs`),
          Reference Monitor (`panels/reference.rs`), Export Mode preview (`panels/export_mode.rs`), and
          fallback when GPU is unavailable.
        • Executes everything on the CPU in linear premultiplied `f32` (`render::Image`) and converts
          to `Rgba8` via `over_black_rgba8()`.
```

### 1.2 Exact Inventory of CPU/GPU Transfers and Readbacks

| # | Location | Direction | Format & 4K Size (`3840×2160`) | When It Happens |
|---|---|---|---|---|
| **1** | `crates/platform/src/mf.rs` | GPU → CPU | NV12 (12.4 MB) or P010 (24.9 MB) | Every hardware-decoded frame on Windows (D3D11 staging texture to `VideoFrame::Yuv8`/`Yuv16`). |
| **2** | `crates/gpu/src/lib.rs` (`upload`) | CPU → GPU | `Yuv8` (12.4 MB), `Yuv16` (24.9 MB), or CPU-fallback `Rgba16Float` (66.4 MB) | Once per unique `Arc<VideoFrame>` pointer via `queue.write_texture`. Cached across paused/repeated frames. |
| **3** | `crates/ui-egui/src/lib.rs` (`gpu_present`) | GPU → GPU | `Rgba8Unorm` (33.2 MB) | **No CPU readback.** `self.output` is bound directly as an `egui::TextureId` for monitor presentation. |
| **4** | `crates/gpu/src/lib.rs` (`read_accumulator`) | GPU → CPU | `Rgba16Float` (66.4 MB → 132.7 MB `f32`) | During GPU-accelerated export (`ExportRenderer::render`). Staging buffer map + `f16_to_f32_fast` on Rayon. |
| **5** | `crates/gpu/src/lib.rs` (`effect_image`) | GPU → CPU | `Rgba32Float` (132.7 MB) | Only in `fx_tests.rs` and GPU benchmarks to verify `FxStage` output directly in `f32` against CPU `FxOp::apply`. |
| **6** | `crates/gpu/src/lib.rs` (`read_output`) | GPU → CPU | `Rgba8Unorm` (33.2 MB) | Only in compositor tests and headless screenshot capture. |

**Why 4K Lumetri Playback Is Slow Today:**
In [`crates/render/src/gpufx.rs`](../crates/render/src/gpufx.rs#L532), `FxOp::Lumetri` sets:
```rust
let gpu_capable = !is_hdr && !has_input_lut && !has_look_lut && look == 0
    && sharpen.abs() <= 1e-3 && !has_curves && !has_wheels && !hsl_on;
```
As soon as a user touches Color Wheels, Curves, Creative Look / Sharpen, or HSL Secondary (or works in an HDR sequence), `gpu_capable` becomes `false`. `plan_frame` abandons `LayerFx`, decodes and grades the 4K frame on the CPU (~284 ms/frame), converts 8.29M pixels from `f32` to `f16` on the CPU, and uploads 66.4 MB of `Rgba16Float` data to the GPU instead of a 12.4 MB `Yuv8` frame.

---

## 2. `FxOp` Structure and Evaluation Architecture

### 2.1 Existing `FxOp` Contract

In [`crates/render/src/gpufx.rs`](../crates/render/src/gpufx.rs):
1. **`FxOp::eval(e: &EffectInstance, cx: &FxCtx, w: usize, h: usize) -> Option<FxOp>`** evaluates all keyframes, defaults, `px_scale`, and working-space parameters at `cx.t` into self-contained values on the `FxOp` enum.
2. **`FxOp::gpu_ok(&self) -> bool`** checks that all evaluated floats are finite and that the GPU shader supports the configuration.
3. **`FxOp::apply(&self, img: &mut Image)`** executes the exact CPU reference math using **only** the fields stored inside `FxOp`.
4. [`crates/render/src/effects.rs::apply`](../crates/render/src/effects.rs#L123) runs:
   ```rust
   if let Some(op) = crate::gpufx::FxOp::eval(e, cx, img.w, img.h)
       .filter(|op| e.effect != "lumetri" || op.gpu_ok())
   {
       op.apply(img);
       return;
   }
   ```
   This guarantees that CPU rendering (`effects::apply`) and GPU rendering (`FxStage`) share identical parameter evaluation.

### 2.2 Proposed `FxOp::Lumetri` Structure (`LumetriOp`)

To keep `FxOp` clean and self-contained while supporting all six C1 sub-steps (`C1.1`–`C1.6`) plus HDR (`GradeSpace`) and Creative Sharpen, we extract a dedicated `LumetriOp` struct boxed inside `FxOp::Lumetri(Box<LumetriOp>)` (so `FxOp` enum size stays small for all other effects):

```rust
/// Evaluated parameters for the full Lumetri color grading pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct LumetriOp {
    // --- Grading Spaces (SDR sRGB, Rec.2100 PQ, or Rec.2100 HLG) ---
    /// Grading space for Basic Correction, Creative Adjustments, and Vignette (`hdr_white`).
    pub basic_gs: filmcraft_color::GradeSpace,
    /// Grading space for Creative Look, Wheels, Curves, and HSL Secondary (`curves_hdr_range`).
    pub adv_gs: filmcraft_color::GradeSpace,

    // --- C1.1: Basic Correction ---
    /// White balance gains × exposure in linear light: `[r_gain * exp, g_gain * exp, b_gain * exp]`.
    pub gains_exp: [f32; 3],
    /// Whites & Blacks endpoints: `b0 = -bl * 0.15`, `w0 = 1.0 - wh * 0.15`.
    pub b0: f32,
    pub w0: f32,
    /// Shadows & Highlights scale factors (`sh * 0.35`, `hl * 0.35`).
    pub sh_k: f32,
    pub hl_k: f32,
    /// Contrast (`-1.0 .. 1.0`).
    pub contrast: f32,
    /// HDR Specular (`hdr_specular / 100.0`, 0.0 in SDR).
    pub specular: f32,
    /// Combined Basic × Creative saturation multiplier.
    pub sat: f32,

    // --- C1.4: Creative Adjustments & Procedural Looks ---
    pub creative_on: bool,
    pub faded: f32,
    pub vib: f32,
    /// Pre-scaled shadow tint offset `(st - 0.5) * 0.3`.
    pub st_k: [f32; 3],
    /// Pre-scaled highlight tint offset `(ht - 0.5) * 0.3`.
    pub ht_k: [f32; 3],
    /// Procedural look choice (`0` = None, `1..=8` = built-in looks).
    pub look: u32,
    /// Look intensity (`0.0 .. 2.0`, default `1.0`).
    pub look_intensity: f32,
    /// Creative sharpen box radii `(rx, ry)` for `1.2 * cx.px_scale.max(0.35)` and amount.
    pub sharpen_rx: Vec<u32>,
    pub sharpen_ry: Vec<u32>,
    pub sharpen_amount: f32,

    // --- C1.2: Color Wheels ---
    pub wheels_on: bool,
    /// Combined shadow wheel offset `ws[k] * 0.3 + ls * 0.3`.
    pub wheel_sh: [f32; 3],
    /// Combined midtone wheel offset `wm[k] * 0.3 + lm * 0.3`.
    pub wheel_mid: [f32; 3],
    /// Combined highlight wheel gain multiplier `1.0 + (wh[k] * 0.5 + lh * 0.5) * whi` -> stores `wh[k] * 0.5 + lh * 0.5`.
    pub wheel_hi: [f32; 3],

    // --- C1.3: Curves (1024-sample 1D LUTs) ---
    pub curves_on: bool,
    /// Pre-evaluated 1024-entry LUTs (`None` when curve is identity/empty, or packed into a 9×1024 table).
    pub curve_luma: Option<Vec<f32>>,
    pub curve_red: Option<Vec<f32>>,
    pub curve_green: Option<Vec<f32>>,
    pub curve_blue: Option<Vec<f32>>,
    pub hue_vs_sat: Option<Vec<f32>>,
    pub hue_vs_hue: Option<Vec<f32>>,
    pub hue_vs_luma: Option<Vec<f32>>,
    pub luma_vs_sat: Option<Vec<f32>>,
    pub sat_vs_sat: Option<Vec<f32>>,

    // --- C1.5: HSL Secondary with Matte Refinement ---
    pub hsl_on: bool,
    pub hsl_hue_center: f32,
    pub hsl_hue_range: f32,
    pub hsl_sat_min: f32,
    pub hsl_luma_min: f32,
    pub hsl_luma_max: f32,
    pub hsl_soft: f32,
    pub hsl_show_mask: u32,
    pub hsl_temp: f32,
    pub hsl_tint: f32,
    pub hsl_sat: f32,
    pub hsl_hue_shift: f32,
    pub hsl_denoise: f32,
    pub hsl_denoise_radius: u32,
    pub hsl_blur_radii: Vec<u32>,

    // --- C1.6: Vignette ---
    pub vignette_on: bool,
    pub va: f32,
    pub vmid: f32,
    pub vround: f32,
    pub vfeather: f32,
    pub aspect: f32,

    // --- Fallback flags ---
    /// True if all active sections are supported on the GPU (false only when custom `.cube`/`.3dl`
    /// `input_lut` or `look_lut` is active, unless 3D LUT binding is also enabled).
    pub gpu_capable: bool,
}
```

---

## 3. Proposed WGSL Passes for Each Lumetri Section (`C1.1` – `C1.6`)

On the CPU ([`crates/render/src/effects.rs`](../crates/render/src/effects.rs)), `lumetri` executes four distinct stages in strict sequential order, with an explicit `GradeSpace::decode` → `GradeSpace::encode` boundary between Stage 1 and Stage 2:

1. **Stage 1 (`fn lumetri` pixel loop, in `basic_gs = GradeSpace(working, hdr_white)`):**
   Basic Correction → Creative Faded Film / Split Tone / Saturation / Vibrance → Vignette.
2. **Stage 2 (`fn lumetri_advanced`, in `adv_gs = GradeSpace(working, curves_hdr_range)`):**
   Creative Look → Color Wheels → RGB/Luma Curves → Hue/Sat Curves.
3. **Stage 3 (`fn hsl_secondary`, in `adv_gs`):**
   HSL Key generation → Matte Denoise (median filter) → Matte Blur (3-pass separable box blur) → HSL Secondary correction / mask preview.
4. **Stage 4 (`fn unsharp`, in linear light):**
   Creative Sharpen (`unsharp(img, 1.2 * cx.px_scale.max(0.35), sharpen.max(-1.0), 0.0)`).

To preserve 100% mathematical parity with the CPU (including intermediate clamping in SDR and `hdr_white` vs `curves_hdr_range` scaling in HDR), the GPU pipeline decomposes `FxOp::Lumetri` into `Step` dispatches in `crates/gpu/src/fx.rs` that mirror these exact four stages.

### 3.1 Grading Space Helpers in `fx.wgsl` (`GradeSpace`)

Both SDR (`Rec709`) and HDR (`Rec2100Pq`, `Rec2100Hlg`) grading spaces are supported in WGSL via two integer mode codes (`0 = Sdr`, `1 = Pq`, `2 = Hlg`) and two float parameters (`white`, `norm_or_gamma`):
- `gs_encode(c: vec3<f32>, mode: u32, p1: f32, p2: f32) -> vec3<f32>`:
  - `Sdr (0)`: `enc(c)` (piecewise sRGB OETF on `max(c, 0.0)`).
  - `Pq (1)`: SMPTE ST 2084 inverse EOTF on `clamp(max(c, 0.0) * (203.0 / 10000.0), 0.0, 1.0) / norm`.
  - `Hlg (2)`: ITU-R BT.2100 inverse OOTF (`yd = dot(fd, vec3(0.2627, 0.6780, 0.0593))`, `k = pow(yd, (1.0 - gamma) / gamma)`) followed by `hlg_oetf(fd * k)`.
- `gs_decode(v: vec3<f32>, mode: u32, p1: f32, p2: f32) -> vec3<f32>`:
  - `Sdr (0)`: `dec(v)` (piecewise sRGB EOTF on `clamp(v, 0.0, 1.0)`).
  - `Pq (1)`: `pq_eotf(clamp(v * norm, 0.0, 1.0)) * (10000.0 / 203.0)`.
  - `Hlg (2)`: `hlg_inverse_oetf(clamp(v, 0.0, 1.5))` followed by forward OOTF `ys^(gamma - 1.0) * white / 203.0`.
- `within_unit` wrapper: in SDR, clamps input to `0.0..1.0`; in HDR, computes `m = max(max(v.r, v.g), max(v.b, 1.0))`, runs the operation on `max(v / m, 0.0)`, and scales the result back by `m`.

### 3.2 Commit `C1.1`: Basic Correction (+ HDR Support) — `OP_LUMETRI`

- **When emitted:** Always emitted as the first pass of `FxOp::Lumetri` (unless all Basic, Creative adjustment, and Vignette controls are at identity).
- **WGSL entry / op:** `OP_LUMETRI` (`31u`) in `fx_px`.
- **Operations per pixel `(x, y)`:**
  1. Unpremultiply straight RGB `c = o.rgb / a` (for `a > 1e-6`).
  2. Linear white balance + exposure: `lin = c * gains_exp`.
  3. Encode to grading signal: `v = gs_encode(lin, basic_gs)`.
  4. Whites / Blacks endpoints: `v = (v - b0) / max(w0 - b0, 1e-3)`.
  5. Highlights / Shadows cubic luma lift:
     - `l = luma709(v)`
     - `ws = pow(clamp(1.0 - l, 0.0, 1.0), 3.0)`, `whl = pow(clamp(l, 0.0, 1.0), 3.0)`
     - `nl = max(l + sh_k * ws + hl_k * whl, 0.0)`
     - `if l > 1e-5 { v *= nl / l; }`
  6. Contrast S-curve around 0.5 (in HDR, components `q > 1.0` bypass contrast as on CPU line 679).
  7. HDR Specular (`if abs(specular) > 1e-4`): for `q > 1.0`, `q = 1.0 + (q - 1.0) * max(1.0 + specular, 0.0)`.
  8. Creative Faded Film, Split Tone (`st_k`, `ht_k`), and Saturation × Vibrance.
  9. Vignette (see §3.7).
  10. Decode back to linear: `res = gs_decode(v, basic_gs)`, premultiply `vec4(res * a, a)`.

### 3.3 Commit `C1.2`: Color Wheels — `OP_LUMETRI_ADV`

- **When emitted:** Emitted when `wheels`, `curves`, or `look > 0` is active in `lumetri_advanced`.
- **WGSL op:** `OP_LUMETRI_ADV` (`32u`) in `fx_px`.
- **Math (matching `effects.rs:1173–1184`):**
  1. Encode linear `c` into `adv_gs`: `v = gs_encode(c, adv_gs)`.
  2. Compute luma weights:
     - `l = clamp(luma709(v), 0.0, 1.0)`
     - `wsh = (1.0 - l) * (1.0 - l)`
     - `whi = l * l`
     - `wmid = max(1.0 - wsh - whi, 0.0)`
  3. Apply per-channel lift, midtone offset, and highlight gain:
     - `v += wheel_sh * wsh + wheel_mid * wmid`
     - `v *= vec3(1.0) + wheel_hi * whi`
     where `wheel_sh[k] = (ws[k] + ls) * 0.3`, `wheel_mid[k] = (wm[k] + lm) * 0.3`, and `wheel_hi[k] = (wh[k] + lh) * 0.5` are pre-evaluated on the CPU in `FxOp::eval`.

### 3.4 Commit `C1.3`: RGB & Hue/Sat Curves — `OP_LUMETRI_ADV` (with 1D LUT Texture)

- **When emitted:** Runs inside `OP_LUMETRI_ADV` (`32u`) immediately after Color Wheels, matching `effects.rs:1185–1220`.
- **Bitmask in `u.i1.y` (`curve_mask`):**
  - Bit 0: `curve_luma`
  - Bit 1: `curve_red`
  - Bit 2: `curve_green`
  - Bit 3: `curve_blue`
  - Bit 4: `hue_vs_sat`
  - Bit 5: `hue_vs_hue`
  - Bit 6: `hue_vs_luma`
  - Bit 7: `luma_vs_sat`
  - Bit 8: `sat_vs_sat`
- **Curve sampling helper `sample_curve(row: i32, x: f32, hdr: bool) -> f32`:**
  Matches the CPU's linear interpolation over `N = 1024` samples (`effects.rs:1154–1163`):
  ```wgsl
  fn sample_curve(row: i32, x: f32, hdr: bool) -> f32 {
      if hdr && x > 1.0 {
          return textureLoad(aux, vec2<i32>(1023, row), 0).r + (x - 1.0);
      }
      let p = clamp(x, 0.0, 1.0) * 1023.0;
      let i = i32(p);
      let j = min(i + 1, 1023);
      let a = textureLoad(aux, vec2<i32>(i, row), 0).r;
      let b = textureLoad(aux, vec2<i32>(j, row), 0).r;
      return a + (b - a) * (p - f32(i));
  }
  ```
- **Execution order:**
  1. If `curve_luma` active: `v = vec3(sample_curve(0, v.r, hdr), sample_curve(0, v.g, hdr), sample_curve(0, v.b, hdr))`.
  2. If `curve_red` active: `v.r = sample_curve(1, v.r, hdr)`.
  3. If `curve_green` active: `v.g = sample_curve(2, v.g, hdr)`.
  4. If `curve_blue` active: `v.b = sample_curve(3, v.b, hdr)`.
  5. If any of the 5 HSL curves (bits 4..=8) is active:
     Run inside `within_unit(v, hdr, ...)`:
     - `var h = rgb_to_hsl(u)` (`h0 = h.x`, `s0 = h.y`, `l0 = h.z`).
     - If `hue_vs_hue` (row 5): `h.x = fract_euclid(h.x + (sample_curve(5, h0, false) - 0.5))`.
     - `var sm = 1.0;`
     - If `hue_vs_sat` (row 4): `sm *= sample_curve(4, h0, false) * 2.0`.
     - If `luma_vs_sat` (row 7): `sm *= sample_curve(7, l0, false) * 2.0`.
     - If `sat_vs_sat` (row 8): `sm *= sample_curve(8, s0, false) * 2.0`.
     - `h.y = clamp(h.y * sm, 0.0, 1.0)`.
     - If `hue_vs_luma` (row 6): `h.z = clamp(h.z + (sample_curve(6, h0, false) - 0.5) * 0.5, 0.0, 1.0)`.
     - Convert back via `hsl_to_rgb(h.x, h.y, h.z)`.

### 3.5 Commit `C1.4`: Creative Looks & Creative Sharpen

1. **Procedural Creative Looks (`look` = `1..=8`, `look_intensity`):**
   - Runs at the start of `OP_LUMETRI_ADV` (`32u`), before Color Wheels, matching `effects.rs:1169–1172`.
   - Implements `apply_look(look: u32, c: vec3<f32>) -> vec3<f32>` in `fx.wgsl` with the exact 8 procedural grades from `effects.rs:1092–1119` (Teal & Orange, Warm, Cool, High Contrast, Bleached, Noir, Golden, Cold Night).
   - Blended by `look_k = look_intensity`: `v = v + (lk - v) * look_k` (wrapped in `within_unit` when `hdr` is true).
2. **Creative Sharpen (`sharpen`):**
   - When `sharpen.abs() > 1e-3`, `steps()` appends the existing `Unsharp` box-blur passes (`sharpen_rx`, `sharpen_ry`, `repeat = true`, `unsharp_blur = true`) followed by `OP_UNSHARP` (`amount = sharpen.max(-1.0)`, `threshold = 0.0`, `combine = true`), matching `effects.rs:721–723` (`unsharp(img, 1.2 * cx.px_scale.max(0.35), sharpen.max(-1.0), 0.0)`).

### 3.6 Commit `C1.5`: HSL Secondary with Matte Refinement (Denoise & Blur)

When `hsl_on` is `true`, there are two execution cases:

#### Case A: Fast Single-Pass (`denoise == 0.0` and `hsl_blur_radii.is_empty()`)
- Emits a single pass `OP_LUMETRI_HSL_APPLY` (`35u`, with flag `has_external_mask = 0u`).
- Computes `m = hsl_key(u, hc, hr, smin, lmin, lmax, soft)` inline per pixel and immediately applies the HSL secondary color correction (or mask preview mode `1`, `2`, `3`).

#### Case B: Multi-Pass with Matte Refinement (`denoise > 0.0` or `!hsl_blur_radii.is_empty()`)
Reuses `FxStage`'s 3-texture `orig` / `aux` retention mechanism (the same mechanism used by `Unsharp` to keep the unblurred source image in `orig` while ping-ponging the intermediate buffer):
1. **Pass 1 — `OP_LUMETRI_HSL_KEY` (`33u`, `unsharp_blur = true` so `orig` retains the graded RGB image):**
   - For each pixel, computes the key `m` (`0.0` if `a <= 1e-6`, else `hsl_key(unit(c), ...)`).
   - Writes `vec4(m, m, m, 1.0)` into the working texture.
2. **Pass 2 — `OP_LUMETRI_HSL_DENOISE` (`34u`, if `denoise > 0.0`, `unsharp_blur = true`):**
   - Radius `r = ((1.0 + 2.0 * denoise) * px_scale).round().max(1.0) as u32`.
   - At typical playback/4K scales (`px_scale <= 1.0`), `r <= 3`, so the window `(2r+1)²` has at most `7×7 = 49` samples (or capped at `r <= 4`, `81` samples).
   - Loads the clamped `(2r+1)²` neighborhood `.r` values into a local fixed-size array in WGSL, finds the exact median element `win.len() / 2` via partial selection sort, and mixes: `m_out = m + (median - m) * min(denoise, 1.0)`.
3. **Pass 3 — `OP_BOX` (`20u`, if `!hsl_blur_radii.is_empty()`, `unsharp_blur = true`):**
   - Emits the 3 horizontal and 3 vertical box-blur passes (`boxes(&hsl_blur_radii, false, true)` and `boxes(&hsl_blur_radii, true, true)`). Because the mask is stored in `.r` (with `.rgb = m, .a = 1.0`), `OP_BOX` blurs the matte with `O(1)` running sums for large radii (`r > 32`) and per-pixel sums for small radii, identically to `effects.rs::blur_plane`!
4. **Pass 4 — `OP_LUMETRI_HSL_APPLY` (`35u`, `combine = true`):**
   - Reads the refined matte `m = ld(p).r` from `src` (`binding 1`) and the pre-HSL graded pixel `orig_px = textureLoad(aux, p, 0)` from `aux` (`binding 3`).
   - Applies the HSL Secondary correction (`hshift`, `hsat`, `htemp`, `htint`) or mask view (`show_mask` = 1 Color/Gray, 2 Color/Black, 3 White/Black) and writes the final linear premultiplied pixel.

### 3.7 Commit `C1.6`: Vignette

- Runs as the final step of `OP_LUMETRI` (`31u`) in `basic_gs` before `gs_decode`, matching `effects.rs:708–716`.
- Note: Basic vignette is already implemented in `fx.wgsl` (`lines 509–528`) for SDR; in `C1.6` we ensure full coverage across SDR and HDR grading spaces, extreme roundness/feather/midpoint combinations, standalone vignette vs chained Lumetri vignette, and dedicated parity test cases in `fx_tests.rs`.

---

## 4. How Curves and Wheels Data Reach the GPU

### 4.1 Uniform Buffer (`struct U` at `@binding(0)`) for Scalars and Wheels

`crates/gpu/src/fx.rs` already uploads a 128-byte uniform buffer per compute pass (`i0: vec4<u32>`, `i1: vec4<u32>`, `p0..p5: 6 × vec4<f32> = 24 floats`).
Because we split Basic+Creative+Vignette (`OP_LUMETRI`), Look+Wheels+Curves (`OP_LUMETRI_ADV`), and HSL Secondary (`OP_LUMETRI_HSL_*`) into separate passes, **every pass's scalar and vector parameters fit inside the existing 24-float uniform array `p[0..24]` without changing the uniform buffer layout**:

- **`OP_LUMETRI` (`31u`) uniform packing (24 floats):**
  - `i1`: `[ flags (bit0: creative_on, bit1: vignette_on), basic_gs_mode, 0, 0 ]`
  - `p[0..3]`: `gains_exp` (`r, g, b`), `p[3]`: `b0`
  - `p[4]`: `w0`, `p[5]`: `sh_k`, `p[6]`: `hl_k`, `p[7]`: `contrast`
  - `p[8]`: `faded`, `p[9]`: `sat`, `p[10]`: `vib`, `p[11]`: `va`
  - `p[12]`: `vmid`, `p[13]`: `vfeather`, `p[14]`: `vround_aspect`, `p[15]`: `specular`
  - `p[16..19]`: `st_k` (`r, g, b`), `p[19]`: `basic_gs_white`
  - `p[20..23]`: `ht_k` (`r, g, b`), `p[23]`: `basic_gs_norm_or_gamma`

- **`OP_LUMETRI_ADV` (`32u`) uniform packing (Wheels + Look + Curves metadata):**
  - `i1`: `[ look, curve_mask (9 bits), adv_gs_mode, wheels_on as u32 ]`
  - `p[0..3]`: `wheel_sh` (`r, g, b`), `p[3]`: `look_intensity`
  - `p[4..7]`: `wheel_mid` (`r, g, b`), `p[7]`: `adv_gs_white`
  - `p[8..11]`: `wheel_hi` (`r, g, b`), `p[11]`: `adv_gs_norm_or_gamma`
  - `p[12..24]`: reserved (`0.0`).

- **`OP_LUMETRI_HSL_KEY` (`33u`) / `OP_LUMETRI_HSL_APPLY` (`35u`) uniform packing:**
  - `i1`: `[ show_mask, has_external_mask, adv_gs_mode, 0 ]`
  - `p[0..6]`: `hc`, `hr`, `smin`, `lmin`, `lmax`, `soft`
  - `p[6..10]`: `htemp`, `htint`, `hsat`, `hshift`
  - `p[10..12]`: `adv_gs_white`, `adv_gs_norm_or_gamma`

### 4.2 2D LUT Texture (`1024 × 9`, `R32Float` or `Rgba32Float`) at `@binding(3)` (`aux`) for Curves

Lumetri has **9 curves** (`curve_luma`, `curve_red`, `curve_green`, `curve_blue`, `hue_vs_sat`, `hue_vs_hue`, `hue_vs_luma`, `luma_vs_sat`, `sat_vs_sat`), each evaluated on the CPU to a `1024`-sample `f32` table via `curve_lut(&pts, 1024)` or `hue_lut(&pts, 1024)`.
- Total size: `9 × 1024 × 4 bytes = 36 KB`.
- **Why `@binding(3)` (`var aux: texture_2d<f32>`) is ideal:**
  1. `@binding(3)` is already declared in `FxStage::bgl` (`crates/gpu/src/fx.rs:299`) and `fx.wgsl:21` as `var aux: texture_2d<f32>` (`TextureSampleType::Float { filterable: false }`, `D2`).
  2. During `OP_LUMETRI_ADV`, `combine` is `false`, so `aux` is otherwise bound to `dummy`.
  3. We allocate a small `1024 × 9` texture (`wgpu::TextureFormat::R32Float` or `Rgba32Float` — both are `TextureSampleType::Float { filterable: false }` and universally supported for `textureLoad` in WebGPU/wgpu without extra features) per `OP_LUMETRI_ADV` step (or cached in `FxStage`), write the 9 rows via `queue.write_texture`, and bind its `TextureView` to `@binding(3)` (`aux`)!
  4. In `fx.wgsl`, `sample_curve(row, x, hdr)` calls `textureLoad(aux, vec2<i32>(i, row), 0).r` and performs the exact `l[i] + (l[j] - l[i]) * (p - f32(i))` interpolation in `f32`, avoiding hardware `f16` linear-filtering quantization so GPU and CPU curve lookups match to `~1e-7`!

---

## 5. Parity-Test Tolerances (`crates/gpu/src/fx_tests.rs`)

Every sub-step (`C1.1`–`C1.6`) adds test cases to `crates/gpu/src/fx_tests.rs` (and CPU-vs-CPU checks in `crates/render/src/effects.rs::lumetri_cpu_parity_tests`) with the following documented tolerances:

| Test Suite | Comparison | Metric | Tolerance | Notes |
|---|---|---|---|---|
| **CPU `FxOp::apply` vs `effects::lumetri`** (`effects.rs`) | `FxOp::eval().apply()` vs legacy `fn lumetri` | `max \|a - b\|` | `<= 1e-6` | Identical `f32` algorithm on CPU; verifies zero drift when `effects::apply` delegates to `FxOp::Lumetri`. |
| **HDR CPU & `FxOp` Regression** (`lumetri_hdr_tests.rs`) | PQ & HLG nit values (`0..10,000 cd/m²`) | `\|got - want\| / max(want, 1.0)` | `<= 2e-3` to `3e-3` | Existing 6 HDR + HSL refine tests in `lumetri_hdr_tests.rs` must pass unmodified. |
| **GPU `FxStage` vs CPU `FxOp::apply` — Continuous** (`C1.1`, `C1.2`, `C1.3`, `C1.4`, `C1.6`) | `GpuCompositor::effect_image` (`Rgba32Float`) vs `FxOp::apply` on `67×41` test frame | `d = \|cpu - gpu\| / (1.0 + \|cpu\|)` | `worst <= 1e-4` (`EXACT`), `flips == 0.0` | Because curves use `R32Float`/`Rgba32Float` `textureLoad` + explicit `f32` lerp in WGSL, curve interpolation matches CPU to `< 1e-5`. |
| **GPU `FxStage` vs CPU `FxOp::apply` — HSL Secondary & Sharpen** (`C1.4` sharpen, `C1.5` HSL) | `GpuCompositor::effect_image` (`Rgba32Float`) vs `FxOp::apply` | `d = \|cpu - gpu\| / (1.0 + \|cpu\|)` | `worst <= 1e-4` (`EXACT`), `flips <= 0.01` (`FLIPS`) | Allows up to 1% boundary flips for achromatic hue tie-breaks or median-filter boundary samples, matching `LeaveColor` and `UnsharpMask`. |
| **Composited Layer Plan** (`gpu_effect_layers_match_cpu_plan`) | `GpuCompositor::read_output` (`Rgba8`) vs `execute_cpu` | Interior 8-bit channel diff (`p99`, `mean`) | `p99 <= 6`, `mean < 1.5` | End-to-end YUV/RGBA decode + `FxStage` + Motion + Blend + sRGB resolve. |

Update to `lumetri_unsupported_sections_fall_back` in `crates/gpu/src/fx_tests.rs`:
- As each sub-step `C1.1`–`C1.6` lands, the corresponding assertion in `lumetri_unsupported_sections_fall_back` moves from asserting `!op.gpu_ok()` to asserting `op.gpu_ok()` and verifying GPU-vs-CPU parity in `cases()`.
- External file LUTs (`input_lut` and `look_lut`) remain `!op.gpu_ok()` (CPU fallback) as required by the test suite.

---

## 6. Commit Breakdown (Phase 2) & Benchmark Plan (Phase 3)

### 6.1 Phase 2 Implementation Order (One Local Commit Each)

1. **`C1.1`: Basic Correction + HDR `GradeSpace` on GPU**
   - Extract `LumetriOp` in `crates/render/src/gpufx.rs`; add `GradeSpace` (`Sdr`, `Pq`, `Hlg`) encode/decode + `hdr_specular` to `FxOp::Lumetri` and `fx.wgsl` (`OP_LUMETRI`).
   - Add SDR + HDR Basic Correction parity cases in `crates/gpu/src/fx_tests.rs`.
2. **`C1.2`: Color Wheels on GPU**
   - Evaluate `wheel_shadows`, `wheel_midtones`, `wheel_highlights` and luma sliders in `FxOp::eval` (`wheel_sh`, `wheel_mid`, `wheel_hi`).
   - Add `OP_LUMETRI_ADV` (`32u`) in `crates/gpu/src/fx.rs` and `fx.wgsl`; add Wheels parity cases in `fx_tests.rs`.
3. **`C1.3`: RGB & Hue/Sat Curves on GPU via 1D LUT Texture**
   - Evaluate the 9 curves (1024 floats each) in `FxOp::eval`; upload active curves into a pooled `1024 × 9` `Rgba32Float` (or `R32Float`) texture bound to `@binding(3)` (`aux`) in `FxStage`.
   - Implement `sample_curve` and RGB + HSL curve evaluation in `OP_LUMETRI_ADV` (`fx.wgsl`); add Luma/RGB and Hue/Sat curve parity cases in `fx_tests.rs`.
4. **`C1.4`: Creative Looks & Creative Sharpen on GPU**
   - Implement `apply_look` (looks `1..=8` + `look_intensity` + HDR `within_unit`) in `OP_LUMETRI_ADV` (`fx.wgsl`).
   - Wire Creative `sharpen` to emit box-blur + `OP_UNSHARP` steps in `crates/gpu/src/fx.rs`; add parity cases for all 8 looks and sharpen in `fx_tests.rs`.
5. **`C1.5`: HSL Secondary with Matte Refinement (Denoise & Blur) on GPU**
   - Add `OP_LUMETRI_HSL_KEY` (`33u`), `OP_LUMETRI_HSL_DENOISE` (`34u`), `OP_BOX` (`20u` reuse), and `OP_LUMETRI_HSL_APPLY` (`35u`) in `fx.rs` and `fx.wgsl`.
   - Add parity cases for HSL Secondary correction, mask preview modes (`1..=3`), Denoise, and Blur in `fx_tests.rs`.
6. **`C1.6`: Vignette Full Parity & Combined Chain Verification**
   - Complete end-to-end verification of Vignette across SDR/HDR, roundness extremes (`-100..+100`), feathering, and full 6-section combined Lumetri chains in `fx_tests.rs`.

### 6.2 Phase 3 Benchmark Extensions (`crates/render/examples/lumetri_bench.rs` / `crates/gpu`)

Because `filmcraft-render` is in L3 and `filmcraft-gpu` depends on `filmcraft-render` (`gpu → render` same-layer edge in `xtask/src/main.rs`), the GPU benchmark will either live in `crates/gpu/examples/lumetri_bench.rs` (or `crates/ui-egui/examples/bench_playback.rs`) and report all three required metrics at `3840×2160`:
- **(a) GPU grading time only:** Dispatching `FxStage` on an already-uploaded 4K frame (`queue.submit` + `device.poll(Wait)`), measuring pure WGSL compute execution time in ms/frame.
- **(b) End-to-end 4K frame time including upload and readback:** Uploading a fresh 4K frame (`queue.write_texture`), running `FxStage`, and reading the `Rgba32Float` / `Rgba16Float` result back to CPU memory (`effect_image` / `read_accumulator`).
- **(c) Playback FPS of a 4K clip in the app with the full Lumetri grade applied:** Measured through the app's playback pipeline (`Target::SequencePlan` + `GpuCompositor::composite_prepared`, where no GPU→CPU readback occurs on the monitor path).

---

## 7. Risks, Mitigations, and Open Questions

### 7.1 Technical Risks & Mitigations

1. **Per-Frame Curve Texture Allocation Overhead:**
   - *Risk:* Creating a new `wgpu::Texture` inside `FxStage::job` on every frame when curves are active would add driver allocation latency.
   - *Mitigation:* Pool `1024 × 9` curve LUT textures inside `FxStage` (keyed by dispatch index, analogous to `self.pool` for working textures) and update contents via `queue.write_texture`.
2. **HSL Secondary Median Filter Neighborhood Size (`OP_LUMETRI_HSL_DENOISE`):**
   - *Risk:* WGSL requires fixed-size local arrays for sorting. On the CPU, `r = ((1.0 + 2.0 * denoise) * px_scale).round().max(1.0) as usize`. At `px_scale = 1.0`, `r <= 3` (`7×7 = 49` elements). If a clip is magnified (`px_scale > 1.0`), `r` could exceed 3.
   - *Mitigation:* Support up to `r = 4` (`9×9 = 81` floats in a fixed WGSL array) and set `gpu_ok = false` (CPU fallback) only if `hsl_denoise_radius > 4`.
3. **Unsharp / HSL Secondary Sequential `orig` Slot in `FxStage::job`:**
   - *Risk:* Both HSL Secondary (with Denoise/Blur) and Creative Sharpen use `orig` (`views[2]`) to hold the pre-blur image across box-blur passes and then `combine = true` to merge with `orig`.
   - *Verification:* In `FxStage::job` (`crates/gpu/src/fx.rs:402–448`), when `s.combine` finishes (at the end of `OP_LUMETRI_HSL_APPLY`), `orig` is reset to `None` (`if s.combine { orig = None; }`). When Creative Sharpen runs immediately after, `orig` is captured anew (`Some(cur)`) from the post-HSL texture! Thus HSL Secondary + Creative Sharpen chain together seamlessly using the existing 3 pooled textures (`n = 3`) with zero conflicts.
4. **Timeline Render-Bar Cost Estimate (`crates/render/src/preview.rs`) and Whole-Frame HDR Check (`crates/render/src/plan.rs`):**
   - In `crates/render/src/plan.rs:266`, `whole_frame_on_cpu` currently returns `true` when `seq.settings.color.working.is_hdr()`.
   - In `crates/render/src/preview.rs:402`, `effect_cost_ms("lumetri")` is hardcoded to `50.0` ms (CPU cost) and `cpu_layer` assumes any clip with standard effects runs on the CPU.

### 7.2 Open Questions for User Approval

1. **Benchmark Location (Phase 3):** Because `filmcraft-gpu` depends on `filmcraft-render` (`gpu → render` in `cargo xtask layers`), `crates/render` cannot depend on `filmcraft-gpu`. Should we add the GPU + end-to-end benchmark as `crates/gpu/examples/lumetri_bench.rs` (keeping `crates/render/examples/lumetri_bench.rs` or moving it to `crates/gpu/examples/lumetri_bench.rs` so one binary runs both CPU and GPU benchmarks side-by-side)?
2. **External `.cube` / `.3dl` LUTs (`input_lut`, `look_lut`):** The existing test `lumetri_unsupported_sections_fall_back` in `crates/gpu/src/fx_tests.rs:520` explicitly asserts that `input_lut` and `look_lut` fall back to the CPU (`!op.gpu_ok()`), and C1's 6 commits (`C1.1`–`C1.6`) cover Basic, Wheels, Curves, Creative Looks, HSL Secondary, and Vignette. Confirming: should external `.cube`/`.3dl` file LUTs remain on the CPU fallback in C1?

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

## Next

Phase 1 approved by user.
Reviewing Section 5 sources and full "to verify" list.
Inspected `composite.wgsl` and updated `docs/C2-DESIGN.md` §12.
Awaiting user confirmation on sandbox permission for real S-Log3 footage `C:\Users\Mohtady\Downloads\C1102.MP4` and `D:\FilmCraftX-ab` directory before running C2.2 A/B check.

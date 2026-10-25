# FilmCraft Fork — Advanced Color Upgrade Plan (v0.1)

Supersedes the "build from scratch" route in `NLEX_MASTER_SPEC.md` (keep that file only as a reference for color/OFX ideas).

## 1. What we found (verified by reading the source)

- The uploaded zip is a **compiled Windows build** (`filmcraft.exe`, `filmcraft-cli.exe`), not source. The public source is `https://github.com/storytold/filmcraft` (shallow-cloned and inspected, latest `main`, which may be newer than 0.12.0).
- FilmCraft is an **open-source, pure-Rust, Premiere-style NLE**. Licence: **MIT OR Apache-2.0** (free to fork and modify; keep `LICENSE-*` and `NOTICE`; the ArtCraft name/logos in `docs/brand/` must be removed from a fork).
- Already present (so we do NOT rebuild it): Premiere-like panels/workspaces, 3-point editing, all trim tools, J/K/L incl. dynamic trimming, exact integer time (254,016,000,000 ticks/s), shortcut editor with presets, multicam, nesting, effects (90+), keyframing with graphs, wgpu GPU compositor, own H.264/HEVC/ProRes/AAC codecs, audio mixer, captions, EDL/XML/AAF/OTIO interchange, 650+ commands, MCP server + JSON control channel (agents can drive the app).
- Existing color (`crates/color`, `crates/render`, `crates/scopes`, `crates/ui-egui/src/panels/lumetri.rs`): Lumetri-style panel (Basic, Creative, Curves incl. hue-vs-sat etc., Wheels, HSL Secondary, Vignette), 32-bit float linear light, HDR PQ/HLG grading space, camera log curves (S-Log3, LogC3/4, V-Log, C-Log2/3, Apple Log, D-Log, F-Log), `.cube/.3dl` LUTs with tetrahedral interpolation, waveform/parade/vectorscope/histogram, colour match.
- **Gaps relevant to us (from their own README / gaps.md):** grading, keys and export rendering still run on the **CPU**; **no node-based grading**; **no ACES/OCIO-style managed pipeline**; **no plugin hosting (OpenFX)**; alpha quality ("ready for real work ~56%"), Windows less tested than macOS; H.265 export only via hardware encoder.

## 2. Strategy

1. **Fork FilmCraft** (`main`), keep upstream remote so we can pull their fixes.
2. **Color is our differentiator.** Target order: speed (GPU) → managed color (ACES) → node grading → qualifiers/windows → scopes → looks → chroma key → OpenFX.
3. Follow upstream rules exactly (`AGENTS.md`, `CLAUDE.md`): never-crash (no `unwrap/expect/panic`, no `unsafe` outside `crates/platform`), clean-room, pure Rust, layering enforced by `cargo xtask layers`, one task id per commit, `cargo xtask ci` green.
4. **Pure Rust color management** (own ACES implementation from public AMPAS specs and published matrices) rather than linking C++ OpenColorIO — keeps the layering/WASM rules and mergeability. Re-evaluate only if a feature needs an OCIO config.
5. Honest scope: matching Lumetri is already largely done upstream; **surpassing Lumetri** (node graph, ACES, GPU, better keys/scopes) is realistic in months; surpassing DaVinci Resolve's full color page is a long-term goal.

## 3. Where things live (starting map)

| Concern | Path |
|---|---|
| Color science (curves, gamuts, transforms, LUT) | `crates/color/src/{log,spaces,transform,lut,grade}.rs` |
| Effect definitions (params, UI, agent docs) | `crates/project/src/effect.rs` (`build_effects()`) |
| CPU effect implementation incl. Lumetri | `crates/render/src/effects.rs` (`fn lumetri`, `fn lumetri_advanced`) |
| GPU effect ops | `crates/render/src/gpufx.rs` (`FxOp`, `GPU_EFFECTS`), `crates/gpu/src/fx.rs`, `crates/gpu/src/fx.wgsl` |
| GPU LUT | `crates/gpu/src/lut.rs`, `lut.wgsl` |
| Masks | `crates/render/src/mask.rs`, `crates/gpu/src/mask.rs` |
| Scopes maths | `crates/scopes/src/lib.rs`; UI `crates/ui-egui/src/panels/scopes.rs` |
| Color panel UI | `crates/ui-egui/src/panels/lumetri.rs` |
| Color commands | `crates/engine/src/color.rs` |
| Project schema | `crates/project/`, docs in `docs/project-files.md` |
| How to add an effect / command / panel | `docs/contributing.md` §6 |

## 4. Work packages (each = a series of small commits with tests)

**C0 — Fork, build, baseline (1–2 days).** Build on Windows (`cargo run --release -p filmcraft`), run the demo, run `cargo xtask ci`, record baseline timings of grading a 4K frame (S-Log3 test clip) on CPU. Rebrand: remove ArtCraft logos from the fork, new app name, keep NOTICE/licences. Write `docs/FORK.md`.

**C1 — GPU grading (biggest user-visible win).** Port the Lumetri chain (basic, creative, curves, wheels, HSL secondary, vignette) to wgpu following the upstream pattern: new `FxOp` variants + WGSL + **parity tests vs the CPU reference** (`fx_tests.rs` style, max-diff thresholds). Target: 4K grading real-time on the user's RTX 5060 Ti 16 GB.

**C2 — Managed color (ACES).** Add working spaces ACEScg (AP1 linear) and grading space ACEScct; AP0 interchange; input transforms for the existing camera log curves → ACES; output transforms (SDR Rec.709 100 nit, HDR PQ/HLG). Pure Rust, unit-tested against published reference values (AMPAS matrices, round-trip tolerances). Add a "Color Space Transform" effect. Project setting: color management mode (Rec.709 legacy / ACES).

**C3 — Node-based grading.** New effect `gradeGraph` (clip- and timeline-level): nodes = serial, parallel, layer-mixer, with per-node primaries (lift/gamma/gain/offset, **log wheels**, contrast + pivot, saturation, temp/tint), curves, LUT, qualifier + window mask, key output. Stored in `.fcproj` with a **schema migration**. UI: node editor panel (`panels/color_nodes.rs`). Commands for every action (agent-drivable). Executes on GPU (reuse C1 kernels).

**C4 — Qualifiers and windows.** HSL + 3D keyer with matte tools (denoise, blur, shrink/grow, in/out ratio), matte preview; power windows: circle, linear, polygon, Bezier, feather/softness, invert; window animation by keyframes (tracking comes later via tracker work).

**C5 — Scopes.** GPU-computed scopes at full resolution; add CIE chromaticity, false color, HDR nit scale, split/compare overlay, reference-still wipe.

**C6 — Grade management.** Stills gallery (save/recall grades), copy/paste/ripple grade, versions per clip, shot-match assist (extend existing `color_match.rs`), grade presets as JSON (original looks only — no third-party LUTs).

**C7 — Look tools.** Film grain, halation, bloom, gate weave, print-film-style tone curves implemented procedurally (own parameters, original), tone-mapping modules.

**C8 — Ultra chroma key.** Upgrade the keyer: key in scene-linear (S-Log3 aware), multi-pass core/edge/detail mattes, multiple despill algorithms, edge decontamination, light wrap, garbage/hold-out mattes, clean-plate assist. Golden-image tests with synthetic green-screen fixtures.

**C9 — OpenFX host (later).** Closes the "no plugins" gap; third-party OFX plugins load if vendors allow unknown hosts. Needs an ADR because FFI conflicts with the pure-Rust/`unsafe` rules (`docs/adr/0001-platform-ffi.md` is the precedent: confine to one crate).

## 5. Definition of done for every task
`cargo fmt --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` · `cargo xtask layers` · `cargo xtask assets` · `cargo xtask wasm` (or `cargo xtask ci`) all green; tests added (GPU/CPU parity for shaders, round-trip for transforms, schema migration test for project changes); docs updated (`docs/`, crate README); no new `unwrap/expect/panic/unsafe`; commit message `C1.3: short description`.

---

# ANTIGRAVITY — Task C0 (paste this first)

You are working on a fork of **FilmCraft** (`https://github.com/storytold/filmcraft`), an open-source pure-Rust video editor (MIT OR Apache-2.0). Our goal is to add best-in-class color grading (see `FILMCRAFT_COLOR_UPGRADE_PLAN.md` in the repo root once I add it; read it fully).

Before writing any code, read in this order: `CLAUDE.md`, `AGENTS.md`, `CONTRIBUTING.md`, `docs/contributing.md`, `docs/architecture.md`, `docs/testing.md`, `crates/color/README.md`, `crates/scopes/README.md`. Follow every rule in them (never-crash, clean-room, layering, one task id per commit).

**Task C0 — fork, build, baseline.**
1. Verify the Windows toolchain (Rust stable via rustup, MSVC Build Tools). Build and run: `cargo run --release -p filmcraft`. If anything fails, fix the environment, not the repo, and write what you did to `docs/FORK.md`.
2. Run `cargo xtask ci` and record which gates pass on Windows. Do not fix unrelated upstream failures; list them in `docs/FORK.md`.
3. Rebranding for the fork: remove `docs/brand/` ArtCraft logos and references from the README/app as required by `docs/brand/LICENSE-brand.txt`; set a new app name (placeholder `FilmCraft-X`, I will rename later); keep `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `ATTRIBUTION.md` intact.
4. Add a benchmark (example or test under `crates/render` or `crates/ui-egui/examples/bench`, following existing bench conventions) that grades a synthetic 3840×2160 f32 frame with the full Lumetri chain on CPU and prints ms/frame. Save results in `docs/FORK.md`.
5. Keep `upstream` as a git remote pointing at storytold/filmcraft and document how to merge upstream changes.

Definition of done: the app launches, CI gates status documented, benchmark committed, `docs/FORK.md` written. Do not start C1. Final message: summary, numbers, blockers.

---

# ANTIGRAVITY — Task C1 (after C0 is accepted)

**Task C1 — GPU Lumetri.** Today color grading runs on the CPU (`crates/render/src/effects.rs`: `fn lumetri`, `fn lumetri_advanced`). Move the whole chain to the GPU using the upstream recipe in `docs/contributing.md` §6 "A video effect" (GPU path): new `FxOp` variants in `crates/render/src/gpufx.rs`, passes in `crates/gpu/src/fx.rs`, math in `crates/gpu/src/fx.wgsl`, parity tests in `crates/gpu/src/fx_tests.rs` comparing against the CPU reference. Order of work: (a) basic correction, (b) wheels, (c) RGB + hue/sat curves via 1D/2D LUT textures, (d) creative looks, (e) HSL secondary with matte, (f) vignette. Keep the CPU path as the reference and fallback. Acceptance: every parity test within a documented tolerance; 4K grade benchmark from C0 improves by at least 10× on GPU; HDR (PQ/HLG) behaviour unchanged (existing `lumetri_hdr_tests.rs` still pass). One commit per sub-step (`C1.1`…`C1.6`).

# FilmCraft-X Fork Documentation

This document records the setup, environment, quality gate results, baseline benchmarks, rebranding changes, and maintenance procedures for the **FilmCraft-X** fork of [FilmCraft](https://github.com/storytold/filmcraft).

---

## 1. Environment Details

- **Operating System:** Windows 11 (x86_64-pc-windows-msvc)
- **Compiler / Toolchain:** Rust `1.99.1` (stable channel via `rustup`)
- **Installed rustup Targets:**
  - `x86_64-pc-windows-msvc` (default host)
  - `wasm32-unknown-unknown` (for `cargo xtask wasm` and web runner)
- **Rust Components:** `rustc`, `cargo`, `rustfmt`, `clippy`
- **C/C++ Build Tools:** Visual Studio Build Tools 2026 (MSVC v14.x / 18.x with "Desktop development with C++")
- **Version Control:** `git version 2.56.0.windows.3`
- **Primary GPU:** NVIDIA GeForce RTX 5060 Ti (16 GB VRAM, Vulkan driver 616.92 / DirectX 12)
- **Secondary GPU:** Intel(R) UHD Graphics (integrated, driver 32.0.101.8860)

---

## 2. Quality Gates & Windows CI Status

### 2.1 Quality Gates Status

| Gate | Command | Status | Notes |
|---|---|---|---|
| **Format** | `cargo fmt --check` | **PASS** | Clean across entire workspace |
| **Linter** | `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** | 0 errors, 0 warnings |
| **Layering** | `cargo xtask layers` | **PASS** | Architecture rules respected (L0–L6) |
| **Assets** | `cargo xtask assets` | **PASS** | All 69 assets valid & attributed in `ATTRIBUTION.md` |
| **WASM** | `cargo xtask wasm` | **PASS** | Checked 49 L0–L4 and web crates |
| **Doc Tests** | `cargo test --workspace --doc` | **PASS** | All documentation tests pass |
| **Unit / Integration Tests** | `cargo test --workspace --exclude filmcraft-automation --lib --bins --tests --no-fail-fast` | **3 Failures** | Documented below |

### 2.2 Test Failures and Hanging Tests on Windows

1. **`filmcraft-automation` bridge tests hang:**
   - Tests: `bridge::tests::a_slow_command_does_not_block_later_calls` and `an_abandoned_call_leaves_no_late_reply_behind`.
   - Behavior: Hangs indefinitely (>30 minutes) on Windows. Must be excluded during automated test runs via `--exclude filmcraft-automation`.
2. **`filmcraft` control server TCP socket permissions:**
   - Tests: 9 tests in `control_server::tests` (`active_connection_can_be_reused`, `authenticated_tcp_request_reaches_the_ui_queue`, etc.).
   - Error: `Os { code: 10013, kind: PermissionDenied, message: "An attempt was made to access a socket in a way forbidden by its access permissions." }`.
   - Cause: Windows sandbox / network isolation prevents localhost socket binding on unprivileged ports during sandboxed test execution.
3. **`filmcraft-platform` NVENC HEVC HDR test failure:**
   - Test: `tests\nvenc_hevc_hdr_export.rs` (`the_factory_rejects_mismatched_pictures_and_clamps_hostile_floats`).
   - Assertion: `eight.sample_entry().video.as_ref().unwrap().color.is_none()`.
   - Cause: On modern NVIDIA drivers (RTX 50-series / driver 616.92), the NVENC bitstream/sample entry outputs explicit color metadata for 8-bit fallback pictures.
4. **`filmcraft-ui-egui` memory allocation error:**
   - Test: `scripted::eyedropper_picks_a_key_color_on_a_selected_clip`.
   - Error: `memory allocation of 4665600 bytes failed`, process exited with code `0xc0000409` (STATUS_STACK_BUFFER_OVERRUN).

---

## 3. Baseline Performance Benchmark (Task C0)

A dedicated benchmark was added in `crates/render/examples/lumetri_bench.rs` following the existing benchmark conventions in `filmcraft-render`.

- **Scenario:** Full Lumetri grading chain on CPU over a synthetic 4K (`3840×2160`) 32-bit float (`Rgba32Float`) image.
- **Active Lumetri Sections:**
  - Basic Correction (Temperature, Tint, Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Saturation)
  - Creative (Look: Teal & Orange, Look Intensity, Faded Film, Sharpen, Vibrance, Creative Saturation)
  - Curves (Luma S-curve, RGB per-channel curves, Hue vs Saturation curve)
  - Color Wheels (Shadows, Midtones, Highlights with offset and lightness adjustments)
  - HSL Secondary (Key qualifier, saturation, temperature shift)
  - Vignette (Amount, Midpoint, Roundness, Feather)
- **Command:** `cargo run --release -p filmcraft-render --example lumetri_bench`
- **Execution Details:**
  - Rayon worker threads: 20
  - Warm-up frames: 3
  - Measurement frames: 10

### Results:
| Metric | CPU Performance (3840×2160) |
|---|---|
| **Median** | **284.23 ms / frame** (~3.52 fps) |
| **Mean** | 287.64 ms / frame |
| **Min** | 279.01 ms / frame |
| **Max** | 302.64 ms / frame |

*Note for Task C1 (GPU Lumetri): The target is to reduce 4K grading time by at least 10× (<28.4 ms/frame, real-time ≥35 fps) by executing the Lumetri passes on the GPU.*

---

## 4. Rebranding Summary

As required by `docs/brand/LICENSE-brand.txt`:
1. **Removed ArtCraft Brand Marks & Logos:**
   - Disabled texture loading and painting of ArtCraft wordmarks and marks in `crates/ui-egui/src/brand.rs` (functions return `None`).
   - Removed the ArtCraft wordmark from the About dialog (`crates/ui-egui/src/panels/dialogs.rs`) and Home / Import screen.
   - Removed the top logo `<picture>`, ArtCraft Discord banners, and ArtCraft Crafting Apps sections from `README.md`.
2. **App Name Rebranded to FilmCraft-X:**
   - Set app window title and dialog title to `FilmCraft-X` in `apps/filmcraft/src/main.rs`.
   - Set About dialog heading to `FilmCraft-X` in `crates/ui-egui/src/panels/dialogs.rs`.
   - Updated `README.md` title to `# FilmCraft-X`.
3. **Preserved Legal Notices:**
   - `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, and `ATTRIBUTION.md` remain completely intact.
   - Preserved plain-text attribution stating that FilmCraft-X is a fork of FilmCraft by the ArtCraft team.

---

## 5. Upstream Git Remote & Merge Workflow

The repository is configured with two git remotes:
- `origin`: `https://github.com/mohtadeayman3-crypto/filmcraft.git` (our fork)
- `upstream`: `https://github.com/storytold/filmcraft.git` (upstream repository)

### How to Merge Upstream Changes:
```powershell
# 1. Fetch latest commits from upstream
git fetch upstream

# 2. Ensure your local main is clean and checkout main
git checkout main

# 3. Merge upstream/main into local main
git merge upstream/main

# 4. If any merge conflicts arise in rebranded files or new features:
#    - Verify brand logos are not reintroduced into the UI
#    - Run quality gates to verify clean state
cargo fmt --check
cargo xtask layers
cargo xtask assets
cargo xtask wasm
cargo test --workspace --exclude filmcraft-automation --lib --bins --tests --no-fail-fast

# 5. Commit the merge
git commit -m "Merge upstream changes into main"
```

# Task C1.8 — Real-Footage CPU vs GPU A/B Check (and C1.9 Acceptance Test)

> **Privacy Note:** All rendered PNG images (`cpu.png`, `gpu.png`, `diff_20x.png`), logs, and the temporary project file (`ab_test.fcproj`) are stored outside the repository in `D:\FilmCraftX-ab\`. No video clips or images are stored in the repository.

## 1. Candidate Footage Search & Selection

A read-only scan of video files ($\ge 20\text{ MB}$) across `C:\Users\Mohtady\Downloads`, `Videos`, `Desktop`, and `D:\` (excluding `D:\FilmCraftX`, `node_modules`, and `target`) identified 105 files, probed with `filmcraft-cli probe`. The top 10 candidates are:

| # | File | Size (MB) | Duration (s) | Format / Resolution / FPS | Last Modified | Notes |
|---:|---|---:|---:|---|---|---|
| **1** | **`C:\Users\Mohtady\Downloads\C1102.MP4`** | **3292.1 MB** | **279.8 s** | **3840×2160 @ 25 fps, H.264 High 4:2:0 Full-Range** | **2023-04-24 04:27** | **Selected:** Genuine **Sony ZV-E10** camera file (`NonRealTimeMeta` XML: `CaptureGammaEquation="s-log3-cine"`, `CaptureColorPrimaries="s-gamut3-cine"`). |
| 2 | `C:\Users\Mohtady\Downloads\إعلان أكاديمية الفون التابع لوزارة الثقافة-004.mp4` | 2708.0 MB | 218.7 s | 3840×2160 @ 30 fps, H.264 High Bt709 | 2026-09-28 16:02 | Graded Rec. 709 master |
| 3 | `D:\protfolio\إعلان مبادرة استاند باي.mp4` | 909.6 MB | 74.4 s | 3840×2160 @ 30 fps, H.264 High Bt709 | 2026-09-28 18:00 | Graded Rec. 709 master |
| 4 | `D:\protfolio\إعلان أكاديمية ماسبيرو التابعة للتلفزيون المصري.mp4` | 847.1 MB | 71.1 s | 3840×2160 @ 30 fps, H.264 High Bt709 | 2026-09-28 18:00 | Graded Rec. 709 master |
| 5 | `C:\Users\Mohtady\Downloads\protfolio-20260928T115122Z-1-003\protfolio\برومو برنامج الاعلامية سلمي معروف علي قناة الشمس.mp4` | 365.0 MB | 43.8 s | 3840×2160 @ 30 fps, H.264 High Bt709 | 2026-09-28 15:53 | Graded Rec. 709 master |
| 6 | `C:\Users\Mohtady\Downloads\0927 (1)(1)-upscaled.mp4` | 160.0 MB | 222.6 s | 3840×2160 @ 30 fps, H.264 High sRGB | 2026-09-29 05:48 | Upscaled sRGB |
| 7 | `C:\Users\Mohtady\Downloads\0925 (1).mp4` | 22.8 MB | 4.9 s | 4096×1746 @ 30 fps, H.264 High Bt709 | 2026-09-25 15:35 | Short DCI-4K clip |
| 8 | `C:\Users\Mohtady\Downloads\إعلان وزارة السياحة المصرية  مشروع (طلبة بكالريوس جامعة القاهرة) .mp4` | 723.7 MB | 101.1 s | 2560×1440 @ 30 fps, H.264 High Bt709 | 2026-09-28 16:01 | 1440p Rec. 709 |
| 9 | `C:\Users\Mohtady\Downloads\1007.mp4` | 798.2 MB | 133.6 s | 1920×1080 @ 50 fps, H.264 Main Bt709 | 2026-10-08 00:54 | 1080p50 Rec. 709 |
| 10 | `C:\Users\Mohtady\Downloads\0927 (1)(8).mp4` | 1223.3 MB | 262.3 s | 1920×1080 @ 50 fps, H.264 Main Bt709 | 2026-09-30 04:48 | 1080p50 Rec. 709 |

---

## 2. Test Configuration (`C1102.MP4`, Frame 3500 at `t = 140.00 s`)

- **Source Clip:** `C:\Users\Mohtady\Downloads\C1102.MP4` (`3840×2160`, 25.0 fps, `279.8 s` / `6995` frames)
- **Interpret Footage Colour Space:** `slog3-sgamut3cine` (`Sony S-Log3 / S-Gamut3.Cine`)
- **Sequence Settings:** `3840×2160` @ `25.0 fps`, working space `Rec. 709 (Linear)`
- **Selected Frame:** Frame `3500` (`t = 140.00 s`, exact midpoint of the `279.8 s` clip)
- **Heavy 6-Section Lumetri Grade Applied (`clip = 9`, `effect = "lumetri"`):**
  1. **Basic Correction (`basic_on = true`):** `temperature = 14.0`, `tint = -8.0`, `exposure = +0.45`, `contrast = 22.0`, `highlights = -18.0`, `shadows = 16.0`, `whites = 8.0`, `blacks = -6.0`, `saturation = 108.0`
  2. **Creative (`creative_on = true`):** `look = 1` (Teal & Orange), `look_intensity = 75.0`, `faded_film = 8.0`, `sharpen = 20.0`, `vibrance = 15.0`, `creative_sat = 105.0`, `shadow_tint = "#788090"`, `highlight_tint = "#8c8274"`
  3. **Curves (`curves_on = true`):**
     - `curve_luma`: `[[0.0, 0.02], [0.25, 0.20], [0.75, 0.82], [1.0, 0.98]]`
     - `curve_red`: `[[0.0, 0.0], [0.5, 0.52], [1.0, 1.0]]`
     - `curve_blue`: `[[0.0, 0.02], [0.5, 0.48], [1.0, 0.98]]`
     - `hue_vs_sat`: `[[0.0, 0.52], [0.08, 0.58], [0.33, 0.45], [0.66, 0.54], [1.0, 0.52]]`
  4. **Color Wheels (`wheels_on = true`):** `wheel_shadows = [-0.08, 0.06]`, `wheel_shadows_l = -5.0`, `wheel_midtones = [0.06, 0.03]`, `wheel_midtones_l = 4.0`, `wheel_highlights = [0.08, -0.04]`, `wheel_highlights_l = 6.0`
  5. **HSL Secondary (`hsl_on = true`, skin-tone range):** `hsl_hue = 22.0`, `hsl_hue_range = 28.0`, `hsl_sat_min = 12.0`, `hsl_luma_min = 15.0`, `hsl_luma_max = 85.0`, `hsl_soft = 20.0`, `hsl_temp = 12.0`, `hsl_tint = -4.0`, `hsl_sat = 112.0`
  6. **Vignette (`vignette_on = true`):** `vignette_amount = -1.5`, `vignette_midpoint = 45.0`, `vignette_roundness = 20.0`, `vignette_feather = 60.0`

---

## 3. CPU vs GPU Render Comparison Results (`3840×2160`, `8,294,400` Pixels)

Rendered headlessly via `filmcraft-cli` (`file.exportMedia` with `format: "png"`, `startFrame: 3500`, `endFrame: 3501`, `gpuRendering: "off"` vs `gpuRendering: "auto"`):
- **External Output Files (in `D:\FilmCraftX-ab\`):**
  - `D:\FilmCraftX-ab\cpu.png` (`11,483,599 bytes`)
  - `D:\FilmCraftX-ab\gpu.png` (`11,484,803 bytes`)
  - `D:\FilmCraftX-ab\diff_20x.png` (20× amplified absolute RGB difference)

| Metric | Measured Value |
|---|---|
| **Resolution** | `3840×2160` (`8,294,400` pixels, `24,883,200` RGB channels) |
| **Max 8-bit Channel Difference** | **`1 / 255`** (`0.003922` normalized) |
| **Mean 8-bit Channel Difference** | **`0.016911 / 255`** (`0.00006632` = `6.63e-5` normalized) |
| **Mean Per-Pixel Max Channel Difference** | **`0.049819 / 255`** |
| **99th Percentile (`p99`) Per-Pixel Max Difference** | **`1 / 255`** |
| **Pixels with `diff == 0` (Bitwise Identical)** | **`7,881,184` px (`95.0181%`)** |
| **Pixels with `diff == 1` (±1 LSB Rounding)** | **`413,216` px (`4.9819%`)** |
| **Pixels with `diff >= 2`** | **`0` px (`0.0000%`)** |
| **Visually Identical?** | **Yes — 100% visually indistinguishable (`max = 1/255`, `95.02%` exact).** |

---

## 4. Live Desktop App Playback Verification (`filmcraft.exe`)

Launched `target\release\filmcraft.exe --control 47832 --no-recover --data-dir D:\FilmCraftX-ab\data D:\FilmCraftX-ab\ab_test.fcproj` and drove playback over the control channel (`playhead.set`, `ui.playback {"action": "play"}`, `perf.stats`):

### GPU Path Confirmation (Logs & `perf.stats`)
- **Application Log (`D:\FilmCraftX-ab\data\Logs\filmcraft.log`):**
  - `filmcraft_gpu::adapter: graphics adapter chosen: "NVIDIA GeForce RTX 5060 Ti" (Vulkan, discrete GPU, driver NVIDIA 616.92, 69 features, bus 0000:02:00.0)`
  - `filmcraft_platform::media_foundation: hardware decoding: Microsoft H264 Video Decoder MFT on NVIDIA GeForce RTX 5060 Ti`
- **`perf.stats` Counters:**
  - `frames.cache`: `plans = 86` (`planMB = 1069.98 MB`), `images = 3` (`imageMB = 0.29 MB`, timeline thumbnails only). Every Program monitor frame is cached and drawn as a GPU `FramePlan::Layers` plan (`LayerFx`) rather than a CPU `FramePlan::Image`.
  - `gpu`: `uploadedBytes = 510,105,600` (Full pass) / `1,082,419,200` (after 1/2 pass), `submitMs = 0.65 ms` (Full) / `0.59 ms` (1/2).

### Measured Real Playback Speed (5-Second Window at `t = 140.0 s`)

| Playback Setting | Clip FPS (`speed = 1.0`) | Frames Shown (5 s) | Frames Dropped (`dropRate`) | UI Repaint FPS (`frameMs`) | Worker `decodeMs` (`p50` / `mean`) | Worker `renderMs` (`p50` / `mean`) | GPU `submitMs` |
|---|---:|---:|---:|---:|---:|---:|---:|
| **`Full` Resolution** | **25.0 fps** | **122** | **0 (`0.0%`)** | **60.17 fps** (`16.62 ms`) | `7.37 ms` / `14.22 ms` | `8.59 ms` / `10.57 ms` | `0.65 ms` |
| **`1/2` Resolution** | **25.0 fps** | **130** | **0 (`0.0%`)** | **60.11 fps** (`16.63 ms`) | `7.09 ms` / `10.33 ms` | `8.93 ms` / `11.10 ms` | `0.59 ms` |

- **Honest breakdown of how real playback works on this clip:**
  1. The 6 background frame workers decode 4K H.264 via Media Foundation hardware decoding on the RTX 5060 Ti (`decodeMs p50 = 7.37 ms`), convert S-Log3 / S-Gamut3.Cine → linear Rec. 709 working space on the CPU (`colorman::decode`), and pre-pack the `f16` upload buffer (`renderMs p50 = 8.59 ms`, `jobMsMean = 24.78 ms` across 6 workers). Note that in the docked workspace layout, the Program monitor scales `RenderOptions.scale` to the monitor viewport size (`decimation = 2` or `4`), whereas full un-decimated `3840×2160` `RgbaF32` playback in `lumetri_bench` runs at `29.92 ms/frame` (`33.4 fps`).
  2. On the UI thread, `GpuCompositor` uploads the prepared `f16` buffer and runs all 10 compute passes of the 6-section Lumetri chain on the GPU (`submitMs = 0.65 ms`), keeping UI repaint locked at **60.17 fps** and playing the 25 fps 4K S-Log3 timeline at full speed with **0 dropped frames (`0.0%`)** and **0 audio underruns**.


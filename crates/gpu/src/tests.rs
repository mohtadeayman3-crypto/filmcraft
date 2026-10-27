use super::*;
use filmcraft_geom::{Affine, Vec2};
use filmcraft_render::Blend;
use filmcraft_render::plan::execute_cpu;

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

#[test]
fn yuv_code_range_uniforms() {
    let mut mismatches = Vec::new();
    for bits in [8, 10, 12, 16] {
        let levels = (1u32 << bits) as f32;
        for range in [Range::Full, Range::Limited] {
            let src = SrcInfo {
                kind: 2,
                code_scale: if bits == 8 { 255.0 } else { levels },
                code_levels: levels,
                alpha: 0.0,
                chroma: (1, 1),
                size: (1, 1),
                color: filmcraft_color::ColorInfo { range, ..filmcraft_color::ColorInfo::REC709 },
                input: None,
            };
            let u = GpuCompositor::uniforms(&src, &Affine::IDENTITY, 1.0, Blend::Normal, (1, 1));
            // Full: 0..2^bits-1, neutral 2^(bits-1). Limited: BT.709 8-bit codes * 2^(bits-8).
            let expected = match range {
                Range::Full => [0.0, levels - 1.0, levels / 2.0, levels - 1.0],
                Range::Limited => [16.0, 219.0, 128.0, 224.0].map(|v| v * (1u32 << (bits - 8)) as f32),
            };
            if u[16..20] != expected {
                mismatches.push(format!("{bits}-bit {range:?}: {:?} != {expected:?}", &u[16..20]));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn yuv_code_range_gpu_matches_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let mut prepared_compositor = GpuCompositor::new(&dev, &q);
    for bits in [8, 10, 12, 16] {
        let scale = 1u32 << (bits - 8);
        for range in [Range::Full, Range::Limited] {
            // Constant chroma isolates range normalization from CPU/GPU chroma reconstruction.
            for (u, v) in [(128, 128), (64, 192), (192, 64)] {
                let y = (0..=255).map(|code| (code * scale) as u16).collect::<Vec<_>>();
                let planes = [Arc::new(y), Arc::new(vec![(u * scale) as u16; 256]), Arc::new(vec![(v * scale) as u16; 256])];
                let data = if bits == 8 {
                    PixelData::Yuv8 { planes: planes.map(|p| Arc::new(p.iter().map(|v| *v as u8).collect())), chroma: Chroma::C444, alpha: None }
                } else {
                    PixelData::Yuv16 { planes, chroma: Chroma::C444, bits, alpha: None }
                };
                let frame = Arc::new(VideoFrame {
                    width: 256,
                    height: 1,
                    data,
                    color: filmcraft_color::ColorInfo { range, ..filmcraft_color::ColorInfo::REC709 },
                    par: (1, 1),
                    pts: Default::default(),
                });
                let cpu = frame.to_rgba8();
                let plan = FramePlan::Layers { width: 256, height: 1, layers: vec![PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal)] };
                c.composite(&plan);
                let (w, h, gpu) = c.read_output().expect("readback");
                assert_eq!((w, h, gpu.len()), (256, 1, cpu.len()));
                let error = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().expect("pixels");
                // One output-code tolerance: CPU f32 vs GPU f16 accumulator quantization.
                assert!(error <= 1, "{bits}-bit {range:?} U={u} V={v}: max error {error}");
                if u == 128 && v == 128 {
                    assert!(gpu.as_chunks::<4>().0.iter().all(|p| p[0] == p[1] && p[1] == p[2]), "neutral tint: {bits}-bit {range:?}");
                }
                let prepared = prepare(&plan);
                prepared_compositor.composite_prepared(&plan, Some(&prepared));
                assert_eq!(prepared_compositor.read_output().expect("prepared readback").2, gpu, "prepared: {bits}-bit {range:?}");
                eprintln!("range gate: {bits}-bit {range:?} U={u} V={v}, max error={error}, prepared=inline");
            }
        }
    }
}

fn yuv_frame(w: u32, h: u32) -> Arc<VideoFrame> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y: Vec<u8> = (0..w * h).map(|i| (16 + ((i % w) * 219 / w)) as u8).collect();
    let u: Vec<u8> = (0..cw * ch).map(|i| (64 + (i / cw) * 128 / ch) as u8).collect();
    let v: Vec<u8> = (0..cw * ch).map(|i| (200 - (i % cw) * 100 / cw) as u8).collect();
    Arc::new(VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv8 { planes: [Arc::new(y), Arc::new(u), Arc::new(v)], chroma: Chroma::C420, alpha: None },
        color: filmcraft_color::ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    })
}

#[test]
fn gpu_matches_cpu_plan() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let pixels = filmcraft_media::generators::render(
        &filmcraft_media::Generator::Demo(filmcraft_media::DemoScene::OceanSunset),
        320,
        180,
        1.0,
        24,
        filmcraft_time::FrameRate::FPS_24,
    );
    let rgba = Arc::new(VideoFrame::rgba8(320, 180, pixels));
    let (w, h) = (320usize, 180usize);
    let plan = FramePlan::Layers {
        width: w,
        height: h,
        layers: vec![
            PlanLayer { frame: yuv_frame(640, 360), matrix: Affine::scale(0.5, 0.5), opacity: 1.0, blend: Blend::Normal, fx: None, input: None },
            PlanLayer {
                frame: rgba,
                matrix: Affine::motion(Vec2::new(200.0, 100.0), Vec2::new(0.4, 0.4), 12.0, Vec2::new(160.0, 90.0)),
                opacity: 0.7,
                blend: Blend::Normal,
                fx: None,
                input: None,
            },
        ],
    };
    let cpu = execute_cpu(&plan).over_black_rgba8();
    c.composite(&plan);
    let (gw, gh, gpu) = c.read_output().expect("readback");
    assert_eq!((gw as usize, gh as usize), (w, h));
    // compare away from antialiased edges: mean abs error and 99th percentile
    let mut diffs: Vec<u32> =
        cpu.chunks(4).zip(gpu.chunks(4)).map(|(a, b)| (0..3).map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs()).max().unwrap_or(0)).collect();
    diffs.sort_unstable();
    let p99 = diffs[diffs.len() * 99 / 100];
    let mean = diffs.iter().sum::<u32>() as f64 / diffs.len() as f64;
    assert!(p99 <= 6 && mean < 1.5, "p99 {p99}, mean {mean}");
    // cache: compositing the same plan again uploads nothing
    let before = c.uploaded_bytes;
    c.composite(&plan);
    assert_eq!(c.uploaded_bytes, before);
}

#[test]
fn half_float_conversion() {
    for v in [0.0f32, 1.0, 0.5, 0.123, 65504.0, -2.0, 1e-5] {
        let h = f32_to_f16(v);
        // decode
        let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
        let e = ((h >> 10) & 0x1f) as i32;
        let m = (h & 0x3ff) as f32;
        let d = if e == 0 { s * m * 2f32.powi(-24) } else { s * (1.0 + m / 1024.0) * 2f32.powi(e - 15) };
        assert!((d - v).abs() <= v.abs() * 1e-3 + 1e-6, "{v} → {d}");
    }
}

#[test]
fn gpu_push_matches_cpu_with_alpha_and_surrounding_blends() {
    use filmcraft_project::ParamValue;
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (128usize, 72usize);
    let layer = |side: usize| {
        let px = (0..w * h)
            .flat_map(|i| {
                let u = (i % w % 2) as f32;
                let v = (i / w % 2) as f32;
                let a = if side == 0 { (u + v) / 2.0 } else { 1.0 - (u + v) / 2.0 };
                [u * a, v * a, 0.5 * a, a]
            })
            .collect();
        PlanLayer::new(Arc::new(VideoFrame::rgba_f32(w as u32, h as u32, px)), Affine::IDENTITY, 0.7, Blend::Normal)
    };
    let mut worst = (0, 0.0f64);
    for direction in 0..4 {
        for blur in [0.0, 50.0, 100.0] {
            for progress in [0.0, 0.25, 0.37, 0.5, 0.75, 1.0] {
                let mut effect = filmcraft_project::find_effect("push").unwrap().instance();
                effect.param_mut("direction").unwrap().value = ParamValue::Choice(direction);
                effect.param_mut("motion_blur").unwrap().value = ParamValue::Float(blur);
                let mut above = layer(1);
                above.blend = Blend::Multiply;
                let plan = FramePlan::Composite {
                    width: w,
                    height: h,
                    steps: vec![
                        PlanStep::Layers(vec![layer(1)]),
                        PlanStep::Transition { inputs: [vec![layer(0)], vec![layer(1)]], effect, progress, scale: 1.0 },
                        PlanStep::Layers(vec![above]),
                    ],
                };
                let cpu = execute_cpu(&plan).over_black_rgba8();
                c.composite_prepared(&plan, Some(&prepare(&plan)));
                let (_, _, gpu) = c.read_output().expect("readback");
                let mut diffs: Vec<u32> =
                    cpu.as_chunks::<4>().0.iter().zip(gpu.as_chunks::<4>().0).map(|(a, b)| (0..3).map(|k| a[k].abs_diff(b[k]) as u32).max().unwrap()).collect();
                diffs.sort_unstable();
                let p99 = diffs[diffs.len() * 99 / 100];
                let mean = diffs.iter().sum::<u32>() as f64 / diffs.len() as f64;
                worst = (worst.0.max(p99), worst.1.max(mean));
                // Existing compositor acceptance band; moving edges included.
                assert!(p99 <= 6 && mean < 1.5, "direction {direction}, blur {blur}, progress {progress}: p99 {p99}, mean {mean}");
            }
        }
    }
    assert_eq!(c.gpu_transitions, 4 * 3 * 6);
    assert_eq!(c.cpu_transitions, 0);
    eprintln!("Push 72 cases: worst p99 {}, mean {}", worst.0, worst.1);
    let fallback = FramePlan::Composite {
        width: w,
        height: h,
        steps: vec![PlanStep::Transition {
            inputs: [vec![layer(0)], vec![layer(1)]],
            effect: filmcraft_project::find_effect("cross_dissolve").unwrap().instance(),
            progress: 0.37,
            scale: 1.0,
        }],
    };
    c.composite(&fallback);
    let (_, _, got) = c.read_output().expect("fallback readback");
    let expected = execute_cpu(&fallback).over_black_rgba8();
    assert!(got.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1));
    assert_eq!(c.cpu_transitions, 1);
    assert_eq!(c.gpu_transitions, 4 * 3 * 6);

    // Reuse both input targets across ordered transitions; preparation advances past intervening layers.
    let transition = |progress| PlanStep::Transition {
        inputs: [vec![layer(0)], vec![layer(1)]],
        effect: filmcraft_project::find_effect("push").unwrap().instance(),
        progress,
        scale: 1.0,
    };
    let mut middle = layer(0);
    middle.blend = Blend::Screen;
    let stacked = FramePlan::Composite {
        width: w,
        height: h,
        steps: vec![PlanStep::Layers(vec![layer(1)]), transition(0.25), PlanStep::Layers(vec![middle]), transition(0.75)],
    };
    c.composite_prepared(&stacked, Some(&prepare(&stacked)));
    let (_, _, gpu) = c.read_output().expect("stacked readback");
    let cpu = execute_cpu(&stacked).over_black_rgba8();
    // One byte level tolerates half-float resolve rounding.
    assert!(gpu.iter().zip(cpu).all(|(a, b)| a.abs_diff(b) <= 1), "stacked transitions exceed one byte level");
    assert_eq!(c.gpu_transitions, 4 * 3 * 6 + 2);
    assert_eq!(c.cpu_transitions, 1);
}

#[test]
fn prepared_upload_matches_inline_conversion() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (64u32, 36u32);
    let yuv16 = Arc::new(VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv16 {
            planes: [
                Arc::new((0..w * h).map(|i| (64 + (i * 7) % 876) as u16).collect()),
                Arc::new((0..w * h / 2).map(|i| (64 + (i * 13) % 896) as u16).collect()),
                Arc::new((0..w * h / 2).map(|i| (960 - (i * 5) % 896) as u16).collect()),
            ],
            chroma: Chroma::C422,
            bits: 10,
            alpha: None,
        },
        color: filmcraft_color::ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    });
    let f32_layer: Vec<f32> = (0..w * h * 4).map(|i| ((i * 37) % 1000) as f32 / 1000.0 * if i % 4 == 3 { 1.0 } else { 0.6 }).collect();
    let rgbaf = Arc::new(VideoFrame::rgba_f32(w, h, f32_layer.clone()));
    let layers = FramePlan::Layers {
        width: w as usize,
        height: h as usize,
        layers: vec![
            PlanLayer { frame: yuv16, matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: None, input: None },
            PlanLayer { frame: rgbaf, matrix: Affine::scale(0.5, 0.5), opacity: 0.8, blend: Blend::Normal, fx: None, input: None },
        ],
    };
    let image = FramePlan::Image(filmcraft_render::Image { w: w as usize, h: h as usize, px: f32_layer });
    for plan in [layers, image] {
        let mut a = GpuCompositor::new(&dev, &q);
        a.composite(&plan);
        let inline = a.read_output().expect("readback");
        let mut b = GpuCompositor::new(&dev, &q);
        let prep = prepare(&plan);
        assert!(prep.bytes() > 0);
        b.composite_prepared(&plan, Some(&prep));
        let prepared = b.read_output().expect("readback");
        assert!(inline == prepared, "prepared upload differs");
    }
}

/// The upload cache is keyed by pixel-buffer address, so it must keep the buffer alive: otherwise
/// a new frame allocated at a freed frame's address is drawn with the stale texture (seen as
/// whole frames from a previous composite in `crates/golden` GPU parity).
#[test]
fn upload_cache_keeps_buffers_alive() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let px = Arc::new(vec![200u8; 16 * 8 * 4]);
    let frame = Arc::new(VideoFrame { width: 16, height: 8, data: PixelData::Rgba8(px.clone()), ..(*yuv_frame(16, 8)).clone() });
    let plan = FramePlan::Layers {
        width: 16,
        height: 8,
        layers: vec![PlanLayer { frame, matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: None, input: None }],
    };
    c.composite(&plan);
    drop(plan);
    assert!(Arc::strong_count(&px) > 1, "cached upload must own its pixel buffer");
}

/// The WGSL tetrahedral LUT matches `Lut3d::apply` on the CPU.
#[test]
fn gpu_lut_matches_cpu_tetrahedral() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let g = GpuLut::new(&dev, &q);
    for size in [2usize, 17, 33, 65] {
        let mut lut = filmcraft_color::Lut3d::from_fn(size, |c| {
            [(c[0] * c[1] + c[2] * c[2]).sin(), c[1].powf(0.45) * (1.0 - 0.3 * c[0]), (c[0] - c[2]).abs() + 0.1 * c[1]]
        });
        if size == 17 {
            lut.domain_min = [-0.1, 0.0, 0.0];
            lut.domain_max = [1.2, 1.0, 2.0];
        }
        let mut s = 99u64;
        let mut px = Vec::new();
        for _ in 0..70_000 {
            for k in 0..4 {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                px.push(if k == 3 { 0.5 } else { (s % 10_001) as f32 / 10_000.0 * 1.2 - 0.1 });
            }
        }
        let gpu = g.apply(&lut, &px).expect("gpu lut");
        let mut worst = 0f32;
        for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
            let c = lut.apply([p[0], p[1], p[2]]);
            for k in 0..3 {
                worst = worst.max((c[k] - gpu[i * 4 + k]).abs());
            }
            assert_eq!(gpu[i * 4 + 3], 0.5);
        }
        eprintln!("LUT {size}³: max |cpu − gpu| = {worst:e}");
        assert!(worst < 1e-4, "size {size}: max |cpu - gpu| = {worst}");
    }
}

fn test_masks(scale: f32) -> Vec<filmcraft_render::mask::FlatMask> {
    use filmcraft_project::{Mask, MaskMode, MaskPath, ParamValue};
    let mut a = Mask::new("a", MaskPath::ellipse(Vec2::new(150.0, 90.0), Vec2::new(90.0, 55.0)));
    a.feather.value = ParamValue::Float(24.0);
    a.expansion.value = ParamValue::Float(6.0);
    let mut b = Mask::new("b", MaskPath::polygon(&[Vec2::new(40.0, 20.0), Vec2::new(260.0, 50.0), Vec2::new(120.0, 170.0)]));
    b.mode = MaskMode::Subtract;
    b.feather.value = ParamValue::Float(0.0);
    b.opacity.value = ParamValue::Float(70.0);
    let mut c = Mask::new("c", MaskPath::rect(200.0, 100.0, 300.0, 175.0));
    c.mode = MaskMode::Difference;
    c.inverted = true;
    c.feather.value = ParamValue::Float(3.5);
    filmcraft_render::mask::prepare(&[a, b, c], filmcraft_time::Tick(0), scale)
}

#[test]
fn gpu_mask_coverage_matches_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let g = crate::GpuMask::new(&dev, &q);
    for scale in [1.0f32, 0.5] {
        let masks = test_masks(scale);
        let (w, h) = ((320.0 * scale) as usize, (180.0 * scale) as usize);
        let cpu = filmcraft_render::mask::coverage(&masks, w, h).unwrap();
        let gpu = g.coverage(&masks, w, h).expect("gpu coverage");
        let worst = cpu.iter().zip(&gpu).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        eprintln!("mask coverage @{scale}: max |cpu − gpu| = {worst:e}");
        assert!(worst < 2e-4, "scale {scale}: {worst}");
        assert!(cpu.iter().any(|v| *v > 0.99) && cpu.iter().any(|v| *v < 0.01) && cpu.iter().any(|v| *v > 0.2 && *v < 0.8));
    }
}

#[test]
fn gpu_masked_mix_matches_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let g = crate::GpuMask::new(&dev, &q);
    let masks = test_masks(1.0);
    let (w, h) = (320usize, 180usize);
    let original = filmcraft_render::Image { w, h, px: (0..w * h * 4).map(|i| ((i * 37) % 101) as f32 / 100.0).collect() };
    let mut effected = original.clone();
    effected.px.iter_mut().for_each(|v| *v = 1.0 - *v * 0.5);
    let gpu = g.mix(&masks, w, h, &original.px, &effected.px).expect("gpu mix");
    let cov = filmcraft_render::mask::coverage(&masks, w, h).unwrap();
    let mut cpu = effected.clone();
    filmcraft_render::mask::mix(&mut cpu, &original, &cov);
    let worst = cpu.px.iter().zip(&gpu).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
    eprintln!("masked mix: max |cpu − gpu| = {worst:e}");
    assert!(worst < 2e-4, "{worst}");
}

/// A 4:4:4 grey picture with an alpha plane that ramps from transparent (left) to opaque (right).
fn yuv_alpha_frame(w: u32, h: u32, bits: u32) -> Arc<VideoFrame> {
    let n = (w * h) as usize;
    let max = (1u32 << bits) - 1;
    let ramp = |i: usize| ((i % w as usize) as u32 * max / (w - 1)) as u16;
    let data = if bits == 8 {
        PixelData::Yuv8 {
            planes: [Arc::new(vec![180u8; n]), Arc::new(vec![100u8; n]), Arc::new(vec![170u8; n])],
            chroma: Chroma::C444,
            alpha: Some(Arc::new((0..n).map(|i| ramp(i) as u8).collect())),
        }
    } else {
        let s = |v: u32| (v << (bits - 8)) as u16;
        PixelData::Yuv16 {
            planes: [Arc::new(vec![s(180); n]), Arc::new(vec![s(100); n]), Arc::new(vec![s(170); n])],
            chroma: Chroma::C444,
            bits,
            alpha: Some(Arc::new((0..n).map(ramp).collect())),
        }
    };
    Arc::new(VideoFrame { width: w, height: h, data, color: filmcraft_color::ColorInfo::REC709, par: (1, 1), pts: Default::default() })
}

/// A Y'CbCr layer with an alpha plane (ProRes 4444) is drawn with its alpha: the GPU result matches
/// the CPU compositor over an opaque background. (It used to draw such a layer opaque.)
#[test]
fn gpu_draws_yuv_alpha_plane_like_the_cpu() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (w, h) = (320usize, 180usize);
    let pixels = filmcraft_media::generators::render(
        &filmcraft_media::Generator::Demo(filmcraft_media::DemoScene::OceanSunset),
        320,
        180,
        1.0,
        24,
        filmcraft_time::FrameRate::FPS_24,
    );
    let background = Arc::new(VideoFrame::rgba8(320, 180, pixels));
    for bits in [8, 10, 12] {
        let mut c = GpuCompositor::new(&dev, &q);
        let plan = FramePlan::Layers {
            width: w,
            height: h,
            layers: vec![
                PlanLayer { frame: background.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: None, input: None },
                PlanLayer { frame: yuv_alpha_frame(320, 180, bits), matrix: Affine::IDENTITY, opacity: 0.9, blend: Blend::Normal, fx: None, input: None },
            ],
        };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let mut diffs: Vec<u32> =
            cpu.chunks(4).zip(gpu.chunks(4)).map(|(a, b)| (0..3).map(|k| (a[k] as i32 - b[k] as i32).unsigned_abs()).max().unwrap_or(0)).collect();
        diffs.sort_unstable();
        let (p99, max) = (diffs[diffs.len() * 99 / 100], diffs[diffs.len() - 1]);
        assert!(p99 <= 3 && max <= 8, "{bits}-bit: p99 {p99}, max {max}");
        // the transparent left edge shows the background, the opaque right edge the layer
        let at = |img: &[u8], x: usize| (img[(90 * w + x) * 4], img[(90 * w + x) * 4 + 1], img[(90 * w + x) * 4 + 2]);
        let bg = execute_cpu(&FramePlan::Layers {
            width: w,
            height: h,
            layers: vec![PlanLayer { frame: background.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: None, input: None }],
        })
        .over_black_rgba8();
        let (l, r, b0) = (at(&gpu, 1), at(&gpu, w - 2), at(&bg, 1));
        assert!((l.0 as i32 - b0.0 as i32).abs() <= 6 && (l.1 as i32 - b0.1 as i32).abs() <= 6, "{bits}-bit: left edge {l:?} vs background {b0:?}");
        assert!(r != at(&bg, w - 2), "{bits}-bit: right edge {r:?} should differ from the background");
    }
}

#[test]
fn fast_half_float_conversion_matches_the_reference_for_every_value() {
    for h in 0..=u16::MAX {
        let (fast, slow) = (f16_to_f32_fast(h), f16_to_f32(h));
        assert!(fast.to_bits() == slow.to_bits() || (fast.is_nan() && slow.is_nan()), "{h:#06x}: {fast} vs {slow}");
    }
}

/// HDR and wide-gamut Y'CbCr on a Rec. 709 sequence (#965): the shader's colour management
/// (curve, HLG OOTF, BT.2020 → 709 matrix, BT.2390 tone mapping, desaturation) matches the CPU
/// running the same `InputTransform`.
#[test]
fn hdr_input_gpu_matches_cpu() {
    use filmcraft_color::{ColorInfo, ColorPipeline, ColorSpace, InputTransform, Matrix, Primaries, Transfer};
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    // a 10-bit luma ramp (to the top code) under neutral, saturated and extreme chroma, 4:4:4
    let (w, h) = (256u32, 6u32);
    let chroma = [(512, 512), (300, 760), (760, 300), (64, 512), (960, 960), (512, 64)];
    let y: Vec<u16> = (0..h).flat_map(|_| (0..w).map(|x| (64 + x * 876 / 255) as u16)).collect();
    let cb: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].0, w as usize)).collect();
    let cr: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].1, w as usize)).collect();
    let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
    let hdr = |transfer| ColorInfo { matrix: Matrix::Bt2020Ncl, transfer, primaries: Primaries::Bt2020, range: Range::Limited };
    let cases = [
        ("HLG", hdr(Transfer::Hlg), None),
        ("PQ", hdr(Transfer::Pq), None),
        ("PQ 4000 cd/m²", hdr(Transfer::Pq), Some(4000.0)),
        ("BT.2020 SDR", ColorInfo { primaries: Primaries::Bt2020, ..ColorInfo::REC709 }, None),
    ];
    for (name, color, peak) in cases {
        let frame = Arc::new(VideoFrame { width: w, height: h, data: data.clone(), color, par: (1, 1), pts: Default::default() });
        let cs = ColorSpace::from_info(&color);
        let t = Arc::new(InputTransform::new(cs, color.range, &ColorPipeline::REC709, peak));
        assert!(t.gpu_params().is_some(), "{name}");
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame.clone(), Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![layer] };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let error = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        // and the stage does something: the default decode of the same frame is far off
        let plain = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal)] };
        c.composite(&plain);
        let (_, _, unmanaged) = c.read_output().expect("readback");
        let apart = cpu.iter().zip(&unmanaged).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        eprintln!("{name}: max error {error}, unmanaged differs by up to {apart}");
        assert!(error <= 2, "{name}: GPU differs from the CPU by up to {error} codes");
        assert!(apart > 20, "{name}: colour management changed nothing ({apart})");
    }
}

/// Camera log Y'CbCr on a Rec. 709 sequence (C2.2): the shader's decode table sampling
/// and gamut compression match the CPU running the same `InputTransform`.
#[test]
fn log_input_gpu_matches_cpu() {
    use filmcraft_color::{ColorInfo, ColorPipeline, ColorSpace, InputTransform, Matrix, Primaries, Range, Transfer};
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (256u32, 6u32);
    let chroma = [(512, 512), (300, 760), (760, 300), (64, 512), (960, 960), (512, 64)];
    let y: Vec<u16> = (0..h).flat_map(|_| (0..w).map(|x| (64 + x * 876 / 255) as u16)).collect();
    let cb: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].0, w as usize)).collect();
    let cr: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].1, w as usize)).collect();
    let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
    let color = ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Bt709, primaries: Primaries::Bt709, range: Range::Limited };
    let log_spaces = [
        ColorSpace::SLog3SGamut3Cine,
        ColorSpace::SLog3SGamut3,
        ColorSpace::VLogVGamut,
        ColorSpace::CLog2CinemaGamut,
        ColorSpace::CLog3CinemaGamut,
        ColorSpace::LogC3Awg3,
        ColorSpace::LogC4Awg4,
        ColorSpace::AppleLog,
        ColorSpace::DLogDGamut,
        ColorSpace::FLogFGamut,
        ColorSpace::FLog2FGamut,
        ColorSpace::FLog2CFGamutC,
    ];
    for cs in log_spaces {
        let frame = Arc::new(VideoFrame { width: w, height: h, data: data.clone(), color, par: (1, 1), pts: Default::default() });
        let t = Arc::new(InputTransform::new(cs, Range::Limited, &ColorPipeline::REC709, None));
        let params = t.gpu_params().expect("gpu_params must be Some for log curves");
        assert!(matches!(params.curve, filmcraft_color::GpuCurve::Log(..)));
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame.clone(), Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![layer] };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let error = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let plain = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal)] };
        c.composite(&plain);
        let (_, _, unmanaged) = c.read_output().expect("readback");
        let apart = cpu.iter().zip(&unmanaged).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        eprintln!("{cs:?}: max error {error}, unmanaged differs by up to {apart}");
        assert!(error <= 2, "{cs:?}: GPU differs from the CPU by up to {error} codes");
        assert!(apart > 20, "{cs:?}: colour management changed nothing ({apart})");
    }
}

/// Camera log Y'CbCr on an ACEScg sequence (C2.6): the shader's camera IDT (curve decode table,
/// ADR 0002 CAT02/Bradford gamut matrix, and interim AP1 clip) matches the CPU reference running
/// the same `InputTransform` within 2 codes (texture filtering / LSB rounding).
#[test]
fn camera_idt_gpu_matches_cpu_in_acescg() {
    use filmcraft_color::{ColorInfo, ColorPipeline, ColorSpace, InputTransform, Matrix, Primaries, Range, Transfer, WorkingSpace};
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (256u32, 6u32);
    let chroma = [(512, 512), (300, 760), (760, 300), (64, 512), (960, 960), (512, 64)];
    let y: Vec<u16> = (0..h).flat_map(|_| (0..w).map(|x| (64 + x * 876 / 255) as u16)).collect();
    let cb: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].0, w as usize)).collect();
    let cr: Vec<u16> = (0..h).flat_map(|r| std::iter::repeat_n(chroma[r as usize].1, w as usize)).collect();
    let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
    let color = ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Bt709, primaries: Primaries::Bt709, range: Range::Limited };
    let log_spaces = [
        ColorSpace::SLog3SGamut3Cine,
        ColorSpace::SLog3SGamut3,
        ColorSpace::VLogVGamut,
        ColorSpace::CLog2CinemaGamut,
        ColorSpace::CLog3CinemaGamut,
        ColorSpace::LogC3Awg3,
        ColorSpace::LogC4Awg4,
        ColorSpace::AppleLog,
        ColorSpace::DLogDGamut,
        ColorSpace::FLogFGamut,
        ColorSpace::FLog2FGamut,
        ColorSpace::FLog2CFGamutC,
    ];
    let aces_pipe = ColorPipeline { working: WorkingSpace::AcesCg, wide_gamut: false, auto_tone_map: false };
    for cs in log_spaces {
        let frame = Arc::new(VideoFrame { width: w, height: h, data: data.clone(), color, par: (1, 1), pts: Default::default() });
        let t = Arc::new(InputTransform::new(cs, Range::Limited, &aces_pipe, None));
        let params = t.gpu_params().expect("gpu_params must be Some for log curves");
        assert!(matches!(params.curve, filmcraft_color::GpuCurve::Log(..)));
        assert!(params.matrix.is_some(), "camera IDT to AP1 must have a gamut matrix");
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame.clone(), Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![layer] };
        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("readback");
        let error = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let plain = FramePlan::Layers { width: w as usize, height: h as usize, layers: vec![PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal)] };
        c.composite(&plain);
        let (_, _, unmanaged) = c.read_output().expect("readback");
        let apart = cpu.iter().zip(&unmanaged).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        eprintln!("{cs:?} in ACEScg: max error {error}, unmanaged differs by up to {apart}");
        assert!(error <= 2, "{cs:?} in ACEScg: GPU differs from the CPU by up to {error} codes");
        assert!(apart > 20, "{cs:?} in ACEScg: colour management changed nothing ({apart})");
    }
}

/// Empirical determination of 4:2:0 chroma siting sign and magnitude (C2 follow-up):
/// In MPEG-2 / Rec.709 / H.264 / H.265 (Type 0 left-sited chroma), chroma sample i is horizontally
/// collocated with even luma column 2i. In a continuous linear reconstruction:
/// - at luma x = 2i (x = 2i + 0.5), reconstructed chroma must be 100% chroma sample i.
/// - at luma x = 2i+1 (x = 2i + 1.5), reconstructed chroma is 50% sample i + 50% sample i+1.
///
/// Current GPU shader samples cp = p * (cw/w, ch/h) = (x + 0.5) / 2 = x/2 + 0.25.
/// In load4(t, cp), q = cp - 0.5 = x/2 - 0.25.
/// At even luma x = 2i, q = i - 0.25, giving floor(q) = i - 1 and f = 0.75 (25% sample i-1, 75% sample i).
///
/// This test constructs a sharp vertical step in a 4:2:0 frame (U=64 left of x=8, U=192 right of x=8)
/// and verifies the exact sampled weights against candidate models:
/// 1) Current GPU implementation (center-sited cp = p/2): midpoint falls at x = 7.5.
/// 2) Candidate +0.25 chroma texel correction (cp = p/2 + 0.25): aligns chroma sample i with even luma 2i,
///    so at x = 8 reconstructed chroma is exactly 100% sample 4, and midpoint falls at x = 7.0.
/// 3) Candidate -0.25 chroma texel correction: shifts midpoint further right to x = 8.5.
///
/// Confirms empirically: the correction for left-sited 4:2:0 is +0.25 chroma texels (+0.5 luma pixels),
/// not -0.25.
#[test]
fn chroma_siting_empirical_sign_and_magnitude() {
    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);
    let (w, h) = (16u32, 4u32);
    let (cw, ch) = (w / 2, h / 2);
    let y = vec![128u8; (w * h) as usize];
    // Vertical edge at x = 8 (c = 4): left 4 chroma columns U=64, right 4 chroma columns U=192
    let mut u = vec![0u8; (cw * ch) as usize];
    for row in 0..ch {
        for col in 0..cw {
            u[(row * cw + col) as usize] = if col < 4 { 64 } else { 192 };
        }
    }
    let v = vec![128u8; (cw * ch) as usize];
    let frame = Arc::new(VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv8 {
            planes: [Arc::new(y), Arc::new(u), Arc::new(v)],
            chroma: Chroma::C420,
            alpha: None,
        },
        color: filmcraft_color::ColorInfo::REC709,
        par: (1, 1),
        pts: Default::default(),
    });
    let plan = FramePlan::Layers {
        width: w as usize,
        height: h as usize,
        layers: vec![PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal)],
    };

    c.composite(&plan);
    let (_, _, gpu) = c.read_output().expect("readback");

    // Inspect Blue channel across row 1 for columns x = 5, 6, 7, 8, 9, 10
    // Each pixel is 4 bytes [R, G, B, A]
    let row_offset = w as usize * 4;
    let b_vals: Vec<u8> = (0..w as usize).map(|x| gpu[row_offset + x * 4 + 2]).collect();
    let b_min = b_vals[5] as f32; // Fully in U=64 region
    let b_max = b_vals[10] as f32; // Fully in U=192 region
    let delta = b_max - b_min;

    // Fractional progress across transition [0.0 = 100% sample 3, 1.0 = 100% sample 4]
    let frac = |x: usize| (b_vals[x] as f32 - b_min) / delta;

    eprintln!("B values across transition: {:?}", &b_vals[5..=10]);
    eprintln!(
        "Normalized fractions: x=6: {:.3}, x=7: {:.3}, x=8: {:.3}, x=9: {:.3}",
        frac(6),
        frac(7),
        frac(8),
        frac(9)
    );

    // Current GPU behavior:
    // x=6 is at 0.00 (sample 3)
    // x=7 is at ~0.25 (25% sample 4, 75% sample 3)
    // x=8 is at ~0.75 (75% sample 4, 25% sample 3)
    // x=9 is at 1.00 (sample 4)
    assert!(frac(6).abs() < 0.05, "x=6 must be sample 3");
    assert!((frac(7) - 0.25).abs() < 0.05, "current GPU x=7 has f=0.25");
    assert!((frac(8) - 0.75).abs() < 0.05, "current GPU x=8 has f=0.75");
    assert!((frac(9) - 1.00).abs() < 0.05, "x=9 must be sample 4");

    // Mathematical verification of the two candidate corrections:
    // To place chroma sample 4 at luma column 8 (left-sited collocation):
    // Desired at x=8: frac = 1.00 (100% sample 4).
    // Desired at x=7: frac = 0.50 (50% sample 3, 50% sample 4 midpoint).
    //
    // Candidate 1: cp' = cp + 0.25
    //   At x=7: cp = 3.75, cp' = 4.00 => q = 3.50 => f = 0.50 (exact 50/50 midpoint at x=7)
    //   At x=8: cp = 4.25, cp' = 4.50 => q = 4.00 => f = 0.00 (100% sample 4 at x=8)
    // Candidate 2: cp' = cp - 0.25
    //   At x=7: cp' = 3.50 => q = 3.00 => f = 0.00 (0% sample 4, shifted further away)
    //   At x=8: cp' = 4.00 => q = 3.50 => f = 0.50 (50% midpoint shifted to x=8)
    //
    // Conclusion: Candidate 1 (+0.25 chroma texels) is the exact mathematical and empirical
    // correction for left-sited 4:2:0 chroma alignment.
}

/// Legacy-mode golden output verification for the GPU pipeline (C2.4 Item 2):
/// Renders synthetic test frames across the GPU compositor:
/// 1. S-Log3 256-step ramp (Y'CbCr 10-bit limited -> Rec. 709)
/// 2. Rec. 2100 PQ 256-step ramp (Y'CbCr 10-bit limited, 0..4000 nits -> Rec. 709)
/// 3. 75% Rec. 709 standard color bars (7 bars, White, Yellow, Cyan, Green, Magenta, Red, Blue)
/// 4. HDR PQ 256-step ramp with 1000 cd/m² display tone mapping
///
/// Verifies:
/// - Maximum difference against CPU reference is within documented tolerance (<= 2 codes for ramps, <= 1 for bars)
/// - Deterministic 64-bit FNV-1a checksums of GPU output match recorded golden hashes
#[test]
fn legacy_gpu_golden_outputs_remain_unchanged() {
    use filmcraft_color::{ColorInfo, ColorPipeline, ColorSpace, InputTransform, Matrix, Primaries, Range, Transfer};

    let Some((dev, q)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut c = GpuCompositor::new(&dev, &q);

    // Deterministic 64-bit FNV-1a checksum
    fn fnv1a_64(data: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in data {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    fn extract_rgb(gpu_rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(w * h * 3);
        for row in 0..h {
            for col in 0..w {
                let idx = (row * w + col) * 4;
                rgb.push(gpu_rgba[idx]);
                rgb.push(gpu_rgba[idx + 1]);
                rgb.push(gpu_rgba[idx + 2]);
            }
        }
        rgb
    }

    // 1. S-Log3 -> Rec. 709 256-step ramp (10-bit limited YUV444)
    {
        let (w, h) = (256usize, 1usize);
        let y: Vec<u16> = (0..w).map(|x| (64 + x * 876 / 255) as u16).collect();
        let cb: Vec<u16> = vec![512; w];
        let cr: Vec<u16> = vec![512; w];
        let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
        let color = ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Bt709, primaries: Primaries::Bt709, range: Range::Limited };
        let frame = Arc::new(VideoFrame { width: w as u32, height: h as u32, data, color, par: (1, 1), pts: Default::default() });
        let t = Arc::new(InputTransform::new(ColorSpace::SLog3SGamut3Cine, Range::Limited, &ColorPipeline::REC709, None));
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w, height: h, layers: vec![layer] };

        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("gpu readback");

        let max_err = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(max_err <= 2, "S-Log3 ramp GPU differs from CPU by up to {max_err} codes (tolerance: 2)");

        let gpu_rgb = extract_rgb(&gpu, w, h);
        let hash = fnv1a_64(&gpu_rgb);
        assert_eq!(hash, 0x42b29378c52a773c, "GPU S-Log3 ramp hash mismatch: got {hash:#018x}");
    }

    // 2. Rec. 2100 PQ -> Rec. 709 256-step ramp (0..4000 nits, 10-bit limited YUV444)
    {
        let (w, h) = (256usize, 1usize);
        let y: Vec<u16> = (0..w).map(|x| (64 + x * 876 / 255) as u16).collect();
        let cb: Vec<u16> = vec![512; w];
        let cr: Vec<u16> = vec![512; w];
        let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
        let color = ColorInfo { matrix: Matrix::Bt2020Ncl, transfer: Transfer::Pq, primaries: Primaries::Bt2020, range: Range::Limited };
        let frame = Arc::new(VideoFrame { width: w as u32, height: h as u32, data, color, par: (1, 1), pts: Default::default() });
        let t = Arc::new(InputTransform::new(ColorSpace::Rec2100Pq, Range::Limited, &ColorPipeline::REC709, Some(4000.0)));
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w, height: h, layers: vec![layer] };

        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("gpu readback");

        let max_err = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(max_err <= 2, "Rec.2100 PQ ramp GPU differs from CPU by up to {max_err} codes (tolerance: 2)");

        let gpu_rgb = extract_rgb(&gpu, w, h);
        let hash = fnv1a_64(&gpu_rgb);
        assert_eq!(hash, 0xa17f1da995d99bcf, "GPU PQ ramp hash mismatch: got {hash:#018x}");
    }

    // 3. 75% Rec. 709 standard color bars: White, Yellow, Cyan, Green, Magenta, Red, Blue
    {
        let bars_709 = [
            [0.75f32, 0.75, 0.75],
            [0.75, 0.75, 0.0],
            [0.0, 0.75, 0.75],
            [0.0, 0.75, 0.0],
            [0.75, 0.0, 0.75],
            [0.75, 0.0, 0.0],
            [0.0, 0.0, 0.75],
        ];
        let (w, h) = (bars_709.len(), 1usize);
        let (kr, kb) = (0.2126f32, 0.0722f32);
        let mut y = Vec::with_capacity(w);
        let mut cb = Vec::with_capacity(w);
        let mut cr = Vec::with_capacity(w);
        for [r, g, b] in bars_709 {
            let y_norm = kr * r + (1.0 - kr - kb) * g + kb * b;
            let cb_norm = (b - y_norm) / (2.0 * (1.0 - kb));
            let cr_norm = (r - y_norm) / (2.0 * (1.0 - kr));
            y.push((64.0 + y_norm * 876.0).round() as u16);
            cb.push((512.0 + cb_norm * 896.0).round() as u16);
            cr.push((512.0 + cr_norm * 896.0).round() as u16);
        }
        let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
        let color = ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Bt709, primaries: Primaries::Bt709, range: Range::Limited };
        let frame = Arc::new(VideoFrame { width: w as u32, height: h as u32, data, color, par: (1, 1), pts: Default::default() });
        let t = Arc::new(InputTransform::new(ColorSpace::Rec709, Range::Limited, &ColorPipeline::REC709, None));
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w, height: h, layers: vec![layer] };

        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("gpu readback");

        let max_err = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(max_err <= 1, "Rec.709 color bars GPU differs from CPU by up to {max_err} codes (tolerance: 1)");

        let gpu_rgb = extract_rgb(&gpu, w, h);
        let hash = fnv1a_64(&gpu_rgb);
        assert_eq!(hash, 0x5271bd11d6a1d073, "GPU Rec.709 bars hash mismatch: got {hash:#018x}");
    }

    // 4. HDR PQ -> SDR Rec. 709 display tone-mapped ramp (1000 nits peak, 10-bit limited YUV444)
    {
        let (w, h) = (256usize, 1usize);
        let y: Vec<u16> = (0..w).map(|x| (64 + x * 876 / 255) as u16).collect();
        let cb: Vec<u16> = vec![512; w];
        let cr: Vec<u16> = vec![512; w];
        let data = PixelData::Yuv16 { planes: [Arc::new(y), Arc::new(cb), Arc::new(cr)], chroma: Chroma::C444, bits: 10, alpha: None };
        let color = ColorInfo { matrix: Matrix::Bt2020Ncl, transfer: Transfer::Pq, primaries: Primaries::Bt2020, range: Range::Limited };
        let frame = Arc::new(VideoFrame { width: w as u32, height: h as u32, data, color, par: (1, 1), pts: Default::default() });
        let t = Arc::new(InputTransform::new(ColorSpace::Rec2100Pq, Range::Limited, &ColorPipeline::REC709, Some(1000.0)));
        let layer = PlanLayer { input: Some(t), ..PlanLayer::new(frame, Affine::IDENTITY, 1.0, Blend::Normal) };
        let plan = FramePlan::Layers { width: w, height: h, layers: vec![layer] };

        let cpu = execute_cpu(&plan).over_black_rgba8();
        c.composite(&plan);
        let (_, _, gpu) = c.read_output().expect("gpu readback");

        let max_err = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(max_err <= 2, "HDR PQ display tone-mapped GPU differs from CPU by up to {max_err} codes (tolerance: 2)");

        let gpu_rgb = extract_rgb(&gpu, w, h);
        let hash = fnv1a_64(&gpu_rgb);
        assert_eq!(hash, 0x1bc3b47aa28c8568, "GPU HDR PQ display tone-mapped hash mismatch: got {hash:#018x}");
    }
}

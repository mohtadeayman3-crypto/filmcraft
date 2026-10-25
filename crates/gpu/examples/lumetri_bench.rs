//! Benchmark GPU Lumetri grading on a 4K (3840×2160) frame, plus end-to-end upload/readback,
//! 4K playback FPS, and CPU `Target::Sequence` scopes refresh time.
//!
//!     cargo run --release -p filmcraft-gpu --example lumetri_bench

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use filmcraft_color::{ColorInfo, WorkingSpace};
use filmcraft_frame::{AudioBuffer, Chroma, PixelData, VideoFrame};
use filmcraft_geom::Affine;
use filmcraft_gpu::{GpuCompositor, adapter, prepare};
use filmcraft_media::{FrameRequest, Generator, MediaError, MediaInfo, MediaKind, MediaSource, Result as MediaResult, VideoStreamInfo};
use filmcraft_project::effect::{EffectDef, find_effect};
use filmcraft_project::{EffectInstance, ItemId, ItemKind, Label, MediaClip, MediaRef, ParamValue, Project, SequenceSettings, TrackKind};
use filmcraft_render::effects::FxCtx;
use filmcraft_render::gpufx::FxOp;
use filmcraft_render::plan::{FramePlan, LayerFx, PlanLayer};
use filmcraft_render::{Blend, Image, RenderOptions, SourceMap, render_sequence};
use filmcraft_scopes::{self as sc, Brightness, ParadeType, Params as ScopeParams, Signal, WaveformType};
use filmcraft_time::{FrameRate, Tick, TimeRange};

const W: u32 = 3840;
const H: u32 = 2160;
const WARMUP_FRAMES: usize = 3;
const BENCH_FRAMES: usize = 15;

#[derive(Clone, Copy)]
struct Stats {
    median_ms: f64,
    mean_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

impl Stats {
    fn from_samples(mut times_ms: Vec<f64>) -> Self {
        times_ms.sort_by(|a, b| a.total_cmp(b));
        let median_ms = times_ms.get(times_ms.len() / 2).copied().unwrap_or(0.0);
        let min_ms = times_ms.first().copied().unwrap_or(0.0);
        let max_ms = times_ms.last().copied().unwrap_or(0.0);
        let mean_ms = times_ms.iter().sum::<f64>() / times_ms.len().max(1) as f64;
        Self { median_ms, mean_ms, min_ms, max_ms }
    }

    fn fps(self) -> f64 {
        1000.0 / self.median_ms.max(0.001)
    }

    fn print(self, label: &str) {
        println!(
            "  {:<52} median {:7.2} ms ({:6.1} fps) | mean {:7.2} ms | min {:7.2} ms | max {:7.2} ms",
            label,
            self.median_ms,
            self.fps(),
            self.mean_ms,
            self.min_ms,
            self.max_ms
        );
    }
}

fn create_full_lumetri_instance(with_hsl_refine: bool) -> Result<EffectInstance, Box<dyn std::error::Error>> {
    let def: &EffectDef = find_effect("lumetri").ok_or("lumetri effect definition not found")?;
    let mut inst = def.instance();

    let set_f = |inst: &mut EffectInstance, id: &str, val: f64| {
        if let Some(p) = inst.params.get_mut(id) {
            p.value = ParamValue::Float(val);
        }
    };
    let set_b = |inst: &mut EffectInstance, id: &str, val: bool| {
        if let Some(p) = inst.params.get_mut(id) {
            p.value = ParamValue::Bool(val);
        }
    };
    let set_ch = |inst: &mut EffectInstance, id: &str, val: u32| {
        if let Some(p) = inst.params.get_mut(id) {
            p.value = ParamValue::Choice(val);
        }
    };
    let set_vec2 = |inst: &mut EffectInstance, id: &str, x: f64, y: f64| {
        if let Some(p) = inst.params.get_mut(id) {
            p.value = ParamValue::Vec2(filmcraft_geom::Vec2::new(x, y));
        }
    };
    let set_curve = |inst: &mut EffectInstance, id: &str, pts: &[[f32; 2]]| {
        if let Some(p) = inst.params.get_mut(id) {
            p.value = ParamValue::Curve(pts.to_vec());
        }
    };

    // 1. Basic Correction
    set_b(&mut inst, "basic_on", true);
    set_f(&mut inst, "temperature", 15.0);
    set_f(&mut inst, "tint", -10.0);
    set_f(&mut inst, "exposure", 0.5);
    set_f(&mut inst, "contrast", 20.0);
    set_f(&mut inst, "highlights", -15.0);
    set_f(&mut inst, "shadows", 10.0);
    set_f(&mut inst, "whites", 5.0);
    set_f(&mut inst, "blacks", -5.0);
    set_f(&mut inst, "saturation", 110.0);

    // 2. Creative (Look + Adjustments)
    set_b(&mut inst, "creative_on", true);
    set_ch(&mut inst, "look", 1); // Teal & Orange
    set_f(&mut inst, "look_intensity", 100.0);
    set_f(&mut inst, "faded_film", 10.0);
    set_f(&mut inst, "sharpen", 20.0);
    set_f(&mut inst, "vibrance", 15.0);
    set_f(&mut inst, "creative_sat", 105.0);

    // 3. Curves
    set_b(&mut inst, "curves_on", true);
    set_curve(&mut inst, "curve_luma", &[[0.0, 0.0], [0.25, 0.20], [0.75, 0.80], [1.0, 1.0]]);
    set_curve(&mut inst, "curve_red", &[[0.0, 0.0], [0.5, 0.52], [1.0, 1.0]]);
    set_curve(&mut inst, "curve_green", &[[0.0, 0.0], [0.5, 0.49], [1.0, 1.0]]);
    set_curve(&mut inst, "curve_blue", &[[0.0, 0.0], [0.5, 0.48], [1.0, 1.0]]);
    set_curve(&mut inst, "hue_vs_sat", &[[0.0, 1.0], [0.33, 1.2], [0.66, 0.9], [1.0, 1.0]]);

    // 4. Color Wheels
    set_b(&mut inst, "wheels_on", true);
    set_vec2(&mut inst, "wheel_shadows", -0.05, -0.05);
    set_f(&mut inst, "wheel_shadows_l", -5.0);
    set_vec2(&mut inst, "wheel_midtones", 0.04, 0.02);
    set_f(&mut inst, "wheel_midtones_l", 2.0);
    set_vec2(&mut inst, "wheel_highlights", 0.05, 0.05);
    set_f(&mut inst, "wheel_highlights_l", 5.0);

    // 5. HSL Secondary
    set_b(&mut inst, "hsl_on", true);
    set_f(&mut inst, "hsl_hue", 30.0);
    set_f(&mut inst, "hsl_hue_range", 25.0);
    set_f(&mut inst, "hsl_sat_min", 20.0);
    set_f(&mut inst, "hsl_luma_min", 15.0);
    set_f(&mut inst, "hsl_luma_max", 85.0);
    set_f(&mut inst, "hsl_soft", 20.0);
    if with_hsl_refine {
        set_f(&mut inst, "hsl_denoise", 10.0);
        set_f(&mut inst, "hsl_blur", 10.0);
    }
    set_f(&mut inst, "hsl_sat", 115.0);
    set_f(&mut inst, "hsl_temp", 10.0);

    // 6. Vignette
    set_b(&mut inst, "vignette_on", true);
    set_f(&mut inst, "vignette_amount", -1.5);
    set_f(&mut inst, "vignette_midpoint", 40.0);
    set_f(&mut inst, "vignette_roundness", 20.0);
    set_f(&mut inst, "vignette_feather", 60.0);

    Ok(inst)
}

fn create_synthetic_rgba_f32(w: u32, h: u32) -> Vec<f32> {
    let (wu, hu) = (w as usize, h as usize);
    let mut px = vec![0.0f32; wu * hu * 4];
    for y in 0..hu {
        let row_offset = y * wu * 4;
        let v = y as f32 / hu.max(1) as f32;
        for x in 0..wu {
            let i = row_offset + x * 4;
            let u = x as f32 / wu.max(1) as f32;
            if let Some(slice) = px.get_mut(i..i + 4) {
                slice[0] = u;
                slice[1] = v;
                slice[2] = (u * v).sqrt();
                slice[3] = 1.0;
            }
        }
    }
    px
}

fn create_synthetic_yuv8(w: u32, h: u32, seed: u8) -> VideoFrame {
    let (wu, hu) = (w as usize, h as usize);
    let (cw, ch) = (wu.div_ceil(2), hu.div_ceil(2));
    let mut y_plane = vec![0u8; wu * hu];
    let mut u_plane = vec![0u8; cw * ch];
    let mut v_plane = vec![0u8; cw * ch];
    for y in 0..hu {
        let row = y * wu;
        let luma = (16 + ((y * 219) / hu.max(1)) as u8).wrapping_add(seed);
        if let Some(s) = y_plane.get_mut(row..row + wu) {
            for (x, dst) in s.iter_mut().enumerate() {
                *dst = luma.wrapping_add((x & 31) as u8);
            }
        }
    }
    for y in 0..ch {
        let row = y * cw;
        let cb = (64 + ((y * 128) / ch.max(1)) as u8).wrapping_add(seed);
        for x in 0..cw {
            let cr = (64 + ((x * 128) / cw.max(1)) as u8).wrapping_add(seed);
            if let Some(u_dst) = u_plane.get_mut(row + x) {
                *u_dst = cb;
            }
            if let Some(v_dst) = v_plane.get_mut(row + x) {
                *v_dst = cr;
            }
        }
    }
    VideoFrame {
        width: w,
        height: h,
        data: PixelData::Yuv8 { planes: [Arc::new(y_plane), Arc::new(u_plane), Arc::new(v_plane)], chroma: Chroma::C420, alpha: None },
        color: ColorInfo::REC709,
        par: (1, 1),
        pts: Tick::ZERO,
    }
}

fn wait_gpu(device: &wgpu::Device) -> Result<(), Box<dyn std::error::Error>> {
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(8)) })
        .map_err(|e| -> Box<dyn std::error::Error> { format!("GPU poll failed: {e:?}").into() })?;
    Ok(())
}

struct BenchSource {
    info: MediaInfo,
    frame: Arc<VideoFrame>,
}

impl MediaSource for BenchSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }
    fn video_frame(&self, _req: FrameRequest) -> MediaResult<Arc<VideoFrame>> {
        Ok(self.frame.clone())
    }
    fn audio(&self, _start_sample: i64, _frames: usize, _rate: u32) -> MediaResult<AudioBuffer> {
        Err(MediaError::NoStream("audio"))
    }
}

fn build_4k_graded_project(lumetri: EffectInstance, frame: Arc<VideoFrame>) -> Result<(Project, ItemId, SourceMap), Box<dyn std::error::Error>> {
    let mut project = Project::new("4K Lumetri Bench");
    let info = MediaInfo {
        name: "synthetic_4k.mp4".into(),
        kind: MediaKind::Movie,
        duration: Tick::from_seconds_f64(10.0),
        video: Some(VideoStreamInfo {
            width: W,
            height: H,
            frame_rate: FrameRate::FPS_24,
            par: (1, 1),
            codec: "raw".into(),
            pixel_format: "yuv420p".into(),
            color: ColorInfo::REC709,
            has_alpha: false,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: Vec::new(),
        container: "mp4".into(),
        start_timecode: None,
        file_size: None,
    };

    let media_id = project.add_item(
        "synthetic_4k.mp4",
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::Generator(Generator::ColorMatte { color: [0.5, 0.5, 0.5, 1.0] }),
            info: info.clone(),
            interpret: Default::default(),
            mark_in: None,
            mark_out: None,
            markers: Vec::new(),
            offline: false,
            proxy: None,
            identity: None,
        }),
        None,
    );

    let seq_id = project.new_sequence("4K Sequence", SequenceSettings { width: W, height: H, frame_rate: FrameRate::FPS_24, ..Default::default() }, 1, 0, None);

    let mut ti = project
        .make_track_item(media_id, TrackKind::Video, Tick::ZERO, TimeRange::new(Tick::ZERO, Tick::from_seconds_f64(10.0)), FrameRate::FPS_24)
        .ok_or("failed to create track item")?;
    ti.effects.push(lumetri);
    if let Some(seq) = project.sequence_mut(seq_id)
        && let Some(track) = seq.video_tracks.first_mut()
    {
        track.items.push(ti);
    }

    let mut map = SourceMap::default();
    map.0.insert(media_id, Arc::new(BenchSource { info, frame }));
    Ok((project, seq_id, map))
}

fn paint_four_scopes(sig: &Signal) {
    let p = ScopeParams { rows: 200, ..ScopeParams::default() };
    let gain = Brightness::Normal.gain();
    let wf = sc::waveform(sig, WaveformType::Rgb, &p);
    let wf_layers: Vec<(&sc::Grid, [f32; 3])> = wf.traces.iter().map(|g| (g, [0.85, 0.85, 0.85])).collect();
    let _ = black_box(sc::paint::grids(&wf_layers, sc::paint::waveform_reference(wf.per_column), gain));

    let pr = sc::parade(sig, ParadeType::Rgb, &p);
    for g in &pr.traces {
        let _ = black_box(sc::paint::grids(&[(g, [0.85, 0.85, 0.85])], sc::paint::waveform_reference(pr.per_column), gain));
    }

    let vs = sc::vectorscope_yuv(sig, &p);
    let _ = black_box(sc::paint::vectorscope(&vs, [0.85, 0.85, 0.85], gain, true));

    let hs = sc::histogram(sig, &p);
    let _ = black_box(hs);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== FilmCraft-X Task C1: GPU Lumetri 4K (3840x2160) Benchmark ===");
    println!("Rayon threads: {}", rayon::current_num_threads());

    let backends = wgpu::Backends::PRIMARY;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends, ..wgpu::InstanceDescriptor::new_without_display_handle() });
    let adapters = pollster::block_on(instance.enumerate_adapters(backends));
    let chosen = adapter::pick(&adapters, None, wgpu::PowerPreference::HighPerformance)?;
    println!("GPU Adapter:   {}", adapter::Candidate::of(&chosen, None));

    let (device, queue) = pollster::block_on(chosen.request_device(&wgpu::DeviceDescriptor {
        label: Some("lumetri-bench"),
        required_limits: wgpu::Limits::default().using_resolution(chosen.limits()),
        ..Default::default()
    }))?;
    let mut comp = GpuCompositor::new(&device, &queue);

    let cx = FxCtx {
        t: Tick::ZERO,
        px_scale: 1.0,
        seconds: 0.0,
        timecode: "00:00:00:00",
        clip_name: "synthetic_4k_test",
        project: None,
        env: None,
        frame_rate: 24.0,
        working: WorkingSpace::Rec709,
    };

    let lumetri_std = create_full_lumetri_instance(false)?;
    let mut lumetri_nosharp = lumetri_std.clone();
    if let Some(p) = lumetri_nosharp.params.get_mut("sharpen") {
        p.value = ParamValue::Float(0.0);
    }
    let lumetri_refine = create_full_lumetri_instance(true)?;
    let op_nosharp = FxOp::eval(&lumetri_nosharp, &cx, W as usize, H as usize).ok_or("FxOp::eval failed for no-sharpen Lumetri")?;
    let op_std = FxOp::eval(&lumetri_std, &cx, W as usize, H as usize).ok_or("FxOp::eval failed for standard Lumetri")?;
    let op_refine = FxOp::eval(&lumetri_refine, &cx, W as usize, H as usize).ok_or("FxOp::eval failed for refine Lumetri")?;
    if !op_nosharp.gpu_ok() || !op_std.gpu_ok() || !op_refine.gpu_ok() {
        return Err("Lumetri FxOp unexpectedly fell back to CPU".into());
    }

    let lfx_nosharp = Arc::new(LayerFx { size: (W, H), decimation: 1, ops: vec![op_nosharp] });
    let lfx_std = Arc::new(LayerFx { size: (W, H), decimation: 1, ops: vec![op_std] });
    let lfx_refine = Arc::new(LayerFx { size: (W, H), decimation: 1, ops: vec![op_refine] });

    let base_px = create_synthetic_rgba_f32(W, H);
    let cached_yuv_frame = Arc::new(create_synthetic_yuv8(W, H, 0));

    // =========================================================================
    // (a) GPU Grading Time Only (source pre-uploaded on GPU, no CPU readback)
    // =========================================================================
    println!("\n--- (a) GPU Grading Time Only (3840x2160, source cached in VRAM, no readback) ---");
    let plan_ungraded = FramePlan::Layers {
        width: W as usize,
        height: H as usize,
        layers: vec![PlanLayer { frame: cached_yuv_frame.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: None, input: None }],
    };
    let plan_graded_nosharp = FramePlan::Layers {
        width: W as usize,
        height: H as usize,
        layers: vec![PlanLayer {
            frame: cached_yuv_frame.clone(),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            blend: Blend::Normal,
            fx: Some(lfx_nosharp),
            input: None,
        }],
    };
    let plan_graded_std = FramePlan::Layers {
        width: W as usize,
        height: H as usize,
        layers: vec![PlanLayer {
            frame: cached_yuv_frame.clone(),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            blend: Blend::Normal,
            fx: Some(lfx_std.clone()),
            input: None,
        }],
    };
    let plan_graded_refine = FramePlan::Layers {
        width: W as usize,
        height: H as usize,
        layers: vec![PlanLayer {
            frame: cached_yuv_frame.clone(),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            blend: Blend::Normal,
            fx: Some(lfx_refine.clone()),
            input: None,
        }],
    };

    for _ in 0..WARMUP_FRAMES {
        let _ = black_box(comp.composite(&plan_ungraded));
        wait_gpu(&device)?;
    }
    let mut ungraded_times = Vec::with_capacity(BENCH_FRAMES);
    for _ in 0..BENCH_FRAMES {
        let t0 = Instant::now();
        let _ = black_box(comp.composite(&plan_ungraded));
        wait_gpu(&device)?;
        ungraded_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_ungraded = Stats::from_samples(ungraded_times);
    st_ungraded.print("Ungraded 4K composite + sRGB resolve (baseline)");

    for _ in 0..WARMUP_FRAMES {
        let _ = black_box(comp.composite(&plan_graded_nosharp));
        wait_gpu(&device)?;
    }
    let mut gpu_nosharp_times = Vec::with_capacity(BENCH_FRAMES);
    for _ in 0..BENCH_FRAMES {
        let t0 = Instant::now();
        let _ = black_box(comp.composite(&plan_graded_nosharp));
        wait_gpu(&device)?;
        gpu_nosharp_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_gpu_nosharp = Stats::from_samples(gpu_nosharp_times);
    st_gpu_nosharp.print("GPU Lumetri (6 sections, per-pixel, sharpen=0)");

    for _ in 0..WARMUP_FRAMES {
        let _ = black_box(comp.composite(&plan_graded_std));
        wait_gpu(&device)?;
    }
    let mut gpu_std_times = Vec::with_capacity(BENCH_FRAMES);
    for _ in 0..BENCH_FRAMES {
        let t0 = Instant::now();
        let _ = black_box(comp.composite(&plan_graded_std));
        wait_gpu(&device)?;
        gpu_std_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_gpu_std = Stats::from_samples(gpu_std_times);
    st_gpu_std.print("GPU Lumetri (all 6 sections, sharpen=20, 10 passes)");
    let net_lumetri_ms = (st_gpu_std.median_ms - st_ungraded.median_ms).max(0.0);
    println!("  {:<52} {:7.2} ms ({:6.1} fps)", "Net GPU Lumetri FxStage compute passes only", net_lumetri_ms, 1000.0 / net_lumetri_ms.max(0.001));

    for _ in 0..WARMUP_FRAMES {
        let _ = black_box(comp.composite(&plan_graded_refine));
        wait_gpu(&device)?;
    }
    let mut gpu_refine_times = Vec::with_capacity(BENCH_FRAMES);
    for _ in 0..BENCH_FRAMES {
        let t0 = Instant::now();
        let _ = black_box(comp.composite(&plan_graded_refine));
        wait_gpu(&device)?;
        gpu_refine_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_gpu_refine = Stats::from_samples(gpu_refine_times);
    st_gpu_refine.print("GPU Lumetri (6 sections + HSL Denoise 10 & Blur 10)");

    // =========================================================================
    // (b) End-to-End 4K Frame Time Including Upload and Readback
    // =========================================================================
    println!("\n--- (b) End-to-End 4K Frame Time Including Upload & Readback (uncached frame) ---");
    // Pre-allocate distinct frame buffers so each iteration is a genuine upload cache miss
    // without measuring Vec clone allocation inside the timer.
    let fresh_rgba_frames: Vec<Arc<VideoFrame>> = (0..(WARMUP_FRAMES + BENCH_FRAMES))
        .map(|i| {
            let mut px = base_px.clone();
            if let Some(first) = px.first_mut() {
                *first = (i as f32) * 0.001;
            }
            Arc::new(VideoFrame::rgba_f32(W, H, px))
        })
        .collect();

    for f in fresh_rgba_frames.iter().take(WARMUP_FRAMES) {
        let _ = black_box(comp.effect_image(f, &lfx_std));
    }
    let mut e2e_effect_image_times = Vec::with_capacity(BENCH_FRAMES);
    for f in fresh_rgba_frames.iter().skip(WARMUP_FRAMES) {
        let t0 = Instant::now();
        let res = comp.effect_image(f, &lfx_std).ok_or("effect_image returned None")?;
        black_box(res.2.first());
        e2e_effect_image_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_e2e_fx = Stats::from_samples(e2e_effect_image_times);
    st_e2e_fx.print("effect_image (f32->f16 + upload + GPU + f32 readback)");

    // Also measure the ExportRenderer path (`prepare` + `render_export_prepared`) with breakdown:
    let export_frames: Vec<Arc<VideoFrame>> = (0..(WARMUP_FRAMES + BENCH_FRAMES))
        .map(|i| {
            let mut px = base_px.clone();
            if let Some(first) = px.first_mut() {
                *first = 0.5 + (i as f32) * 0.001;
            }
            Arc::new(VideoFrame::rgba_f32(W, H, px))
        })
        .collect();

    for f in export_frames.iter().take(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let prep = prepare(&plan);
        let _ = black_box(comp.render_export_prepared(&plan, Some(&prep)));
    }
    let mut prep_times = Vec::with_capacity(BENCH_FRAMES);
    let mut upload_gpu_readback_times = Vec::with_capacity(BENCH_FRAMES);
    let mut export_total_times = Vec::with_capacity(BENCH_FRAMES);
    for f in export_frames.iter().skip(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let t0 = Instant::now();
        let prep = prepare(&plan);
        let t_prep = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let res = comp.render_export_prepared(&plan, Some(&prep)).ok_or("render_export_prepared returned None")?;
        black_box(res.2.first());
        let t_gpu_rb = t1.elapsed().as_secs_f64() * 1000.0;
        prep_times.push(t_prep);
        upload_gpu_readback_times.push(t_gpu_rb);
        export_total_times.push(t_prep + t_gpu_rb);
        filmcraft_frame::pool::recycle_f32(res.2);
    }
    let st_prep = Stats::from_samples(prep_times);
    let st_gpu_rb = Stats::from_samples(upload_gpu_readback_times);
    let st_export_tot = Stats::from_samples(export_total_times);
    st_export_tot.print("render_export_prepared total (RgbaF32 -> RgbaF32)");
    st_prep.print("  ├─ CPU prepare (3840x2160 f32 -> f16 conversion)");
    st_gpu_rb.print("  └─ Upload (66 MB) + GPU grade + f16 readback & convert");

    // Also measure ExportRenderer path when input is a decoded 4K YUV8 frame (no CPU f32->f16 prepare):
    let export_yuv_frames: Vec<Arc<VideoFrame>> =
        (0..(WARMUP_FRAMES + BENCH_FRAMES)).map(|i| Arc::new(create_synthetic_yuv8(W, H, (i as u8).wrapping_add(1)))).collect();
    for f in export_yuv_frames.iter().take(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let prep = prepare(&plan);
        if let Some(res) = comp.render_export_prepared(&plan, Some(&prep)) {
            filmcraft_frame::pool::recycle_f32(res.2);
        }
    }
    let mut export_yuv_times = Vec::with_capacity(BENCH_FRAMES);
    for f in export_yuv_frames.iter().skip(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let t0 = Instant::now();
        let prep = prepare(&plan);
        let res = comp.render_export_prepared(&plan, Some(&prep)).ok_or("render_export_prepared (YUV8) returned None")?;
        black_box(res.2.first());
        export_yuv_times.push(t0.elapsed().as_secs_f64() * 1000.0);
        filmcraft_frame::pool::recycle_f32(res.2);
    }
    let st_export_yuv = Stats::from_samples(export_yuv_times);
    st_export_yuv.print("render_export_prepared total (4K YUV8 -> RgbaF32)");

    // =========================================================================
    // (c) Playback FPS of Playing a 4K Clip in the App with the Grade Applied
    // =========================================================================
    println!("\n--- (c) 4K Playback in the App (streaming uncached frames -> GPU output, no readback) ---");
    let playback_yuv_frames: Vec<Arc<VideoFrame>> =
        (0..(WARMUP_FRAMES + BENCH_FRAMES)).map(|i| Arc::new(create_synthetic_yuv8(W, H, (i as u8).wrapping_add(100)))).collect();

    for f in playback_yuv_frames.iter().take(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let prep = prepare(&plan);
        let _ = black_box(comp.composite_prepared(&plan, Some(&prep)));
        wait_gpu(&device)?;
    }
    let mut playback_yuv_times = Vec::with_capacity(BENCH_FRAMES);
    for f in playback_yuv_frames.iter().skip(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let t0 = Instant::now();
        let prep = prepare(&plan);
        let _ = black_box(comp.composite_prepared(&plan, Some(&prep)));
        wait_gpu(&device)?;
        playback_yuv_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_play_yuv = Stats::from_samples(playback_yuv_times);
    st_play_yuv.print("4K YUV 4:2:0 8-bit playback + 6-section Lumetri");

    let playback_yuv_refine_frames: Vec<Arc<VideoFrame>> =
        (0..(WARMUP_FRAMES + BENCH_FRAMES)).map(|i| Arc::new(create_synthetic_yuv8(W, H, (i as u8).wrapping_add(150)))).collect();
    for f in playback_yuv_refine_frames.iter().take(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer {
                frame: f.clone(),
                matrix: Affine::IDENTITY,
                opacity: 1.0,
                blend: Blend::Normal,
                fx: Some(lfx_refine.clone()),
                input: None,
            }],
        };
        let prep = prepare(&plan);
        let _ = black_box(comp.composite_prepared(&plan, Some(&prep)));
        wait_gpu(&device)?;
    }
    let mut playback_yuv_refine_times = Vec::with_capacity(BENCH_FRAMES);
    for f in playback_yuv_refine_frames.iter().skip(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer {
                frame: f.clone(),
                matrix: Affine::IDENTITY,
                opacity: 1.0,
                blend: Blend::Normal,
                fx: Some(lfx_refine.clone()),
                input: None,
            }],
        };
        let t0 = Instant::now();
        let prep = prepare(&plan);
        let _ = black_box(comp.composite_prepared(&plan, Some(&prep)));
        wait_gpu(&device)?;
        playback_yuv_refine_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_play_yuv_refine = Stats::from_samples(playback_yuv_refine_times);
    st_play_yuv_refine.print("4K YUV 4:2:0 8-bit playback + Lumetri + Denoise/Blur");

    // Also measure 4K RgbaF32 playback (worker prepare + UI composite_prepared):
    let playback_rgba_frames: Vec<Arc<VideoFrame>> = (0..(WARMUP_FRAMES + BENCH_FRAMES))
        .map(|i| {
            let mut px = base_px.clone();
            if let Some(first) = px.first_mut() {
                *first = 0.25 + (i as f32) * 0.001;
            }
            Arc::new(VideoFrame::rgba_f32(W, H, px))
        })
        .collect();
    for f in playback_rgba_frames.iter().take(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let prep = prepare(&plan);
        let _ = black_box(comp.composite_prepared(&plan, Some(&prep)));
        wait_gpu(&device)?;
    }
    let mut play_rgba_ui_times = Vec::with_capacity(BENCH_FRAMES);
    let mut play_rgba_total_times = Vec::with_capacity(BENCH_FRAMES);
    for f in playback_rgba_frames.iter().skip(WARMUP_FRAMES) {
        let plan = FramePlan::Layers {
            width: W as usize,
            height: H as usize,
            layers: vec![PlanLayer { frame: f.clone(), matrix: Affine::IDENTITY, opacity: 1.0, blend: Blend::Normal, fx: Some(lfx_std.clone()), input: None }],
        };
        let t0 = Instant::now();
        let prep = prepare(&plan);
        let t_prep = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let _ = black_box(comp.composite_prepared(&plan, Some(&prep)));
        wait_gpu(&device)?;
        let t_ui = t1.elapsed().as_secs_f64() * 1000.0;
        play_rgba_ui_times.push(t_ui);
        play_rgba_total_times.push(t_prep + t_ui);
    }
    let st_play_rgba_tot = Stats::from_samples(play_rgba_total_times);
    let st_play_rgba_ui = Stats::from_samples(play_rgba_ui_times);
    st_play_rgba_tot.print("4K RgbaF32 playback (worker prepare + GPU composite)");
    st_play_rgba_ui.print("  └─ UI thread only (upload 66 MB f16 + GPU grade)");

    // =========================================================================
    // (d) Scopes Refresh Time on a Graded 4K Clip (CPU Target::Sequence path)
    // =========================================================================
    println!("\n--- (d) One Scopes Refresh Time on a Graded 4K Clip (CPU Target::Sequence follow-up) ---");
    let (project, seq_id, provider) = build_4k_graded_project(lumetri_std.clone(), cached_yuv_frame.clone())?;

    // 1. UI Scopes Panel SDR path (`scale = 0.25` -> 960x540 CPU render + `over_black_rgba8` + `Signal::from_rgba8` + 4 scopes)
    let opts_qtr = RenderOptions { scale: 0.25, captions: true, ..Default::default() };
    for _ in 0..WARMUP_FRAMES {
        let img = render_sequence(&project, seq_id, Tick::ZERO, opts_qtr, &provider);
        let rgba = img.over_black_rgba8();
        let sig = Signal::from_rgba8(img.w, img.h, &rgba);
        paint_four_scopes(&sig);
    }
    let mut scopes_render_times = Vec::with_capacity(BENCH_FRAMES);
    let mut scopes_paint_times = Vec::with_capacity(BENCH_FRAMES);
    let mut scopes_total_times = Vec::with_capacity(BENCH_FRAMES);
    for _ in 0..BENCH_FRAMES {
        let t0 = Instant::now();
        let img = render_sequence(&project, seq_id, Tick::ZERO, opts_qtr, &provider);
        let rgba = img.over_black_rgba8();
        let t_ren = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        let sig = Signal::from_rgba8(img.w, img.h, &rgba);
        paint_four_scopes(&sig);
        let t_sc = t1.elapsed().as_secs_f64() * 1000.0;
        scopes_render_times.push(t_ren);
        scopes_paint_times.push(t_sc);
        scopes_total_times.push(t_ren + t_sc);
    }
    let st_sc_tot = Stats::from_samples(scopes_total_times);
    let st_sc_ren = Stats::from_samples(scopes_render_times);
    let st_sc_pnt = Stats::from_samples(scopes_paint_times);
    st_sc_tot.print("UI Scopes refresh total (scale=0.25, 960x540 + 4 scopes)");
    st_sc_ren.print("  ├─ CPU render_sequence (scale=0.25, 960x540 Lumetri)");
    st_sc_pnt.print("  └─ Signal::from_rgba8 + compute & paint 4 scopes");

    // 2. UI Scopes Panel HDR Rec. 2100 path (`scale = 0.125` -> 480x270 CPU working_output render + PQ signal + 4 scopes)
    let opts_hdr = RenderOptions { scale: 0.125, working_output: true, ..Default::default() };
    let mut scopes_hdr_times = Vec::with_capacity(BENCH_FRAMES);
    for _ in 0..BENCH_FRAMES {
        let t0 = Instant::now();
        let img = render_sequence(&project, seq_id, Tick::ZERO, opts_hdr, &provider);
        let sig = Signal::from_rgba_f32_with(img.w, img.h, &img.px, sc::MAX_W, sc::MAX_H, sc::linear_to_pq);
        paint_four_scopes(&sig);
        scopes_hdr_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_sc_hdr = Stats::from_samples(scopes_hdr_times);
    st_sc_hdr.print("UI HDR Scopes refresh (scale=0.125, 480x270 + 4 scopes)");

    // 3. Engine `scopes.read` default (`scale = 0.5` -> 1920x1080 CPU render)
    let opts_half = RenderOptions { scale: 0.5, captions: true, ..Default::default() };
    let mut scopes_half_times = Vec::with_capacity(5);
    for _ in 0..5 {
        let t0 = Instant::now();
        let img = render_sequence(&project, seq_id, Tick::ZERO, opts_half, &provider);
        let rgba = img.over_black_rgba8();
        let sig = Signal::from_rgba8(img.w, img.h, &rgba);
        paint_four_scopes(&sig);
        scopes_half_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_sc_half = Stats::from_samples(scopes_half_times);
    st_sc_half.print("Engine scopes.read default (scale=0.5, 1920x1080 CPU)");

    // 4. CPU 4K full-res reference (5 frames) for direct comparison on the same run
    let base_img = Image { w: W as usize, h: H as usize, px: base_px };
    let mut cpu_times = Vec::with_capacity(5);
    for _ in 0..5 {
        let mut img = base_img.clone();
        let t0 = Instant::now();
        filmcraft_render::effects::apply(&mut img, &lumetri_std, &cx);
        black_box(&img);
        cpu_times.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let st_cpu = Stats::from_samples(cpu_times);
    println!("\n--- Reference Comparison (3840x2160 6-Section Lumetri) ---");
    st_cpu.print("CPU Reference (rayon parallel 3840x2160 f32)");
    println!(
        "  Speedup (GPU grading only vs CPU):                   {:6.1}x ({:.2} ms -> {:.2} ms)",
        st_cpu.median_ms / st_gpu_std.median_ms.max(0.001),
        st_cpu.median_ms,
        st_gpu_std.median_ms
    );
    println!(
        "  Speedup (4K YUV8 playback vs CPU):                   {:6.1}x ({:.2} ms -> {:.2} ms, {:.1} fps)",
        st_cpu.median_ms / st_play_yuv.median_ms.max(0.001),
        st_cpu.median_ms,
        st_play_yuv.median_ms,
        st_play_yuv.fps()
    );
    println!(
        "  Speedup (End-to-end export YUV8->f32 vs CPU):        {:6.1}x ({:.2} ms -> {:.2} ms, {:.1} fps)",
        st_cpu.median_ms / st_export_yuv.median_ms.max(0.001),
        st_cpu.median_ms,
        st_export_yuv.median_ms,
        st_export_yuv.fps()
    );

    Ok(())
}

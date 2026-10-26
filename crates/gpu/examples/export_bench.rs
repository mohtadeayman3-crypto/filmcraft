//! Export benchmark: GPU export vs CPU export on C1102.MP4 Frame 3500.
//!
//! Run with:
//!     cargo run --release -p filmcraft-gpu --example export_bench

use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use filmcraft_color::{ColorPipeline, ColorSpace, InputTransform};
use filmcraft_geom::Affine;
use filmcraft_gpu::{GpuCompositor, prepare};
use filmcraft_media::FrameRequest;
use filmcraft_project::{EffectInstance, ParamValue, find_effect};
use filmcraft_render::effects::FxCtx;
use filmcraft_render::gpufx::FxOp;
use filmcraft_render::plan::{FramePlan, LayerFx, PlanLayer, execute_cpu};
use filmcraft_render::Blend;
use filmcraft_time::Tick;

fn fl(v: f64) -> ParamValue {
    ParamValue::Float(v)
}
fn ch(c: u32) -> ParamValue {
    ParamValue::Choice(c)
}
fn pt(x: f64, y: f64) -> ParamValue {
    ParamValue::Vec2(filmcraft_geom::Vec2::new(x, y))
}

fn create_lumetri_instance() -> EffectInstance {
    let mut e = find_effect("lumetri").expect("lumetri def").instance();
    let params: &[(&str, ParamValue)] = &[
        ("temperature", fl(14.0)),
        ("tint", fl(-8.0)),
        ("exposure", fl(0.45)),
        ("contrast", fl(22.0)),
        ("highlights", fl(-18.0)),
        ("shadows", fl(16.0)),
        ("whites", fl(8.0)),
        ("blacks", fl(-6.0)),
        ("saturation", fl(108.0)),
        ("look", ch(1)),
        ("look_intensity", fl(75.0)),
        ("faded_film", fl(8.0)),
        ("sharpen", fl(20.0)),
        ("vibrance", fl(15.0)),
        ("creative_sat", fl(105.0)),
        ("curve_luma", ParamValue::Curve(vec![[0.0, 0.02], [0.25, 0.20], [0.75, 0.82], [1.0, 0.98]])),
        ("curve_red", ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.52], [1.0, 1.0]])),
        ("curve_blue", ParamValue::Curve(vec![[0.0, 0.02], [0.5, 0.48], [1.0, 0.98]])),
        ("hue_vs_sat", ParamValue::Curve(vec![[0.0, 0.52], [0.08, 0.58], [0.33, 0.45], [0.66, 0.54], [1.0, 0.52]])),
        ("wheel_shadows", pt(-0.08, 0.06)),
        ("wheel_shadows_l", fl(-5.0)),
        ("wheel_midtones", pt(0.06, 0.03)),
        ("wheel_midtones_l", fl(4.0)),
        ("wheel_highlights", pt(0.08, -0.04)),
        ("wheel_highlights_l", fl(6.0)),
        ("hsl_on", ParamValue::Bool(true)),
        ("hsl_hue", fl(22.0)),
        ("hsl_hue_range", fl(28.0)),
        ("hsl_sat_min", fl(12.0)),
        ("hsl_luma_min", fl(15.0)),
        ("hsl_luma_max", fl(85.0)),
        ("hsl_soft", fl(20.0)),
        ("hsl_temp", fl(12.0)),
        ("hsl_tint", fl(-4.0)),
        ("hsl_sat", fl(112.0)),
        ("vignette_amount", fl(-1.5)),
        ("vignette_midpoint", fl(45.0)),
        ("vignette_roundness", fl(20.0)),
        ("vignette_feather", fl(60.0)),
    ];
    for (k, v) in params {
        if let Some(p) = e.params.get_mut(*k) {
            p.value = v.clone();
        }
    }
    e
}

struct TimingStats {
    min_ms: f64,
    mean_ms: f64,
    median_ms: f64,
}

impl TimingStats {
    fn from_samples(mut samples: Vec<f64>) -> Self {
        samples.sort_by(|a, b| a.total_cmp(b));
        let median_ms = samples[samples.len() / 2];
        let min_ms = *samples.first().unwrap_or(&0.0);
        let mean_ms = samples.iter().sum::<f64>() / samples.len().max(1) as f64;
        Self { min_ms, mean_ms, median_ms }
    }
}

fn compare_pixels(cpu_rgba8: &[u8], gpu_rgba8: &[u8], w: u32, h: u32) {
    let total_pixels = (w as u64) * (h as u64);
    let total_channels = total_pixels * 3;

    let mut max_diff = 0u8;
    let mut total_diff = 0u64;
    let mut diff_0 = 0u64;
    let mut diff_1 = 0u64;
    let mut diff_2 = 0u64;
    let mut diff_gt_20 = 0u64;

    let mut pixel_max_diffs = Vec::with_capacity(total_pixels as usize);

    for (c, g) in cpu_rgba8.as_chunks::<4>().0.iter().zip(gpu_rgba8.as_chunks::<4>().0) {
        let dr = c[0].abs_diff(g[0]);
        let dg = c[1].abs_diff(g[1]);
        let db = c[2].abs_diff(g[2]);

        max_diff = max_diff.max(dr).max(dg).max(db);
        total_diff += (dr as u64) + (dg as u64) + (db as u64);

        let p_max = dr.max(dg).max(db);
        pixel_max_diffs.push(p_max);

        match p_max {
            0 => diff_0 += 1,
            1 => diff_1 += 1,
            2 => diff_2 += 1,
            _ => (),
        }
        if p_max > 20 {
            diff_gt_20 += 1;
        }
    }

    pixel_max_diffs.sort_unstable();
    let p99 = pixel_max_diffs[(total_pixels as f64 * 0.99) as usize];
    let mean_chan = total_diff as f64 / total_channels as f64;
    let total_pixel_max: u64 = pixel_max_diffs.iter().map(|&d| d as u64).sum();
    let mean_pixel_max = total_pixel_max as f64 / total_pixels as f64;

    println!("   Pixel Differences:");
    println!("     Max channel difference:     {}/255 ({:.4}%)", max_diff, max_diff as f64 / 255.0 * 100.0);
    println!("     Mean channel difference:    {:.4}/255 ({:.4}%)", mean_chan, mean_chan / 255.0 * 100.0);
    println!("     Mean per-pixel max diff:    {:.4}/255", mean_pixel_max);
    println!("     99th percentile (p99) diff: {}/255", p99);
    println!("     Distribution:");
    println!("       diff == 0 (identical):    {:8} ({:.2}%)", diff_0, diff_0 as f64 / total_pixels as f64 * 100.0);
    println!("       diff <= 1 (rounding/LSB): {:8} ({:.2}%)", diff_0 + diff_1, (diff_0 + diff_1) as f64 / total_pixels as f64 * 100.0);
    println!("       diff <= 2:                {:8} ({:.2}%)", diff_0 + diff_1 + diff_2, (diff_0 + diff_1 + diff_2) as f64 / total_pixels as f64 * 100.0);
    println!("       diff > 20 (chroma edges): {:8} ({:.4}%)", diff_gt_20, diff_gt_20 as f64 / total_pixels as f64 * 100.0);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let default_media = "test_assets/C1102.MP4";
    let media_env = std::env::var("FILMCRAFT_TEST_MEDIA").unwrap_or_else(|_| default_media.to_string());
    let movie_path = Path::new(&media_env);
    if !movie_path.exists() {
        eprintln!("Error: Movie not found at {:?}. Set FILMCRAFT_TEST_MEDIA env var.", movie_path);
        return Ok(());
    }

    println!("Opening media source: {:?}", movie_path);
    let _ = filmcraft_platform::register();
    let file = Arc::new(filmcraft_media::reader::FileReader::open(movie_path)?);
    let src = filmcraft_media::reader::open_reader(
        "C1102.MP4",
        file,
        &filmcraft_codecs::reader_openers(),
        &filmcraft_codecs::openers(),
    )?;

    // Frame 3500 at 25 fps = t = 140.0s
    let t = Tick::from_seconds_f64(140.0);
    println!("Decoding video frame at t = 140.0s...");
    let frame = src.video_frame(FrameRequest { time: t, scale: 1.0 })?;
    let (w, h) = (frame.width, frame.height);
    println!("Decoded 4K frame: {}x{}", w, h);

    // Init GPU device and compositor
    println!("Initializing GPU compositor...");
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("adapter");
    println!("Using GPU: {:?}", adapter.get_info().name);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
    let mut comp = GpuCompositor::new(&device, &queue);

    let cs = ColorSpace::SLog3SGamut3Cine;
    let input_tf = Arc::new(InputTransform::new(cs, frame.color.range, &ColorPipeline::REC709, None));

    // ------------------------------------------------------------------------
    // Part 1: Raw S-Log3 export (No Effects)
    // ------------------------------------------------------------------------
    println!("\n=======================================================");
    println!("1. Raw S-Log3 Export (Decode to Rec.709, No Effects)");
    println!("=======================================================");
    let plan_raw = FramePlan::Layers {
        width: w as usize,
        height: h as usize,
        layers: vec![PlanLayer {
            frame: frame.clone(),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            blend: Blend::Normal,
            fx: None,
            input: Some(input_tf.clone()),
        }],
    };

    // Warmup CPU
    let _ = execute_cpu(&plan_raw).over_black_rgba8();
    // Warmup GPU
    let prep_raw = prepare(&plan_raw);
    let _ = comp.render_export_prepared(&plan_raw, Some(&prep_raw));
    comp.composite(&plan_raw);
    let _ = comp.read_output();

    // Bench CPU (5 iterations)
    let mut cpu_times = Vec::new();
    let mut cpu_raw_rgba8 = Vec::new();
    for i in 0..5 {
        let t0 = Instant::now();
        let img = execute_cpu(&plan_raw).over_black_rgba8();
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        cpu_times.push(elapsed);
        if i == 0 {
            cpu_raw_rgba8 = img;
        }
    }
    let st_cpu_raw = TimingStats::from_samples(cpu_times);

    // Bench GPU (10 iterations)
    let mut gpu_times = Vec::new();
    let mut gpu_raw_rgba8 = Vec::new();
    for i in 0..10 {
        let t0 = Instant::now();
        let prep = prepare(&plan_raw);
        let res = comp.render_export_prepared(&plan_raw, Some(&prep)).expect("export prepared");
        black_box(res.2.first());
        comp.composite(&plan_raw);
        let (_, _, px) = comp.read_output().expect("gpu readback");
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        gpu_times.push(elapsed);
        if i == 0 {
            gpu_raw_rgba8 = px;
        }
    }
    let st_gpu_raw = TimingStats::from_samples(gpu_times);

    println!("   CPU Export: min = {:.1} ms, mean = {:.1} ms, median = {:.1} ms", st_cpu_raw.min_ms, st_cpu_raw.mean_ms, st_cpu_raw.median_ms);
    println!("   GPU Export: min = {:.1} ms, mean = {:.1} ms, median = {:.1} ms", st_gpu_raw.min_ms, st_gpu_raw.mean_ms, st_gpu_raw.median_ms);
    println!("   Speedup:    {:.2}x", st_cpu_raw.median_ms / st_gpu_raw.median_ms);
    compare_pixels(&cpu_raw_rgba8, &gpu_raw_rgba8, w, h);

    // ------------------------------------------------------------------------
    // Part 2: Full 6-Section Lumetri Chain on S-Log3
    // ------------------------------------------------------------------------
    println!("\n=======================================================");
    println!("2. Full 6-Section Lumetri Chain on S-Log3 Export");
    println!("=======================================================");
    let lumetri = create_lumetri_instance();
    let cx = FxCtx {
        t,
        px_scale: 1.0,
        seconds: 140.0,
        timecode: "00:02:20:00",
        clip_name: "C1102.MP4",
        project: None,
        env: None,
        frame_rate: 25.0,
        working: ColorPipeline::REC709.working,
    };
    let ops = vec![FxOp::eval(&lumetri, &cx, w as usize, h as usize).expect("lumetri ops")];
    let fx = Arc::new(LayerFx {
        size: (w, h),
        decimation: 1,
        ops,
    });

    let plan_fx = FramePlan::Layers {
        width: w as usize,
        height: h as usize,
        layers: vec![PlanLayer {
            frame: frame.clone(),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            blend: Blend::Normal,
            fx: Some(fx),
            input: Some(input_tf.clone()),
        }],
    };

    // Warmup CPU
    let _ = execute_cpu(&plan_fx).over_black_rgba8();
    // Warmup GPU
    let prep_fx = prepare(&plan_fx);
    let _ = comp.render_export_prepared(&plan_fx, Some(&prep_fx));
    comp.composite(&plan_fx);
    let _ = comp.read_output();

    // Bench CPU (5 iterations)
    let mut cpu_fx_times = Vec::new();
    let mut cpu_fx_rgba8 = Vec::new();
    for i in 0..5 {
        let t0 = Instant::now();
        let img = execute_cpu(&plan_fx).over_black_rgba8();
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        cpu_fx_times.push(elapsed);
        if i == 0 {
            cpu_fx_rgba8 = img;
        }
    }
    let st_cpu_fx = TimingStats::from_samples(cpu_fx_times);

    // Bench GPU (10 iterations)
    let mut gpu_fx_times = Vec::new();
    let mut gpu_fx_rgba8 = Vec::new();
    for i in 0..10 {
        let t0 = Instant::now();
        let prep = prepare(&plan_fx);
        let res = comp.render_export_prepared(&plan_fx, Some(&prep)).expect("export prepared");
        black_box(res.2.first());
        comp.composite(&plan_fx);
        let (_, _, px) = comp.read_output().expect("gpu readback");
        let elapsed = t0.elapsed().as_secs_f64() * 1000.0;
        gpu_fx_times.push(elapsed);
        if i == 0 {
            gpu_fx_rgba8 = px;
        }
    }
    let st_gpu_fx = TimingStats::from_samples(gpu_fx_times);

    println!("   CPU Export: min = {:.1} ms, mean = {:.1} ms, median = {:.1} ms", st_cpu_fx.min_ms, st_cpu_fx.mean_ms, st_cpu_fx.median_ms);
    println!("   GPU Export: min = {:.1} ms, mean = {:.1} ms, median = {:.1} ms", st_gpu_fx.min_ms, st_gpu_fx.mean_ms, st_gpu_fx.median_ms);
    println!("   Speedup:    {:.2}x", st_cpu_fx.median_ms / st_gpu_fx.median_ms);
    compare_pixels(&cpu_fx_rgba8, &gpu_fx_rgba8, w, h);

    println!("\n=======================================================");
    println!("Summary & Recommendation:");
    println!("  Without effects: GPU export is {:.2}x ({:.1} ms vs {:.1} ms CPU)", st_cpu_raw.median_ms / st_gpu_raw.median_ms, st_gpu_raw.median_ms, st_cpu_raw.median_ms);
    println!("  With Lumetri:    GPU export is {:.2}x ({:.1} ms vs {:.1} ms CPU)", st_cpu_fx.median_ms / st_gpu_fx.median_ms, st_gpu_fx.median_ms, st_cpu_fx.median_ms);
    println!("=======================================================");

    Ok(())
}

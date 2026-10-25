//! Benchmark grading a synthetic 3840×2160 f32 frame with the full Lumetri chain on the CPU.
//!
//!     cargo run --release -p filmcraft-render --example lumetri_bench
//!
//! Measures per-frame grading duration (ms per frame) on the CPU with all Lumetri sections active:
//! Basic Correction, Creative (Look + Adjustments), Curves, Color Wheels, HSL Secondary, and Vignette.

use std::hint::black_box;
use std::time::Instant;

use filmcraft_color::WorkingSpace;
use filmcraft_project::effect::{EffectDef, find_effect};
use filmcraft_project::{EffectInstance, ParamValue};
use filmcraft_render::Image;
use filmcraft_render::effects::{FxCtx, apply};
use filmcraft_time::Tick;

const W: usize = 3840;
const H: usize = 2160;
const WARMUP_FRAMES: usize = 3;
const BENCH_FRAMES: usize = 10;

fn create_full_lumetri_instance() -> Result<EffectInstance, Box<dyn std::error::Error>> {
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

fn create_synthetic_frame(w: usize, h: usize) -> Image {
    let mut px = vec![0.0f32; w * h * 4];
    for y in 0..h {
        let row_offset = y * w * 4;
        let v = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let i = row_offset + x * 4;
            let u = x as f32 / w.max(1) as f32;
            px[i] = u;
            px[i + 1] = v;
            px[i + 2] = (u * v).sqrt();
            px[i + 3] = 1.0;
        }
    }
    Image { w, h, px }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Lumetri CPU 4K (3840x2160) Benchmark ===");
    println!("Rayon threads: {}", rayon::current_num_threads());

    let base_frame = create_synthetic_frame(W, H);
    let lumetri = create_full_lumetri_instance()?;

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

    println!("Warming up ({WARMUP_FRAMES} frames)...");
    for _ in 0..WARMUP_FRAMES {
        let mut img = base_frame.clone();
        apply(&mut img, &lumetri, &cx);
        black_box(&img);
    }

    println!("Benchmarking ({BENCH_FRAMES} frames)...");
    let mut times_ms = Vec::with_capacity(BENCH_FRAMES);
    for i in 0..BENCH_FRAMES {
        let mut img = base_frame.clone();
        let start = Instant::now();
        apply(&mut img, &lumetri, &cx);
        black_box(&img);
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        println!("  Frame {:2}: {:7.2} ms", i + 1, elapsed_ms);
        times_ms.push(elapsed_ms);
    }

    times_ms.sort_by(|a, b| a.total_cmp(b));
    let median = times_ms[times_ms.len() / 2];
    let min = times_ms.first().copied().unwrap_or(0.0);
    let max = times_ms.last().copied().unwrap_or(0.0);
    let mean = times_ms.iter().sum::<f64>() / times_ms.len().max(1) as f64;

    println!("\n-- Summary (3840x2160 CPU Lumetri) --");
    println!("  Median: {:7.2} ms/frame ({:.2} fps)", median, 1000.0 / median.max(0.001));
    println!("  Mean:   {:7.2} ms/frame", mean);
    println!("  Min:    {:7.2} ms/frame", min);
    println!("  Max:    {:7.2} ms/frame", max);

    Ok(())
}

//! Real-footage A/B comparison on C1102.MP4 (Frame 3500):
//! - Legacy mode: Rec.709 working space, S-Log3 input, Lumetri grade, sRGB/Rec.709 display
//! - ACEScg mode: ACEScg working space, S-Log3 camera IDT (CAT02 to AP1 + interim clip),
//!   same Lumetri grade in ACEScct, FilmCraft view output (AP1 -> Rec.709 display)
//!
//! Writes outputs to D:\FilmCraftX-ab:
//! - legacy_mode.png
//! - acescg_mode.png
//! - side_by_side.png
//!
//! Reports:
//! - Mean and max difference across the frame
//! - Hue, saturation and brightness difference on skin tones

use std::path::{Path, PathBuf};
use std::sync::Arc;

use filmcraft_color::{ColorPipeline, ColorSpace, InputTransform, OutputTransform, WorkingSpace};
use filmcraft_geom::Affine;
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

fn find_media() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FILMCRAFT_TEST_MEDIA") {
        let pb = PathBuf::from(p);
        if pb.exists() { return Some(pb); }
    }
    let mut candidates = vec![
        PathBuf::from("test_assets/C1102.MP4"),
        PathBuf::from("D:/FilmCraftX/test_assets/C1102.MP4"),
    ];
    if let Ok(profile) = std::env::var("USERPROFILE") {
        candidates.push(PathBuf::from(profile).join("Downloads").join("C1102.MP4"));
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join("Downloads").join("C1102.MP4"));
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Convert linear light RGB to sRGB encoded u8
#[inline]
fn linear_to_srgb_u8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// RGB to HSL (Hue in 0..360, Saturation in 0..100, Brightness/Luma in 0..100)
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let l = (max + min) / 2.0;

    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / d) % 6.0)
    } else if (max - g).abs() < 1e-6 {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };

    let s = if d < 1e-6 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs())
    };

    (h, s * 100.0, l * 100.0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(media_path) = find_media() else {
        eprintln!("Error: C1102.MP4 not found. Set FILMCRAFT_TEST_MEDIA or place C1102.MP4 in Downloads.");
        return Ok(());
    };
    println!("Found test media: {:?}", media_path);

    let _ = filmcraft_platform::register();
    let file = Arc::new(filmcraft_media::reader::FileReader::open(&media_path)?);
    let src = filmcraft_media::reader::open_reader(
        "C1102.MP4",
        file,
        &filmcraft_codecs::reader_openers(),
        &filmcraft_codecs::openers(),
    )?;

    // Frame 3500 at 25 fps = t = 140.0s
    let t = Tick::from_seconds_f64(140.0);
    println!("Decoding frame 3500 at t = 140.0s...");
    let frame = src.video_frame(FrameRequest { time: t, scale: 1.0 })?;
    let (w, h) = (frame.width as usize, frame.height as usize);
    println!("Frame decoded: {}x{}", w, h);

    let lumetri = create_lumetri_instance();
    let cs = ColorSpace::SLog3SGamut3Cine;

    // =========================================================================
    // 1. Legacy Mode: Rec.709 working, S-Log3 input, Lumetri in linear Rec.709
    // =========================================================================
    println!("\nRendering Legacy Mode (Rec.709 working, Lumetri in linear Rec.709)...");
    let pipe_legacy = ColorPipeline::REC709;
    let tf_legacy = Arc::new(InputTransform::new(cs, frame.color.range, &pipe_legacy, None));
    let cx_legacy = FxCtx {
        t,
        px_scale: 1.0,
        seconds: 140.0,
        timecode: "00:02:20:00",
        clip_name: "C1102.MP4",
        project: None,
        env: None,
        frame_rate: 25.0,
        working: pipe_legacy.working,
    };
    let ops_legacy = vec![FxOp::eval(&lumetri, &cx_legacy, w, h).expect("legacy lumetri op")];
    let plan_legacy = FramePlan::Layers {
        width: w,
        height: h,
        layers: vec![PlanLayer {
            frame: frame.clone(),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            blend: Blend::Normal,
            fx: Some(Arc::new(LayerFx { size: (w as u32, h as u32), decimation: 1, ops: ops_legacy })),
            input: Some(tf_legacy),
        }],
    };
    let legacy_linear = execute_cpu(&plan_legacy);
    let legacy_rgba8 = legacy_linear.over_black_rgba8();

    // =========================================================================
    // 2. ACEScg Mode: ACEScg working, S-Log3 IDT, Lumetri in ACEScct, FilmCraft view
    // =========================================================================
    println!("Rendering ACEScg Mode (ACEScg working, S-Log3 IDT, Lumetri in ACEScct, FilmCraft view)...");
    let pipe_aces = ColorPipeline {
        working: WorkingSpace::AcesCg,
        wide_gamut: false,
        auto_tone_map: false,
    };
    let tf_aces = Arc::new(InputTransform::new(cs, frame.color.range, &pipe_aces, None));
    let cx_aces = FxCtx {
        t,
        px_scale: 1.0,
        seconds: 140.0,
        timecode: "00:02:20:00",
        clip_name: "C1102.MP4",
        project: None,
        env: None,
        frame_rate: 25.0,
        working: pipe_aces.working,
    };
    let ops_aces = vec![FxOp::eval(&lumetri, &cx_aces, w, h).expect("aces lumetri op")];
    let plan_aces = FramePlan::Layers {
        width: w,
        height: h,
        layers: vec![PlanLayer {
            frame: frame.clone(),
            matrix: Affine::IDENTITY,
            opacity: 1.0,
            blend: Blend::Normal,
            fx: Some(Arc::new(LayerFx { size: (w as u32, h as u32), decimation: 1, ops: ops_aces })),
            input: Some(tf_aces),
        }],
    };
    let mut aces_linear = execute_cpu(&plan_aces);

    // FilmCraft view: display transform from ACEScg (AP1 linear) to Rec.709 display
    let disp_tf = OutputTransform::display(&pipe_aces);
    aces_linear.map_rgb(|c, _, _| disp_tf.apply(c));

    // Convert to RGBA8 (sRGB encoded for display viewing)
    let mut acescg_rgba8 = vec![0u8; w * h * 4];
    for (i, px) in aces_linear.px.chunks_exact(4).enumerate() {
        let r = linear_to_srgb_u8(px[0]);
        let g = linear_to_srgb_u8(px[1]);
        let b = linear_to_srgb_u8(px[2]);
        let a = (px[3] * 255.0).round().clamp(0.0, 255.0) as u8;
        acescg_rgba8[i * 4] = r;
        acescg_rgba8[i * 4 + 1] = g;
        acescg_rgba8[i * 4 + 2] = b;
        acescg_rgba8[i * 4 + 3] = a;
    }

    // =========================================================================
    // 3. Side-by-Side Image Construction (Left = Legacy, Right = ACEScg)
    // =========================================================================
    println!("Constructing side-by-side comparison...");
    let split_x = w / 2;
    let mut sbs_rgba8 = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) * 4;
            if x.abs_diff(split_x) <= 1 {
                // White divider line
                sbs_rgba8[idx..idx + 4].copy_from_slice(&[255, 255, 255, 255]);
            } else if x < split_x {
                sbs_rgba8[idx..idx + 4].copy_from_slice(&legacy_rgba8[idx..idx + 4]);
            } else {
                sbs_rgba8[idx..idx + 4].copy_from_slice(&acescg_rgba8[idx..idx + 4]);
            }
        }
    }

    // =========================================================================
    // 4. Save PNGs to D:\FilmCraftX-ab
    // =========================================================================
    let out_dir = Path::new("D:/FilmCraftX-ab");
    if !out_dir.exists() {
        std::fs::create_dir_all(out_dir)?;
    }

    let p_legacy = out_dir.join("legacy_mode.png");
    let p_aces = out_dir.join("acescg_mode.png");
    let p_sbs = out_dir.join("side_by_side.png");

    println!("Saving PNGs to {:?}...", out_dir);
    image::save_buffer(&p_legacy, &legacy_rgba8, w as u32, h as u32, image::ColorType::Rgba8)?;
    println!("Saved: {:?}", p_legacy);
    image::save_buffer(&p_aces, &acescg_rgba8, w as u32, h as u32, image::ColorType::Rgba8)?;
    println!("Saved: {:?}", p_aces);
    image::save_buffer(&p_sbs, &sbs_rgba8, w as u32, h as u32, image::ColorType::Rgba8)?;
    println!("Saved: {:?}", p_sbs);

    // =========================================================================
    // 5. Compute Frame Difference Metrics & Skin-Tone Analysis
    // =========================================================================
    let total_pixels = (w * h) as f64;
    let mut max_diff = 0u8;
    let mut total_channel_diff = 0u64;

    // Skin tone sampling:
    // Typical skin tones in sRGB: R > G > B, Hue around 15°..45°, moderate saturation
    let mut skin_pixels = 0u64;
    let mut leg_skin_h = 0.0f64;
    let mut leg_skin_s = 0.0f64;
    let mut leg_skin_l = 0.0f64;
    let mut ace_skin_h = 0.0f64;
    let mut ace_skin_s = 0.0f64;
    let mut ace_skin_l = 0.0f64;

    for i in 0..(w * h) {
        let idx = i * 4;
        let lr = legacy_rgba8[idx];
        let lg = legacy_rgba8[idx + 1];
        let lb = legacy_rgba8[idx + 2];

        let ar = acescg_rgba8[idx];
        let ag = acescg_rgba8[idx + 1];
        let ab = acescg_rgba8[idx + 2];

        let dr = lr.abs_diff(ar);
        let dg = lg.abs_diff(ag);
        let db = lb.abs_diff(ab);

        max_diff = max_diff.max(dr).max(dg).max(db);
        total_channel_diff += (dr as u64) + (dg as u64) + (db as u64);

        // Check if pixel falls into face/skin range
        let (lh, ls, ll) = rgb_to_hsl(lr as f32 / 255.0, lg as f32 / 255.0, lb as f32 / 255.0);
        let (ah, as_, al) = rgb_to_hsl(ar as f32 / 255.0, ag as f32 / 255.0, ab as f32 / 255.0);

        if lr > lg && lg > lb && (12.0..=45.0).contains(&lh) && (15.0..=75.0).contains(&ls) && (20.0..=85.0).contains(&ll) {
            skin_pixels += 1;
            leg_skin_h += lh as f64;
            leg_skin_s += ls as f64;
            leg_skin_l += ll as f64;
            ace_skin_h += ah as f64;
            ace_skin_s += as_ as f64;
            ace_skin_l += al as f64;
        }
    }

    let mean_channel_diff = total_channel_diff as f64 / (total_pixels * 3.0);
    println!("\n=======================================================");
    println!("A/B Comparison Results (Legacy Mode vs ACEScg Mode)");
    println!("=======================================================");
    println!("Resolution:                    {}x{} ({} pixels)", w, h, total_pixels as u64);
    println!("Max 8-bit channel difference:   {}/255 ({:.4}%)", max_diff, max_diff as f64 / 255.0 * 100.0);
    println!("Mean 8-bit channel difference:  {:.4}/255 ({:.4}%)", mean_channel_diff, mean_channel_diff / 255.0 * 100.0);

    if skin_pixels > 0 {
        let leg_avg_h = leg_skin_h / skin_pixels as f64;
        let leg_avg_s = leg_skin_s / skin_pixels as f64;
        let leg_avg_l = leg_skin_l / skin_pixels as f64;

        let ace_avg_h = ace_skin_h / skin_pixels as f64;
        let ace_avg_s = ace_skin_s / skin_pixels as f64;
        let ace_avg_l = ace_skin_l / skin_pixels as f64;

        let delta_h = ace_avg_h - leg_avg_h;
        let delta_s = ace_avg_s - leg_avg_s;
        let delta_l = ace_avg_l - leg_avg_l;

        println!("\nSkin Tones Analysis (sampled over {} detected skin pixels):", skin_pixels);
        println!("  Legacy Mode:  Hue = {:.2}°, Saturation = {:.2}%, Brightness/Luma = {:.2}%", leg_avg_h, leg_avg_s, leg_avg_l);
        println!("  ACEScg Mode:  Hue = {:.2}°, Saturation = {:.2}%, Brightness/Luma = {:.2}%", ace_avg_h, ace_avg_s, ace_avg_l);
        println!("  Deltas:       ΔHue = {:+.2}°, ΔSat = {:+.2}%, ΔBrightness = {:+.2}%", delta_h, delta_s, delta_l);
    } else {
        println!("\nWarning: No skin tone pixels detected with strict criteria.");
    }

    println!("\nDone! All files generated in D:\\FilmCraftX-ab.");
    Ok(())
}

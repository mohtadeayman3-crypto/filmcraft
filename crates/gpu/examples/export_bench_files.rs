//! Full 10-second sequence export comparison: writes cpu_export.mp4 and gpu_export.mp4
//! to D:\FilmCraftX-ab, comparing render times, speedup, and GPU frame utilization.
//!
//! Run with:
//!     cargo run --release -p filmcraft-gpu --example export_bench_files

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use filmcraft_color::{ColorPipeline, ColorSpace};
use filmcraft_export::{
    export, gpu_render_stats, register_frame_renderer, ExportSettings, Format, FrameRenderer,
    GpuRendering, HardwareEncoding, Progress,
};
use filmcraft_gpu::ExportRenderer;
use filmcraft_media::MediaSource;
use filmcraft_project::graphic::new_text_layer;
use filmcraft_project::{
    find_effect, EffectInstance, ItemId, ItemKind, Label, MediaClip, MediaRef, ParamValue, Project,
    SequenceSettings, TrackKind, Transition, TransitionAlign, TransitionId,
};
use filmcraft_render::{RenderOptions, SourceMap, SourceProvider};
use filmcraft_time::{FrameRate, Tick, TimeRange};

struct GpuFrameRenderer(ExportRenderer);
impl FrameRenderer for GpuFrameRenderer {
    fn render(
        &mut self,
        project: &Project,
        seq: ItemId,
        t: Tick,
        opts: RenderOptions,
        sources: &dyn SourceProvider,
    ) -> Option<filmcraft_render::Image> {
        self.0.render(project, seq, t, opts, sources)
    }
}

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

fn build_sequence(
    src: Arc<dyn MediaSource>,
    with_lumetri: bool,
) -> (Arc<Project>, ItemId, SourceMap) {
    let mut p = Project::new("export_bench");
    let seq_rate = FrameRate::FPS_25;

    let mut interpret = filmcraft_project::Interpretation::default();
    interpret.color_space = Some(ColorSpace::SLog3SGamut3Cine);

    let info = src.info().clone();
    let media_id = p.add_item(
        "C1102.MP4",
        Label::Iris,
        ItemKind::Media(MediaClip {
            media: MediaRef::File { path: "C1102.MP4".into() },
            info,
            interpret,
            mark_in: None,
            mark_out: None,
            markers: vec![],
            offline: false,
            proxy: None,
            identity: None,
        }),
        None,
    );

    let seq_settings = SequenceSettings {
        width: 3840,
        height: 2160,
        frame_rate: seq_rate,
        color: ColorPipeline::REC709,
        ..Default::default()
    };
    let seq = p.new_sequence("Benchmark_10s", seq_settings, 2, 2, None);

    // Clip 1: 0.0s to 5.0s (125 frames), source_in = 10.0s (frame 250)
    let mut clip1 = p
        .make_track_item(
            media_id,
            TrackKind::Video,
            Tick::ZERO,
            TimeRange::new(seq_rate.tick_of(250), seq_rate.tick_of(125)),
            seq_rate,
        )
        .unwrap();

    // Clip 2: 5.0s to 10.0s (125 frames), source_in = 60.0s (frame 1500), speed = 1.5x
    let mut clip2 = p
        .make_track_item(
            media_id,
            TrackKind::Video,
            seq_rate.tick_of(125),
            TimeRange::new(seq_rate.tick_of(1500), seq_rate.tick_of(125)),
            seq_rate,
        )
        .unwrap();
    clip2.speed = 1.5;

    if with_lumetri {
        let lum = create_lumetri_instance();
        clip1.effects.push(lum.clone());
        clip2.effects.push(lum);
    }

    let c1_id = clip1.id;
    let c2_id = clip2.id;
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(clip1);
    p.sequence_mut(seq).unwrap().video_tracks[0].items.push(clip2);

    // Cross-dissolve transition at the cut: 4.5s to 5.5s (26 frames duration)
    let tr = Transition {
        id: TransitionId(1),
        effect: find_effect("cross_dissolve").unwrap().instance(),
        start: seq_rate.tick_of(112),
        duration: seq_rate.tick_of(26),
        from: Some(c1_id),
        to: Some(c2_id),
        align: TransitionAlign::CenterAtCut,
        reverse: false,
    };
    p.sequence_mut(seq).unwrap().video_tracks[0].transitions.push(tr);

    // Title Graphic Clip on Track 1: 2.0s to 8.0s (frames 50 to 200)
    let g_id = p.add_item("Title", Label::Rose, ItemKind::Graphic { width: 3840, height: 2160, rate: seq_rate }, None);
    let mut title_item = p
        .make_track_item(
            g_id,
            TrackKind::Video,
            seq_rate.tick_of(50),
            TimeRange::new(Tick::ZERO, seq_rate.tick_of(150)),
            seq_rate,
        )
        .unwrap();
    title_item.effects.push(new_text_layer(
        "FilmCraft C2 Benchmark",
        filmcraft_geom::Vec2::new(1920.0, 1080.0),
        120.0,
    ));
    p.sequence_mut(seq).unwrap().video_tracks[1].items.push(title_item);

    let mut map = SourceMap::default();
    map.0.insert(media_id, src);

    (Arc::new(p), seq, map)
}

fn locate_media_file() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FILMCRAFT_TEST_MEDIA") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
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
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Some(movie_path) = locate_media_file() else {
        eprintln!("Error: C1102.MP4 not found. Set FILMCRAFT_TEST_MEDIA or place C1102.MP4 in Downloads.");
        return Ok(());
    };

    let out_dir = Path::new("D:/FilmCraftX-ab");
    if !out_dir.exists() {
        fs::create_dir_all(out_dir).ok();
    }

    println!("=======================================================");
    println!("FilmCraft-X: 10-Second Sequence Export Comparison (F1)");
    println!("=======================================================");
    println!("Source Media : {:?}", movie_path);
    println!("Output Folder: {:?}", out_dir);

    // Register hardware decoders and encoders
    let _ = filmcraft_platform::register();

    // Register GPU frame renderer factory for export
    register_frame_renderer(|| {
        filmcraft_gpu::ExportRenderer::new().map(|r| Box::new(GpuFrameRenderer(r)) as Box<dyn filmcraft_export::FrameRenderer>)
    });

    let file = Arc::new(filmcraft_media::reader::FileReader::open(&movie_path)?);
    let src = filmcraft_media::reader::open_reader(
        "C1102.MP4",
        file,
        &filmcraft_codecs::reader_openers(),
        &filmcraft_codecs::openers(),
    )?;

    // Build 10s sequence with full Lumetri grade
    let (project, seq, sources) = build_sequence(src, true);
    let seq_rate = FrameRate::FPS_25;
    let range = TimeRange::new(Tick::ZERO, seq_rate.tick_of(250)); // 250 frames (10.0s)

    // 1. CPU Export (gpu_rendering = Off)
    let cpu_out = out_dir.join("cpu_export.mp4");
    println!("\n[1/2] Exporting CPU baseline to {:?} ...", cpu_out);
    let cpu_file = fs::File::create(&cpu_out)?;
    let mut cpu_settings = ExportSettings::default();
    cpu_settings.format = Format::H264;
    cpu_settings.gpu_rendering = GpuRendering::Off;
    cpu_settings.hardware_encoding = HardwareEncoding::Off;
    cpu_settings.range = Some(range);
    cpu_settings.path = cpu_out.to_string_lossy().to_string();
    cpu_settings.output_file = Some(Arc::new(cpu_file));
    cpu_settings.include_audio = false;

    let t0_cpu = Instant::now();
    let cpu_prog = Progress::default();
    let _cpu_report = export(&project, seq, &cpu_settings, &sources, &cpu_prog)?;
    let cpu_duration = t0_cpu.elapsed();
    let cpu_fps = 250.0 / cpu_duration.as_secs_f64();
    let cpu_size = fs::metadata(&cpu_out).map(|m| m.len()).unwrap_or(0);
    println!(
        "  -> CPU Export finished in {:.2}s ({:.1} fps), size: {:.2} MB",
        cpu_duration.as_secs_f64(),
        cpu_fps,
        cpu_size as f64 / (1024.0 * 1024.0)
    );

    // 2. GPU Export (gpu_rendering = Auto)
    let gpu_out = out_dir.join("gpu_export.mp4");
    println!("\n[2/2] Exporting GPU accelerated to {:?} ...", gpu_out);
    let gpu_file = fs::File::create(&gpu_out)?;
    let mut gpu_settings = ExportSettings::default();
    gpu_settings.format = Format::H264;
    gpu_settings.gpu_rendering = GpuRendering::Auto;
    gpu_settings.hardware_encoding = HardwareEncoding::Off;
    gpu_settings.range = Some(range);
    gpu_settings.path = gpu_out.to_string_lossy().to_string();
    gpu_settings.output_file = Some(Arc::new(gpu_file));
    gpu_settings.include_audio = false;

    let t0_gpu = Instant::now();
    let gpu_prog = Progress::default();
    let _gpu_report = export(&project, seq, &gpu_settings, &sources, &gpu_prog)?;
    let gpu_duration = t0_gpu.elapsed();
    let gpu_fps = 250.0 / gpu_duration.as_secs_f64();
    let gpu_size = fs::metadata(&gpu_out).map(|m| m.len()).unwrap_or(0);
    let stats = gpu_render_stats();

    println!(
        "  -> GPU Export finished in {:.2}s ({:.1} fps), size: {:.2} MB",
        gpu_duration.as_secs_f64(),
        gpu_fps,
        gpu_size as f64 / (1024.0 * 1024.0)
    );
    println!("  -> GPU Stats: {} frames rendered by GPU, {} fallbacks", stats.frames, stats.fallbacks);

    // 3. Summary & Comparison
    let speedup = cpu_duration.as_secs_f64() / gpu_duration.as_secs_f64();
    println!("\n=======================================================");
    println!("Export Benchmark Summary (250 frames @ 3840x2160 UHD):");
    println!("  CPU Render Time : {:.2} s ({:.1} FPS)", cpu_duration.as_secs_f64(), cpu_fps);
    println!("  GPU Render Time : {:.2} s ({:.1} FPS)", gpu_duration.as_secs_f64(), gpu_fps);
    println!("  Export Speedup  : {:.2}x faster", speedup);
    println!("  GPU Frame Share : {} / 250 frames", stats.frames);
    println!("  Outputs written :");
    println!("    - {:?}", cpu_out);
    println!("    - {:?}", gpu_out);
    println!("=======================================================");

    // Open the folder for visual inspection
    #[cfg(target_os = "windows")]
    {
        println!("\nOpening output folder in Windows Explorer...");
        let _ = std::process::Command::new("explorer.exe")
            .arg(out_dir)
            .spawn();
    }

    Ok(())
}

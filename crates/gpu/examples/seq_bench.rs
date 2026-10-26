//! Full-sequence export comparison: 10-second timeline from C1102.MP4
//! with two clips, a cross-dissolve transition, a title graphic, and a speed change (1.5x),
//! compared CPU vs GPU, with and without full 6-section Lumetri grading.
//!
//! Run with:
//!     cargo run --release -p filmcraft-gpu --example seq_bench

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use filmcraft_color::{ColorPipeline, ColorSpace};
use filmcraft_gpu::ExportRenderer;
use filmcraft_media::MediaSource;
use filmcraft_project::graphic::new_text_layer;
use filmcraft_project::{
    find_effect, EffectInstance, ItemId, ItemKind, Label, MediaClip, MediaRef, ParamValue, Project,
    SequenceSettings, TrackKind, Transition, TransitionAlign, TransitionId,
};
use filmcraft_render::{render_sequence, RenderOptions, SourceMap};
use filmcraft_time::{FrameRate, Tick, TimeRange};

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

#[derive(Default, Clone)]
struct PixelDiffStats {
    total_pixels: u64,
    total_chan_diff: u64,
    max_chan_diff: u8,
    diff_0: u64,
    diff_1: u64,
    diff_2: u64,
    diff_3_to_5: u64,
    diff_6_to_20: u64,
    diff_gt_20: u64,
    p99_sample: Vec<u8>,
}

impl PixelDiffStats {
    fn accumulate(&mut self, cpu: &[u8], gpu: &[u8]) {
        let n = cpu.len() / 4;
        self.total_pixels += n as u64;
        for (c, g) in cpu.chunks_exact(4).zip(gpu.chunks_exact(4)) {
            let dr = c[0].abs_diff(g[0]);
            let dg = c[1].abs_diff(g[1]);
            let db = c[2].abs_diff(g[2]);
            self.total_chan_diff += (dr as u64) + (dg as u64) + (db as u64);
            let p_max = dr.max(dg).max(db);
            self.max_chan_diff = self.max_chan_diff.max(p_max);
            match p_max {
                0 => self.diff_0 += 1,
                1 => self.diff_1 += 1,
                2 => self.diff_2 += 1,
                3..=5 => self.diff_3_to_5 += 1,
                6..=20 => self.diff_6_to_20 += 1,
                _ => self.diff_gt_20 += 1,
            }
            if self.p99_sample.len() < 1_000_000 {
                self.p99_sample.push(p_max);
            }
        }
    }

    fn summarize(&mut self) -> (f64, u8, f64, f64, f64) {
        let mean_chan = self.total_chan_diff as f64 / (self.total_pixels as f64 * 3.0);
        self.p99_sample.sort_unstable();
        let p99 = if !self.p99_sample.is_empty() {
            self.p99_sample[(self.p99_sample.len() as f64 * 0.99) as usize]
        } else {
            0
        };
        let pct_le_1 = (self.diff_0 + self.diff_1) as f64 / self.total_pixels as f64 * 100.0;
        let pct_le_2 = (self.diff_0 + self.diff_1 + self.diff_2) as f64 / self.total_pixels as f64 * 100.0;
        let pct_gt_20 = self.diff_gt_20 as f64 / self.total_pixels as f64 * 100.0;
        (mean_chan, p99, pct_le_1, pct_le_2, pct_gt_20)
    }
}

fn save_diff_20x(cpu: &[u8], gpu: &[u8], w: u32, h: u32, out_path: &Path) {
    let mut diff_rgba = vec![0u8; cpu.len()];
    for (i, (c, g)) in cpu.chunks_exact(4).zip(gpu.chunks_exact(4)).enumerate() {
        let dr = c[0].abs_diff(g[0]).saturating_mul(20);
        let dg = c[1].abs_diff(g[1]).saturating_mul(20);
        let db = c[2].abs_diff(g[2]).saturating_mul(20);
        let idx = i * 4;
        diff_rgba[idx] = dr;
        diff_rgba[idx + 1] = dg;
        diff_rgba[idx + 2] = db;
        diff_rgba[idx + 3] = 255;
    }
    image::save_buffer(out_path, &diff_rgba, w, h, image::ExtendedColorType::Rgba8).ok();
}

fn build_sequence(
    src: Arc<dyn MediaSource>,
    with_lumetri: bool,
) -> (Project, ItemId, SourceMap) {
    let mut p = Project::new("seq_bench");
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

    (p, seq, map)
}

fn run_benchmark_pass(
    pass_name: &str,
    project: &Project,
    seq: ItemId,
    sources: &SourceMap,
    renderer: &mut ExportRenderer,
    out_dir: &Path,
    save_images: bool,
) {
    println!("\n=======================================================");
    println!("Benchmark Pass: {}", pass_name);
    println!("=======================================================");

    let seq_rate = FrameRate::FPS_25;
    let total_frames = 250; // 10 seconds at 25 fps
    let opts = RenderOptions::default();

    let mut cpu_times = Vec::with_capacity(total_frames);
    let mut gpu_times = Vec::with_capacity(total_frames);
    let mut gpu_rendered_count = 0;
    let mut cpu_fallback_count = 0;

    let mut stats_total = PixelDiffStats::default();
    let mut stats_clip1 = PixelDiffStats::default(); // 0..50
    let mut stats_title1 = PixelDiffStats::default(); // 50..112
    let mut stats_trans = PixelDiffStats::default(); // 113..137
    let mut stats_speed_title = PixelDiffStats::default(); // 138..200
    let mut stats_speed_alone = PixelDiffStats::default(); // 201..250

    println!("Rendering {} frames (CPU vs GPU)...", total_frames);
    let pass_start = Instant::now();

    for f in 0..total_frames {
        let t = seq_rate.tick_of(f as i64);

        // CPU Render
        let t_cpu0 = Instant::now();
        let cpu_img = render_sequence(project, seq, t, opts, sources);
        let cpu_elapsed = t_cpu0.elapsed().as_secs_f64() * 1000.0;
        cpu_times.push(cpu_elapsed);
        let cpu_rgba8 = cpu_img.over_black_rgba8();

        // GPU Render (with ExportRenderer, fallback to CPU if None)
        let t_gpu0 = Instant::now();
        let (gpu_img, was_gpu) = if let Some(g_img) = renderer.render(project, seq, t, opts, sources) {
            (g_img, true)
        } else {
            (render_sequence(project, seq, t, opts, sources), false)
        };
        let gpu_elapsed = t_gpu0.elapsed().as_secs_f64() * 1000.0;
        gpu_times.push(gpu_elapsed);
        let gpu_rgba8 = gpu_img.over_black_rgba8();

        if was_gpu {
            gpu_rendered_count += 1;
        } else {
            cpu_fallback_count += 1;
        }

        // Accumulate pixel diffs
        stats_total.accumulate(&cpu_rgba8, &gpu_rgba8);
        match f {
            0..=49 => stats_clip1.accumulate(&cpu_rgba8, &gpu_rgba8),
            50..=112 => stats_title1.accumulate(&cpu_rgba8, &gpu_rgba8),
            113..=137 => stats_trans.accumulate(&cpu_rgba8, &gpu_rgba8),
            138..=200 => stats_speed_title.accumulate(&cpu_rgba8, &gpu_rgba8),
            _ => stats_speed_alone.accumulate(&cpu_rgba8, &gpu_rgba8),
        }

        // Save representative sample frames on first pass
        if save_images {
            let prefix = if pass_name.contains("Lumetri") { "lumetri" } else { "raw" };
            if f == 25 {
                // Section 1: Clip 1 alone (t = 1.0s)
                image::save_buffer(out_dir.join(format!("{prefix}_frame_025_cpu.png")), &cpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
                image::save_buffer(out_dir.join(format!("{prefix}_frame_025_gpu.png")), &gpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
            } else if f == 75 {
                // Section 2: Clip 1 + Title (t = 3.0s)
                image::save_buffer(out_dir.join(format!("{prefix}_frame_075_title_cpu.png")), &cpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
                image::save_buffer(out_dir.join(format!("{prefix}_frame_075_title_gpu.png")), &gpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
                save_diff_20x(&cpu_rgba8, &gpu_rgba8, 3840, 2160, &out_dir.join(format!("{prefix}_frame_075_title_diff_20x.png")));
            } else if f == 125 {
                // Section 3: Transition midpoint (t = 5.0s)
                image::save_buffer(out_dir.join(format!("{prefix}_frame_125_trans_cpu.png")), &cpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
                image::save_buffer(out_dir.join(format!("{prefix}_frame_125_trans_gpu.png")), &gpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
                save_diff_20x(&cpu_rgba8, &gpu_rgba8, 3840, 2160, &out_dir.join(format!("{prefix}_frame_125_trans_diff_20x.png")));
            } else if f == 175 {
                // Section 4: Clip 2 (1.5x) + Title (t = 7.0s)
                image::save_buffer(out_dir.join(format!("{prefix}_frame_175_speed_cpu.png")), &cpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
                image::save_buffer(out_dir.join(format!("{prefix}_frame_175_speed_gpu.png")), &gpu_rgba8, 3840, 2160, image::ExtendedColorType::Rgba8).ok();
            }
        }

        if (f + 1) % 50 == 0 || f == total_frames - 1 {
            println!("  [frame {:3}/{}] elapsed: {:.1}s", f + 1, total_frames, pass_start.elapsed().as_secs_f64());
        }
    }

    let cpu_total_sec: f64 = cpu_times.iter().sum::<f64>() / 1000.0;
    let gpu_total_sec: f64 = gpu_times.iter().sum::<f64>() / 1000.0;
    let cpu_mean_ms = cpu_times.iter().sum::<f64>() / total_frames as f64;
    let gpu_mean_ms = gpu_times.iter().sum::<f64>() / total_frames as f64;

    println!("\n--- Export Speed Results ---");
    println!("  Frames rendered by GPU:     {} / {} ({:.1}%)", gpu_rendered_count, total_frames, gpu_rendered_count as f64 / total_frames as f64 * 100.0);
    println!("  Frames CPU fallback:        {} / {} ({:.1}%)", cpu_fallback_count, total_frames, cpu_fallback_count as f64 / total_frames as f64 * 100.0);
    println!("  Total CPU Export Time:      {:.2} s ({:.1} ms/frame)", cpu_total_sec, cpu_mean_ms);
    println!("  Total GPU Export Time:      {:.2} s ({:.1} ms/frame)", gpu_total_sec, gpu_mean_ms);
    println!("  Speedup (CPU / GPU):        {:.2}x", cpu_total_sec / gpu_total_sec);

    let (tot_mean, tot_p99, tot_le1, tot_le2, tot_gt20) = stats_total.summarize();
    println!("\n--- Overall Pixel Differences (Total Sequence) ---");
    println!("  Max channel difference:     {}/255 ({:.4}%)", stats_total.max_chan_diff, stats_total.max_chan_diff as f64 / 255.0 * 100.0);
    println!("  Mean channel difference:    {:.4}/255 ({:.4}%)", tot_mean, tot_mean / 255.0 * 100.0);
    println!("  99th percentile (p99) diff: {}/255", tot_p99);
    println!("  <= 1 code difference:       {:.2}%", tot_le1);
    println!("  <= 2 code difference:       {:.2}%", tot_le2);
    println!("  > 20 code difference:       {:.4}%", tot_gt20);

    println!("\n--- Regional Pixel Differences Breakdown ---");
    let print_region = |name: &str, mut s: PixelDiffStats| {
        let (mean, p99, _le1, le2, gt20) = s.summarize();
        println!("  {:28} max: {:2}/255 | mean: {:.4}/255 | p99: {}/255 | <=2: {:.2}% | >20: {:.4}%",
            name, s.max_chan_diff, mean, p99, le2, gt20);
    };
    print_region("Clip 1 alone (0..2s)", stats_clip1);
    print_region("Clip 1 + Title (2..4.5s)", stats_title1);
    print_region("Transition + Title (4.5..5.5s)", stats_trans);
    print_region("Speed 1.5x + Title (5.5..8s)", stats_speed_title);
    print_region("Speed 1.5x alone (8..10s)", stats_speed_alone);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let default_media = "test_assets/C1102.MP4";
    let media_env = std::env::var("FILMCRAFT_TEST_MEDIA").unwrap_or_else(|_| default_media.to_string());
    let movie_path = Path::new(&media_env);
    if !movie_path.exists() {
        eprintln!("Error: Media file not found at {:?}. Set FILMCRAFT_TEST_MEDIA env var.", movie_path);
        return Ok(());
    }

    let out_dir = Path::new("D:/FilmCraftX-ab");
    if !out_dir.exists() {
        fs::create_dir_all(out_dir).ok();
    }

    println!("Opening media file: {:?}", movie_path);
    let _ = filmcraft_platform::register();
    let file = Arc::new(filmcraft_media::reader::FileReader::open(movie_path)?);
    let src = filmcraft_media::reader::open_reader(
        "C1102.MP4",
        file,
        &filmcraft_codecs::reader_openers(),
        &filmcraft_codecs::openers(),
    )?;

    println!("Initializing ExportRenderer (wgpu)...");
    let mut renderer = ExportRenderer::new().expect("ExportRenderer init failed");

    // Pass 1: Raw S-Log3 (no Lumetri)
    let (p_raw, seq_raw, map_raw) = build_sequence(src.clone(), false);
    run_benchmark_pass(
        "Raw S-Log3 (Two clips, Dissolve, Title, 1.5x Speed)",
        &p_raw,
        seq_raw,
        &map_raw,
        &mut renderer,
        out_dir,
        true,
    );

    // Pass 2: With full 6-Section Lumetri Chain
    let (p_lum, seq_lum, map_lum) = build_sequence(src, true);
    run_benchmark_pass(
        "Lumetri S-Log3 (6-Section Grade, Dissolve, Title, 1.5x Speed)",
        &p_lum,
        seq_lum,
        &map_lum,
        &mut renderer,
        out_dir,
        true,
    );

    println!("\nFull-sequence export benchmark complete! Check outputs in {:?}", out_dir);
    Ok(())
}

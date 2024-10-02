use av_metrics_decoders::{Decoder, VapoursynthDecoder};
use vapoursynth::core::CoreRef;
use vapoursynth::prelude::*;
use crossterm::tty::IsTty;
use indicatif::{HumanDuration, ProgressBar, ProgressDrawTarget, ProgressState, ProgressStyle};
use ssimulacra2::{
    ColorPrimaries as Primaries, MatrixCoefficients as Matrices,
    TransferCharacteristic as Transfers, *,
};
use std::cmp::min;
use std::collections::BTreeMap;
use std::io::{stderr, prelude::*};
use std::path::{absolute as abs, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::available_parallelism;
use std::time::Duration;
use std::process::exit;

trait FromSize {
    fn from_size(width: usize, height: usize, matrix: Option<Matrices>) -> Self;
}

impl FromSize for Matrices {
    fn from_size(width: usize, height: usize, _matrix: Option<Matrices>) -> Matrices {
        if width >= 1280 || height > 576 {
            Matrices::BT709
        } else if height == 576 {
            Matrices::BT470BG
        } else {
            Matrices::ST170M
        }
    }
}

impl FromSize for Primaries {
    fn from_size(width: usize, height: usize, matrix: Option<Matrices>) -> Self {
        if matrix == Some(Matrices::BT2020NonConstantLuminance)
            || matrix == Some(Matrices::BT2020ConstantLuminance)
        {
            ColorPrimaries::BT2020
        } else if matrix == Some(Matrices::BT709) || width >= 1280 || height > 576 {
            ColorPrimaries::BT709
        } else if height == 576 {
            ColorPrimaries::BT470BG
        } else if height == 480 || height == 488 {
            ColorPrimaries::ST170M
        } else {
            ColorPrimaries::BT709
        }
    }
}

fn to_primaries(input: String) -> Primaries {
    match input.as_str() {
        "bt709" => Primaries::BT709,
        "bt470m" => Primaries::BT470M,
        "bt470bg" => Primaries::BT470BG,
        "smpte170m" => Primaries::ST170M,
        "smpte240m" => Primaries::ST240M,
        "film" => Primaries::Film,
        "bt2020" => Primaries::BT2020,
        "smpte428" => Primaries::ST428,
        "smpte428_1" => Primaries::ST428,
        "smpte431" => Primaries::P3DCI,
        "smpte432" => Primaries::P3Display,
        "ebu3213" => Primaries::Tech3213,
        "unknown" | "jedec-p22" | "unspecified" | _ => Primaries::Unspecified,
    }
}

fn to_matrices(input: String) -> Matrices {
    match input.as_str() {
        "rgb" => Matrices::Identity,
        "bt709" => Matrices::BT709,
        "fcc" => Matrices::BT470M,
        "bt470bg" => Matrices::BT470BG,
        "smpte170m" => Matrices::ST170M,
        "smpte240m" => Matrices::ST240M,
        "ycgco" | "ycocg" | "ycgco-re" | "ycgco-ro" => Matrices::YCgCo,
        "bt2020nc" | "bt2020ncl" => Matrices::BT2020NonConstantLuminance,
        "bt2020c" | "bt2020_cl" => Matrices::BT2020ConstantLuminance,
        "smpte2085" => Matrices::ST2085,
        "chroma-derived-nc" => Matrices::ChromaticityDerivedNonConstantLuminance,
        "chroma-derived-c" => Matrices::ChromaticityDerivedConstantLuminance,
        "ictcp" => Matrices::ICtCp,
        "unknown" | "unspecified" | _ => Matrices::Unspecified,
    }
}

fn to_transfers(input: String) -> Transfers {
    match input.as_str() {
        "bt709" => Transfers::BT1886,
        "gamma22" => Transfers::BT470M,
        "gamma28" => Transfers::BT470BG,
        "smpte170m" => Transfers::ST170M,
        "smpte240m" => Transfers::ST240M,
        "linear" => Transfers::Linear,
        "log100" | "log" => Transfers::Logarithmic100,
        "log316" | "log_sqrt" => Transfers::Logarithmic316,
        "iec61966-2-4" | "iec61966_2_4" => Transfers::XVYCC,
        "bt1361e" | "bt1361" => Transfers::BT1361E,
        "iec61966-2-1" | "iec61966_2_1" => Transfers::SRGB,
        "bt2020-10" | "bt2020_10bit" => Transfers::BT2020Ten,
        "bt2020-12" | "bt2020_12bit" => Transfers::BT2020Twelve,
        "smpte2084" => Transfers::PerceptualQuantizer,
        "smpte428" | "smpte428_1" => Transfers::ST428,
        "arib-std-b67" => Transfers::HybridLogGamma,
        "unknown" | "unspecified" | _ => Transfers::Unspecified,
    }
}

const PROGRESS_CHARS: &str = "█▉▊▋▌▍▎▏  ";
const INDICATIF_PROGRESS_TEMPLATE: &str = if cfg!(windows) {
    // Do not use a spinner on Windows since the default console cannot display
    // the characters used for the spinner
    "{elapsed_precise:.bold} ▕{wide_bar:.blue/white.dim}▏ {percent:.bold}  {pos} ({fps:.bold}, eta {fixed_eta}{msg})"
} else {
    "{spinner:.green.bold} {elapsed_precise:.bold} ▕{wide_bar:.blue/white.dim}▏ {percent:.bold}  {pos} ({fps:.bold}, eta {fixed_eta}{msg})"
};

const INDICATIF_SPINNER_TEMPLATE: &str = if cfg!(windows) {
    // Do not use a spinner on Windows since the default console cannot display
    // the characters used for the spinner
    "{elapsed_precise:.bold} {pos} ({fps:.bold}{msg})"
} else {
    "{spinner:.green.bold} {elapsed_precise:.bold} {pos} ({fps:.bold}{msg})"
};

fn pretty_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template(INDICATIF_PROGRESS_TEMPLATE)
        .unwrap()
        .with_key(
            "fps",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                if state.pos() == 0 || state.elapsed().as_secs_f32() < f32::EPSILON {
                    write!(w, "0 fps").unwrap();
                } else {
                    let fps = state.pos() as f32 / state.elapsed().as_secs_f32();
                    if fps < 1.0 {
                        write!(w, "{:.2} s/fr", 1.0 / fps).unwrap();
                    } else {
                        write!(w, "{:.2} fps", fps).unwrap();
                    }
                }
            },
        )
        .with_key(
            "fixed_eta",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                if state.pos() == 0 || state.elapsed().as_secs_f32() < f32::EPSILON {
                    write!(w, "unknown").unwrap();
                } else {
                    let spf = state.elapsed().as_secs_f32() / state.pos() as f32;
                    let remaining = state.len().unwrap_or(0) - state.pos();
                    write!(
                        w,
                        "{:#}",
                        HumanDuration(Duration::from_secs_f32(spf * remaining as f32))
                    )
                    .unwrap();
                }
            },
        )
        .with_key(
            "pos",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                write!(w, "{}/{}", state.pos(), state.len().unwrap_or(0)).unwrap();
            },
        )
        .with_key(
            "percent",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                write!(w, "{:>3.0}%", state.fraction() * 100_f32).unwrap();
            },
        )
        .progress_chars(PROGRESS_CHARS)
}

fn pretty_spinner_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template(INDICATIF_SPINNER_TEMPLATE)
        .unwrap()
        .with_key(
            "fps",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                if state.pos() == 0 || state.elapsed().as_secs_f32() < f32::EPSILON {
                    write!(w, "0 fps").unwrap();
                } else {
                    let fps = state.pos() as f32 / state.elapsed().as_secs_f32();
                    if fps < 1.0 {
                        write!(w, "{:.2} s/fr", 1.0 / fps).unwrap();
                    } else {
                        write!(w, "{:.2} fps", fps).unwrap();
                    }
                }
            },
        )
        .with_key(
            "pos",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                write!(w, "{}", state.pos()).unwrap();
            },
        )
        .progress_chars(PROGRESS_CHARS)
}

fn calc_score<S: Pixel, D: Pixel, E: Decoder, F: Decoder>(
    mtx: &Mutex<(usize, (E, F))>,
    src_yuvcfg: &YuvConfig,
    dst_yuvcfg: &YuvConfig,
    inc: usize,
    verbose: bool,
) -> Option<(usize, f64)> {
    let (frame_idx, (src_frame, dst_frame)) = {
        let mut guard = mtx.lock().unwrap();
        let curr_frame = guard.0;

        let src_frame = guard.1 .0.read_video_frame::<S>();
        let dst_frame = guard.1 .1.read_video_frame::<D>();

        if let (Some(sf), Some(df)) = (src_frame, dst_frame) {
            // skip remaining frames in increment size
            for ii in 1..inc {
                let _src_frame = guard.1 .0.read_video_frame::<S>();
                let _dst_frame = guard.1 .1.read_video_frame::<D>();
                if _src_frame.is_none() || _dst_frame.is_none() {
                    break;
                }
                if verbose {
                    println!("Frame {}: skip", curr_frame + ii);
                }
            }

            guard.0 += inc;
            (curr_frame, (sf, df))
        } else {
            return None;
        }
    };

    let src_yuv = Yuv::new(src_frame, *src_yuvcfg).unwrap();
    let dst_yuv = Yuv::new(dst_frame, *dst_yuvcfg).unwrap();

    Some((
        frame_idx,
        compute_frame_ssimulacra2(src_yuv, dst_yuv).expect("Failed to calculate ssimulacra2"),
    ))
}

fn lwlibavsource<'a>(file: &PathBuf, api: &API, core: &CoreRef<'a>, format: &str) -> Node<'a> {
    let lsmas = core.get_plugin_by_namespace("lsmas").unwrap().expect("Failed to find lsmas namespace! Is the plugin installed?");
    let mut args = OwnedMap::new(*api);
    args.set_data("source", file.to_str().unwrap().as_bytes()).unwrap();
    args.set_data("cachedir", file.parent().unwrap().to_str().unwrap().as_bytes()).unwrap();
    args.set_data("format", format.as_bytes()).unwrap();
    args.set_int("prefer_hw", 3).unwrap();
    let func = lsmas.invoke("LWLibavSource", &args).unwrap();
    if func.error().is_some() {
        panic!("{}", func.error().unwrap());
    }
    func.get_node("clip").unwrap()
}

fn bestsource<'a>(file: &PathBuf, api: &API, core: &CoreRef<'a>) -> Node<'a> {
    let bs = core.get_plugin_by_namespace("bs").unwrap().expect("Failed to find bs namespace! Is the plugin installed?");
    let abspath = abs(file.parent().unwrap()).unwrap();
    let mut root = abspath.components().next().unwrap().as_os_str().to_string_lossy().to_string();
    if !root.ends_with('/') {
        root.push('/');
    }
    let mut args = OwnedMap::new(*api);
    args.set_data("source", abs(file).unwrap().to_str().unwrap().as_bytes()).unwrap();
    args.set_data("cachepath", root.as_bytes()).unwrap();
    let func = bs.invoke("VideoSource", &args).unwrap();
    if func.error().is_some() {
        panic!("{}", func.error().unwrap());
    }
    func.get_node("clip").unwrap()
}

fn dgdecodenv<'a>(file: &PathBuf, api: &API, core: &CoreRef<'a>) -> Node<'a> {
    let dgdecodenv = core.get_plugin_by_namespace("dgdecodenv").unwrap().expect("Failed to find dgdecodenv namespace! Is the plugin installed?");
    let mut args = OwnedMap::new(*api);
    args.set_data("source", file.to_str().unwrap().as_bytes()).unwrap();
    let func = dgdecodenv.invoke("DGSource", &args).unwrap();
    if func.error().is_some() {
        panic!("{}", func.error().unwrap());
    }
    func.get_node("clip").unwrap()
}

pub fn get_vs_ssimu2(src: &PathBuf, distorted: &PathBuf, cycle: u8, algo: &String) -> BTreeMap<usize, f64> {
    let threads = available_parallelism().unwrap().get();
    let api = API::get().unwrap();
    let core = api.create_core(threads as i32);
    let vszip = core.get_plugin_by_namespace("vszip").unwrap().expect("Failed to find vszip namespace! Is the plugin installed?");
    let skip_content = if src.extension().is_some_and(|e| e.to_ascii_lowercase() == "vpy") {
        Some(Environment::from_file(src, EvalFlags::Nothing).unwrap())
    } else {
        None
    };
    let reference = if skip_content.as_ref().is_some() {
        skip_content.as_ref().unwrap().get_output(0).unwrap().0
    } else {
        if algo == "lsmash" {
            lwlibavsource(&src, &api, &core, "YUV420P8")
        } else if algo == "bestsource" {
            bestsource(&src, &api, &core)
        } else if algo == "dgdecnv" {
            dgdecodenv(&src, &api, &core)
        } else {
            unreachable!()
        }
    };
    let distort = if algo == "lsmash" {
        lwlibavsource(&distorted, &api, &core, "YUV420P8")
    } else if algo == "bestsource" {
        bestsource(&distorted, &api, &core)
    } else if algo == "dgdecnv" {
        dgdecodenv(&distorted, &api, &core)
    } else {
        unreachable!()
    };
    let frames = reference.info().num_frames;
    let mut args = OwnedMap::new(api);
    args.set_node("reference", &reference).unwrap();
    args.set_node("distorted", &distort).unwrap();
    args.set_int("mode", 0).unwrap();
    let scored = vszip.invoke("Metrics", &args).unwrap();
    if scored.error().is_some() {
        panic!("{}", scored.error().unwrap());
    }
    let scored_node = scored.get_node("clip").unwrap();
    let progress = if stderr().is_tty() {
        let pb = ProgressBar::new(frames as u64)
            .with_style(pretty_progress_style())
            .with_message(", avg: N/A");
        pb.set_draw_target(ProgressDrawTarget::stderr());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.reset();
        pb.reset_eta();
        pb.reset_elapsed();
        pb.set_position(0);
        pb
    } else {
        ProgressBar::hidden()
    };
    let mut avg = 0f64;
    let mut results = BTreeMap::new();
    let mut jobs = 0u8;
    for index in 0..frames {
        loop {
            if (jobs as usize) < threads {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        jobs += 1;
        scored_node.get_frame_async(index, |frame, n, _| {
            let frame = frame.expect("Failed to generate frame!");
            let score = frame.props().get_float("_SSIMULACRA2").expect("Failed to get SSIMULACRA2 score!");
            results.insert(n * cycle as usize, score);
            avg = avg + (score - avg) / (min(results.len(), 10) as f64);
            if progress.is_finished() {
                eprintln!("Got frame but progress bar is finished! Frame: {n}");
            } else {
                progress.set_message(format!(", avg: {:.1$}", avg, 2));
                progress.inc(1);
            }
            jobs -= 1;
        });
    }
    loop {
        if results.len() >= frames {
            progress.finish();
            break;
        }
        if jobs == 0 {
            eprintln!("Number of SSIMULACRA2 jobs has reached 0, but results only has {} entries instead of {frames}!", results.len());
            println!("PAUSED: Would you like to continue?");
            print!("(yes/no): ");
            std::io::stdout().flush().expect("Failed to flush!");
            let mut input: String = String::new();
            std::io::stdin().read_line(&mut input).expect("Failed to read input!");
            if input != "yes\n" {
                eprintln!("\nAborted. Exiting script.");
                exit(0);
            }
            println!("Continuing.");
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    progress.finish();
    results
}

pub fn get_ssimu2(src: &PathBuf, distorted: &PathBuf, cycle: u8, cr: String, matrix: String, transfer: String, primaries: String) -> BTreeMap<usize, f64> {
    let threads = available_parallelism().unwrap().get() / 2usize;
    let skip_content = if src.extension().is_some_and(|e| e.to_ascii_lowercase() == "vpy") {
        VapoursynthDecoder::new_from_script(&src).unwrap()
    } else {
        VapoursynthDecoder::new_from_video(&src).unwrap()
    };
    println!("{}", distorted.display());
    let distort_content = VapoursynthDecoder::new_from_video(&distorted).unwrap();
    let distort_frames = distort_content.get_frame_count().ok();
    let total_frames = skip_content.get_frame_count().ok();
    if distort_frames.is_some()
        && total_frames.is_some()
        && (distort_frames.unwrap() != total_frames.unwrap())
    {
        eprintln!("WARNING: Frame count mismatch detected, scores may be inaccurate");
    }
    let src_info = skip_content.get_video_details();
    let distort_info = distort_content.get_video_details();
    let src_ss = src_info.chroma_sampling.get_decimation().unwrap_or((0, 0));
    let dist_ss = distort_info.chroma_sampling.get_decimation().unwrap_or((0, 0));
    let (width, height) = (src_info.width, src_info.height);
    let (range, matrices, transfers, _primaries): (bool, Matrices, Transfers, Primaries);
    range = cr == "pc" || cr == "jpeg" || cr == "full";
    matrices = if to_matrices(matrix.clone()) != Matrices::Unspecified { to_matrices(matrix.clone()) } else { Matrices::from_size(width, height, None) };
    transfers = to_transfers(transfer.clone());
    _primaries = if to_primaries(primaries.clone()) != Primaries::Unspecified { to_primaries(primaries.clone()) } else { Primaries::from_size(width, height, Some(matrices)) };
    let src_config = YuvConfig {
        bit_depth: src_info.bit_depth as u8,
        subsampling_x: src_ss.0 as u8,
        subsampling_y: src_ss.1 as u8,
        full_range: range,
        matrix_coefficients: matrices,
        transfer_characteristics: transfers,
        color_primaries: _primaries,
    };
    let mut dst_config = src_config.clone();
    dst_config.bit_depth = distort_info.bit_depth as u8;
    dst_config.subsampling_x = dist_ss.0 as u8;
    dst_config.subsampling_y = dist_ss.1 as u8;
    let (result_tx, result_rx) = mpsc::channel();
    let current_frame = 0usize;
    let decoders = Arc::new(Mutex::new((current_frame, (skip_content, distort_content))));
    for _ in 0..threads {
        let decoders = Arc::clone(&decoders);
        let result_tx = result_tx.clone();
        std::thread::spawn(move || {
            loop {
                let score = match (src_info.bit_depth, distort_info.bit_depth) {
                    (8, 8) => calc_score::<u8, u8, _, _>(
                        &decoders,
                        &src_config,
                        &dst_config,
                        1,
                        false,
                    ),
                    (8, _) => calc_score::<u8, u16, _, _>(
                        &decoders,
                        &src_config,
                        &dst_config,
                        1,
                        false,
                    ),
                    (_, 8) => calc_score::<u16, u8, _, _>(
                        &decoders,
                        &src_config,
                        &dst_config,
                        1,
                        false,
                    ),
                    (_, _) => calc_score::<u16, u16, _, _>(
                        &decoders,
                        &src_config,
                        &dst_config,
                        1,
                        false,
                    ),
                };

                if let Some(result) = score {
                    result_tx.send(result).unwrap();
                } else {
                    break;
                }
            }
        });
    }
    drop(result_tx);
    let progress = if stderr().is_tty() {
        let frame_count = total_frames.or(distort_frames);
        let pb = if let Some(frame_count) = frame_count {
            ProgressBar::new(frame_count as u64)
                .with_style(pretty_progress_style())
                .with_message(", avg: N/A")
        } else {
            ProgressBar::new_spinner().with_style(pretty_spinner_style())
        };
        pb.set_draw_target(ProgressDrawTarget::stderr());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.reset();
        pb.reset_eta();
        pb.reset_elapsed();
        pb.set_position(0);
        pb
    } else {
        ProgressBar::hidden()
    };
    let mut results = BTreeMap::new();
    let mut avg = 0f64;
    for score in result_rx {
        results.insert(score.0 * cycle as usize, score.1);
        avg = avg + (score.1 - avg) / (min(results.len(), 10) as f64);
        progress.set_message(format!(", avg: {:.1$}", avg, 2));
        progress.inc(1);
    }
    progress.finish();
    results
}

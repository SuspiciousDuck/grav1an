use clap::Parser;
use std::path::PathBuf;
use std::thread::available_parallelism;

/// AV1 Encoding Script using VS filters, av1an, opusenc, grav1synth, and mkvmerge
#[derive(Parser, Debug)]
#[command(version, about, long_about = None, arg_required_else_help(true))]
pub struct Args {
    /// Input directory containing video files
    #[arg(short, long)]
    pub input_directory: PathBuf,
    /// Output directory for processed video files
    #[arg(short, long)]
    pub output_directory: PathBuf,
    /// Group name
    #[arg(short, long, default_value_t = String::from("[Group]"))]
    pub group: String,
    /// Series title
    #[arg(short, long)]
    pub name: String,
    /// Filename suffix
    #[arg(long, default_value_t = String::from("[1080p.AV1]"))]
    pub suffix: String,
    /// Episode pattern for output (1 = "XX", 2 = "SXXEXX", or string)
    #[arg(long, default_value_t = String::from("1"))]
    pub episode_pattern: String,
    /// Set season for pattern 2
    #[arg(long, default_value_t = String::from("01"))]
    pub season: String,
    /// Pauses operation to review and manually edit VapourSynth scripts and tags per episode
    #[arg(short, long, num_args = 0, default_value_t = false)]
    pub review: bool,
    /// Skip creating VapourSynth filters
    #[arg(long, num_args = 0, default_value_t = false)]
    pub no_filter: bool,
    /// Apply rescale, skip if unfamiliar
    #[arg(long, requires_all(["algo", "height"]), num_args = 0, default_value_t = false)]
    pub rescale: bool,
    /// Rescale adjust for match centers model
    #[arg(
        long = "match",
        requires = "rescale",
        num_args = 0,
        default_value_t = false
    )]
    pub _match: bool,
    /// Frame height used for descale
    #[arg(long, requires = "rescale", default_value = None)]
    pub height: Option<u16>,
    /// Frame width used for descale
    #[arg(long, requires = "rescale", default_value = None)]
    pub width: Option<u16>,
    /// Algorithm used for descale
    #[arg(long, requires = "rescale", default_value = None)]
    pub algo: Option<String>,
    /// Border padding
    #[arg(long, requires = "rescale", default_value_t = 0)]
    pub borders: u16,
    /// Upscale precision
    #[arg(long, requires = "rescale", default_value_t = true)]
    pub fp16: bool,
    /// Tiles for upscale to reduce vram usage
    #[arg(long, requires = "rescale", default_value_t = 4)]
    pub dstiles: u8,
    /// Skip denoise
    #[arg(long, num_args = 0, default_value_t = false)]
    pub no_denoise: bool,
    /// Strength of denoise
    #[arg(long, default_value_t = 0.3)]
    pub denoise: f32,
    /// Extra weighting calculation for denoise
    #[arg(long, num_args = 0, default_value_t = false)]
    pub ref_calc: bool,
    /// Dehalo/dering
    #[arg(long, num_args = 0, default_value_t = false)]
    pub dehalo: bool,
    /// Number of av1an workers
    #[arg(short, long, default_value_t = available_parallelism().unwrap().get() as u8)]
    pub workers: u8,
    /// Max cache size per vspipe/worker in GB
    #[arg(short, long, default_value_t = 1)]
    pub mem: u8,
    /// For chunking and VS scripts
    #[arg(long = "source_filter", value_parser(["lsmash","dgdecnv","bestsource"]), default_value = "bestsource")]
    pub source_filter: String,
    /// Video encoder
    #[arg(short, long, value_parser(["svt-av1","rav1e"]), default_value = "svt-av1")]
    pub encoder: String,
    /// Pixel format
    #[arg(long, default_value_t = String::from("yuv420p10le"))]
    pub pixel_format: String,
    /// Quality setting [default: 100 (rav1e)/40 (svt-av1)]
    #[arg(
        short,
        long,
        default_value_if("encoder", "rav1e", "100"),
        default_value = "40.0",
        visible_alias = "crf"
    )]
    pub quantizer: f32,
    /// speed/preset setting [default: 2 (rav1e)/4 (svt)]
    #[arg(
        short,
        long,
        default_value_if("encoder", "rav1e", "2"),
        default_value = "4",
        visible_alias = "preset"
    )]
    pub speed: u8,
    /// rav1e-only setting
    #[arg(short, long, default_value_t = 8)]
    pub tiles: u8,
    /// Only use 1-pass encoding and static quality
    #[arg(long, num_args = 0, default_value_t = false)]
    pub single_pass: bool,
    /// Adjust quality per scene with multipass encoding to target mean SSIMU2 score
    #[arg(long, default_value_t = 80.0)]
    pub target_quality: f32,
    #[arg(short, long, default_value_t = 10)]
    pub cycle: u8,
    /// Q/crf range for target quality calculations [default: 30 (rav1e)/7.5 (svt-av1)]
    #[arg(
        long,
        default_value_t = 7.5,
        default_value_if("encoder", "rav1e", "30")
    )]
    pub quantizer_calc: f32,
    /// Q/crf range allowed for final pass [default: [40,160] (rav1e)/[25,55] (svt-av1)]
    #[arg(long, default_value = None)]
    pub quantizer_range: Option<String>, // ARGHHHHH clap has no support for conditional default valueS, this SHOULDVE been a (f32, f32), but clap doesnt have default_values_if
    /// Skip FGS
    #[arg(long, num_args = 0, default_value_t = false)]
    pub no_grain: bool,
    /// Estimated FGS
    #[arg(long, num_args = 0, default_value_t = false)]
    pub diff_grain: bool,
    /// Lehmer merge 2nd source for FGS
    #[arg(
        long,
        requires = "src2_directory",
        num_args = 0,
        default_value_t = false
    )]
    pub lehmer_merge: bool,
    /// Grain intensity as ISO value, --chroma optional
    #[arg(long, default_value_t = 400)]
    pub photon_noise: u16,
    /// Raws source
    #[arg(long, default_value_t = String::from("BD"))]
    pub raws: String,
    /// Audio source, 1, 2, or both
    #[arg(long, value_parser(["1","2","both"]), requires_ifs = [("both","src2_directory"),("2","src2_directory")], default_value = "1")]
    pub audio: String,
    /// Subtitles source, 1, 2, or both
    #[arg(long, value_parser(["1","2","both"]), requires_ifs = [("both","src2_directory"),("2","src2_directory")], default_value = "1")]
    pub subs: String,
    /// Input directory containing 2nd sources
    #[arg(long, value_enum, default_value = None)]
    pub src2_directory: Option<PathBuf>,
    /// Manually set offset for 2nd sources in milliseconds
    #[arg(long, default_value_t = 0)]
    pub sync: u32,
    /// Skip creating a torrent file
    #[arg(long, num_args = 0, default_value_t = false)]
    pub no_torrent: bool,
    /// Url for source file
    #[arg(long, default_value = None)]
    pub source_url: Option<String>,
    /// Url for series info
    #[arg(long, default_value = None)]
    pub source_info: Option<String>,
    /// Torrent contains output folder
    #[arg(short, long, num_args = 0, default_value_t = false)]
    pub batch: bool,
}
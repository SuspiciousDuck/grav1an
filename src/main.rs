use core::str;
use clap::Parser;
use fancy_regex::Regex;
use futures::io::AllowStdIo as asyncio;
use isolang::Language;
use itertools::Itertools;
use phf::phf_map;
use polyfit_rs::polyfit_rs::polyfit;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use statrs::statistics::{Distribution, Median, OrderStatistics};
use std::ffi::{OsStr, OsString};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::process::{exit, Command, Stdio};
use std::{fmt::Debug, fs::File, path::absolute as abs, path::PathBuf};
use which::which;
mod ssimulacra2;
mod args;
mod torrent;
use self::args::Args;
use self::torrent::create_torrent;
use self::ssimulacra2::get_ssimu2;

// mixing &str and String is painful
macro_rules! vec_into {
    ($($x:expr),*) => (vec![$($x.into()),*]);
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ScenesInfo {
    scenes: Vec<Scene>,
    frames: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Scene {
    quantizer_scores: Option<HashMap<usize, QuantizerScores>>,
    final_quantizer: Option<f32>,
    start_frame: u32,
    end_frame: u32,
    zone_overrides: Option<ZoneOverrides>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct QuantizerScores {
    mean: f64,
    median: f64,
    std_dev: f64,
    percentile_5th: f64,
    percentile_95th: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct ZoneOverrides {
    encoder: String,
    passes: u8,
    video_params: Vec<String>,
    photon_noise: Option<u16>,
    extra_split_sec: u8,
    min_scene_len: u8,
}

#[derive(Deserialize, Clone, Debug)]
struct Stream {
    index: u8,
    codec_name: String,
    codec_type: String,
    avg_frame_rate: Option<String>,
    start_pts: u16,
    channels: Option<u8>,
    width: Option<u16>,
    height: Option<u16>,
    display_aspect_ratio: Option<String>,
    pix_fmt: Option<String>,
    color_space: Option<String>,
    color_range: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    disposition: Disposition,
    tags: Tags,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Disposition {
    forced: u8,
}

#[derive(serde::Deserialize, Clone, Debug)]
struct Tags {
    #[serde(rename = "BPS")]
    bps: Option<String>,
    #[serde(rename = "ENCODER_OPTIONS")]
    encoder_options: Option<String>,
    language: Option<String>,
    title: Option<String>,
}

#[derive(PartialEq, Eq, Hash)]
struct Track {
    language: Language,
    title: String,
    forced: bool,
}

#[derive(serde::Deserialize, Clone)]
struct FileProbe {
    streams: Vec<Stream>,
}

#[derive(Clone, Debug)]
struct Probe {
    stream: Stream,
    file: PathBuf,
    offset: u32,
    index: Option<u8>,
}
impl Probe {
    fn language(&self) -> Language {
        let lang = self.stream.tags.language.clone();
        if lang.is_none() {
            return Language::Und;
        }
        let _code = lang.unwrap().split('-').next().unwrap().to_string();
        let code = _code.as_str();
        let _general_lang = if code.len() == 3 {
            return Language::from_639_3(code).unwrap_or(Language::Und);
        } else {
            return Language::from_639_1(code).unwrap_or(Language::Und);
        };
    }
    fn bit_rate(&self) -> u32 {
        let bps = self.stream.tags.bps.clone();
        return bps.or(Some(0.to_string())).unwrap().parse().unwrap();
    }
    fn pix_fmt(&self, vs: bool) -> String {
        let pix_fmt = &self.stream.pix_fmt;
        if pix_fmt.is_none() {
            return String::new();
        }
        if !vs {
            return pix_fmt.clone().unwrap();
        } else {
            let mut upper = pix_fmt.clone().unwrap().to_uppercase();
            if upper.ends_with("P") {
                upper.push('8');
            }
            if upper == "XYZ12LE" {
                upper = upper.replace("LE", "");
            }
            return upper;
        }
    }
    fn ratio(&self) -> f64 {
        let stream = &self.stream;
        let dar = stream.display_aspect_ratio.clone().unwrap_or(String::new());
        // very convoluted
        #[rustfmt::skip]
        let (width, height) = if dar != String::new() {
            let (a, b) = dar.split(":").collect_tuple().unwrap();
            (a.to_string(), b.to_string())
        } else {
            (stream.width.unwrap().to_string(), stream.height.unwrap().to_string())
        };
        width.parse::<f64>().unwrap() / height.parse::<f64>().unwrap()
    }
    fn fps(&self) -> f64 {
        let stream = &self.stream;
        // pray that this always works
        #[rustfmt::skip]
        let (numerator, denominator) = stream.avg_frame_rate.as_ref().unwrap().split("/").collect_tuple().unwrap();
        numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap()
    }
    fn color_data(&self, rav1e: bool) -> (String, String, String, String) {
        let stream = self.stream.clone();
        let range = stream.color_range.unwrap_or("tv".to_string());
        let matrix = stream.color_space.unwrap_or("bt709".to_string());
        let transfer = stream.color_transfer.unwrap_or("bt709".to_string());
        let primaries = stream.color_primaries.unwrap_or("bt709".to_string());
        if rav1e {
            let rav1e_range = phf_map! {
                "tv" => "limited",
                "mpeg" => "limited",
                "limited" => "limited",
                "pc" => "full",
                "jpeg" => "full",
                "full" => "full",
            };
            #[rustfmt::skip]
            return (rav1e_range.get(range.as_str()).unwrap().to_string(),matrix,transfer,primaries);
        } else {
            let svt_range = phf_map! {
                "tv" => "0",
                "mpeg" => "0",
                "Limited" => "0",
                "pc" => "1",
                "jpeg" => "1",
                "Full" => "1",
            };
            let svt_matrix = phf_map! {
                "identity" => "0",
                "bt709" => "1",
                "fcc" => "4",
                "bt470bg" => "5",
                "bt601" => "6",
                "smpte240" => "7",
                "ycgco" => "8",
                "bt2020ncl" => "9",
                "bt2020cl" => "10",
                "smpte2085" => "11",
                "chromatncl" => "12",
                "chromatcl" => "13",
                "ictcp" => "14",
            };
            let svt_transfer = phf_map! {
                "bt709" => "1",
                "bt470m" => "4",
                "bt470bg" => "5",
                "bt601" => "6",
                "smpte240" => "7",
                "linear" => "8",
                "log100" => "9",
                "log100sqrt10" => "10",
                "iec61966" => "11",
                "bt1361" => "12",
                "SRGB" => "13",
                "bt2020_10bit" => "14",
                "bt2020_12bit" => "15",
                "smpte2084" => "16",
                "smpte428" => "17",
                "hlg" => "18",
            };
            let svt_primaries = phf_map! {
                "bt709" => "1",
                "bt470m" => "4",
                "bt470bg" => "5",
                "bt601" => "6",
                "smpte240" => "7",
                "genericfilm" => "8",
                "bt2020" => "9",
                "xyz" => "10",
                "smpte431" => "11",
                "smpte432" => "12",
                "ebu3213" => "22",
            };
            #[rustfmt::skip]
            return (svt_range.get(range.as_str()).unwrap().to_string(),svt_matrix.get(matrix.as_str()).unwrap().to_string(),svt_transfer.get(transfer.as_str()).unwrap().to_string(),svt_primaries.get(primaries.as_str()).unwrap().to_string());
        }
    }
}

fn get_binary(path: &str) -> PathBuf {
    return which(path).expect(format!("Couldn't find {path} in PATH").as_str());
}

fn main() {
    let args = Args::parse();
    process_command(args);
}

#[rustfmt::skip]
fn ffprobe(file: &PathBuf) -> FileProbe {
    let mut ffprobe: Vec<u8> = Vec::new();
    let ffprobe_save = PathBuf::from(format!("{}.ffprobe", file.as_path().display()));
    if ffprobe_save.try_exists().is_ok_and(|b| b == true) {
        File::open(ffprobe_save).unwrap().read_to_end(&mut ffprobe).unwrap();
    } else {
        ffprobe = Command::new("ffprobe")
            .args(["-v","error","-print_format","json","-show_streams","-hide_banner","-i",file.to_str().unwrap()])
            .output()
            .unwrap().stdout;
        File::create(ffprobe_save).unwrap().write_all(&ffprobe).unwrap();
    }
    let out = str::from_utf8(&ffprobe).unwrap();
    serde_json::from_str(out).unwrap()
}

fn match_episode(file_name: &OsString, episode_number: String, season: String) -> bool {
    let temp_str = file_name.to_str().unwrap();
    let patterns = [
        Regex::new(format!("(?i)S{}E{}", season, episode_number).as_str()).unwrap(),
        Regex::new(format!("(?i)(?<!\\d)\\b{}\\b(?!\\d)", episode_number).as_str()).unwrap(),
    ];
    let mut regex_matched = false;
    for pattern in patterns {
        let result = pattern.captures(temp_str);
        if result.is_err() || result.unwrap().is_none() {
            continue;
        }
        regex_matched = true;
        break;
    }
    return regex_matched;
}

fn check_audio_encoding(input_directory: &PathBuf) -> String {
    let mut opus_string: String = String::new();
    for path in input_directory.read_dir().unwrap() {
        let dir_entry = path.unwrap();
        if dir_entry.path().extension().unwrap() != "opus" {
            continue;
        }
        let ffprobe_input = ffprobe(&dir_entry.path());
        let streams = get_medium_streams(&ffprobe_input, &dir_entry.path(), "audio", None);
        let stream = &streams[0].stream;
        if stream.tags.encoder_options.is_none() {
            continue;
        }
        opus_string = stream.tags.encoder_options.clone().unwrap();
        break;
    }
    return opus_string;
}

#[rustfmt::skip]
fn is_video(file: &PathBuf) -> bool {
    let tmp_str = file.extension().unwrap();
    let video_extensions: Vec<&'static str> = vec!["mkv", "mp4", "webm", "avi", "mov", "ts", "m2t"];
    return video_extensions.iter().any(|extension| tmp_str == *extension);
}

#[rustfmt::skip]
fn is_temporary_file(file: &OsString) -> bool {
    let tmp_str = file.to_str().unwrap();
    let temp_extensions: Vec<&'static str> = vec!["_enc.mkv","_grained.mkv","_lowest.mkv","_low.mkv","_high.mkv","_highest.mkv","_grainy.mkv","_cleaned.mkv","_clip.mkv", ".ffprobe", ".offset", ".ssimu2"];
    return temp_extensions.iter().any(|extension| tmp_str.ends_with(extension));
}

#[rustfmt::skip]
fn extract_episode_number(base: &OsStr, pattern: String, season: Option<String>) -> Result<String, String> {
    let temp_str = base.to_str().unwrap();
    if pattern == "1" || pattern == "2" {
        let patterns = [
            Regex::new(format!("(?i)S{}E(\\d{{2}})(?!\\d)", season.as_ref().unwrap()).as_str()).unwrap(),
            Regex::new(r" - (\d{2})(?!\d)").unwrap(),
            Regex::new(r"(?i)[^S]\d{2}(?!\d)").unwrap(),
        ];
        let mut regex_match: Option<String> = None;
        for pattern in patterns {
            let result = pattern.captures(temp_str);
            if result.is_err() || result.as_ref().unwrap().is_none() { continue; }
            let caps = result.unwrap().unwrap();
            regex_match = Some(caps.get(1).unwrap().as_str().to_owned());
            break;
        }
        if regex_match.is_none() {
            return Err("Failed to find episode number!".to_string());
        }
        if pattern == "2" {
            let formatted_episode = format!("S{}E{}", season.as_ref().unwrap(), regex_match.unwrap());
            return Ok(formatted_episode);
        } else {
            return Ok(regex_match.unwrap());
        }
    } else {
        return Ok(pattern.clone());
    }
}

fn enc_opus(source: &PathBuf, stream: &mut Probe, bitrate: &str) {
    let s = &stream.stream;
    let index = s.index;
    let lang = stream.language().to_639_3();
    let mut audio_path = source.clone();
    audio_path.set_extension(format!("{index}.{lang}.opus"));
    stream.file = audio_path.clone();
    if s.start_pts != 0 {
        stream.offset += s.start_pts.clone() as u32;
    }
    if audio_path.try_exists().is_ok_and(|r| r == false) {
        #[rustfmt::skip]
        let mut flac_pipe = Command::new(get_binary("ffmpeg"))
            .args(["-i",source.to_str().unwrap(),"-map",format!("0:{index}").as_str(),"-v","16","-hide_banner","-f","flac","-"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("FFmpeg broken pipe!");
        let mut opusenc = Command::new(get_binary("opusenc"))
            .args(["--bitrate", bitrate, "-", audio_path.to_str().unwrap()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("opusenc encode failed!");
        if let Some(ref mut stdout) = flac_pipe.stdout {
            if let Some(ref mut stdin) = opusenc.stdin {
                let buf = BufReader::new(stdout);
                let writer = BufWriter::new(stdin);
                futures::executor::block_on(futures::io::copy(
                    &mut asyncio::new(buf),
                    &mut asyncio::new(writer),
                ))
                .unwrap();
            }
        }
        opusenc.wait().unwrap();
    }
}

#[rustfmt::skip]
fn get_medium_streams(ffprobe_input: &FileProbe, file_path: &PathBuf, medium: &str, offset: Option<u32>) -> Vec<Probe> {
    let result = ffprobe_input.streams.iter().filter(|s| s.codec_type == medium).map(|s| Probe {stream: s.clone(),file: file_path.clone(),offset: offset.unwrap_or(0),index: None});
    return Vec::from_iter(result);
}

#[rustfmt::skip]
fn compare_streams(probe1: Probe, probe2: Probe) -> Probe {
    let stream1 = &probe1.stream;
    let stream2 = &probe2.stream;
    if stream1.codec_type == "audio" {
        let channels1 = stream1.channels.unwrap();
        let channels2 = stream2.channels.unwrap();
        if channels1 != channels2 {
            return if channels1 > channels2 { probe1 } else { probe2 };
        }
        // TODO: figure out what codec is PCM, Wave64, or Wave
        let codec_priority: Vec<&'static str> = vec!["mlp", "truehd", "flac", "PCM", "Wave64", "Wave", "eac3", "aac", "opus", "ac3", "vorbis", "mp3", "mp2", "mp1"];
        let codec1_piority = codec_priority.iter().position(|c| *c == stream1.codec_name).unwrap_or(14);
        let codec2_piority = codec_priority.iter().position(|c| *c == stream2.codec_name).unwrap_or(14);
        if codec1_piority != codec2_piority {
            return if codec1_piority < codec2_piority { probe1 } else { probe2 };
        }
        let bps1 = probe1.bit_rate();
        let bps2 = probe2.bit_rate();
        if bps1 != bps2 {
            return if bps1 > bps2 { probe1 } else { probe2 };
        }
        return probe1;
    } else {
        let codec_priority: Vec<&'static str> = vec!["ass", "subrip", "hdmv_pgs_subtitle"];
        let codec1_piority = codec_priority.iter().position(|c| *c == stream1.codec_name).unwrap_or(2);
        let codec2_piority = codec_priority.iter().position(|c| *c == stream2.codec_name).unwrap_or(2);
        return if codec1_piority < codec2_piority { probe2 } else { probe1 };
    }
}

fn get_title(lang: &Language, title: &String) -> String {
    let final_title;
    let re = Regex::new(r"(?i)(\(([a-z]| |_)+\)|Forced|Dub|Simplified|Traditional)").unwrap();
    let mut tag: String = "".to_string();
    let re_success = re.find(title.as_str());
    if re_success.is_ok() && re_success.clone().unwrap().is_some() {
        tag = re_success.unwrap().unwrap().as_str().to_string();
        if tag.chars().next().is_some_and(|t| t != '(') {
            tag.push(')');
            tag.insert(0, '(');
        }
        tag.insert(0, ' ');
    }
    final_title = format!("{}{tag}", lang.to_name());
    return final_title;
}

fn filter_redundant_tracks(streams: &mut Vec<Probe>) -> Vec<Probe> {
    let mut unique_tracks: HashMap<Track, Probe> = HashMap::new();
    for stream in streams {
        let s = stream.stream.clone();
        let origin_lang = stream.language();
        let origin_title = s.tags.title.clone().unwrap_or("".to_string());
        let new_title = get_title(&origin_lang, &origin_title);
        let _ = stream.stream.tags.title.insert(new_title.clone());
        let key = Track {
            language: origin_lang,
            title: new_title,
            forced: s.disposition.forced == 1,
        };
        if unique_tracks.keys().find(|e| **e == key).is_none() {
            unique_tracks.insert(key, stream.clone());
        } else {
            let stream2 = unique_tracks.get(&key).unwrap();
            let winner = compare_streams(stream.clone(), stream2.clone());
            unique_tracks.remove_entry(&key);
            unique_tracks.insert(key, winner);
        }
    }
    return Vec::from_iter(unique_tracks.values().cloned());
}

#[rustfmt::skip]
fn get_offset(file_path: &PathBuf, src2_path: &PathBuf) -> u32 {
    println!("Determining offsets for {}", src2_path.display());
    let ref_clip = file_path.parent().unwrap().join(format!("{}_clip.mkv",file_path.file_stem().unwrap().to_str().unwrap()));
    let src_clip = src2_path.parent().unwrap().join(format!("{}_clip.mkv",src2_path.file_stem().unwrap().to_str().unwrap()));
    let offset_save = PathBuf::from(format!("{}.offset", src2_path.display()));
    let offset: f32;
    if offset_save.try_exists().is_ok_and(|b| b == true) {
        let mut temp: String = String::new();
        File::open(offset_save).unwrap().read_to_string(&mut temp).unwrap();
        offset = temp.parse().unwrap();
    } else {
        let start = "0".to_string();
        let duration = "60".to_string();
        if ref_clip.try_exists().is_ok_and(|v| v==false) {
            Command::new(get_binary("ffmpeg"))
                .args(["-hide_banner", "-loglevel", "error", "-ss", start.as_str(), "-i", file_path.to_str().unwrap(), "-t", duration.as_str(), "-c:V", "libx264", "-q", "0", ref_clip.to_str().unwrap()])
                .output().unwrap();
        }
        if src_clip.try_exists().is_ok_and(|v| v==false) {
            Command::new(get_binary("ffmpeg"))
                .args(["-hide_banner", "-loglevel", "error", "-ss", start.as_str(), "-i", src2_path.to_str().unwrap(), "-t", duration.as_str(), "-c:V", "libx264", "-q", "0", src_clip.to_str().unwrap()])
                .output().unwrap();
        }
        let position_info = Command::new(get_binary("ffmpeg"))
            .args(["-i", ref_clip.to_str().unwrap(), "-i", src_clip.to_str().unwrap(), "-filter_complex", "signature=detectmode=fast:nb_inputs=2:th_xh=50", "-f", "null", "-"])
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output().unwrap();
        let re = Regex::new(r"(?i)matching of video 0 at ([0-9]+\.[0-9]+) and 1 at ([0-9]+\.[0-9]+)").unwrap();
        let result = re.captures(core::str::from_utf8(&position_info.stderr).unwrap())
            .expect("Failed to load regex")
            .expect("Failed to determine offsets");
        offset = result.get(1).unwrap().as_str().parse::<f32>().unwrap() - result.get(2).unwrap().as_str().parse::<f32>().unwrap();
        File::create(offset_save).unwrap().write_fmt(format_args!("{offset}")).unwrap();
    }
    return (offset * 1000.0) as u32
}

#[rustfmt::skip]
fn get_info(file_path: &PathBuf, src2_paths: &Option<PathBuf>, args: &Args) -> (Vec<Probe>,Vec<Probe>,Vec<Probe>) {
    println!("Collecting video information for {}", file_path.display());
    let ffprobe_input = ffprobe(file_path);
    let mut video_streams = get_medium_streams(&ffprobe_input, &file_path, "video", None);
    let mut audio_streams = Vec::new();
    if args.audio == "1" || args.audio == "both" {
        audio_streams = get_medium_streams(&ffprobe_input, &file_path, "audio", None);
        for mut stream in &mut audio_streams {
            let audio = &stream.stream;
            let channels = audio.channels.unwrap();
            let bps: u32 = stream.bit_rate();
            if (channels < 6 && bps == 0) || (channels < 6 && bps > 128000) {
                enc_opus(&file_path, &mut stream, "128");
                stream.stream.index = 0;
                stream.stream.tags.bps = Some("128000".to_string());
            } else if (channels == 6 && bps == 0) || (channels == 6 && bps > 256000) {
                enc_opus(&file_path, &mut stream, "256");
                stream.stream.index = 0;
                stream.stream.tags.bps = Some("256000".to_string());
            } else if (channels > 6 && bps == 0) || (channels < 6 && bps > 320000) {
                enc_opus(&file_path, &mut stream, "320");
                stream.stream.index = 0;
                stream.stream.tags.bps = Some("320000".to_string());
            }
        }
    }
    let mut subtitle_streams = Vec::new();
    if args.subs == "1" || args.subs == "both" {
        subtitle_streams = get_medium_streams(&ffprobe_input, &file_path, "subtitle", None);
    }
    if args.audio == "2" || args.audio == "both" || args.subs == "2" || args.subs == "both" {
        for path in src2_paths.clone().unwrap().read_dir().unwrap() {
            let dir_entry = path.unwrap();
            if is_temporary_file(&dir_entry.file_name()) {
                continue;
            }
            let ffprobe_input = ffprobe(&dir_entry.path());
            let mut v_streams = get_medium_streams(&ffprobe_input, &dir_entry.path(), "video", None);
            let video_stream = v_streams.get(0);
            let offset;
            if args.sync != 0 {
                offset = args.sync;
            } else if video_stream.is_some() {
                offset = get_offset(&file_path, &dir_entry.path());
            } else {
                offset = 0;
            }
            println!("{offset}");
            if args.lehmer_merge {
                v_streams = get_medium_streams(&ffprobe_input, &dir_entry.path(), "video", Some(offset));
                video_streams.append(&mut v_streams);
            }
            if args.audio == "2" || args.audio == "both" {
                let mut a_streams = get_medium_streams(&ffprobe_input, &dir_entry.path(), "audio", Some(offset));
                for mut stream in &mut audio_streams {
                    let audio = &stream.stream;
                    let channels = audio.channels.unwrap();
                    let bps: u32 = stream.bit_rate();
                    if (channels < 6 && bps == 0) || (channels < 6 && bps > 128000) {
                        enc_opus(&file_path, &mut stream, "128");
                        stream.stream.index = 0;
                        stream.stream.tags.bps = Some("128000".to_string());
                    } else if (channels == 6 && bps == 0) || (channels == 6 && bps > 256000) {
                        enc_opus(&file_path, &mut stream, "256");
                        stream.stream.index = 0;
                        stream.stream.tags.bps = Some("256000".to_string());
                    } else if (channels > 6 && bps == 0) || (channels < 6 && bps > 320000) {
                        enc_opus(&file_path, &mut stream, "320");
                        stream.stream.index = 0;
                        stream.stream.tags.bps = Some("320000".to_string());
                    }
                }
                audio_streams.append(&mut a_streams);
            }
            if args.subs == "2" || args.subs == "both" {
                let mut s_streams = get_medium_streams(&ffprobe_input, &dir_entry.path(), "subtitle", Some(offset));
                subtitle_streams.append(&mut s_streams);
            }
        }
    }
    audio_streams = filter_redundant_tracks(&mut audio_streams);
    let audio_order: Vec<&'static str> = vec!["ja", "en", "es", "ar", "fr", "de", "it", "pt", "pl", "nl", "nb", "fi", "tr", "sv", "el", "he", "ro", "id", "th", "ko", "da", "zh", "vi", "uk", "ru", "hu", "cs", "hr", "ms", "hi"];
    audio_streams.sort_by(|a, b| {audio_order.iter().position(|l| *l == a.language().to_639_1().unwrap()).unwrap_or(audio_order.len()).cmp(&audio_order.iter().position(|l| *l == b.language().to_639_1().unwrap()).unwrap_or(audio_order.len()))});
    subtitle_streams = filter_redundant_tracks(&mut subtitle_streams);
    let sub_order: Vec<&'static str> = vec!["en", "es", "ar", "fr", "de", "it", "ja", "pt", "pl", "nl", "nb", "fi", "tr", "sv", "el", "he", "ro", "id", "th", "ko", "da", "zh", "vi", "uk", "ru", "hu", "cs", "hr", "ms", "hi"];
    subtitle_streams.sort_by(|a, b| {sub_order.iter().position(|l| *l == a.language().to_639_1().unwrap()).unwrap_or(sub_order.len()).cmp(&sub_order.iter().position(|l| *l == b.language().to_639_1().unwrap()).unwrap_or(audio_order.len()))});
    // obnoxiously long sort, TODO: make readable
    let mut ainfo: Vec<Probe> = Vec::new();
    let mut sinfo: Vec<Probe> = Vec::new();
    let mut vinfo: Vec<Probe> = Vec::new();
    let mut file_to_source_map: HashMap<PathBuf, u8> = HashMap::new();
    let mut source_index: u8 = 0;
    for (idx, entry) in audio_streams.iter().enumerate() {
        if !file_to_source_map.contains_key(&entry.file.to_path_buf()) {
            file_to_source_map.insert(entry.file.to_path_buf(), source_index);
            source_index += 1;
        }
        let mut info = entry.clone();
        info.index = Some(*file_to_source_map.get(&info.file.to_path_buf()).unwrap());
        ainfo.insert(idx, info.clone());
    }
    file_to_source_map.clear();
    let next_index: u8 = source_index;
    source_index = 0;
    for (idx, entry) in subtitle_streams.iter().enumerate() {
        if !file_to_source_map.contains_key(&entry.file.to_path_buf()) {
            file_to_source_map.insert(entry.file.to_path_buf(), source_index);
            source_index += 1;
        }
        let mut info = entry.clone();
        info.index = Some(*file_to_source_map.get(&info.file.to_path_buf()).unwrap() + next_index);
        sinfo.insert(idx, info.clone());
    }
    for (idx, entry) in video_streams.iter().enumerate() {
        vinfo.insert(idx, entry.clone());
    }
    (vinfo, ainfo, sinfo)
}

fn get_encoder_version(encoder: &str) -> Result<String, String> {
    if encoder == "rav1e" {
        let output = Command::new(get_binary("rav1e"))
            .arg("-V")
            .output()
            .map_err(|_| "Failed to get encoder version!");
        #[rustfmt::skip]
        return Ok(format!("rav1e v{}", String::from_utf8(output.unwrap().stdout).unwrap().split(" ").nth(1).unwrap().to_string()));
    } else if encoder == "svt-av1" {
        let output = Command::new(get_binary("SvtAv1EncApp"))
            .arg("--version")
            .output()
            .map_err(|_| "Failed to get encoder version!");
        #[rustfmt::skip]
        return Ok(format!("svt-av1-psy {}", String::from_utf8(output.unwrap().stdout).unwrap().split(' ').nth(1).unwrap().to_string()));
    } else if encoder == "opusenc" {
        let output = Command::new(get_binary("opusenc"))
            .arg("--version")
            .output()
            .map_err(|_| "Failed to get encoder version!");
        let mut result = String::from_utf8(output.unwrap().stdout).unwrap();
        result = result.split("libopus").nth(1).unwrap().to_string();
        result = result.split(")").nth(0).unwrap().to_string();
        return Ok(result);
    } else {
        return Err("Encoder not supported!".to_string());
    }
}

#[rustfmt::skip]
fn get_encoder_params(args: &Args, vinfo: &Vec<Probe>, speed: Option<u8>, quantizer: Option<f32>, encoder: Option<&str>, display: bool) -> String {
    let speed = speed.unwrap_or(args.speed);
    let q = quantizer.unwrap_or(args.quantizer);
    let encoder = encoder.unwrap_or(&args.encoder);
    let range = quantizer_range(args.quantizer_range.clone(), encoder.to_string());
    let q_display = format!("{:.1}-{:.1}", range[0], range[1]);
    let quantizer = if display {
        q_display
    } else {
        q.to_string()
    };
    let params = format!(" {}", args.parameters.as_deref().unwrap_or(" ".into()));
    let (cr, matrix, transfer, primaries) = vinfo[0].color_data(args.encoder == "rav1e");
    let result = if encoder == "svt-av1" {
        format!("--crf {quantizer}{params} --preset {speed} --tune 3 --sharpness 2 --variance-boost-strength 4 --variance-octile 4 --frame-luma-bias 100 --keyint 0 --enable-dlf 2 --enable-cdef 0 --enable-restoration 0 --enable-tf 0 --color-range {cr} --matrix-coefficients {matrix} --transfer-characteristics {transfer} --color-primaries {primaries}")
    } else if encoder == "rav1e" {
        let tiles = args.tiles;
        format!("--quantizer {quantizer}{params} -s {speed} --tiles {tiles} --keyint 0 --no-scene-detection --range {cr} --matrix {matrix} --transfer {transfer} --primaries {primaries}")
    } else if encoder == "x264" {
        format!("-q 0")
    } else {
        String::new()
    };
    if result.is_empty() {
        panic!("Unsupported encoder!");
    }
    return result;
}

fn get_grain_string(args: &Args) -> String {
    if args.diff_grain {
        return if args.lehmer_merge {
            "diff + lehmer merge with vs-denoise: \"lowpass = lambda i: box_blur(i, passes=2)\""
                .to_string()
        } else {
            "diff".to_string()
        };
    } else {
        return format!("--iso {}", args.photon_noise);
    }
}

fn get_denoise_string(args: &Args) -> String {
    let mut denoise_string = format!(
        "strength={}, tr=2, sr=[3,2,2], planes=[0,1,2]",
        args.denoise
    );
    if args.ref_calc {
        denoise_string.push_str(", ref=MVToolsPresets.FAST");
    }
    return denoise_string;
}

fn get_filter_string(args: &Args) -> String {
    let mut filter_string = String::new();
    if !args.no_denoise {
        filter_string = format!("Denoise with vs-denoise: \"{}\"", get_denoise_string(&args));
    }
    if args.dehalo {
        if !filter_string.is_empty() {
            filter_string.push_str(", dering with vs-dehalo: \"planes=[0,1,2]\"");
        } else {
            filter_string = String::from("Dering with vs-dehalo: \"planes=[0,1,2]\"");
        }
    }
    if !filter_string.is_empty() {
        filter_string.push_str(", deband with vs-deband");
    } else {
        filter_string = String::from("Deband with vs-deband");
    }
    if args.retinex {
        filter_string.push_str(", grain=0\" + retinex mask: \"rg_mode=0");
    }
    filter_string.push_str("\", dither with vs-tools");
    return filter_string;
}

fn get_rescale_string(args: &Args) -> String {
    let mut rescale_string = String::new();
    if args.rescale {
        rescale_string = String::from("Rescale with vodesfunc: ");
        if args._match {
            rescale_string = format!(
                "{rescale_string}\"native res with lvsfunc: \"target_height={}, target_width={}\"",
                args.height.unwrap(),
                args.width.unwrap()
            );
        } else {
            rescale_string = format!("{rescale_string}\"height={}, width={}", args.height.unwrap(), args.width.unwrap());
        }
        rescale_string = format!("{rescale_string}, kernel={}, border_handling={}\", upscale with vs-scale: \"Waifu2x\", downscale with vs-kernels: \"Hermite(linear=True)\"", args.algo.as_ref().unwrap(), args.borders);
    }
    return rescale_string;
}

#[rustfmt::skip]
fn get_source_string(file: &PathBuf, args: &Args, format: Option<String>) -> String {
    if args.source_filter == "lsmash" {
        let pass1 = format!("lsmas.LWLibavSource(r'{}', cachedir=r'{}', prefer_hw=3", file.display(), args.input_directory.display());
        if format.is_some() {
            format!("{pass1}, format='{}')", format.unwrap())
        } else {
            pass1 + ")"
        }
    } else if args.source_filter == "bestsource" {
        let abspath = abs(&args.input_directory).unwrap();
        let mut root = abspath.components().next().unwrap().as_os_str().to_string_lossy().to_string();
        if !root.ends_with('/') {
            root.push('/');
        }
        format!("bs.VideoSource(r'{}', cachepath=r'{}')", abs(&file).unwrap().display(), root)
    } else {
        format!("dgdecodenv.DGSource(r'{}')", file.display())
    }
}

#[rustfmt::skip]
fn sd_script(vpy_path: &PathBuf, args: &Args, vinfo: &Vec<Probe>) {
    let mut file = File::create(vpy_path).unwrap();
    let source_string = get_source_string(&vinfo[0].file, &args, Some(vinfo[0].pix_fmt(true)));
    let contents = format!("import vapoursynth as vs\ncore = vs.core\nsrc = core.{source_string}\n# clip1 = src[1004:10893]\n# clip2 = src[11194:44161]\n# src = clip1+clip2\n# src = core.vivtc.VFM(src, 1, mode=3) # 60i to 30p\n# src = core.vivtc.VDecimate(src, 5) # 30p to 24p\nsrc.set_output(0)");
    file.write_all(contents.as_bytes()).unwrap();
}

fn get_descale_dimensions(height: &Option<u16>, width: &Option<u16>) -> (u16, u16) {
    if height.is_some() && width.is_none() {
        (height.unwrap(), (height.unwrap() as f64 * 16f64/9f64) as u16)
    } else if width.is_some() && height.is_none() {
        ((width.unwrap() as f64 * 9f64/16f64) as u16, width.unwrap())
    } else {
        (height.unwrap(), width.unwrap())
    }
}

#[rustfmt::skip]
fn create_vpy_script(vpy_path: &PathBuf, file_path: &PathBuf, args: &Args, vinfo: &Vec<Probe>) {
    let mut file = File::create(vpy_path).unwrap();
    let source_string = get_source_string(&vinfo[0].file, &args, Some(vinfo[0].pix_fmt(true)));
    let mut contents = format!("import vapoursynth as vs\nfrom vstools import initialize_clip, depth\nimport lvsfunc as lvs\nfrom vodesfunc import RescaleBuilder\nimport vskernels as vsk\nfrom vsscale import Waifu2x\nfrom vsdenoise import nl_means, MVTools, MVToolsPresets\nfrom vsdehalo import fine_dehalo\nfrom vsdeband import F3kdb, masked_deband\ncore = vs.core\ncore.max_cache_size = {}\nsrc = core.{source_string}\n# clip1 = src[1004:10893]\n# clip2 = src[11194:44161]\n# src = clip1+clip2\n# src = core.vivtc.VFM(src, 1, mode=3) # 60i to 30p\n# src = core.vivtc.VDecimate(src, 5) # 30p to 24p\nsrc = initialize_clip(src)\n", args.mem as u32 * 1024);
    let (descale_height, descale_width) = get_descale_dimensions(&args.height, &args.width);
    if args.rescale {
        let mut rescale_string = if args._match {
            let target_string = format!("target_height={descale_height}, target_width={descale_width},");
            contents = format!("{contents}native_res = lvs.get_match_centers_scaling(src, {target_string}) # Disable for integer scaling and set height in DescaleTarget\n");
            format!("**native_res")
        } else {
            format!("height={descale_height}, width={descale_width}")
        };
        rescale_string = format!("{rescale_string}, kernel=vsk.{}, border_handling={}, upscaler=Waifu2x(cuda=\"trt\", fp16={}, tiles={}), downscaler=vsk.Hermite(linear=True)", args.algo.as_ref().unwrap(), args.borders, args.fp16, args.dstiles);
        if args.shift.is_some() {
            rescale_string = format!("{rescale_string}, shift={}", args.shift.as_ref().unwrap());
        }
        contents = format!("{contents}builder, src = (\nRescaleBuilder(src)\n.descale(vsk.{}(border_handling={}), {rescale_string})\n.double(Waifu2x(cuda=\"trt\", fp16={}, tiles={}))\n.errormask()\n.linemask()\n.downscale(vsk.Hermite(linear=True))\n.final()\n)\n", args.algo.as_ref().unwrap(), args.borders, args.fp16, args.dstiles);
    }
    if !args.no_denoise {
        let mut denoise_string = format!("strength={}, tr=2, sr=[3,2,2], planes=[0,1,2]", args.denoise);
        if args.ref_calc {
            denoise_string = format!("{denoise_string}, ref=MVTools.denoise(src, **MVToolsPresets.FAST)");
        }
        contents = format!("{contents}src = nl_means(src, {denoise_string}) # smaller window size for chroma subsampling\n");
    }
    if args.dehalo {
        contents = format!("{contents}src = fine_dehalo(src, planes=[0,1,2])\n");
    }
    let deband_string: &'static str = if args.retinex {
        "masked_deband(src, grain=0, rg_mode=0"
    } else {
        "F3kdb.deband(src"
    };
    contents = format!("{contents}deband = {deband_string}, thr={}, planes=[0,1,2])\ndown = depth(deband, 10)\ndown.set_output(0)\n# audio = core.bs.AudioSource(r'{}', cachepath=r'{}/')\n# start1 = round(1004*48*1001/30) # Values based on audio sample rate. Multiply video frame number by sample rate in kHz/original framerate\n# end1 = round(10893*48*1001/30)\n# start2 = round(11194*48*1001/30)\n# end2 = round(44161*48*1001/30)\n# a1 = audio[start1:end1]\n# a2 = audio[start2:end2]\n# audio=a1+a2\n# audio.set_output(1)", args.deband, file_path.display(), args.input_directory.display());
    file.write_all(contents.as_bytes()).unwrap();
}

#[rustfmt::skip]
fn multi_script(vpy_path: &PathBuf, args: &Args, vinfo: &Vec<Probe>) {
    let mut vpy_file = File::create(vpy_path).unwrap();
    let source_string = get_source_string(&vinfo[0].file, &args, Some(vinfo[0].pix_fmt(true)));
    let content = format!("import vapoursynth as vs\ncore = vs.core\nsrc = core.{source_string}\n# clip1 = src[1004:10893]\n# clip2 = src[11194:44161]\n# src = clip1+clip2\n# src = core.vivtc.VFM(src, 1, mode=3) # 60i to 30p\n# src = core.vivtc.VDecimate(src, 5) # 30p to 24p\nsrc = src[::{}]\nsrc.set_output(0)\n", args.cycle);
    vpy_file.write_all(content.as_bytes()).unwrap();
}

#[rustfmt::skip]
fn denoise_script(vpy_path: &PathBuf, args: &Args, vinfo: &Vec<Probe>) {
    let mut vpy_file = File::create(vpy_path).unwrap();
    let source_string = get_source_string(&vinfo[0].file, &args, None);
    let mut denoise_string = format!("strength={}, tr=2, sr=[3,2,2], planes=[0,1,2]", args.denoise);
    if args.ref_calc {
        denoise_string = format!("{denoise_string}, ref=MVTools.denoise(src, **MVToolsPresets.FAST)");
    }
    let content = format!("import vapoursynth as vs\nfrom vstools import initialize_clip, depth\nfrom vsdenoise import nl_means, MVTools, MVToolsPresets\ncore = vs.core\ncore.max_cache_size = {}\nsrc = core.{source_string}\n# clip1 = src[1004:10893]\n# clip2 = src[11194:44161]\n# src = clip1+clip2\n# src = core.vivtc.VFM(src, 1, mode=3) # 60i to 30p\n# src = core.vivtc.VDecimate(src, 5) # 30p to 24p\nsrc = initialize_clip(src)\nnlm = nl_means(src, {denoise_string}) # smaller window size for chroma subsampling\ndown = depth(nlm, 10)\ndown.set_output(0)\n", args.mem as u32 * 1024);
    vpy_file.write_all(content.as_bytes()).unwrap();
}

#[rustfmt::skip]
fn merge_script(vpy_path: &PathBuf, args: &Args, vinfo: &Vec<Probe>) {
    let mut vpy_file = File::create(vpy_path).unwrap();
    let (source1_string, source2_string) = (get_source_string(&vinfo[0].file, &args, None), get_source_string(&vinfo[1].file, &args, None));
    let content = format!("import vapoursynth as vs\nfrom vstools import initialize_clip, depth\nfrom vsdenoise import frequency_merge\nfrom vsrgtools import box_blur\ncore = vs.core\nsrc1 = core.{source1_string}\nsrc1 = initialize_clip(src1)\nsrc2 = core.{source2_string}\nsrc2 = initialize_clip(src2)\n# clip1 = src1[1004:10893]\n# clip2 = src1[11194:44161]\n# src1 = clip1+clip2\n# src1 = core.vivtc.VFM(src1, 1, mode=3) # 60i to 30p\n# src1 = core.vivtc.VDecimate(src1, 5) # 30p to 24p\noffset = {} # from get_info\nframerate = src1.fps\n# Calculate the frame offset\noffset_frames = int(offset * framerate / -1000)\n# Conditional slicing based on the offset value\nif offset_frames >= 0:\nsrc2 = src2[offset_frames:]\nelse:\nsrc1 = src1[abs(offset_frames):]\nsrcs = [src1, src2]\nlehmer = frequency_merge(srcs, lowpass = lambda i: box_blur(i, passes=3))\ndown = depth(lehmer, 10)\ndown.set_output(0)\n", vinfo[1].offset);
    vpy_file.write_all(content.as_bytes()).unwrap();
}

#[rustfmt::skip]
fn scene_detection(vpy_path: &PathBuf, encode: &PathBuf, scenes: &PathBuf, temp: &PathBuf, args: &Args, vinfo: &Vec<Probe>) {
    let (cr, matrix, transfer, primaries) = vinfo[0].color_data(false);
    let (quantizer, speed) = (args.quantizer, args.speed);
    Command::new(get_binary("av1an")).args([
        "-i", vpy_path.to_str().unwrap(),
        "-o", encode.to_str().unwrap(), "--temp", temp.to_str().unwrap(),
        "--verbose", "-w", args.workers.to_string().as_str(),
        "--scenes", scenes.to_str().unwrap(), "--sc-only", "--sc-pix-format", vinfo[0].pix_fmt(false).as_str(), "--sc-downscale-height", "720",
        "-e", "svt-av1", "-v", format!("--crf {quantizer} --preset {speed} --tune 3 --sharpness 2 --variance-boost-strength 4 --variance-octile 4 --frame-luma-bias 100 --keyint 0 --enable-dlf 2 --enable-cdef 0 --enable-restoration 0 --enable-tf 0 --color-range {cr} --matrix-coefficients {matrix} --transfer-characteristics {transfer} --color-primaries {primaries}").as_str(),
        "-m", args.source_filter.as_str(), "-c", "mkvmerge", "--pix-format", args.pixel_format.as_str()
    ]).spawn().unwrap().wait().unwrap();
}

fn quantizer_range(range: Option<String>, encoder: String) -> [f32; 2] {
    if range.is_none() {
        if encoder == "rav1e" {
            [40.0, 160.0]
        } else {
            [25.0, 55.0]
        }
    } else {
        serde_json::from_str::<[f32; 2]>(range.unwrap().as_str())
            .expect("Failed to parse quantizer range!")
    }
}

#[rustfmt::skip]
fn calculate_quantizer(args: &Args, modifier: i8) -> f32 {
    let part1: f32 = args.quantizer + args.quantizer_calc * modifier as f32;
    let range = quantizer_range(args.quantizer_range.clone(), args.encoder.clone());
    part1.clamp(range[0], range[1])
}

fn temp_path(file_path: &PathBuf, ext: &str) -> PathBuf {
    let base = file_path.file_stem().unwrap();
    let parent = file_path.parent().unwrap();
    parent.join(format!("{}{}", base.to_str().unwrap(), ext))
}

#[rustfmt::skip]
fn encode_file(scene_detect: &PathBuf, script: &PathBuf, encode: &PathBuf, temp: &PathBuf, scenes: &PathBuf, speed: Option<u8>, quantizer: Option<f32>, encoder: Option<&str>, keep: bool, args: &Args, vinfo: &Vec<Probe>) {
    let input = if args.no_filter {
        scene_detect
    } else {
        script
    };
    let params = get_encoder_params(&args, &vinfo, speed, quantizer, encoder, false);
    let (input, encode, temp, workers, scenes, pf) = (input.to_str().unwrap(), encode.to_str().unwrap(), temp.to_str().unwrap(), args.workers.to_string(), scenes.to_str().unwrap(), vinfo[0].pix_fmt(false));
    let mut args = vec![
        "-i", input,
        "-o", encode, "--temp", temp,
        "--verbose", "--resume", "-w", workers.as_str(),
        "--scenes", scenes, "--sc-pix-format", pf.as_str(), "--sc-downscale-height", "360",
        "-e", encoder.unwrap_or(args.encoder.as_str()), "-v", params.as_str(),
        "-m", args.source_filter.as_str(), "-c", "mkvmerge", "--pix-format", args.pixel_format.as_str()
    ];
    if keep {
        args.push("--keep");
    }
    Command::new(get_binary("av1an")).args(args).spawn().unwrap().wait().unwrap();
}

fn get_ssimulacra2(src: &PathBuf, distorted: &PathBuf, scenes_info: &mut ScenesInfo, quantizer: f32, cycle: u8, cr: &String, matrix: &String, transfer: &String, primaries: &String) {
    let cache = temp_path(distorted, ".ssimu2");
    let results = if cache.try_exists().is_ok_and(|b| b == false) {
        let hi = get_ssimu2(src, distorted, cycle, cr.clone(), matrix.clone(), transfer.clone(), primaries.clone());
        let file = File::create(cache).unwrap();
        serde_json::to_writer(file, &hi).expect("Failed to cache SSIMULCRA2 scores!");
        hi
    } else {
        let mut file = File::open(cache).expect("Failed to open SSIMULACRA2 cache!");
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        serde_json::from_str(contents.as_str()).unwrap()
    };
    let filtered: BTreeMap<usize, f64> = results.into_iter().filter(|e| e.1 > 0f64).collect();
    for scene in scenes_info.scenes.iter_mut() {
        let (start, end) = (scene.start_frame, scene.end_frame);
        let scene_scores: BTreeMap<usize, f64> = filtered.to_owned().into_iter().filter(|e| start <= e.0 as u32 && e.0 as u32 <= end).collect();
        if scene_scores.is_empty() { continue; }
        let scores = &mut scene.quantizer_scores;
        if scores.is_none() {
            let _ = scores.insert(HashMap::new());
        }
        let mut data = statrs::statistics::Data::new(scene_scores.values().copied().collect::<Vec<f64>>());
        let (mean, median, std_dev, p5, p95) = (data.mean().unwrap(), data.median(), data.std_dev().unwrap(), data.percentile(5), data.percentile(95));
        let score_data = QuantizerScores { mean: (mean), median: (median), std_dev: (std_dev), percentile_5th: p5, percentile_95th: p95 };
        let mut new = scores.clone().unwrap();
        new.insert(quantizer as usize, score_data);
        let _ = scores.insert(new);
    }
}

fn zone_overrides(
    scenes_info: &mut ScenesInfo,
    scenes_path: &PathBuf,
    scenes_over: &PathBuf,
    args: &Args,
    cr: &String,
    matrix: &String,
    transfer: &String,
    primaries: &String,
) {
    let mut quantizers: Vec<f64> = Vec::new();
    let mut mean_values: Vec<f64> = Vec::new();
    for scene in &mut scenes_info.scenes {
        for (quantizer, data) in scene.quantizer_scores.as_ref().unwrap() {
            quantizers.push(quantizer.clone() as f64);
            if args.encoder == "rav1e" {
                mean_values.push(data.mean + 2.);
            } else {
                mean_values.push(data.mean + 1.);
            }
        }
        let mean_corr = polyfit(&mean_values, &quantizers, 3).unwrap();
        let q_range = quantizer_range(args.quantizer_range.clone(), args.encoder.clone());
        let q = if !mean_corr.iter().all(|f| *f == 0.) {
            let polynomial = polynomial::Polynomial::new(mean_corr);
            (polynomial.eval(args.target_quality as f64) as f32).clamp(q_range[0], q_range[1])
        } else {
            q_range[1]
        };
        if args.encoder == "rav1e" {
            scene.final_quantizer = Some((q as i8) as f32);
        } else {
            scene.final_quantizer = Some((q * 4.).round() / 4.);
        }
        quantizers.clear();
        mean_values.clear();
    }
    let scenes_o_read = File::open(scenes_path).unwrap();
    let mut scenes_o: ScenesInfo = serde_json::from_reader(scenes_o_read).unwrap();
    for scene in scenes_info.scenes.clone() {
        let q_32 = scene.final_quantizer.unwrap();
        let (q, speed, tiles) = (
            q_32.to_string(),
            args.speed.to_string(),
            args.tiles.to_string(),
        );
        for scene_o in &mut scenes_o.scenes {
            if scene_o.start_frame != scene.start_frame && scene_o.end_frame != scene.end_frame {
                continue;
            }
            if args.encoder == "rav1e" {
                let params: Vec<&str> = vec_into![
                    "--quantizer", q.as_str(),
                    "-s", speed.as_str(),
                    "--tiles", tiles.as_str(),
                    "--keyint", "0",
                    "--no-scene-detection",
                    "--range", cr.as_str(),
                    "--matrix", matrix.as_str(),
                    "--transfers", transfer.as_str(),
                    "--primaries", primaries.as_str()
                ];
                scene_o.zone_overrides = Some(ZoneOverrides {
                    encoder: "rav1e".to_string(),
                    passes: 1,
                    video_params: params.into_iter().map(|a| a.to_string()).collect(),
                    photon_noise: None,
                    extra_split_sec: 10,
                    min_scene_len: 24,
                });
                break;
            } else {
                let params: Vec<&str> = vec![
                    "--crf", q.as_str(),
                    "--preset", speed.as_str(),
                    "--tune", "3",
                    "--sharpness", "2",
                    "--variance-boost-strength", "4",
                    "--variance-octile", "4",
                    "--frame-luma-bias", "100",
                    "--keyint", "0",
                    "--enable-dlf", "2",
                    "--enable-cdef", "0",
                    "--enable-restoration", "0",
                    "--enable-tf", "0",
                    "--color-range", cr.as_str(),
                    "--matrix-coefficients", matrix.as_str(),
                    "--transfer-characteristics", transfer.as_str(),
                    "--color-primaries", primaries.as_str(),
                ];
                scene_o.zone_overrides = Some(ZoneOverrides {
                    encoder: "svt_av1".to_string(),
                    passes: 1,
                    video_params: params.into_iter().map(|a| a.to_string()).collect(),
                    photon_noise: None,
                    extra_split_sec: 10,
                    min_scene_len: 24,
                });
                break;
            }
        }
    }
    let writer = File::create(scenes_over).unwrap();
    serde_json::to_writer(writer, &scenes_o).unwrap();
}

#[rustfmt::skip]
fn add_grain_table(encode: &PathBuf, grained: &PathBuf, photon_noise: u16) {
    Command::new(get_binary("grav1synth"))
        .args([
            "generate", encode.to_str().unwrap(),
            "-o", grained.to_str().unwrap(),
            "--iso", photon_noise.to_string().as_str(),
        ])
        .spawn().unwrap().wait().unwrap();
}

fn grain_chunks(
    grainy_dir: &PathBuf,
    cleaned_dir: &PathBuf,
    encode_dir: &PathBuf,
    grained_dir: &PathBuf,
    chunk: &String,
) {
    let grainy = abs(grainy_dir.join(format!("{chunk}.mkv"))).unwrap();
    let cleaned = abs(cleaned_dir.join(format!("{chunk}.ivf"))).unwrap();
    let gtable = abs(grainy_dir.join(format!("{chunk}_table.txt"))).unwrap();
    let encode = abs(encode_dir.join(format!("{chunk}.ivf"))).unwrap();
    let grained = abs(grained_dir.join(format!("{chunk}.ivf"))).unwrap();
    if gtable.try_exists().is_ok_and(|b| b == false) {
        Command::new(get_binary("grav1synth"))
            .args([
                "diff", grainy.to_str().unwrap(), cleaned.to_str().unwrap(),
                "-o", gtable.to_str().unwrap(),
            ]).spawn().unwrap().wait().unwrap();
    }
    if grained.try_exists().is_ok_and(|b| b == false) {
        Command::new(get_binary("grav1synth"))
            .args([
                "apply", encode.to_str().unwrap(),
                "-o", grained.to_str().unwrap(),
                "-g", gtable.to_str().unwrap(),
            ]).spawn().unwrap().wait().unwrap();
    }
}

fn get_diff_grain(
    grainy_temp: &PathBuf,
    cleaned_temp: &PathBuf,
    temp: &PathBuf,
    grained: &PathBuf,
) {
    let grainy_dir = grainy_temp.join("encode");
    let cleaned_dir = cleaned_temp.join("encode");
    let encode_dir = temp.join("encode");
    let grained_dir = temp.join("grained");
    if grained_dir.try_exists().is_ok_and(|b| b == false) {
        std::fs::create_dir_all(&grained_dir).unwrap();
    }
    // absolutely disgusting
    let matching_files = cleaned_dir.read_dir().unwrap().map(|f| {
        f.unwrap().path().file_stem().unwrap().to_string_lossy().to_string()
    });
    for chunk in matching_files {
        grain_chunks(&grainy_dir, &cleaned_dir, &encode_dir, &grained_dir, &chunk);
    }
    let input_files = Vec::from_iter(grained_dir.read_dir().unwrap().map(|f| abs(f.unwrap().path()).unwrap().to_string_lossy().to_string()));
    let mut vec_input: Vec<&str> = input_files.iter().map(|f| &**f).collect();
    let mut args = vec!["mkvmerge", "-o", grained.to_str().unwrap(), "["];
    args.append(&mut vec_input);
    args.append(&mut vec!["]"]);
    Command::new(get_binary("mkvmerge"))
        .args(args)
        .current_dir(&grained_dir)
        .spawn().unwrap().wait().unwrap();
}

fn get_tags(tags_file: &PathBuf, encoder_options: Option<String>, args: &Args) {
    let mut tags = format!("<Tags>\n");
    if !args.single_pass {
        tags = format!("{tags}  <Tag>\n    <Simple>\n      <Name>Target SSIMULACRA 2</Name>\n      <String>Mean: {}</String>\n    </Simple>\n  </Tag>\n", args.target_quality);
    }
    tags = format!("{tags}  <Tag>\n    <Simple>\n      <Name>Encoder settings</Name>\n      <String>{}: \"{}\"</String>\n    </Simple>\n  </Tag>\n", get_encoder_version(args.encoder.clone().as_str()).unwrap(), encoder_options.unwrap());
    if !args.no_grain {
        tags = format!("{tags}  <Tag>\n    <Simple>\n      <Name>Film grain synthesis settings</Name>\n      <String>grav1synth: {}</String>\n    </Simple>\n  </Tag>\n", get_grain_string(&args));
    }
    if !args.no_filter {
        tags = format!("{tags}  <Tag>\n    <Simple>\n      <Name>Vapoursynth filters</Name>\n      <String>{}</String>\n    </Simple>\n  </Tag>\n", get_filter_string(&args));
    }
    if args.rescale {
        tags = format!("{tags}  <Tag>\n    <Simple>\n      <Name>Rescale settings</Name>\n      <String>{}</String>\n    </Simple>\n  </Tag>\n", get_rescale_string(&args));
    }
    tags = format!("{tags}</Tags>");
    let mut file = File::create(tags_file).unwrap();
    file.write_all(tags.as_bytes()).unwrap();
}

fn mux_file(
    video_path: &PathBuf,
    encode: &PathBuf,
    output_path: &PathBuf,
    tags: &PathBuf,
    vinfo: &Vec<Probe>,
    ainfo: &Vec<Probe>,
    sinfo: &Vec<Probe>,
    args: &Args,
) {
    let atracks: Vec<String> = ainfo.iter().map(|p| format!("{}:{}", p.index.unwrap() + 2, p.stream.index)).collect();
    let stracks: Vec<String> = sinfo.iter().map(|p| format!("{}:{}", p.index.unwrap() + 2, p.stream.index)).collect();
    let track_order = [vec!["1:0".to_string()], atracks, stracks].concat().join(",");
    let mut arguments: Vec<String> = vec_into![
        "--output", output_path.to_str().unwrap(),
        "-D", "-A", "-S",
        encode.to_str().unwrap(),
        "--language", "0:und", "--track-name", format!("0:{}", args.raws), "-t", format!("0:{}", tags.display()),
        "--aspect-ratio", format!("0:{}", vinfo[0].ratio()),
        "--default-duration", format!("0:{}p", vinfo[0].fps()), "-A", "-S",
        video_path.to_str().unwrap()
    ];
    let title = vinfo[0].stream.tags.title.as_ref();
    if title.is_some() {
        arguments = [vec_into!["--title", title.unwrap()], arguments].concat();
    }
    let mut audio_files = Vec::new();
    let mut unique_files: HashSet<PathBuf> = HashSet::new();
    for track in ainfo {
        if !unique_files.contains(&track.file) {
            unique_files.insert(track.file.clone());
            audio_files.push(track.file.clone());
        }
    }
    for path in audio_files {
        let mut audio_tracks = Vec::new();
        for track in ainfo {
            if track.file == path {
                audio_tracks.push(track.stream.index);
            }
        }
        let audio_tracks_str = audio_tracks.iter().join(",");
        arguments.append(&mut vec_into!["-a", audio_tracks_str, "-D", "-S"]);
        for track in ainfo {
            if track.file == path {
                arguments.append(&mut vec_into!["--track-name", format!("{}:{}", track.stream.index, track.stream.tags.title.as_ref().unwrap()), "--language", format!("{}:{}", track.stream.index, track.language().to_639_3()), "-y", format!("{}:{}", track.stream.index, track.offset)]);
            }
        }
        arguments.push(path.to_string_lossy().to_string());
    }
    let mut sub_files = Vec::new();
    unique_files.clear();
    for track in sinfo {
        if !unique_files.contains(&track.file) {
            unique_files.insert(track.file.clone());
            sub_files.push(track.file.clone());
        }
    }
    for path in sub_files {
        let mut sub_tracks = Vec::new();
        for track in sinfo {
            if track.file == path {
                sub_tracks.push(track.stream.index);
            }
        }
        let sub_tracks_str = sub_tracks.iter().join(",");
        arguments.append(&mut vec_into!["-s", sub_tracks_str, "-D", "-A", "--compression", "-1:zlib"]);
        for track in sinfo {
            if track.file == path {
                arguments.append(&mut vec_into!["--track-name", format!("{}:{}", track.stream.index, track.stream.tags.title.as_ref().unwrap()), "--language", format!("{}:{}", track.stream.index, track.language().to_639_3()), "-y", format!("{}:{}", track.stream.index, track.offset)])
            }
        }
        arguments.push(path.to_string_lossy().to_string());
    }
    arguments.append(&mut vec_into!["--track-order", track_order]);
    Command::new(get_binary("mkvmerge"))
        .args(&arguments)
        .spawn().unwrap().wait().unwrap();
}

fn process_command(args: Args) {
    println!("Input directory: {:#?}", args.input_directory);
    let input_directory_exists = args.input_directory.try_exists().unwrap();
    assert!(input_directory_exists, "Input directory does not exist!");
    let mut torrent_path: Option<PathBuf> = None;
    let mut torrent_files: Option<PathBuf> = None;
    let mut src2_paths: Option<Vec<PathBuf>> = None;
    let mut encoder_options: Option<String> = None;
    for path in args.input_directory.read_dir().unwrap() {
        let dir_entry = path.unwrap();
        let file_path = dir_entry.path();
        let file_name = dir_entry.file_name();
        let base = file_path.file_stem().unwrap();
        if !is_video(&file_path) || is_temporary_file(&file_name) {
            continue;
        }
        println!("{}", dir_entry.path().display());
        let episode_number_try = extract_episode_number(&base, args.episode_pattern.clone(), Some(args.season.clone()));
        if episode_number_try.is_err() {
            println!("Failed to get episode number from {base:#?}");
            continue;
        }
        let episode_number = episode_number_try.unwrap();
        println!("Episode {episode_number}");
        let filename_output = format!("[{}] {} - {episode_number} [{}]", args.group, args.name, args.suffix);
        let output_path = args.output_directory.clone().join(format!("{filename_output}.mkv"));
        println!("Output path: {}", output_path.display());
        if args.batch {
            torrent_files = Some(args.output_directory.clone());
            torrent_path = Some(args.input_directory.clone().join(format!(
                    "{}.torrent",
                    args.output_directory.clone().file_stem().unwrap().to_str().unwrap())));
        } else {
            torrent_files = Some(output_path.clone());
            torrent_path = Some(args.input_directory.clone().join(format!("{filename_output}.torrent")));
        }
        if !args.no_torrent
            && torrent_path.clone().unwrap().try_exists().is_ok_and(|b| b == true)
            || args.no_torrent && output_path.clone().try_exists().is_ok_and(|b| b == true)
        {
            continue;
        }
        if (args.audio == "2" || args.audio == "both") || (args.subs == "2" || args.subs == "both")
        {
            let mut temp_files = args.src2_directory.clone().unwrap().read_dir().unwrap()
                .filter(|file| {
                    let hi = file.as_ref().unwrap().file_name();
                    is_video(&file.as_ref().unwrap().path())
                        && !is_temporary_file(&hi)
                        && match_episode(&hi, episode_number.clone(), args.season.clone())
                }).peekable();
            if temp_files.peek().is_some() {
                let mut temp_list: Vec<PathBuf> = vec![];
                temp_files.for_each(|file| {
                    temp_list.push(args.src2_directory.clone().unwrap().join(file.unwrap().path()),)
                });
                src2_paths = Some(temp_list.clone());
            }
        }
        let (vinfo, ainfo, sinfo) = get_info(&file_path, &args.src2_directory, &args);
        let (cr, matrix, transfer, primaries) = vinfo[0].color_data(args.encoder == "rav1e");
        encoder_options = Some(get_encoder_params(&args, &vinfo, None, None, None, true));
        let multi_speed: u8 = if args.encoder == "rav1e" { 10 } else { 8 };

        let scene_detect = temp_path(&file_path, "_scene_detect.vpy");
        let skip_frames = temp_path(&file_path, "_skip.vpy");
        let script = temp_path(&file_path, ".vpy");
        let clean = temp_path(&file_path, "_clean.vpy");
        let merge = temp_path(&file_path, "_merge.vpy");
        let scenes = temp_path(&file_path, "_scenes.json");
        let scenes_skip = temp_path(&file_path, "_skip.json");
        let scenes_over = temp_path(&file_path, "_override.json");
        let encode = temp_path(&file_path, "_enc.mkv");
        let grainy = temp_path(&file_path, "_grainy.mkv");
        let cleaned = temp_path(&file_path, "_cleaned.mkv");
        let grained = temp_path(&file_path, "_grained.mkv");
        let tags = temp_path(&file_path, "_tags.xml");

        if scene_detect.try_exists().is_ok_and(|b| b == false) {
            sd_script(&scene_detect, &args, &vinfo);
        }
        if script.try_exists().is_ok_and(|b| b == false) && !args.no_filter {
            create_vpy_script(&script, &file_path, &args, &vinfo);
        }
        if skip_frames.try_exists().is_ok_and(|b| b == false) && !args.single_pass {
            multi_script(&skip_frames, &args, &vinfo);
        }
        if clean.try_exists().is_ok_and(|b| b == false) && args.diff_grain && args.no_filter {
            denoise_script(&clean, &args, &vinfo);
        }
        if merge.try_exists().is_ok_and(|b| b == false) && args.lehmer_merge {
            merge_script(&merge, &args, &vinfo);
        }
        if args.review {
            println!("PAUSED: Review and edit your filters for {}. Ready to continue?", file_path.display());
            print!("(yes/no): ");
            io::stdout().flush().expect("Failed to flush!");
            let mut input: String = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read input!");
            if input != "yes\n" {
                eprintln!("\nAborted. Exiting script.");
                exit(0);
            }
            println!("Continuing to encode.");
        }
        if encode.try_exists().is_ok_and(|b| b == false) {
            let scenes_file;
            let temp = file_path.parent().unwrap().join(base);
            if scenes.try_exists().is_ok_and(|b| b == false) {
                scene_detection(&scene_detect, &encode, &scenes, &temp, &args, &vinfo);
            }
            if !args.single_pass && args.parameters.is_none() {
                if scenes_over.try_exists().is_ok_and(|b| b == false) {
                    let scenes_info_read = File::open(&scenes).unwrap();
                    let mut scenes_info: ScenesInfo = serde_json::from_reader(&scenes_info_read).unwrap();
                    if scenes_skip.try_exists().is_ok_and(|b| b == false) {
                        scene_detection(&skip_frames, &encode, &scenes_skip, &temp, &args, &vinfo);
                    }
                    let lowest_quantizer = calculate_quantizer(&args, 2);
                    let lowest = temp_path(&file_path, "_lowest.mkv");
                    let lowest_temp = file_path.parent().unwrap().join(lowest.file_stem().unwrap());
                    if lowest.try_exists().is_ok_and(|b| b == false) {
                        encode_file(&skip_frames, &skip_frames, &lowest, &lowest_temp, &scenes_skip, Some(multi_speed), Some(lowest_quantizer), None, false, &args, &vinfo);
                    }
                    get_ssimulacra2(&skip_frames, &lowest, &mut scenes_info, lowest_quantizer, args.cycle, &cr, &matrix, &transfer, &primaries);

                    let low_quantizer = calculate_quantizer(&args, 1);
                    let low = temp_path(&file_path, "_low.mkv");
                    let low_temp = file_path.parent().unwrap().join(low.file_stem().unwrap());
                    if low.try_exists().is_ok_and(|b| b == false) {
                        encode_file(&skip_frames, &skip_frames, &low, &low_temp, &scenes_skip, Some(multi_speed), Some(low_quantizer), None, false, &args, &vinfo);
                    }
                    get_ssimulacra2(&skip_frames, &low, &mut scenes_info, low_quantizer, args.cycle, &cr, &matrix, &transfer, &primaries);

                    let high_quantizer = calculate_quantizer(&args, -1);
                    let high = temp_path(&file_path, "_high.mkv");
                    let high_temp = file_path.parent().unwrap().join(high.file_stem().unwrap());
                    if high.try_exists().is_ok_and(|b| b == false) {
                        encode_file(&skip_frames, &skip_frames, &high, &high_temp, &scenes_skip, Some(multi_speed), Some(high_quantizer), None, false, &args, &vinfo);
                    }
                    get_ssimulacra2(&skip_frames, &high, &mut scenes_info, high_quantizer, args.cycle, &cr, &matrix, &transfer, &primaries);

                    let highest_quantizer = calculate_quantizer(&args, -2);
                    let highest = temp_path(&file_path, "_highest.mkv");
                    let highest_temp = file_path.parent().unwrap().join(highest.file_stem().unwrap());
                    if highest.try_exists().is_ok_and(|b| b == false) {
                        encode_file(&skip_frames, &skip_frames, &highest, &highest_temp, &scenes_skip, Some(multi_speed), Some(highest_quantizer), None, false, &args, &vinfo);
                    }
                    get_ssimulacra2(&skip_frames, &highest, &mut scenes_info, highest_quantizer, args.cycle, &cr, &matrix, &transfer, &primaries);

                    zone_overrides(&mut scenes_info, &scenes, &scenes_over, &args, &cr, &matrix, &transfer, &primaries);
                }
                scenes_file = scenes_over.clone();
            } else {
                scenes_file = scenes.clone();
            }
            encode_file(&scene_detect, &script, &encode, &temp, &scenes_file, Some(args.speed), Some(args.quantizer), None, true, &args, &vinfo);
        }
        if grained.try_exists().is_ok_and(|b| b == false) {
            if args.diff_grain {
                if grainy.try_exists().is_ok_and(|b| b == false) {
                    let script = if args.lehmer_merge {
                        merge
                    } else {
                        scene_detect.clone()
                    };
                    let temp = file_path.parent().unwrap().join(grainy.file_stem().unwrap());
                    encode_file(&scene_detect, &script, &grainy, &temp, &scenes, None, None, Some("x264"), true, &args, &vinfo);
                }
                let cleaned_temp = if args.no_filter {
                    let cleaned_temp = file_path.parent().unwrap().join(cleaned.file_stem().unwrap());
                    if cleaned.try_exists().is_ok_and(|b| b == false) {
                        let scenes_file = if args.single_pass {
                            scenes.clone()
                        } else {
                            scenes_over.clone()
                        };
                        encode_file(&clean, &clean, &cleaned, &cleaned_temp, &scenes_file, Some(multi_speed), None, None, true, &args, &vinfo);
                    }
                    cleaned_temp
                } else {
                    file_path.parent().unwrap().join(file_path.file_stem().unwrap())
                };
                let grainy_temp = temp_path(&grainy, "");
                get_diff_grain(&grainy_temp, &cleaned_temp, &grainy_temp, &grained);
            } else if !args.no_grain {
                add_grain_table(&encode, &grained, args.photon_noise);
            }
        }
        if tags.try_exists().is_ok_and(|b| b == false) {
            get_tags(&tags, Some(get_encoder_params(&args, &vinfo, None, None, None, true)), &args);
        }
        if args.review {
            println!("PAUSED: Review and edit your tags for {}. Ready to continue?", file_path.display());
            print!("(yes/no): ");
            io::stdout().flush().expect("Failed to flush!");
            let mut input: String = String::new();
            io::stdin().read_line(&mut input).expect("Failed to read input!");
            if input.to_lowercase() != "yes\n" {
                eprintln!("\nAborted. Exiting script.");
                exit(0);
            }
            println!("Continuing to mux.");
        }
        if output_path.try_exists().is_ok_and(|b| b == false) {
            let video_path = if args.no_grain {
                encode.clone()
            } else {
                grained.clone()
            };
            mux_file(&video_path, &encode, &output_path, &tags, &vinfo, &ainfo, &sinfo, &args);
            println!("{filename_output} done!");
        }
        if !args.batch && !args.no_torrent && torrent_path.clone().unwrap().try_exists().is_ok_and(|b| b == false) {
            let opus_options: String = if src2_paths.is_some() {
                check_audio_encoding(&args.src2_directory.clone().unwrap())
            } else {
                check_audio_encoding(&args.input_directory.clone())
            };
            create_torrent(opus_options, encoder_options.clone().unwrap(), &torrent_path.clone().unwrap(), &torrent_files.clone().unwrap(), &args);
        }
    }
    let opus_options: String = if src2_paths.is_some() {
        check_audio_encoding(&args.src2_directory.clone().unwrap())
    } else {
        check_audio_encoding(&args.input_directory.clone())
    };
    if args.batch
        && !args.no_torrent
        && torrent_path.clone().is_some()
        && torrent_path.clone().unwrap().try_exists().is_ok_and(|b| b == false)
    {
        create_torrent(opus_options, encoder_options.unwrap(), &torrent_path.unwrap(), &torrent_files.unwrap(), &args);
    }
}

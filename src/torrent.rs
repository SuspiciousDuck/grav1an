use super::{get_encoder_version, get_filter_string, get_grain_string, get_rescale_string, Args};
use core::str;
use lava_torrent::bencode::BencodeElem::{Integer as bInt, String as bString};
use lava_torrent::torrent::v1::TorrentBuilder;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn pieces(file: &PathBuf) -> u64 {
    let size = file.metadata().unwrap().len();
    let min_size = 16u64 * 1024u64; // 16 KB
    let max_size = 16u64 * 1024u64 * 1024u64; // 16 MB
    let max_pieces = if size <= 2u64.pow(30) {
        512u64
    } else if size <= 8 * 2u64.pow(30) {
        1024u64
    } else if size <= 16 * 2u64.pow(30) {
        1536u64
    } else {
        2048u64
    };
    let exponent = (size as f64 / max_pieces as f64).log2().ceil() as u32;
    2u64.pow(exponent).clamp(min_size, max_size)
}

pub fn create_torrent(
    opus_options: String,
    encoder_options: String,
    torrent_path: &PathBuf,
    torrent_files: &PathBuf,
    args: &Args,
) {
    let mut comment_string;
    if args.source_info.clone().is_some() {
        comment_string = format!("Source: {}\n", args.source_info.clone().unwrap().clone());
    } else {
        comment_string = "AV1 encode with some filters\n".into();
    }
    if !args.single_pass {
        comment_string = format!(
            "{comment_string}Target SSIMULACRA 2: Mean: {}\n",
            args.target_quality
        );
    }
    comment_string = format!(
        "{comment_string}Encoding settings: {}: \"{}\"",
        get_encoder_version(args.encoder.clone().as_str()).unwrap(),
        encoder_options
    );
    if opus_options != "" {
        comment_string = format!(
            "{comment_string} + opusenc libopus {}: \"{opus_options}\"",
            get_encoder_version("opusenc").unwrap()
        );
    }
    comment_string.push('\n');
    if !args.no_grain {
        comment_string = format!(
            "{comment_string}Film grain synthesis settings: grav1synth: {}\n",
            get_grain_string(&args)
        );
    }
    if !args.no_filter {
        comment_string = format!("{comment_string}Filters: {}\n", get_filter_string(&args));
    }
    if args.rescale {
        comment_string = format!("{comment_string}Rescale: {}\n", get_rescale_string(&args));
    }
    comment_string.push_str("Interested in AV1?: https://discord.gg/83dRFDFDp7");
    let announce: &'static str = "http://nyaa.tracker.wf:7777/announce";
    let announce_list: [[&'static str; 1]; 11] = [
        ["http://nyaa.tracker.wf:7777/announce"],
        ["http://tracker.anirena.com:80/announce"],
        ["udp://tracker.opentrackr.org:1337/announce"],
        ["udp://open.stealth.si:80/announce"],
        ["udp://tracker.torrent.eu.org:451/announce"],
        ["udp://open.demonii.com:1337/announce"],
        ["udp://open.tracker.cl:1337/announce"],
        ["udp://explodie.org:6969/announce"],
        ["https://tracker.gbitt.info:443/announce"],
        ["http://tracker.gbitt.info:80/announce"],
        ["udp://tracker-udp.gbitt.info:80/announce"],
    ];
    #[rustfmt::skip]
    let creation_date = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let name = torrent_files.file_name().unwrap().to_str().unwrap();
    let piece_length = pieces(&torrent_files);
    #[rustfmt::skip]
    let mut torrent_build = TorrentBuilder::new(&torrent_files, piece_length as i64)
        .set_announce(Some(announce.into()))
        .set_announce_list(announce_list.map(|v| [v[0].to_string()].to_vec()).to_vec())
        .set_name(name.into())
        .add_extra_info_field("private".into(), bInt(0))
        .add_extra_field("creation date".into(), bInt(creation_date as i64))
        .add_extra_field("comment".into(), bString(comment_string.clone()))
        .add_extra_field("created by".into(), bString(args.group.clone()));
    if args.source_url.is_some() {
        torrent_build = torrent_build
            .clone()
            .add_extra_info_field("source".into(), bString(args.source_url.clone().unwrap()));
    }
    let torrent = torrent_build.build().unwrap();
    torrent.write_into_file(&torrent_path).unwrap();
    println!(
        "Torrent for {} done at {}",
        torrent_files.display(),
        torrent_path.display()
    );
}

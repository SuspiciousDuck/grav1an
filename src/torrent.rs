use super::{get_encoder_version, get_filter_string, get_grain_string, get_rescale_string, Args};
use core::str;
use std::collections::HashMap;
use lava_torrent::bencode::BencodeElem::{Integer as bInt, String as bString};
use lava_torrent::torrent::v1::TorrentBuilder;
use reqwest::header::{HeaderMap, HeaderValue, REFERER};
use std::path::PathBuf;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn random_string() -> String {
    let chars = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i'];
    let time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs().to_string();
    time.chars().map(|c| chars[c.to_digit(10).unwrap() as usize]).collect::<String>()
}

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
    let mut comment_string = String::new();
    if args.series_info.as_ref().is_some() {
        comment_string = format!("Series: https://www.thetvdb.com/dereferrer/series/{}\n", args.series_info.as_ref().unwrap());
    }
    comment_string = format!("{comment_string}AV1 encode with some filters\n");
    if !args.single_pass {
        comment_string = format!(
            "{comment_string}Target SSIMULACRA 2: 16th percentile: {}\n",
            args.target_quality
        );
    }
    comment_string = format!(
        "{comment_string}Encoding settings: {}: \"{}\"",
        get_encoder_version(&args.encoder).unwrap(),
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
    if args.source_urls.is_some() {
        torrent_build = torrent_build
            .clone()
            .add_extra_info_field("source".into(), bString(args.source_urls.as_ref().unwrap_or(&vec![]).join(", ")));
    }
    let torrent = torrent_build.build().unwrap();
    torrent.write_into_file(&torrent_path).unwrap();
    let open = open::that(&torrent_path);
    if open.is_err() {
        eprintln!("Failed to open {} automatically!", torrent_path.display());
    }
    println!(
        "Torrent for {} done at {}",
        torrent_files.display(),
        torrent_path.display()
    );
}

pub fn get_rentry(output_path: &PathBuf, filename_output: &String) -> Result<String, String> {
    let hasher = format!("{:x}", md5::compute(filename_output)).chars().take(8).collect::<String>();
    let mut headers = HeaderMap::new();
    headers.insert(REFERER, HeaderValue::from_static("https://rentry.co"));
    let client = reqwest::blocking::ClientBuilder::new().default_headers(headers).use_rustls_tls().build().unwrap();
    let check = client.get(format!("https://rentry.co/api/raw/{}", hasher.as_str())).send().unwrap();
    if !check.status().is_success() {
        return Err("Failed to make request to rentry.co!".into());
    }
    let json = check.json::<HashMap<String, String>>().map_err(|e| e.to_string())?;
    if json.get("status").unwrap() == "200" {
        println!("Entry already exists at URL: https://rentry.co/{hasher}");
        return Ok(hasher);
    }
    eprintln!("Rentry URL does not exist, proceeding with upload...");
    let mut media = mediainfo::MediaInfo::new();
    media.open(&output_path).expect("MediaInfo failed to open output file!");
    let info = media.inform().expect("MediaInfo failed to get output data!").replace(output_path.to_str().unwrap(), output_path.file_name().unwrap().to_str().unwrap());
    media.close();
    let csrftoken_rq = client.get("https://rentry.co").send().unwrap();
    if !csrftoken_rq.status().is_success() {
        return Err("Failed to make request to rentry.co!".into());
    }
    let cookie = csrftoken_rq.headers().get("Set-Cookie").expect("Failed to get Set-Cookie header!").to_str().unwrap();
    let csrftoken = cookie.split(';').next().unwrap().split('=').nth(1).unwrap();
    let edit_code = random_string();
    let json = json!({
        "csrfmiddlewaretoken": csrftoken,
        "url": hasher,
        "edit_code": edit_code,
        "text": info
    });
    println!("Rentry edit code: {edit_code}");
    let post_rq = client.post("https://rentry.co/api/new").json(&json).send().unwrap();
    if !post_rq.status().is_success() {
        return Err("Failed to make POST request to rentry.co!".into());
    }
    let response = post_rq.json::<HashMap<String, String>>().unwrap();
    if response.get("status").unwrap() == "200" {
        let url = response.get("url").map(|e| e.to_owned()).ok_or("Rentry upload response returned no url!".to_string())?;
        println!("Rentry upload successful! URL: {}", url);
        println!("Edit code: {}", response.get("edit_code").unwrap_or(&edit_code));
        Ok(url)
    } else {
        let mut err = format!("Error: {}", response.get("content").unwrap());
        if response.contains_key("errors") {
            err = format!("Details: {}", response.get("errors").unwrap());
        }
        Err(err)
    }
}
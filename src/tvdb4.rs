use std::io::{self, prelude::*};
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tvdb4::apis::{configuration::Configuration, movies_api::*, series_api::*};
use tvdb4::models::{ArtworkBaseRecord, ArtworkExtendedRecord, EpisodeBaseRecord, MovieBaseRecord, SeriesBaseRecord, Translation};
use std::process::exit;

fn choose_media(shows: Vec<&SeriesBaseRecord>, movies: Vec<&MovieBaseRecord>) -> (Option<SeriesBaseRecord>, Option<MovieBaseRecord>) {
    if shows.is_empty() && movies.is_empty() {
        return (None, None);
    }
    println!("[-1] Skip\n[0] Cancel");
    for (idx, show) in shows.iter().enumerate() {
        println!("[{}] Series: {}", idx + 1usize, show.name.as_ref().unwrap());
    }
    for (idx, movie) in movies.iter().enumerate() {
        println!("[{}] Movie: {}", idx + shows.len() + 1usize, movie.name.as_ref().unwrap());
    }
    print!("[0..{}]: ", shows.len() + movies.len());
    io::stdout().flush().expect("Failed to flush!");
    let mut input: String = String::new();
    io::stdin().read_line(&mut input).expect("Failed to read input!");
    if input.ends_with('\n') {
        input = input.trim_end().into();
    }
    let pick = input.parse::<isize>().expect("Unexpected input!");
    if pick == 0 {
        eprintln!("\nAborted. Exiting script.");
        exit(0);
    } else if pick == -1 {
        println!("\nSkipping TVDB search!");
        return (None, None);
    }
    if pick as usize > shows.len() {
        (None, movies.get(pick as usize - shows.len() - 1usize).map(|m| Some(*m)).unwrap_or(None).cloned())
    } else {
        (shows.get(pick as usize - 1usize).map(|s| Some(*s)).unwrap_or(None).cloned(), None)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetSeriesEpisodesTranslated200Response {
    #[serde(rename = "data", skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<tvdb4::models::SeriesBaseRecord>>,
    #[serde(rename = "status", skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

struct ArtExtRecord(ArtworkExtendedRecord);
impl From<ArtExtRecord> for ArtworkBaseRecord {
    fn from(value: ArtExtRecord) -> Self {
        ArtworkBaseRecord {
            height:  value.0.height,
            id: value.0.id.map(|i| i as i32),
            image: value.0.image,
            includes_text: value.0.includes_text,
            language: value.0.language,
            score: value.0.score,
            thumbnail: value.0.thumbnail,
            r#type: value.0.r#type,
            width: value.0.width
        }
    }
}

async fn get_series_episodes_translated(
    configuration: &Configuration,
    page: i32,
    id: f32,
    season_type: &str,
    lang: &str
) -> Result<GetSeriesEpisodesTranslated200Response, tvdb4::apis::Error<tvdb4::apis::series_api::GetSeriesSeasonEpisodesTranslatedError>> {
    let local_var_configuration = configuration;

    let local_var_client = &local_var_configuration.client;

    let local_var_uri_str = format!(
        "{}/series/{}/episodes/{}/{}",
        local_var_configuration.base_path,
        id,
        tvdb4::apis::urlencode(season_type),
        tvdb4::apis::urlencode(lang)
    );
    let mut local_var_req_builder =
        local_var_client.request(reqwest::Method::GET, local_var_uri_str.as_str());

    local_var_req_builder = local_var_req_builder.query(&[("page", &page.to_string())]);
    if let Some(ref local_var_user_agent) = local_var_configuration.user_agent {
        local_var_req_builder =
            local_var_req_builder.header(reqwest::header::USER_AGENT, local_var_user_agent.clone());
    }
    if let Some(ref local_var_token) = local_var_configuration.bearer_access_token {
        local_var_req_builder = local_var_req_builder.bearer_auth(local_var_token.to_owned());
    };

    let local_var_req = local_var_req_builder.build()?;
    let local_var_resp = local_var_client.execute(local_var_req).await?;

    let local_var_status = local_var_resp.status();
    let local_var_content = local_var_resp.text().await?;

    if !local_var_status.is_client_error() && !local_var_status.is_server_error() {
        serde_json::from_str(&local_var_content).map_err(tvdb4::apis::Error::from)
    } else {
        let local_var_entity: Option<tvdb4::apis::series_api::GetSeriesSeasonEpisodesTranslatedError> =
            serde_json::from_str(&local_var_content).ok();
        let local_var_error = tvdb4::apis::ResponseContent {
            status: local_var_status,
            content: local_var_content,
            entity: local_var_entity,
        };
        Err(tvdb4::apis::Error::ResponseError(local_var_error))
    }
}

pub struct Tvdb {
    conf: Configuration
}
impl Tvdb {
    pub fn new() -> Self {
        let cfg = Configuration::new();
        Tvdb {
            conf: cfg
        }
    }
    pub fn login(&mut self, key: String) -> Result<(), String> {
        let rt = Runtime::new().expect("Failed to create tokio runtime!");
        let login_post_rq = tvdb4::models::LoginPostRequest::new(key);
        let login_response = rt.block_on(tvdb4::apis::login_api::login_post(&self.conf, login_post_rq)).map_err(|_| "Failed to login to the TVDB API!".to_string())?;
        let login_data = login_response.data.ok_or("Login response is empty!".to_string())?;
        let token = login_data.token.ok_or("Failed to retrieve token!".to_string())?;
        self.conf.bearer_access_token = Some(token);
        Ok(())
    }
    pub fn tvdb_lookup(&self, tvdb_id: Option<u32>, imdb_id: Option<&String>, name: &String) -> Result<(Option<SeriesBaseRecord>, Option<MovieBaseRecord>, Option<u32>), String> {
        let rt = Runtime::new().expect("Failed to create tokio runtime!");
        let conf = &self.conf;
        let (series, movie) = if tvdb_id.is_some() {
            let movie_q = rt.block_on(get_movie_base(&conf, tvdb_id.unwrap() as f32));
            let series_q = rt.block_on(get_series_base(&conf, tvdb_id.unwrap() as f32));
            if series_q.is_err() && movie_q.is_err() {
                return Err("Provided TVDB id doesn't exist or isn't a series/movie!".into());
            } else {
                (series_q.map(|s| Some(*s.data.unwrap())).unwrap_or(None),
                movie_q.map(|m| Some(*m.data.unwrap())).unwrap_or(None))
            }
        } else if imdb_id.is_some() {
            let query = rt.block_on(tvdb4::apis::search_api::get_search_results_by_remote_id(&conf, imdb_id.unwrap().as_str())).map_err(|_| "Failed to receive TVDB remote search results!".to_string())?;
            let results = query.data.ok_or("TVDB remote search returned no results!".to_string())?;
            let movies = results.iter().filter(|s| s.movie.is_some()).map(|s| s.movie.as_ref().unwrap()).collect::<Vec<&MovieBaseRecord>>();
            let shows = results.iter().filter(|s| s.series.is_some()).map(|s| s.series.as_ref().unwrap()).collect::<Vec<&SeriesBaseRecord>>();
            if movies.is_empty() && shows.is_empty() {
                return Err("No results found when searching TVDB for IMDB id!".into());
            } else if results.len() == 1 {
                (results[0].series.clone(), results[0].movie.clone())
            } else {
                println!("Multiple results found for TVDB remote search!");
                choose_media(shows, movies)
            }
        } else {
            let query = rt.block_on(tvdb4::apis::search_api::get_search_results(&conf, Some(name.as_str()), None, None, None, None, None, None, None, None, None, None, None, None)).map_err(|_| "Failed to receive TVDB search results!".to_string())?;
            let results = query.data.ok_or("TVDB search returned no results!".to_string())?;
            let movies = results.iter().filter(|s| s.r#type.as_ref().is_some_and(|m| m=="movie")).map(|m| *rt.block_on(get_movie_base(&conf, m.tvdb_id.as_ref().unwrap().parse().unwrap())).unwrap().data.unwrap()).collect::<Vec<MovieBaseRecord>>();
            let shows = results.iter().filter(|s| s.r#type.as_ref().is_some_and(|s| s=="series")).map(|s| *rt.block_on(get_series_base(&conf, s.tvdb_id.as_ref().unwrap().parse().unwrap())).unwrap().data.unwrap()).collect::<Vec<SeriesBaseRecord>>();
            if movies.is_empty() && shows.is_empty() {
                return Err("No results found when searching TVDB!".into());
            } else if movies.len() + shows.len() == 1 {
                (shows.get(0).cloned(), movies.get(0).cloned())
            } else {
                println!("Multiple results found for TVDB search!");
                choose_media(shows.iter().collect(), movies.iter().collect())
            }
        };
        let id = series.as_ref().map(|s| s.id.map(|n| n as i64)).or(movie.as_ref().map(|m| m.id)).unwrap_or(None);
        Ok((series, movie, id.map(|n| n as u32)))
    }
    pub fn get_artworks(&self, id: u32, is_show: bool) -> Result<ArtworkBaseRecord, String> {
        let rt = Runtime::new().expect("Failed to create tokio runtime!");
        let conf = &self.conf;
        if is_show {
            let query = rt.block_on(get_series_artworks(&conf, id as f32, None, Some(1))).or(rt.block_on(get_series_artworks(&conf, id as f32, None, Some(2)))).map_err(|_| "Failed to retrieve banner or poster!".to_string())?;
            let data = query.data.ok_or("Banner/Poster query returned nothing!".to_string())?;
            data.artworks.unwrap_or_default().get(0).map_or(Err("No artworks found!".into()), |a| Ok(ArtworkBaseRecord::from(ArtExtRecord(a.to_owned()))))
        } else {
            let query = rt.block_on(get_movie_extended(&conf, id as f32, None, None)).map_err(|_| "Failed to retrieve movie extended record!".to_string())?;
            let data = query.data.ok_or("Movie extended record query returned nothing!".to_string())?;
            data.artworks.unwrap_or_default().get(0).map_or(Err("No artworks found!".into()), |a| Ok(a.to_owned()))
        }
    }
    pub fn get_translation(&self, id: u32, lang: &str, is_show: bool) -> Result<Translation, String> {
        let rt = Runtime::new().expect("Failed to create tokio runtime!");
        let conf = &self.conf;
        if is_show {
            let query = rt.block_on(get_series_translation(&conf, id as f32, lang)).map_err(|_| "Failed to retrieve series translation!".to_string())?;
            let data = query.data.ok_or("Series translation query returned nothing!".to_string())?;
            Ok(*data)
        } else {
            let query = rt.block_on(get_movie_translation(&conf, id as f32, lang)).map_err(|_| "Failed to retrieve movie translation!".to_string())?;
            let data = query.data.ok_or("Movie translation query returned nothing!".to_string())?;
            Ok(*data)
        }
    }
    pub fn get_series_episode(&self, id: u32, season: u8, episode: u16) -> Result<EpisodeBaseRecord, String> {
        let rt = Runtime::new().expect("Failed to create tokio runtime!");
        let conf = &self.conf;
        let query = rt.block_on(get_series_episodes(&conf, 0, id as f32, "default", Some(season as i32), Some(episode as i32), None)).map_err(|e| e.to_string())?;
        let data = query.data.ok_or("Series episode query returned nothing!".to_string())?;
        let episodes = data.episodes.map(|e| if e.len() == 0 { Err("Series episode query returned no episodes!".to_string()) } else if e.len() > 1 { Err("Series episode query returned more than one episode!".to_string()) } else { Ok(e) }).ok_or("Series episode query returned no episodes!".to_string())??;
        Ok(episodes[0].clone())
    }
    pub fn get_series_episode_translation(&self, id: u32, lang: &str) -> Result<Vec<EpisodeBaseRecord>, String> {
        let rt = Runtime::new().expect("Failed to create tokio runtime!");
        let conf = &self.conf;
        let query = rt.block_on(get_series_episodes_translated(&conf, 0, id as f32, "default", lang)).map_err(|e| e.to_string())?;
        let data = query.data.ok_or("Series episode translation query returned nothing!".to_string())?;
        Ok(data.episodes.unwrap_or_default())
    }
}
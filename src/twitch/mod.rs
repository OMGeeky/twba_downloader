use chrono::{NaiveDateTime, Utc};
use futures_util::{StreamExt, TryStreamExt};
use reqwest_backoff::ReqwestClient;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::Instant;
use tracing::instrument;

use crate::errors::{DownloadError, DownloadFileError, MalformedPlaylistError, PlaylistParseError};
use crate::prelude::*;

mod access_token;
use access_token::TwitchVideoAccessTokenResponse;

#[derive(Debug)]
pub struct TwitchClient {
    client: ReqwestClient,
    config: Conf,
}
//region public functions
impl TwitchClient {
    #[tracing::instrument]
    pub fn new(config: Conf) -> Self {
        let client = reqwest::Client::new().into();
        Self { client, config }
    }
    #[tracing::instrument(skip(self))]
    pub async fn download_video<VideoId: DIntoString, QUALITY: DIntoString>(
        &self,
        video_id: VideoId,
        quality: QUALITY,
        output_folder: &Path,
    ) -> Result<PathBuf> {
        let video_id = video_id.into();
        let folder_path = output_folder.join(&video_id);
        let final_path = output_folder.join(format!("{}.mp4", video_id));
        if final_path.exists() {
            return Err(DownloadFileError::TargetAlreadyExists(final_path).into());
        }
        if !folder_path.exists() {
            std::fs::create_dir_all(&folder_path)
                .map_err(DownloadFileError::CouldNotCreateTargetFolder)?;
        } else if !folder_path.is_dir() {
            return Err(DownloadFileError::TargetFolderIsNotADirectory(folder_path).into());
        } else {
            // folder exists and is a directory
            if folder_path
                .read_dir()
                .map_err(DownloadFileError::Read)?
                .next()
                .is_some()
            {
                // folder is not empty
                return Err(DownloadFileError::TargetFolderIsNotEmpty(folder_path).into());
            }
        }

        let mut parts = self
            .download_all_parts(quality, &video_id, &folder_path)
            .await?;

        sort_parts(&mut parts);
        let mp4_file_path = combine_parts_to_mp4(&parts, &folder_path).await?;

        tokio::fs::rename(&mp4_file_path, &final_path)
            .await
            .map_err(DownloadFileError::Filesystem)?;
        //clean up the leftover parts
        tokio::fs::remove_dir_all(folder_path)
            .await
            .map_err(DownloadFileError::Filesystem)?;
        Ok(final_path)
    }
}
//endregion
impl TwitchClient {
    async fn download_all_parts<QUALITY: DIntoString>(
        &self,
        quality: QUALITY,
        video_id: &String,
        folder_path: &Path,
    ) -> Result<Vec<PathBuf>> {
        let download_info = self.get_download_info(video_id, quality).await?;
        let parts = download_info.parts;
        let base_url = download_info.base_url;
        let age = download_info.vod_age;
        if parts.is_empty() {
            return Err(MalformedPlaylistError::Empty.into());
        }
        let try_unmute = age.unwrap_or(999) < 24; //hours i think
        let amount_of_parts = parts.len() as u64;
        let thread_count = self.config.twitch.downloader_thread_count;
        let thread_count: u64 = if thread_count < 1 {
            1
        } else if thread_count > amount_of_parts {
            amount_of_parts
        } else {
            thread_count
        };

        // todo!("maybe add a progress bar/indicator?");
        let it = parts
            .into_iter()
            .map(|part| {
                let folder_path = folder_path.clone();
                let client = self.client.clone();
                let url = base_url.clone();
                async move {
                    // download
                    let result = download_part(part, url, folder_path, try_unmute, client).await;
                    // report progress
                    trace!("downloaded part: {:?}", result);
                    // return result
                    result
                }
            })
            .map(|x| async {
                x.await.and_then(|x: PathBuf| {
                    x.canonicalize()
                        .map_err(DownloadFileError::Canonicalization)
                })
            });
        let x = futures::stream::iter(it)
            .buffer_unordered(thread_count as usize)
            .try_collect::<Vec<_>>()
            .await?;

        Ok(x)
    }
    #[tracing::instrument(skip(self))]
    async fn get_download_info<ID: DIntoString, QUALITY: DIntoString>(
        &self,
        video_id: ID,
        quality: QUALITY,
    ) -> Result<DownloadInfo> {
        let playlist = self.get_video_playlist(video_id, quality).await?;
        let playlist_content = self
            .client
            .execute_with_backoff(self.client.get(&playlist).build()?)
            .await?
            .text()
            .await?;
        let base_url = &playlist[..playlist
            .rfind('/')
            .ok_or(MalformedPlaylistError::InvalidUrl)?
            + 1];
        let parts = parse_playlist(playlist_content)?;
        // dbg!(&parts);
        Ok(DownloadInfo {
            vod_age: parts.0,
            parts: parts.1,
            base_url: base_url.to_string(),
        })
    }

    #[tracing::instrument(skip(self))]
    async fn get_video_token_and_signature<S: DIntoString>(
        &self,
        video_id: S,
    ) -> Result<(String, String)> {
        let video_id = video_id.into();
        trace!("Getting access token & signature for video {}", video_id,);

        const URL: &str = "https://gql.twitch.tv/gql";
        let json = json!({"operationName":"PlaybackAccessToken_Template",
            "query": "query PlaybackAccessToken_Template($login: String!, $isLive: Boolean!, $vodID: ID!, $isVod: Boolean!, $playerType: String!) {  streamPlaybackAccessToken(channelName: $login, params: {platform: \"web\", playerBackend: \"mediaplayer\", playerType: $playerType}) @include(if: $isLive) {    value    signature    __typename  }  videoPlaybackAccessToken(id: $vodID, params: {platform: \"web\", playerBackend: \"mediaplayer\", playerType: $playerType}) @include(if: $isVod) {    value    signature    __typename  }}",
            "variables": {
            "isLive": false,
            "login": "",
            "isVod": true,
            "vodID": video_id,
            "playerType": "embed"
            }
        }).to_string();
        let request = self
            .client
            .post(URL)
            .header("Client-ID", &self.config.twitch.downloader_id)
            .body(json)
            .build()?;

        let response = self.client.execute_with_backoff(request).await?;
        let json = response.text().await?;
        // trace!("Got json response: {}", json);
        let token_response: TwitchVideoAccessTokenResponse =
            serde_json::from_str(&json).map_err(DownloadError::AccessTokenJsonParse)?;
        trace!(
            "Got access token & signature for video {}=>{:?}",
            video_id,
            token_response
        );
        let access_token = token_response
            .data
            .video_playback_access_token
            .ok_or(DownloadError::AccessTokenEmpty)?;

        Ok((access_token.value, access_token.signature))
    }

    #[tracing::instrument(skip(self))]
    async fn get_video_playlist<ID: DIntoString, QUALITY: DIntoString>(
        &self,
        video_id: ID,
        quality: QUALITY,
    ) -> Result<String> {
        let video_id = video_id.into();
        let quality = quality.into();

        trace!(
            "Getting video playlist with quality for video {} with quality {}",
            video_id,
            quality
        );

        let playlist = self.get_video_playlist_per_quality(&video_id).await?;
        let playlist = get_playlist_from_quality_list(playlist, &quality)?;

        Ok(playlist)
    }

    #[tracing::instrument(skip(self))]
    async fn get_video_playlist_per_quality(&self, video_id: &str) -> Result<String> {
        let (token, signature) = self.get_video_token_and_signature(video_id).await?;

        let playlist_url = format!(
            "https://usher.ttvnw.net/vod/{}?nauth={}&nauthsig={}&allow_source=true&player=twitchweb",
            video_id, token, signature
        );

        let request = self.client.get(playlist_url).build()?;
        let playlist = self.client.execute_with_backoff(request).await?;
        let playlist = playlist.text().await?;
        Ok(playlist)
    }
}
#[instrument]
async fn download_part(
    part: (String, f32),
    base_url: String,
    folder_path: &Path,
    try_unmute: bool,
    client: ReqwestClient,
) -> StdResult<PathBuf, DownloadFileError> {
    trace!("downloading part: {:?}", part);
    let (part, _duration) = part;

    let part_url = format!("{}{}", base_url, part);
    let part_url_unmuted = format!("{}{}", base_url, part.replace("-muted", ""));

    let try_unmute = try_unmute && part.contains("-muted");
    let target_path = folder_path.join(&part);

    if try_unmute {
        trace!("trying to download unmuted part: {}", part_url_unmuted);
        match try_download_part(part_url_unmuted, &target_path, &client).await {
            Ok(path) => Ok(path),
            Err(_) => {
                trace!("failed to download unmuted part. trying muted part");
                try_download_part(part_url, folder_path, &client).await
            }
        }
    } else {
        trace!("not trying to unmute: {}", part_url);
        try_download_part(part_url, &target_path, &client).await
    }
}
async fn try_download_part(
    url: String,
    target_path: &Path,
    client: &ReqwestClient,
) -> StdResult<PathBuf, DownloadFileError> {
    let request = client
        .get(url)
        .build()
        .map_err(DownloadFileError::DownloadReqwest)?;
    let mut response = client
        .execute_with_backoff(request)
        .await
        .map_err(DownloadFileError::DownloadBackoff)?;

    let mut file = fs::File::create(target_path)
        .await
        .map_err(DownloadFileError::FileCreation)?;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(DownloadFileError::DownloadReqwest)?
    {
        file.write_all(&chunk)
            .await
            .map_err(DownloadFileError::Filesystem)?;
    }
    Ok(target_path.to_path_buf())
}

#[instrument]
async fn convert_ts_to_mp4(ts_file: &Path, mp4_file: &Path) -> Result<()> {
    info!("converting to mp4");
    if mp4_file.exists() {
        tokio::fs::remove_file(&mp4_file)
            .await
            .map_err(DownloadFileError::Filesystem)?;
    }
    debug!(
        "running ffmpeg command: ffmpeg -i {} -c {}",
        ts_file.display(),
        mp4_file.display()
    );
    let mut cmd = Command::new("ffmpeg");
    let start_time = Instant::now();
    cmd.arg("-i")
        .arg(ts_file)
        .arg("-c")
        .arg("copy")
        .arg(mp4_file);
    let result = cmd.output().await;
    let duration = Instant::now().duration_since(start_time);
    debug!("ffmpeg command finished after duration: {:?}", duration);
    result.map_err(DownloadFileError::Ffmpeg)?;
    Ok(())
}

fn parse_playlist(
    playlist: String,
) -> StdResult<(Option<usize>, HashMap<String, f32>), MalformedPlaylistError> {
    info!("Parsing playlist");
    const STREAMED_DATE_IDENT: &str = "#ID3-EQUIV-TDTG:";

    let mut age = None;
    let mut parts = HashMap::new();
    dbg!(&playlist);
    let mut lines = playlist.lines();
    loop {
        let line = lines.next();
        trace!("line: {:?}", line);
        if line.is_none() {
            trace!("line is none. done parsing playlist");
            break;
        }
        let line = line.unwrap();
        if let Some(date) = line.strip_prefix(STREAMED_DATE_IDENT) {
            let date = date.trim();
            let date: chrono::DateTime<Utc> = convert_twitch_date(date)?;
            let now = Utc::now();
            let duration = now.signed_duration_since(date);
            age = Some(duration.num_hours() as usize);
            continue;
        }
        if let Some(part_duration) = line.strip_prefix("#EXTINF:") {
            let mut line = lines.next().ok_or(PlaylistParseError::Eof)?;
            if line.starts_with("#EXT-X-BYTERANGE:") {
                warn!("Found byterange, ignoring the line and moving on");
                line = lines.next().ok_or(PlaylistParseError::Eof)?;
            }

            let part_duration: f32 = part_duration.trim_matches(',').parse().unwrap_or(0.0);

            parts.insert(line.trim().to_string(), part_duration);
        } else {
            //ignore everything but content lines
            continue;
        }
    }
    dbg!(&parts.len());
    Ok((age, parts))
}

/// Converts a twitch date string to a chrono::DateTime<Utc>
/// Example: 2021-05-01T18:00:00
pub fn convert_twitch_date(date: &str) -> StdResult<chrono::DateTime<Utc>, PlaylistParseError> {
    let date = date.trim();
    let date = date.trim_matches('"');

    //parse the date from a string like this: 2023-10-07T23:33:29
    NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S")
        .map(|e| e.and_utc())
        .map_err(PlaylistParseError::InvalidTimeFormat)
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};
    #[test]
    fn test_convert_twitch_date() {
        let date = "2021-05-01T18:00:00";
        let date = convert_twitch_date(date).unwrap();
        assert_eq!(date.year(), 2021);
        assert_eq!(date.month(), 5);
        assert_eq!(date.day(), 1);
        assert_eq!(date.hour(), 18);
        assert_eq!(date.minute(), 0);
        assert_eq!(date.second(), 0);
    }
}
#[tracing::instrument(skip(playlist))]
fn get_playlist_from_quality_list(playlist: String, quality: &str) -> Result<String> {
    trace!("Parsing playlist:\n{}", playlist);

    let mut qualties = HashMap::new();

    let mut highest_quality = String::new();
    let test: Vec<&str> = playlist.lines().collect();
    for (i, line) in test.iter().enumerate() {
        if !line.contains("#EXT-X-MEDIA") {
            continue;
        }

        let found_quality = line.split("NAME=\"").collect::<Vec<&str>>()[1]
            .split('"')
            .collect::<Vec<&str>>()[0];

        if qualties.get(found_quality).is_some() {
            continue;
        }
        if qualties.is_empty() {
            // the first one is the highest quality
            highest_quality = found_quality.to_string();
        }

        let url = test[i + 2];
        qualties.insert(found_quality, url);
    }
    if let Some(quality) = qualties.get(quality) {
        Ok(quality.to_string())
    } else {
        warn!(
            "Given quality not found ({}), using highest quality: {}",
            quality, highest_quality
        );
        Ok(qualties
            .get(highest_quality.as_str())
            .ok_or(MalformedPlaylistError::NoQualities)?
            .to_string())
    }
}
#[derive(Debug, Clone)]
struct DownloadInfo {
    vod_age: Option<usize>,
    parts: HashMap<String, f32>,
    base_url: String,
}
#[cfg(test)]
mod abc {
    use futures_util::{StreamExt, TryStreamExt};
    #[tokio::test]
    async fn test1() {
        let v = vec![1, 3, 5];
        let x1 = run(v).await;
        assert!(x1.is_err());
        assert_eq!(x1.unwrap_err(), 5i64);
    }
    #[tokio::test]
    async fn test2() {
        let v = vec![1, 5, 1];
        let x1 = run(v).await;
        assert!(x1.is_err());
        assert_eq!(x1.unwrap_err(), 5i64);
    }
    #[tokio::test]
    async fn test3() {
        let v = vec![1, 3, 2, 2];
        let x1 = run(v).await;
        assert!(x1.is_ok());
        assert_eq!(x1.unwrap(), vec![1, 3, 2, 2]);
    }
    async fn run(v: Vec<i32>) -> Result<Vec<i16>, i64> {
        async fn sample(part: i32) -> Result<i16, i64> {
            dbg!(part);
            if part <= 3 {
                Ok(part as i16)
            } else {
                Err(part as i64)
            }
        }
        let thread_count = 2;
        let it = v.into_iter().map(sample);
        let x = futures::stream::iter(it);
        let x1: Result<Vec<i16>, i64> = x.buffer_unordered(thread_count).try_collect().await;
        dbg!(&x1);
        x1
    }
}

fn sort_parts(files: &mut [PathBuf]) {
    files.sort_by_key(|path| {
        let number = path
            .file_stem()
            .map(|x| {
                x.to_str()
                    .unwrap_or("")
                    .replace("-muted", "")
                    .replace("-unmuted", "")
            })
            .unwrap_or(String::from("0"));
        match number.parse::<u32>() {
            Ok(n) => n,
            Err(e) => {
                warn!(
                    "potentially catchable error while parsing the file number: {}\n{}",
                    number, e
                );
                if !number.contains('-') {
                    error!("Error while parsing the file number: {}", number);
                    panic!("Error while parsing the file number: {}", number)
                }
                let number = number.split('-').collect::<Vec<&str>>()[1];
                number
                    .parse()
                    .unwrap_or_else(|_| panic!("Error while parsing the file number: {}", number))
            }
        }
    });
}

#[instrument(skip(files), fields(part_amount=files.len()))]
async fn combine_parts_to_single_ts(files: &[PathBuf], target: &Path) -> Result<()> {
    debug!("combining all parts of video");
    debug!("part amount: {}", files.len());
    let mut target = fs::File::create(target)
        .await
        .map_err(DownloadFileError::FileCreation)?;
    for file_path in files {
        trace!("{:?}", file_path.file_name());
        let mut file = fs::File::open(&file_path)
            .await
            .map_err(DownloadFileError::Read)?;
        tokio::io::copy(&mut file, &mut target)
            .await
            .map_err(DownloadFileError::Write)?;
        tokio::fs::remove_file(&file_path)
            .await
            .map_err(DownloadFileError::Write)?;
    }

    Ok(())
}

async fn combine_parts_to_mp4(parts: &[PathBuf], folder_path: &Path) -> Result<PathBuf> {
    let ts_file_path = folder_path.join("video.ts");
    let mp4_file_path = folder_path.join("video.mp4");

    combine_parts_to_single_ts(parts, &ts_file_path).await?;
    convert_ts_to_mp4(&ts_file_path, &mp4_file_path).await?;
    tokio::fs::remove_file(ts_file_path)
        .await
        .map_err(DownloadFileError::Filesystem)?;

    Ok(mp4_file_path)
}

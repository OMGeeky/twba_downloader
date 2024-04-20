use futures_util::{StreamExt, TryStreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::Instant;
use tracing::instrument;
use twba_reqwest_backoff::ReqwestClient;

use crate::errors::*;
use crate::prelude::*;

mod access_token;
use crate::twitch::parts_util::*;
use crate::twitch::twitch_utils::*;
use access_token::TwitchVideoAccessTokenResponse;

mod parts_util;
pub mod twitch_utils;

#[derive(Debug)]
pub struct TwitchClient {
    client: ReqwestClient,
    pub config: Conf,
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
            serde_json::from_str(&json).map_err(DownloaderError::AccessTokenJsonParse)?;
        trace!(
            "Got access token & signature for video {}=>{:?}",
            video_id,
            token_response
        );
        let access_token = token_response
            .data
            .video_playback_access_token
            .ok_or(DownloaderError::AccessTokenEmpty)?;

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

#[derive(Debug, Clone)]
struct DownloadInfo {
    vod_age: Option<usize>,
    parts: HashMap<String, f32>,
    base_url: String,
}

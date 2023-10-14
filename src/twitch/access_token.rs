use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct TwitchVideoAccessTokenResponse {
    pub data: VideoAccessTokenResponseData,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VideoAccessTokenResponseData {
    #[serde(rename = "videoPlaybackAccessToken")]
    pub video_playback_access_token: Option<VideoAccessTokenResponseDataAccessToken>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VideoAccessTokenResponseDataAccessToken {
    pub value: String,
    pub signature: String,
}

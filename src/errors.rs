use reqwest_backoff::ReqwestBackoffError;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("Malformed playlist")]
    MalformedPlaylist(#[from] MalformedPlaylistError),

    #[error("Backoff error")]
    Backoff(#[from] ReqwestBackoffError),
    #[error("Database Error")]
    Database(#[from] local_db::re_exports::sea_orm::errors::DbErr),

    #[error("Reqwest error")]
    Reqwest(#[from] reqwest::Error),

    #[error("Could not parse json to access token value and signature")]
    AccessTokenJsonParse(#[source] serde_json::Error),
    #[error("The server did not provide an access token")]
    AccessTokenEmpty,
    #[error("Got an error with the Filesystem")]
    File(#[from] DownloadFileError),
    #[error("Error while loading config")]
    LoadConfig(#[source] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum MalformedPlaylistError {
    #[error("Playlist was empty/did not contain any useful information")]
    Empty,
    #[error("Playlist did not specify any qualities")]
    NoQualities,

    #[error("Could not parse the playlist")]
    Parse(#[from] PlaylistParseError),
    #[error("Could not parse the url/the url did not contain the expected information")]
    InvalidUrl,
}
#[derive(Debug, thiserror::Error)]
pub enum PlaylistParseError {
    #[error("Unexpected end of file while parsing playlist")]
    Eof,
    #[error("Invalid time format in playlist")]
    InvalidTimeFormat(#[source] chrono::ParseError),
}
#[derive(Debug, thiserror::Error)]
pub enum DownloadFileError {
    #[error("The target folder is not empty {0:?}")]
    TargetFolderIsNotEmpty(PathBuf),
    #[error("The target folder is not a directory {0:?}")]
    TargetFolderIsNotADirectory(PathBuf),
    #[error("The target path already exists: {0:?}")]
    TargetAlreadyExists(PathBuf),
    #[error("Could not create the target folder")]
    CouldNotCreateTargetFolder(#[source] std::io::Error),
    #[error("Could not create a needed file")]
    FileCreation(#[source] std::io::Error),
    #[error("Could not read the folder/file")]
    Read(#[source] std::io::Error),
    #[error("Could not write the folder/file")]
    Write(#[source] std::io::Error),
    #[error("There was some error during a filesystem operation")]
    Filesystem(#[source] tokio::io::Error),

    #[error("The ffmpeg command returned an error")]
    Ffmpeg(#[source] tokio::io::Error),

    #[error("could not canonicalize path: {0:?}")]
    Canonicalization(#[source] std::io::Error),

    #[error("could not download file: {0:?}")]
    DownloadBackoff(#[source] ReqwestBackoffError),
    #[error("Got an Error during a reqwest request (download)")]
    DownloadReqwest(#[source] reqwest::Error),
}

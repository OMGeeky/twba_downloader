use crate::prelude::*;
use crate::twitch::TwitchClient;
use std::path::Path;
use twba_local_db::prelude::*;
use twba_local_db::re_exports::sea_orm::ActiveValue::Set;
use twba_local_db::re_exports::sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter, QuerySelect};

#[derive(Debug)]
pub struct DownloaderClient {
    db: DatabaseConnection,
    pub twitch_client: TwitchClient,
}

impl DownloaderClient {
    pub fn new(twitch_client: TwitchClient, db: DatabaseConnection) -> Self {
        Self { twitch_client, db }
    }
    #[tracing::instrument(skip(self))]
    pub async fn download_not_downloaded_videos(&self) -> Result<()> {
        info!("Downloading not downloaded videos");
        let output_folder: &Path =
            Path::new(self.twitch_client.config.download_folder_path.as_str());
        let videos = Videos::find()
            .filter(VideosColumn::Status.eq(Status::NotStarted))
            .limit(self.twitch_client.config.max_items_to_process)
            .all(&self.db)
            .await?;
        info!("Found {} videos to download", videos.len());

        for video in videos {
            let id = video.id;
            let quality = "max";
            let success = self.download_video(video, quality, output_folder).await;
            if let Err(err) = success {
                error!(
                    "Could not download video with id: {} because of err: {:?}",
                    id, err
                );
            } else {
                info!("Downloaded video with id: {}", id);
            }
        }
        info!("Finished downloading videos");

        Ok(())
    }

    pub async fn download_video_by_id<VideoId: DIntoString, Quality: DIntoString>(
        &self,
        video_id: VideoId,
        quality: Quality,
        output_folder: &Path,
    ) -> Result<()> {
        let video_id = video_id.into();
        let quality = quality.into();

        let video = Videos::find()
            .filter(VideosColumn::TwitchId.eq(&video_id))
            .one(&self.db)
            .await?
            .ok_or_else(|| DownloaderError::VideoNotFound(video_id))?;

        self.download_video(video, &quality, output_folder).await
    }

    pub async fn download_video(
        &self,
        video: VideosModel,
        quality: &str,
        output_folder: &Path,
    ) -> Result<()> {
        let id = video.id;
        let video_id = video.twitch_id.clone();
        let mut video = video.into_active_model();
        video.status = Set(Status::Downloading);
        video.clone().update(&self.db).await?;
        let download_result = self
            .twitch_client
            .download_video(id, video_id, quality, output_folder)
            .await;
        match download_result {
            Ok(path) => {
                info!("Downloaded video to {:?}", path);
                video.status = Set(Status::Downloaded);
                video.clone().update(&self.db).await?;
                Ok(())
            }
            Err(err) => {
                error!("Could not download video: {:?}", err);
                video.status = Set(Status::Failed);
                video.clone().update(&self.db).await?;
                Err(err)
            }
        }
    }
}

pub mod prelude;

use twba_backup_config::get_default_builder;
use prelude::*;
pub mod client;
mod errors;
pub mod twitch;

#[tokio::main]
async fn main() -> Result<()> {
    let _guard = twba_common::init_tracing("twba_downloader");
    info!("Hello, world!");

    let x = run().await;
    x.or_else(|e| match e {
        DownloaderError::LoadConfig(e) => {
            println!("Error while loading config: {}", e);
            Ok(())
        }
        e => Err(e),
    })?;

    info!("Bye");
    Ok(())
}

#[tracing::instrument]
async fn run() -> Result<()> {
    let conf = get_default_builder()
        .load()
        .map_err(|e| {
            error!("Failed to load config: {:?}", e);
            DownloaderError::LoadConfig(e.into())
        })?;

    let db = twba_local_db::open_database(Some(&conf.db_url)).await?;
    twba_local_db::migrate_db(&db).await?;
    // local_db::print_db(&db).await?;

    dbg!(&conf);
    let twitch_client = twitch::TwitchClient::new(conf);
    let client = client::DownloaderClient::new(twitch_client, db);

    client.download_not_downloaded_videos().await?;

    Ok(())
}

pub mod prelude;

use prelude::*;
pub mod client;
mod errors;
pub mod twitch;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_env_filter(
            "sea_orm=warn,sea_orm_migration=warn,sqlx=warn,downloader=trace,local_db=warn,reqwest-backoff=warn",
        )
        .init();
    info!("Hello, world!");

    run().await?;

    info!("Bye");
    Ok(())
}

#[tracing::instrument]
async fn run() -> Result<()> {
    let conf = Conf::builder()
        .env()
        .file("./settings.toml")
        .file("/home/omgeeky/twba/config.toml")
        .load()
        .map_err(|e| DownloaderError::LoadConfig(e.into()))?;

    let db = local_db::open_database(Some(&conf.db_url)).await?;
    local_db::migrate_db(&db).await?;
    // local_db::print_db(&db).await?;

    // dbg!(&conf);
    let twitch_client = twitch::TwitchClient::new(conf);
    let client = client::DownloaderClient::new(twitch_client, db);

    client.download_not_downloaded_videos().await?;

    Ok(())
}

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
            "sea_orm=warn,sea_orm_migration=warn,sqlx=warn,twba_downloader=trace,local_db=warn,twba_reqwest_backoff=warn",
        )
        .init();
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
    let conf = Conf::builder()
        .env()
        .file("./settings.toml")
        .file(shellexpand::tilde("~/twba/config.toml").to_string())
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

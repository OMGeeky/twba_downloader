use prelude::*;
use twba_backup_config::get_default_builder;
use twba_local_db::prelude::{Status, Videos, VideosColumn};
pub mod client;
mod errors;
pub mod prelude;
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
    let conf = get_default_builder().load().map_err(|e| {
        error!("Failed to load config: {:?}", e);
        DownloaderError::LoadConfig(e.into())
    })?;

    let db = twba_local_db::open_database(Some(&conf.db_url)).await?;
    twba_local_db::migrate_db(&db).await?;
    // local_db::print_db(&db).await?;

    dbg!(&conf);
    let amount_of_downloaded_but_not_uploaded_videos =
        get_amount_of_downloaded_but_not_uploaded_videos(&db).await?;
    //TODO: make configurable
    if amount_of_downloaded_but_not_uploaded_videos >= 3 {
        info!(
            "There are {} videos that are downloaded but not uploaded. Not downloading anything to prevent taking up all the space.",
            amount_of_downloaded_but_not_uploaded_videos
        );
        return Ok(());
    } else {
        info!(
            "There are {} videos that are downloaded but not uploaded. Downloading more videos.",
            amount_of_downloaded_but_not_uploaded_videos
        );
    }
    // let continue_ = wait_for_user().unwrap_or(true);
    // if !continue_ {
    //     info!("Quitting because user requested it.");
    //     return Ok(());
    // }
    let twitch_client = twitch::TwitchClient::new(conf);
    let client = client::DownloaderClient::new(twitch_client, db);

    client.download_not_downloaded_videos().await?;

    Ok(())
}

async fn get_amount_of_downloaded_but_not_uploaded_videos<C>(db: &C) -> Result<u64>
where
    C: twba_local_db::re_exports::sea_orm::ConnectionTrait,
{
    use twba_local_db::re_exports::sea_orm::*;
    Ok(Videos::find()
        .filter(VideosColumn::Status.between(Status::Downloading, Status::Uploading))
        .order_by_asc(VideosColumn::CreatedAt)
        .count(db)
        .await?)
}

pub fn wait_for_user() -> StdResult<bool, Box<dyn StdError>> {
    use std::io::{self, Write};
    loop {
        print!("Press Enter to continue or 'q' to quit: ");
        io::stdout().flush()?; // Make sure the prompt is immediately displayed

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        match input.trim() {
            "" => return Ok(true), // User pressed Enter
            "q" => {
                println!("Quitting...");
                return Ok(false);
            }
            _ => {
                println!("Invalid input. Please try again.");
                continue;
            } // Any other input, repeat the loop
        }
    }
}

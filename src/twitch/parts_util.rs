use super::*;
use tokio::io::BufWriter;

/// Sorts the parts by their number.
///  
/// The parts must be named like this: `1.ts`, `2.ts`, `3-muted.ts`, `4-unmuted.ts`, etc.
///
/// Optionally if  the number contains a single `-` like this: `1094734-1.ts`, `1094734-2.ts`, `1094734-3-muted.ts`, `1094734-4-unmuted.ts`, etc.
/// everything before the `-` will be ignored and it will try to parse the rest as a number.
///
/// If that all fails, it will panic!
pub fn sort_parts(files: &mut [PathBuf]) {
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
pub async fn combine_parts_to_single_ts(files: &[PathBuf], target: &Path) -> Result<()> {
    debug!("combining all parts of video");
    debug!("part amount: {}", files.len());
    let target = fs::File::create(target)
        .await
        .map_err(DownloadFileError::FileCreation)?;
    let mut target_buf = BufWriter::new(target);
    for file_path in files {
        trace!("{:?}", file_path.file_name());
        let mut file = fs::File::open(&file_path)
            .await
            .map_err(DownloadFileError::Read)?;

        tokio::io::copy(&mut file, &mut target_buf)
            .await
            .map_err(DownloadFileError::Write)?;

        tokio::fs::remove_file(&file_path)
            .await
            .map_err(DownloadFileError::Write)?;
    }
    target_buf.flush().await.map_err(DownloadFileError::Write)?;

    Ok(())
}

pub async fn combine_parts_to_mp4(parts: &[PathBuf], folder_path: &Path) -> Result<PathBuf> {
    let ts_file_path = folder_path.join("video.ts");
    let mp4_file_path = folder_path.join("video.mp4");

    combine_parts_to_single_ts(parts, &ts_file_path).await?;
    convert_ts_to_mp4(&ts_file_path, &mp4_file_path).await?;
    tokio::fs::remove_file(ts_file_path)
        .await
        .map_err(DownloadFileError::Filesystem)?;

    Ok(mp4_file_path)
}

#[instrument]
pub async fn convert_ts_to_mp4(ts_file: &Path, mp4_file: &Path) -> Result<()> {
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
#[instrument]
pub async fn download_part(
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
pub async fn try_download_part(
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

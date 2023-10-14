use crate::errors::{MalformedPlaylistError, PlaylistParseError};
use crate::prelude::StdResult;
use crate::prelude::*;
use chrono::{NaiveDateTime, Utc};
use std::collections::HashMap;

/// Converts a twitch date string to a chrono::DateTime<Utc>
///
/// Example: 2023-10-07T23:33:29
pub fn convert_twitch_date(date: &str) -> StdResult<chrono::DateTime<Utc>, PlaylistParseError> {
    let date = date.trim();
    let date = date.trim_matches('"');

    //parse the date from a string like this: 2023-10-07T23:33:29
    NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S")
        .map(|x| x.and_utc())
        .map_err(PlaylistParseError::InvalidTimeFormat)
}

pub fn parse_playlist(
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

#[tracing::instrument(skip(playlist))]
pub fn get_playlist_from_quality_list(playlist: String, quality: &str) -> Result<String> {
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

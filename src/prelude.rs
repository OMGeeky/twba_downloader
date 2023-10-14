pub use crate::errors::DownloaderError;
// pub(crate) use anyhow::Result;
pub(crate) use backup_config::prelude::*;
use std::fmt::Debug;
pub(crate) use tracing::{debug, error, info, trace, warn};

pub(crate) use std::result::Result as StdResult;

/// Just a wrapper around Into<String> that implements Debug.
///
/// This is just for convenience so we dont need to write
/// '`impl Into<String> + Debug`' everywhere.
pub trait DIntoString: Into<String> + Debug {}
impl<T> DIntoString for T where T: Into<String> + Debug {}

pub type Result<T> = StdResult<T, DownloaderError>;

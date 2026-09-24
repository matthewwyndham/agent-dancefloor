use crate::model::{ProcessSnapshot, Session};
use anyhow::Result;
use std::path::Path;

/// Claude's mature scanner remains the source of truth; this adapter is the
/// provider boundary used by the mixed refresh path.
pub fn discover(home: &Path, _processes: &ProcessSnapshot) -> Result<Vec<Session>> {
    crate::discovery::scan(home)
}

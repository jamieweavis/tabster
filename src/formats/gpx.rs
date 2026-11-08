//! Guitar Pro 6 `.gpx`: BCFZ (compressed) / BCFS file system containing `score.gpif`.
//! Owner: Phase 1 agent B.

use super::ParseError;
use crate::model::Score;

pub fn parse(bytes: &[u8]) -> Result<Score, ParseError> {
    let _ = bytes;
    Err(ParseError::Invalid(
        "GPX importer not implemented yet".into(),
    ))
}

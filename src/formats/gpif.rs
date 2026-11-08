//! GPIF XML (`score.gpif`) → [`Score`]. Shared by `.gp` (GP7/8) and `.gpx` (GP6).
//! Owner: Phase 1 agent A.

use super::ParseError;
use crate::model::Score;

/// Parse the contents of a `score.gpif` document.
pub fn parse_gpif(xml: &[u8]) -> Result<Score, ParseError> {
    let _ = xml;
    Err(ParseError::Invalid(
        "GPIF parser not implemented yet".into(),
    ))
}

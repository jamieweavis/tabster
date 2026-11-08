//! Guitar Pro 7/8 `.gp`: a zip containing `Content/score.gpif`.
//! Owner: Phase 1 agent A.

use super::ParseError;
use crate::model::Score;

pub fn parse(bytes: &[u8]) -> Result<Score, ParseError> {
    let _ = bytes;
    Err(ParseError::Invalid(
        "GP7 importer not implemented yet".into(),
    ))
}

//! Guitar Pro 5 `.gp5` binary format.
//! Owner: Phase 1 agent B.

use super::ParseError;
use crate::model::Score;

pub fn parse(bytes: &[u8]) -> Result<Score, ParseError> {
    let _ = bytes;
    Err(ParseError::Invalid(
        "GP5 importer not implemented yet".into(),
    ))
}

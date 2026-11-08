//! Tab file importers. Each format module turns raw file bytes into a [`Score`].
//!
//! Frozen contract: `detect` and `parse` signatures, and [`ParseError`]. Format modules
//! (`gp7`, `gpx`, `gp5`, `gpif`) are owned by the Phase 1 parser agents.

use crate::model::Score;

pub mod gp5;
pub mod gp7;
pub mod gpif;
pub mod gpx;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, uniffi::Enum,
)]
pub enum Format {
    /// Guitar Pro 7/8 `.gp`: zip containing `Content/score.gpif`.
    Gp7,
    /// Guitar Pro 6 `.gpx`: BCFZ/BCFS container around `score.gpif`.
    Gpx,
    /// Guitar Pro 5 `.gp5` (binary).
    Gp5,
    /// Guitar Pro 4 `.gp4` (binary). Phase 6.
    Gp4,
    /// Guitar Pro 3 `.gp3` (binary). Phase 6.
    Gp3,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Gp7 => "gp",
            Format::Gpx => "gpx",
            Format::Gp5 => "gp5",
            Format::Gp4 => "gp4",
            Format::Gp3 => "gp3",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Format> {
        match ext.to_ascii_lowercase().as_str() {
            "gp" => Some(Format::Gp7),
            "gpx" => Some(Format::Gpx),
            "gp5" => Some(Format::Gp5),
            "gp4" => Some(Format::Gp4),
            "gp3" => Some(Format::Gp3),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unrecognised file format")]
    UnknownFormat,
    #[error("{0:?} files aren't supported yet")]
    Unsupported(Format),
    #[error("corrupt container: {0}")]
    Container(String),
    #[error("invalid score data: {0}")]
    Invalid(String),
    #[error("unexpected end of file")]
    Eof,
}

/// Sniff the format from magic bytes, falling back to the file extension (without the dot).
pub fn detect(bytes: &[u8], extension: Option<&str>) -> Option<Format> {
    if bytes.starts_with(b"PK\x03\x04") {
        return Some(Format::Gp7);
    }
    if bytes.starts_with(b"BCFZ") || bytes.starts_with(b"BCFS") {
        return Some(Format::Gpx);
    }
    // GP3–5 start with a Pascal string: length byte then "FICHIER GUITAR PRO v5.xx" etc.
    if let Some(rest) = bytes.get(1..) {
        let head = &rest[..rest.len().min(30)];
        if head.starts_with(b"FICHIER GUITAR PRO v5") {
            return Some(Format::Gp5);
        }
        if head.starts_with(b"FICHIER GUITAR PRO v4") {
            return Some(Format::Gp4);
        }
        if head.starts_with(b"FICHIER GUITAR PRO v3") {
            return Some(Format::Gp3);
        }
    }
    extension.and_then(Format::from_extension)
}

/// Parse a tab file of any supported format.
pub fn parse(bytes: &[u8], extension: Option<&str>) -> Result<Score, ParseError> {
    match detect(bytes, extension).ok_or(ParseError::UnknownFormat)? {
        Format::Gp7 => gp7::parse(bytes),
        Format::Gpx => gpx::parse(bytes),
        Format::Gp5 => gp5::parse(bytes),
        f @ (Format::Gp4 | Format::Gp3) => Err(ParseError::Unsupported(f)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_magic() {
        assert_eq!(detect(b"PK\x03\x04....", None), Some(Format::Gp7));
        assert_eq!(detect(b"BCFZ....", None), Some(Format::Gpx));
        assert_eq!(
            detect(b"\x18FICHIER GUITAR PRO v5.10", None),
            Some(Format::Gp5)
        );
        assert_eq!(detect(b"????", Some("GP")), Some(Format::Gp7));
        assert_eq!(detect(b"????", Some("txt")), None);
    }
}

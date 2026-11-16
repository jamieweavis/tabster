//! Reading YouTube's search results: the JSON the web client gets from `youtubei/v1/search`, or the
//! same data embedded in a results page as `ytInitialData`.
//!
//! The structure isn't documented and moves around, so this doesn't follow a path: it walks the
//! whole tree for `videoRenderer` objects and takes what it understands from each. Live streams,
//! premieres and Shorts are dropped (nothing to sync to). Anything unexpected is skipped.

use serde_json::Value;

use super::text::{parse_clock, parse_count};
use super::{ChannelBadge, SearchError, VideoCandidate};
use crate::library::parse_youtube_id;

/// The videos in a search response, in YouTube's order. An error if it isn't a search
/// response at all (a consent page, an error page, a changed format).
pub fn search_response(json: &str) -> Result<Vec<VideoCandidate>, SearchError> {
    let value: Value = serde_json::from_str(json).map_err(|_| SearchError::Unrecognised)?;
    from_value(&value)
}

/// The videos in a `https://www.youtube.com/results?…` page, from its `ytInitialData`.
pub(crate) fn results_page(html: &str) -> Result<Vec<VideoCandidate>, SearchError> {
    let start = [
        "var ytInitialData = ",
        "window[\"ytInitialData\"] = ",
        "ytInitialData = ",
    ]
    .iter()
    .find_map(|marker| html.find(marker).map(|i| i + marker.len()))
    .ok_or(SearchError::Unrecognised)?;
    // The object is followed by `;</script>`; let the JSON parser find where it ends.
    let value: Value = serde_json::Deserializer::from_str(&html[start..])
        .into_iter::<Value>()
        .next()
        .and_then(Result::ok)
        .ok_or(SearchError::Unrecognised)?;
    from_value(&value)
}

fn from_value(value: &Value) -> Result<Vec<VideoCandidate>, SearchError> {
    // Every search response has `contents`; error and consent responses don't.
    if value.get("contents").is_none() {
        return Err(SearchError::Unrecognised);
    }
    let mut renderers = Vec::new();
    collect_renderers(value, &mut renderers, 0);
    let mut out: Vec<VideoCandidate> = Vec::new();
    for renderer in renderers {
        if let Some(mut video) = video(renderer)
            && out.iter().all(|v| v.youtube_id != video.youtube_id)
        {
            video.rank = out.len() as u32;
            out.push(video);
        }
    }
    Ok(out)
}

/// Responses are a few levels of nesting deep; this bounds a pathological one.
const MAX_DEPTH: usize = 64;

fn collect_renderers<'a>(value: &'a Value, out: &mut Vec<&'a Value>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "videoRenderer" {
                    out.push(child);
                } else if key != "adSlotRenderer" && key != "promotedSparklesWebRenderer" {
                    collect_renderers(child, out, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_renderers(item, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn video(r: &Value) -> Option<VideoCandidate> {
    let youtube_id = r.get("videoId")?.as_str()?;
    // Validate it, since it ends up in URLs.
    let youtube_id = parse_youtube_id(youtube_id).filter(|id| id == youtube_id)?;
    if is_live_or_upcoming(r) || is_short(r) {
        return None;
    }
    let title = text(r.get("title")?)?;
    let channel = ["ownerText", "longBylineText", "shortBylineText"]
        .iter()
        .find_map(|k| r.get(*k).and_then(text))
        .unwrap_or_default();
    let duration_secs = r
        .get("lengthText")
        .and_then(text)
        .and_then(|t| parse_clock(&t));
    let views = r
        .get("viewCountText")
        .and_then(text)
        .and_then(|t| parse_count(&t));
    let description = r
        .get("detailedMetadataSnippets")
        .and_then(|s| s.get(0))
        .and_then(|s| s.get("snippetText"))
        .or_else(|| r.get("descriptionSnippet"))
        .and_then(text)
        .unwrap_or_default();
    Some(VideoCandidate {
        youtube_id,
        title,
        channel,
        duration_secs,
        views,
        badge: badge(r),
        description,
        rank: 0,
    })
}

/// `{"simpleText": "…"}` or `{"runs": [{"text": "…"}, …]}`.
fn text(v: &Value) -> Option<String> {
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return Some(s.trim().to_string());
    }
    let runs = v.get("runs")?.as_array()?;
    let joined: String = runs
        .iter()
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .collect();
    let joined = joined.trim();
    (!joined.is_empty()).then(|| joined.to_string())
}

fn badge_styles(v: Option<&Value>) -> impl Iterator<Item = &str> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| b.get("metadataBadgeRenderer")?.get("style")?.as_str())
}

fn badge(r: &Value) -> ChannelBadge {
    let mut badge = ChannelBadge::None;
    for style in badge_styles(r.get("ownerBadges")) {
        match style {
            "BADGE_STYLE_TYPE_VERIFIED_ARTIST" => return ChannelBadge::OfficialArtist,
            "BADGE_STYLE_TYPE_VERIFIED" => badge = ChannelBadge::Verified,
            _ => {}
        }
    }
    badge
}

fn is_live_or_upcoming(r: &Value) -> bool {
    r.get("upcomingEventData").is_some()
        || badge_styles(r.get("badges")).any(|s| s == "BADGE_STYLE_TYPE_LIVE_NOW")
        || overlay_styles(r).any(|s| s == "LIVE" || s == "UPCOMING")
}

fn is_short(r: &Value) -> bool {
    overlay_styles(r).any(|s| s == "SHORTS")
        || r.pointer("/navigationEndpoint/reelWatchEndpoint").is_some()
}

fn overlay_styles(r: &Value) -> impl Iterator<Item = &str> {
    r.get("thumbnailOverlays")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| {
            o.get("thumbnailOverlayTimeStatusRenderer")?
                .get("style")?
                .as_str()
        })
}

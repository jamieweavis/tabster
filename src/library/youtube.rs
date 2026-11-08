//! YouTube URL parsing, by hand (no regex).

/// See [`super::parse_youtube_id`].
pub(super) fn parse_youtube_id(input: &str) -> Option<String> {
    let input = input.trim();
    if is_video_id(input) {
        return Some(input.to_string());
    }
    let rest = strip_prefix_ignore_case(input, "https://")
        .or_else(|| strip_prefix_ignore_case(input, "http://"))
        .unwrap_or(input);
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(host_end);
    let tail = tail.split('#').next().unwrap_or_default();
    let (path, query) = tail.split_once('?').unwrap_or((tail, ""));
    let mut segments = path.split('/').filter(|s| !s.is_empty());

    let id = match host.to_ascii_lowercase().as_str() {
        "youtu.be" | "www.youtu.be" => segments.next(),
        "youtube.com"
        | "www.youtube.com"
        | "m.youtube.com"
        | "music.youtube.com"
        | "youtube-nocookie.com"
        | "www.youtube-nocookie.com" => match segments.next() {
            Some("watch") => query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find_map(|(key, value)| (key == "v").then_some(value)),
            Some("shorts" | "embed" | "live" | "v" | "e") => segments.next(),
            _ => None,
        },
        _ => None,
    }?;
    is_video_id(id).then(|| id.to_string())
}

/// Exactly 11 characters of `[A-Za-z0-9_-]`.
fn is_video_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

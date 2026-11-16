//! Search YouTube for a song and print how Tabster ranks the results.
//!
//! `cargo run --example yt_search -- "<artist>" "<title>" <tab length, seconds or m:ss> [results.json]`
//!
//! With a saved `youtubei/v1/search` response, ranks that instead of searching.

use tabster_core::video_search::{self, Song, YouTubeClient};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [artist, title, length, rest @ ..] = args.as_slice() else {
        eprintln!("usage: yt_search <artist> <title> <length> [results.json]");
        std::process::exit(2);
    };
    let length = parse_length(length).unwrap_or_else(|| {
        eprintln!("length must be seconds or m:ss");
        std::process::exit(2);
    });
    let song = Song::new(title, artist, length, "");
    println!("query: {}\n", song.query());
    let results = match rest.first() {
        Some(path) => {
            let json = std::fs::read_to_string(path).expect("reading the results file");
            let candidates =
                video_search::parse::search_response(&json).expect("a search response");
            video_search::rank(&song, &candidates)
        }
        None => video_search::find(&YouTubeClient::new(), &song).unwrap_or_else(|e| {
            eprintln!("search failed: {e}");
            std::process::exit(1);
        }),
    };
    for m in results {
        let c = &m.candidate;
        let length = c
            .duration_secs
            .map(|d| format!("{}:{:02}", d as u64 / 60, d as u64 % 60))
            .unwrap_or_else(|| "?".into());
        println!(
            "{:.3} {:<6} {:<12} {} {:>6}  {}  [{}]",
            m.score,
            format!("{:?}", m.confidence),
            m.label,
            c.youtube_id,
            length,
            c.title,
            c.channel
        );
        println!("{:>34}{}", "", m.reasons.join(" · "));
    }
}

fn parse_length(s: &str) -> Option<f64> {
    match s.split_once(':') {
        Some((m, sec)) => Some(m.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?),
        None => s.parse().ok(),
    }
}

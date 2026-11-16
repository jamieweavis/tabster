use std::path::Path;

use parking_lot::Mutex;

use super::client::Searcher;
use super::text::{fold, parse_clock, parse_count, split_brackets, tokens, word_similarity};
use super::*;

// ---------------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------------

#[test]
fn folds_case_accents_and_punctuation() {
    assert_eq!(fold("Beyoncé — Déjà Vu!"), "beyonce deja vu");
    assert_eq!(fold("Guns N' Roses"), "guns n roses");
    assert_eq!(fold("Don’t Stop Me Now"), "dont stop me now");
    assert_eq!(fold("Simon & Garfunkel"), "simon and garfunkel");
    assert_eq!(fold("AC/DC"), "ac dc");
    assert_eq!(fold("  (Live)  "), "live");
    assert_eq!(
        tokens("Master of Puppets [HD]"),
        ["master", "of", "puppets", "hd"]
    );
}

#[test]
fn words_with_a_typo_are_similar_but_short_words_and_numbers_must_match() {
    assert!(word_similarity("sultans", "sultnas") > 0.7);
    assert!(word_similarity("swing", "swnig") > 0.0);
    assert_eq!(word_similarity("one", "ono"), 0.0);
    assert_eq!(word_similarity("1999", "1998"), 0.0);
    assert!(word_similarity("puppets", "muppets") > 0.8);
    assert_eq!(word_similarity("hello", "world"), 0.0);
}

#[test]
fn splits_brackets() {
    assert_eq!(
        split_brackets("Song (Live) [HD]"),
        (
            "Song    ".to_string(),
            vec!["Live".to_string(), "HD".to_string()]
        )
    );
    assert_eq!(
        split_brackets("Song (Live (Remastered))").1,
        vec!["Live (Remastered)".to_string()]
    );
    // Unclosed: kept as text.
    assert_eq!(split_brackets("Song (Live").0.trim(), "Song  Live");
}

#[test]
fn parses_clocks_and_counts() {
    assert_eq!(parse_clock("4:05"), Some(245.0));
    assert_eq!(parse_clock("1:02:03"), Some(3723.0));
    assert_eq!(parse_clock("0:09"), Some(9.0));
    assert_eq!(parse_clock("LIVE"), None);
    assert_eq!(parse_clock(""), None);
    assert_eq!(parse_clock("1:2:3:4"), None);
    assert_eq!(parse_count("1,234,567 views"), Some(1_234_567));
    assert_eq!(parse_count("No views"), Some(0));
    assert_eq!(parse_count("views"), None);
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

fn fixture() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/youtube/search.json"),
    )
    .unwrap()
}

#[test]
fn parses_a_search_response() {
    let videos = parse::search_response(&fixture()).unwrap();
    let ids: Vec<&str> = videos.iter().map(|v| v.youtube_id.as_str()).collect();
    // Ads, duplicates, live streams, premieres, Shorts, bad ids and untitled entries are dropped.
    assert_eq!(ids, ["abcdefghijk", "lmnopqrstuv", "minimal0000"]);

    let first = &videos[0];
    assert_eq!(first.title, "Example Song (Remastered)");
    assert_eq!(first.channel, "The Examples");
    assert_eq!(first.duration_secs, Some(245.0));
    assert_eq!(first.views, Some(1_234_567));
    assert_eq!(first.badge, ChannelBadge::OfficialArtist);
    assert_eq!(first.description, "Provided to YouTube by Example Records");
    assert_eq!(first.rank, 0);

    let nested = &videos[1];
    assert_eq!(nested.channel, "Fan Uploads");
    assert_eq!(nested.duration_secs, Some(3723.0));
    assert_eq!(nested.views, Some(0));
    assert_eq!(nested.badge, ChannelBadge::Verified);
    assert_eq!(nested.rank, 1);

    let minimal = &videos[2];
    assert_eq!(minimal.title, "Minimal");
    assert_eq!(
        (
            minimal.channel.as_str(),
            minimal.duration_secs,
            minimal.views
        ),
        ("", None, None)
    );
}

#[test]
fn parses_a_results_page() {
    let html = format!(
        "<html><script>var ytInitialData = {};</script><script>var other = {{}};</script></html>",
        fixture()
    );
    assert_eq!(parse::results_page(&html).unwrap().len(), 3);
}

#[test]
fn rejects_responses_that_arent_searches() {
    assert_eq!(
        parse::search_response("not json"),
        Err(SearchError::Unrecognised)
    );
    assert_eq!(
        parse::search_response(r#"{"error": {"code": 400}}"#),
        Err(SearchError::Unrecognised)
    );
    assert_eq!(
        parse::results_page("<html>consent</html>"),
        Err(SearchError::Unrecognised)
    );
    // A search with no videos is fine.
    assert_eq!(parse::search_response(r#"{"contents": {}}"#), Ok(vec![]));
}

#[test]
fn survives_absurd_nesting() {
    let mut json = String::new();
    for _ in 0..10_000 {
        json.push_str("{\"contents\":");
    }
    json.push('1');
    for _ in 0..10_000 {
        json.push('}');
    }
    // serde_json refuses to go this deep, which is an error, not a crash.
    assert!(parse::search_response(&json).is_err());
}

// ---------------------------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------------------------

struct C<'a> {
    title: &'a str,
    channel: &'a str,
    secs: f64,
    badge: ChannelBadge,
    description: &'a str,
}

fn video(id: usize, c: C) -> VideoCandidate {
    VideoCandidate {
        youtube_id: format!("vid{id:08}"),
        title: c.title.into(),
        channel: c.channel.into(),
        duration_secs: Some(c.secs),
        views: Some(1_000_000),
        badge: c.badge,
        description: c.description.into(),
        rank: id as u32,
    }
}

fn plain(title: &str, channel: &str, secs: f64) -> C<'static> {
    C {
        title: Box::leak(title.to_string().into_boxed_str()),
        channel: Box::leak(channel.to_string().into_boxed_str()),
        secs,
        badge: ChannelBadge::None,
        description: "",
    }
}

fn official(title: &str, channel: &str, secs: f64) -> C<'static> {
    C {
        badge: ChannelBadge::OfficialArtist,
        ..plain(title, channel, secs)
    }
}

fn art_track(title: &str, channel: &str, secs: f64) -> C<'static> {
    C {
        description: "Provided to YouTube by Example Records",
        ..official(title, channel, secs)
    }
}

fn ranked(song: &Song, videos: Vec<C>) -> Vec<VideoMatch> {
    let candidates: Vec<VideoCandidate> = videos
        .into_iter()
        .enumerate()
        .map(|(i, c)| video(i, c))
        .collect();
    rank(song, &candidates)
}

fn one(song: &Song, c: C) -> VideoMatch {
    ranked(song, vec![c]).remove(0)
}

fn titles(matches: &[VideoMatch]) -> Vec<&str> {
    matches.iter().map(|m| m.candidate.title.as_str()).collect()
}

#[test]
fn prefers_official_audio_then_the_artists_video_then_fan_uploads() {
    let song = Song::new("Hold the Line", "Toto", 236.0, "");
    let results = ranked(
        &song,
        vec![
            plain("Toto - Hold The Line (Lyrics)", "Rock Lyrics", 235.0),
            official("Toto - Hold The Line (Official Video)", "TOTO", 239.0),
            art_track("Hold the Line", "TOTO", 236.0),
        ],
    );
    assert_eq!(
        titles(&results),
        [
            "Hold the Line",
            "Toto - Hold The Line (Official Video)",
            "Toto - Hold The Line (Lyrics)"
        ]
    );
    assert_eq!(results[0].confidence, MatchConfidence::High);
    assert_eq!(results[0].label, "Original");
    assert_eq!(
        results[0].reasons,
        ["Official audio", "Same length as the tab"]
    );
}

#[test]
fn topic_and_vevo_channels_are_the_artists() {
    let song = Song::new("Example Song", "The Examples", 200.0, "");
    let topic = one(&song, plain("Example Song", "The Examples - Topic", 201.0));
    assert_eq!(topic.reasons[0], "Official audio");
    assert_eq!(topic.confidence, MatchConfidence::High);
    let vevo = one(
        &song,
        plain("The Examples - Example Song", "TheExamplesVEVO", 214.0),
    );
    assert_eq!(vevo.reasons[0], "Artist's channel");
}

#[test]
fn length_separates_versions_of_the_same_song() {
    let song = Song::new("Hold the Line", "Toto", 236.0, "");
    let results = ranked(
        &song,
        vec![
            art_track("Hold the Line (Single Version)", "TOTO", 211.0),
            art_track("Hold the Line", "TOTO", 236.0),
        ],
    );
    assert_eq!(results[0].candidate.title, "Hold the Line");
    assert_eq!(results[1].confidence, MatchConfidence::Medium);
    assert!(
        results[1]
            .reasons
            .contains(&"0:25 shorter than the tab".to_string()),
        "{:?}",
        results[1].reasons
    );
    assert!(
        results[1]
            .reasons
            .contains(&"Alternate version".to_string())
    );
}

#[test]
fn a_music_video_with_an_intro_is_still_a_confident_match() {
    let song = Song::new("Example Song", "The Examples", 240.0, "");
    let video = one(
        &song,
        official(
            "The Examples - Example Song (Official Music Video)",
            "The Examples",
            275.0,
        ),
    );
    assert_eq!(video.confidence, MatchConfidence::High);
    assert_eq!(
        video.reasons,
        ["Artist's channel", "0:35 longer than the tab"]
    );
    assert_eq!(video.length_difference_secs, Some(35.0));
}

#[test]
fn clips_and_albums_are_not_matches() {
    let song = Song::new("Playing God", "Polyphia", 204.0, "");
    let intro = one(
        &song,
        plain("Polyphia - Playing God intro", "Guitar Bits", 39.0),
    );
    assert_eq!(intro.confidence, MatchConfidence::Low);
    let album = one(
        &song,
        plain("Polyphia - Playing God [Full Album]", "Uploads", 2400.0),
    );
    assert_eq!(album.confidence, MatchConfidence::Low);
}

#[test]
fn covers_lessons_and_backing_tracks_are_labelled_and_never_confident() {
    let song = Song::new("Playing God", "Polyphia", 204.0, "");
    for (title, label) in [
        ("Polyphia - Playing God (Guitar Cover)", "Cover"),
        ("Playing God - Polyphia | Guitar Lesson + TABS", "Lesson"),
        ("Polyphia - Playing God (Band Playthrough)", "Lesson"),
        ("How To Play Playing God by Polyphia", "Lesson"),
        ("Polyphia - Playing God Backing Track", "Backing Track"),
        ("Polyphia - Playing God (Acoustic)", "Acoustic"),
    ] {
        let m = one(&song, official(title, "Polyphia", 205.0));
        assert_eq!(m.label, label, "{title}");
        assert_ne!(m.confidence, MatchConfidence::High, "{title}");
    }
    for title in [
        "Polyphia - Playing God (slowed + reverb)",
        "Polyphia - Playing God but it's jazz",
        "First Time Hearing Polyphia - Playing God (Reaction)",
        "Polyphia - Playing God Nightcore",
    ] {
        let m = one(&song, plain(title, "Someone", 205.0));
        assert_eq!(m.label, "Original", "{title}");
        assert_ne!(m.confidence, MatchConfidence::High, "{title}");
    }
}

#[test]
fn recognises_live_recordings_that_dont_say_live() {
    let song = Song::new("Master of Puppets", "Metallica", 515.0, "");
    for title in [
        "Metallica: Master of Puppets (Manchester, England - June 18, 2019)",
        "Metallica - Master of Puppets [Seattle '89]",
        "Metallica - Master of Puppets - Manchester 2005",
        "Metallica - Master of Puppets (Manchester 11-7-2025)",
        "Metallica - Master of Puppets (Live in Moscow, 1991)",
    ] {
        let m = one(&song, official(title, "Metallica", 530.0));
        assert_eq!(m.label, "Live", "{title}");
        assert_ne!(m.confidence, MatchConfidence::High, "{title}");
    }
    let described = one(
        &song,
        C {
            description: "Filmed at Etihad Stadium in Manchester",
            ..official("Metallica: Master of Puppets", "Metallica", 530.0)
        },
    );
    assert_eq!(described.label, "Live");
    // Not dates or places.
    for title in [
        "Metallica - Master of Puppets (Remastered 2017)",
        "Metallica - Master of Puppets (Official Audio) 1986",
        "Master of Puppets (Remastered)",
    ] {
        let m = one(&song, official(title, "Metallica", 516.0));
        assert_eq!(m.label, "Original", "{title}");
        assert_eq!(m.confidence, MatchConfidence::High, "{title}");
    }
}

#[test]
fn a_song_named_live_isnt_a_live_recording() {
    let song = Song::new("Live Forever", "Oasis", 276.0, "");
    let studio = one(
        &song,
        art_track("Live Forever (Remastered)", "Oasis", 277.0),
    );
    assert_eq!(
        (studio.label.as_str(), studio.confidence),
        ("Original", MatchConfidence::High)
    );
    let live = one(
        &song,
        official("Oasis - Live Forever (Live at Wembley)", "Oasis", 300.0),
    );
    assert_eq!(live.label, "Live");
    let tagged = one(
        &song,
        plain(
            "[LIVE 25] Oasis play Live Forever at Wembley",
            "Fans",
            290.0,
        ),
    );
    assert_eq!(tagged.label, "Live");
}

#[test]
fn a_tab_that_asks_for_a_version_prefers_it() {
    let song = Song::new("Hold the Line (Live)", "Toto", 290.0, "");
    assert_eq!(song.query(), "Toto Hold the Line live");
    let results = ranked(
        &song,
        vec![
            art_track("Hold the Line", "TOTO", 236.0),
            official("Toto - Hold the Line (Live in Paris 1990)", "TOTO", 288.0),
        ],
    );
    assert_eq!(results[0].label, "Live");
    // So does the user's own search.
    let song = Song::new(
        "Hold the Line",
        "Toto",
        236.0,
        "toto hold the line acoustic",
    );
    let m = one(
        &song,
        official("Toto - Hold the Line (Acoustic)", "TOTO", 240.0),
    );
    assert_eq!(
        (m.label.as_str(), m.confidence),
        ("Acoustic", MatchConfidence::High)
    );
}

#[test]
fn tolerates_typos_and_word_order() {
    let song = Song::new("Sultans of Swing", "Dire Straits", 348.0, "");
    let typo = one(
        &song,
        official("Dire Straits - Sultans of Swnig", "Dire Straits", 349.0),
    );
    assert_eq!(typo.confidence, MatchConfidence::High);
    let other = one(
        &song,
        official("Dire Straits - Walk of Life", "Dire Straits", 252.0),
    );
    assert_eq!(other.confidence, MatchConfidence::Low);
    assert!(other.reasons.contains(&"Different title".to_string()));
}

#[test]
fn another_artists_song_of_the_same_name_is_not_confident() {
    let song = Song::new("Hello", "Adele", 295.0, "");
    let m = one(
        &song,
        official("Lionel Richie - Hello", "Lionel Richie", 248.0),
    );
    assert_ne!(m.confidence, MatchConfidence::High);
    let right = one(
        &song,
        official("Adele - Hello (Official Music Video)", "Adele", 366.0),
    );
    assert!(right.score > m.score);
}

#[test]
fn prepares_the_song_from_messy_metadata() {
    assert_eq!(
        Song::new("Dire Straits - Sultans of Swing", "", 348.0, "").query(),
        "Dire Straits Sultans of Swing"
    );
    assert_eq!(
        Song::new("Toto - Hold the Line", "Toto", 236.0, "").query(),
        "Toto Hold the Line"
    );
    assert_eq!(
        Song::new("Señorita (feat. Camila Cabello)", "Shawn Mendes", 190.0, "").query(),
        "Shawn Mendes Señorita"
    );
    assert_eq!(
        Song::new("Stay ft. Justin Bieber", "The Kid LAROI", 141.0, "").query(),
        "The Kid LAROI Stay"
    );
    assert_eq!(Song::new("(Untitled)", "", 0.0, "").query(), "(Untitled)");
    assert!(!Song::new("Sultans of Swing", "", 0.0, "").has_artist());
}

#[test]
fn a_missing_artist_is_inferred_from_the_results_but_never_confident() {
    let song = Song::new("Sultans of Swing", "", 348.0, "");
    let results = ranked(
        &song,
        vec![
            plain(
                "Dire Straits - Sultans Of Swing (Lyrics)",
                "Lyrics Channel",
                349.0,
            ),
            art_track("Sultans Of Swing", "Dire Straits", 355.0),
            plain("Dire Straits - Sultans Of Swing", "Someone", 347.0),
            plain("Sultans of Swing - Guitar Cover", "Guitarist", 350.0),
        ],
    );
    assert_eq!(results[0].candidate.title, "Sultans Of Swing");
    assert_eq!(results[0].reasons[0], "Official audio");
    assert!(
        results
            .iter()
            .all(|m| m.confidence != MatchConfidence::High)
    );
    assert_eq!(results[0].confidence, MatchConfidence::Medium);
}

#[test]
fn a_tab_without_a_usable_length_is_never_confident() {
    let song = Song::new("Example Song", "The Examples", 4.0, "");
    let m = one(&song, art_track("Example Song", "The Examples", 200.0));
    assert_eq!(m.length_difference_secs, None);
    assert_eq!(m.confidence, MatchConfidence::Medium);
}

// ---------------------------------------------------------------------------------------------
// Searching
// ---------------------------------------------------------------------------------------------

/// Canned responses, by query; records the queries.
#[derive(Default)]
struct Canned {
    responses: Vec<(String, Vec<VideoCandidate>)>,
    queries: Mutex<Vec<String>>,
}

impl Searcher for Canned {
    fn search(&self, query: &str) -> Result<Vec<VideoCandidate>, SearchError> {
        self.queries.lock().push(query.to_string());
        Ok(self
            .responses
            .iter()
            .find(|(q, _)| q == query)
            .map(|(_, r)| r.clone())
            .unwrap_or_default())
    }
}

#[test]
fn a_confident_first_search_is_enough() {
    let song = Song::new("Hold the Line", "Toto", 236.0, "");
    let searcher = Canned {
        responses: vec![(
            "Toto Hold the Line".into(),
            vec![video(0, art_track("Hold the Line", "TOTO", 236.0))],
        )],
        ..Canned::default()
    };
    let results = find(&searcher, &song).unwrap();
    assert_eq!(results[0].confidence, MatchConfidence::High);
    assert_eq!(*searcher.queries.lock(), ["Toto Hold the Line"]);
}

#[test]
fn asks_again_for_official_audio_and_merges_the_results() {
    let song = Song::new("Hold the Line", "Toto", 236.0, "");
    let searcher = Canned {
        responses: vec![
            (
                "Toto Hold the Line".into(),
                vec![
                    video(
                        0,
                        plain("Toto - Hold the Line (Guitar Cover)", "Guitarist", 236.0),
                    ),
                    video(1, plain("Toto - Hold the Line (Lyrics)", "Fan", 236.0)),
                ],
            ),
            (
                "Toto Hold the Line official audio".into(),
                vec![
                    video(1, plain("Toto - Hold the Line (Lyrics)", "Fan", 236.0)),
                    video(2, art_track("Hold the Line", "TOTO", 236.0)),
                ],
            ),
        ],
        ..Canned::default()
    };
    let results = find(&searcher, &song).unwrap();
    assert_eq!(searcher.queries.lock().len(), 2);
    assert_eq!(results.len(), 3, "duplicates merged");
    assert_eq!(results[0].candidate.title, "Hold the Line");
    assert!(
        results[0].candidate.rank >= 2,
        "ranked after the first search's results"
    );
}

//! Finding videos through `Core`: automatic searches after import, attaching confident matches,
//! suggestions, manual searches, dismissing, and rate limiting. YouTube is a fake.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tabster_core::library::{TabFilter, TabId, TabSummary, VideoSearchState};
use tabster_core::timeline::Timeline;
use tabster_core::video_search::{
    ChannelBadge, MatchConfidence, SearchError, Searcher, VideoCandidate,
};
use tabster_core::{Core, LibraryObserver};

type Respond = dyn Fn(&str) -> Result<Vec<VideoCandidate>, SearchError> + Send + Sync;

/// Answers each query with `respond`, recording the queries.
struct Fake {
    respond: Mutex<Box<Respond>>,
    queries: Mutex<Vec<String>>,
}

impl Fake {
    fn new(
        respond: impl Fn(&str) -> Result<Vec<VideoCandidate>, SearchError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            respond: Mutex::new(Box::new(respond)),
            queries: Mutex::new(Vec::new()),
        })
    }

    fn queries(&self) -> Vec<String> {
        self.queries.lock().clone()
    }
}

impl Searcher for Fake {
    fn search(&self, query: &str) -> Result<Vec<VideoCandidate>, SearchError> {
        self.queries.lock().push(query.to_string());
        (self.respond.lock())(query)
    }
}

/// `testdata/gp5/basics.gp5` ("Tabster Basics" by "Tabster") retitled, so each copy is distinct.
fn tab_file(dir: &Path, title: &str) -> PathBuf {
    let mut bytes =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/gp5/basics.gp5")).unwrap();
    const OFFSET: usize = 0x24;
    assert_eq!(&bytes[OFFSET..OFFSET + 14], b"Tabster Basics");
    assert_eq!(title.len(), 14);
    bytes[OFFSET..OFFSET + 14].copy_from_slice(title.as_bytes());
    let path = dir.join(format!("{title}.gp5"));
    fs::write(&path, bytes).unwrap();
    path
}

/// The fixture's playing length.
fn tab_length() -> f64 {
    let bytes =
        fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/gp5/basics.gp5")).unwrap();
    let score = tabster_core::formats::parse(&bytes, Some("gp5")).unwrap();
    Timeline::new(&score).duration_secs()
}

/// An auto-generated upload of `title` by Tabster, `secs` long.
fn art_track(id: &str, title: &str, secs: f64) -> VideoCandidate {
    VideoCandidate {
        youtube_id: id.into(),
        title: title.into(),
        channel: "Tabster - Topic".into(),
        duration_secs: Some(secs),
        views: Some(10_000),
        badge: ChannelBadge::None,
        description: "Provided to YouTube by Tabster Records".into(),
        rank: 0,
    }
}

/// "Tabster Variant 001" → "Variant 001".
fn title_of(query: &str) -> String {
    query
        .strip_prefix("Tabster ")
        .unwrap_or(query)
        .trim_end_matches(" official audio")
        .to_string()
}

fn core(dir: &Path, fake: &Arc<Fake>, config: &str) -> Arc<Core> {
    let config_path = dir.join("tabster.toml");
    fs::write(&config_path, config).unwrap();
    Core::with_searcher(
        &dir.join("data"),
        &config_path,
        fake.clone(),
        Duration::ZERO,
    )
    .unwrap()
}

fn summary(core: &Core, id: TabId) -> TabSummary {
    core.tab(id).unwrap()
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn import(core: &Core, path: &Path) -> TabId {
    let outcomes = core.import_paths(vec![path.to_string_lossy().into_owned()]);
    outcomes[0].id.unwrap()
}

fn searched(core: &Core, id: TabId) -> bool {
    core.video_search(id)
        .unwrap()
        .is_some_and(|s| s.state != VideoSearchState::Pending)
}

#[derive(Default)]
struct Finished(Mutex<Vec<TabId>>);

impl LibraryObserver for Finished {
    fn import_progress(&self, _done: u32, _total: u32) {}
    fn library_changed(&self) {}
    fn tab_opened(&self, _id: TabId) {}
    fn video_search_finished(&self, tab: TabId) {
        self.0.lock().push(tab);
    }
}

#[test]
fn importing_attaches_a_confident_match() {
    let dir = tempfile::tempdir().unwrap();
    let length = tab_length();
    let fake = Fake::new(move |q| Ok(vec![art_track("aaaaaaaaaaa", &title_of(q), length)]));
    let core = core(dir.path(), &fake, "");
    let finished = Arc::new(Finished::default());
    core.set_library_observer(Some(finished.clone()));

    let id = import(&core, &tab_file(dir.path(), "Variant 000001"));
    wait_until("the search", || searched(&core, id));

    assert_eq!(fake.queries(), ["Tabster Variant 000001"]);
    let videos = core.videos(id).unwrap();
    assert_eq!(videos.len(), 1);
    assert_eq!(videos[0].youtube_id, "aaaaaaaaaaa");
    assert_eq!(videos[0].label, "Original");
    assert_eq!(videos[0].title, "Variant 000001");
    assert_eq!(videos[0].channel, "Tabster - Topic");
    assert!(videos[0].is_default);
    let search = core.video_search(id).unwrap().unwrap();
    assert_eq!(search.attached_youtube_id.as_deref(), Some("aaaaaaaaaaa"));
    assert_eq!(search.results[0].confidence, MatchConfidence::High);
    let tab = summary(&core, id);
    assert!(tab.has_video && !tab.finding_video && !tab.video_suggested);
    wait_until("the callback", || finished.0.lock().contains(&id));

    // Importing the same file again doesn't search again.
    import(&core, &tab_file(dir.path(), "Variant 000001"));
    thread::sleep(Duration::from_millis(50));
    assert_eq!(fake.queries().len(), 1);
}

#[test]
fn a_likely_match_is_suggested_not_attached() {
    let dir = tempfile::tempdir().unwrap();
    let length = tab_length();
    // A live recording: the song, but not the version the tab is.
    let fake = Fake::new(move |q| {
        Ok(vec![art_track(
            "bbbbbbbbbbb",
            &format!("{} (Live)", title_of(q)),
            length,
        )])
    });
    let core = core(dir.path(), &fake, "");
    let id = import(&core, &tab_file(dir.path(), "Variant 000002"));
    wait_until("the search", || searched(&core, id));

    assert!(core.videos(id).unwrap().is_empty());
    let search = core.video_search(id).unwrap().unwrap();
    assert_eq!(search.state, VideoSearchState::Done);
    assert_eq!(search.attached_youtube_id, None);
    assert_eq!(search.results[0].confidence, MatchConfidence::Medium);
    assert!(summary(&core, id).video_suggested);
    // Not confident, so it also asked for the official audio.
    assert_eq!(
        fake.queries(),
        [
            "Tabster Variant 000002",
            "Tabster Variant 000002 official audio"
        ]
    );

    // Attaching the suggestion keeps its details and ends the suggestion.
    assert_eq!(search.results[0].label, "Live");
    let best = search.results[0].candidate.clone();
    core.attach_video(id, best, "Live".into()).unwrap();
    assert_eq!(core.videos(id).unwrap()[0].title, "Variant 000002 (Live)");
    let tab = summary(&core, id);
    assert!(tab.has_video && !tab.video_suggested);

    // Dismissing hides suggestions for good.
    let other = import(&core, &tab_file(dir.path(), "Variant 000003"));
    wait_until("the search", || searched(&core, other));
    assert!(summary(&core, other).video_suggested);
    core.dismiss_video_suggestions(other).unwrap();
    assert!(!summary(&core, other).video_suggested);
    assert!(core.video_search(other).unwrap().unwrap().dismissed);
}

#[test]
fn settings_turn_off_finding_and_attaching() {
    let dir = tempfile::tempdir().unwrap();
    let length = tab_length();
    let fake = Fake::new(move |q| Ok(vec![art_track("ccccccccccc", &title_of(q), length)]));
    let core = core(dir.path(), &fake, "auto_find_videos = false\n");
    let id = import(&core, &tab_file(dir.path(), "Variant 000004"));
    thread::sleep(Duration::from_millis(50));
    assert!(fake.queries().is_empty());
    assert_eq!(core.video_search(id).unwrap(), None);

    // Asking still searches; with attaching off the match is only suggested.
    let mut config = core.config();
    config.auto_attach_videos = false;
    core.set_config(config);
    core.find_videos(vec![id]).unwrap();
    wait_until("the search", || searched(&core, id));
    assert!(core.videos(id).unwrap().is_empty());
    assert!(summary(&core, id).video_suggested);
}

#[test]
fn asking_searches_tabs_that_already_have_a_video_without_attaching() {
    let dir = tempfile::tempdir().unwrap();
    let length = tab_length();
    let fake = Fake::new(move |q| Ok(vec![art_track("ddddddddddd", &title_of(q), length)]));
    let core = core(dir.path(), &fake, "auto_find_videos = false\n");
    let id = import(&core, &tab_file(dir.path(), "Variant 000005"));
    core.add_video(id, "eeeeeeeeeee".into(), "Live".into())
        .unwrap();

    core.find_videos(vec![id]).unwrap();
    assert!(summary(&core, id).finding_video || searched(&core, id));
    wait_until("the search", || searched(&core, id));
    let videos = core.videos(id).unwrap();
    assert_eq!(videos.len(), 1, "a tab's own choice is left alone");
    assert_eq!(core.video_search(id).unwrap().unwrap().results.len(), 1);
}

#[test]
fn failures_are_recorded_and_rate_limits_retried() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::new(|_| Err(SearchError::Network("offline".into())));
    let core = core(dir.path(), &fake, "");
    let id = import(&core, &tab_file(dir.path(), "Variant 000006"));
    wait_until("the search", || searched(&core, id));
    let search = core.video_search(id).unwrap().unwrap();
    assert_eq!(search.state, VideoSearchState::Failed);
    assert!(search.error.unwrap().contains("offline"));
    assert!(!summary(&core, id).finding_video);

    // A rate limit keeps the tab pending (the finder pauses, then retries).
    *fake.respond.lock() = Box::new(|_| Err(SearchError::RateLimited));
    core.find_videos(vec![id]).unwrap();
    wait_until("the attempt", || fake.queries().len() >= 2);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        core.video_search(id).unwrap().unwrap().state,
        VideoSearchState::Pending
    );
    assert!(summary(&core, id).finding_video);
}

#[test]
fn pending_searches_resume_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let length = tab_length();
    let id;
    {
        // Rate-limited until dropped: the search stays pending.
        let fake = Fake::new(|_| Err(SearchError::RateLimited));
        let core = core(dir.path(), &fake, "");
        id = import(&core, &tab_file(dir.path(), "Variant 000007"));
        wait_until("the attempt", || !fake.queries().is_empty());
    }
    let fake = Fake::new(move |q| Ok(vec![art_track("fffffffffff", &title_of(q), length)]));
    let core = core(dir.path(), &fake, "");
    wait_until("the resumed search", || searched(&core, id));
    assert_eq!(core.videos(id).unwrap()[0].youtube_id, "fffffffffff");
}

#[test]
fn searching_by_hand_ranks_against_the_tab() {
    let dir = tempfile::tempdir().unwrap();
    let length = tab_length();
    let fake = Fake::new(move |q| {
        Ok(vec![
            VideoCandidate {
                title: "Variant 000008 (Guitar Lesson)".into(),
                ..art_track("ggggggggggg", "", length)
            },
            art_track("hhhhhhhhhhh", &title_of(q), length),
        ])
    });
    let core = core(dir.path(), &fake, "auto_find_videos = false\n");
    let id = import(&core, &tab_file(dir.path(), "Variant 000008"));
    assert_eq!(core.video_query(id).unwrap(), "Tabster Variant 000008");

    let results = core
        .search_videos(id, "Tabster Variant 000008".into())
        .unwrap();
    assert_eq!(results[0].candidate.youtube_id, "hhhhhhhhhhh");
    assert_eq!(results[1].label, "Lesson");
    // A query of its own is searched as typed, once; nothing is stored or attached.
    let results = core
        .search_videos(id, "variant 000008 lesson".into())
        .unwrap();
    assert_eq!(fake.queries().last().unwrap(), "variant 000008 lesson");
    assert_eq!(results[0].label, "Lesson", "the search asked for a lesson");
    assert_eq!(core.video_search(id).unwrap(), None);
    assert!(core.videos(id).unwrap().is_empty());
    assert_eq!(core.list_tabs(TabFilter::default()).unwrap().len(), 1);
}

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;

use super::*;
use crate::model::{Bar, Beat, MasterBar, Track, Voice};
use crate::sync::SyncPoint;

const STANDARD: [u8; 6] = [64, 59, 55, 50, 45, 40];

/// A one-track score of `bars` 4/4 bars of quarter notes.
fn score(title: &str, artist: &str, album: &str, tuning: &[u8], bars: usize) -> Score {
    let bar = Bar {
        voices: vec![Voice {
            beats: vec![Beat::default(); 4],
        }],
    };
    Score {
        title: title.into(),
        artist: artist.into(),
        album: album.into(),
        initial_tempo: 120.0,
        tracks: vec![Track {
            name: "Guitar".into(),
            tuning: tuning.to_vec(),
            bars: vec![bar; bars],
            ..Track::default()
        }],
        master_bars: vec![MasterBar::default(); bars],
        ..Score::default()
    }
}

/// Fake parser: the file holds `title|artist|album`; "BAD" fails to parse.
fn fake_parse(bytes: &[u8], _extension: Option<&str>) -> Result<Score, ParseError> {
    let text = std::str::from_utf8(bytes).map_err(|e| ParseError::Invalid(e.to_string()))?;
    if text.starts_with("BAD") {
        return Err(ParseError::Invalid("bad".into()));
    }
    let mut fields = text.split('|');
    let mut next = || fields.next().unwrap_or_default().trim().to_string();
    let (title, artist, album) = (next(), next(), next());
    Ok(score(&title, &artist, &album, &STANDARD, 4))
}

struct Fixture {
    dir: TempDir,
    library: Library,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(&dir.path().join("data")).unwrap();
        Self { dir, library }
    }

    fn file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, contents).unwrap();
        path
    }

    fn import(&self, name: &str, contents: &str) -> ImportOutcome {
        let path = self.file(name, contents);
        self.library.import_with(&path, &fake_parse)
    }

    fn add(&self, name: &str, contents: &str) -> TabId {
        let outcome = self.import(name, contents);
        assert_eq!(outcome.error, None);
        assert!(!outcome.duplicate);
        outcome.id.unwrap()
    }

    fn titles(&self, filter: &TabFilter) -> Vec<String> {
        self.library
            .list(filter)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect()
    }

    fn search(&self, query: &str) -> Vec<String> {
        self.titles(&TabFilter {
            query: query.into(),
            ..TabFilter::default()
        })
    }

    fn files_in(&self, sub: &str) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(self.library.data_dir().join(sub))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        files
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn library_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Library>();
}

#[test]
fn open_creates_schema_and_layout() {
    let f = Fixture::new();
    let data = f.library.data_dir();
    assert!(data.join("library.sqlite").is_file());
    for dir in [data.to_path_buf(), data.join("tabs"), data.join("cache")] {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode(&data.join("library.sqlite")), 0o600);
    let version: i64 = f
        .library
        .reader
        .lock()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, schema::latest_version());
    assert!(f.library.list(&TabFilter::default()).unwrap().is_empty());
}

#[test]
fn reopen_keeps_data_and_migration_is_idempotent() {
    let f = Fixture::new();
    let data = f.library.data_dir().to_path_buf();
    let id = f.add("a.gp", "Song|Band|Record");
    f.library.set_favourite(id, true).unwrap();
    drop(f.library);

    for _ in 0..2 {
        let library = Library::open(&data).unwrap();
        let tab = library.get(id).unwrap();
        assert_eq!(tab.title, "Song");
        assert!(tab.favourite);
    }
    // Running the migrations again on a current database is a no-op.
    let mut connection = Connection::open(data.join("library.sqlite")).unwrap();
    schema::migrate(&mut connection).unwrap();
    // A database from the future is refused.
    connection
        .pragma_update(None, "user_version", schema::latest_version() + 1)
        .unwrap();
    drop(connection);
    assert!(matches!(Library::open(&data), Err(LibraryError::Db(_))));
}

#[test]
fn import_stores_catalogues_and_dedupes() {
    let f = Fixture::new();
    let outcome = f.import("song.gp", "Song|Band|Record");
    assert_eq!(outcome.error, None);
    let id = outcome.id.unwrap();
    assert!(!outcome.duplicate);

    let tab = f.library.get(id).unwrap();
    assert_eq!(
        (tab.title.as_str(), tab.artist.as_str(), tab.album.as_str()),
        ("Song", "Band", "Record")
    );
    assert_eq!(tab.format, Format::Gp7);
    assert_eq!(tab.tuning, "Standard");
    assert_eq!((tab.track_count, tab.bar_count), (1, 4));
    // 4 bars of 4/4 at 120 bpm.
    assert!((tab.duration_secs - 8.0).abs() < 1e-9);
    assert!(!tab.favourite && !tab.has_video && tab.tags.is_empty());
    assert!(tab.imported_at > 0);
    assert_eq!(tab.last_opened_at, None);

    let stored = f.library.file_path(id).unwrap();
    assert_eq!(fs::read_to_string(&stored).unwrap(), "Song|Band|Record");
    assert_eq!(stored.extension().unwrap(), "gp");
    assert_eq!(stored.parent().unwrap(), f.library.data_dir().join("tabs"));
    assert_eq!(mode(&stored), 0o600);
    assert_eq!(f.files_in("cache").len(), 1);

    // Same content under another name.
    let again = f.import("copy.gp", "Song|Band|Record");
    assert_eq!(
        (again.id, again.duplicate, again.error),
        (Some(id), true, None)
    );
    assert_eq!(f.library.list(&TabFilter::default()).unwrap().len(), 1);
    assert_eq!(f.files_in("tabs").len(), 1);
}

#[test]
fn title_falls_back_to_file_stem() {
    let f = Fixture::new();
    let id = f.add("Untitled Riff.gp5", " |Band|");
    assert_eq!(f.library.get(id).unwrap().title, "Untitled Riff");
}

#[test]
fn failed_imports_store_nothing() {
    let f = Fixture::new();
    let unknown = f.import("notes.txt", "Song|Band|Record");
    assert_eq!(unknown.id, None);
    assert_eq!(unknown.error.as_deref(), Some("unrecognised file format"));

    let bad = f.import("bad.gp", "BAD");
    assert_eq!(bad.id, None);
    assert!(bad.error.unwrap().contains("bad"));

    let missing = f
        .library
        .import_with(&f.dir.path().join("missing.gp"), &fake_parse);
    assert!(missing.error.is_some());

    assert!(f.library.list(&TabFilter::default()).unwrap().is_empty());
    assert!(f.files_in("tabs").is_empty());
    assert!(f.files_in("cache").is_empty());
}

#[test]
fn search() {
    let f = Fixture::new();
    f.add("1.gp", "Enter Sandman|Metallica|Metallica");
    f.add("2.gp", "Master of Puppets|Metallica|Master of Puppets");
    f.add("3.gp", "Paranoid|Black Sabbath|Paranoid");
    f.add("4.gp", "Hells Bells|AC/DC|Back in Black");
    f.add("5.gp", "Café del Mar|Energy 52|");

    assert_eq!(f.search("").len(), 5);
    assert_eq!(f.search("   ").len(), 5);
    // Prefixes, any column.
    assert_eq!(
        f.search("metal"),
        vec!["Enter Sandman", "Master of Puppets"]
    );
    assert_eq!(f.search("PUPP"), vec!["Master of Puppets"]);
    // Every word must match (in any column).
    assert_eq!(f.search("metallica enter"), vec!["Enter Sandman"]);
    assert_eq!(f.search("black"), vec!["Hells Bells", "Paranoid"]);
    assert!(f.search("metallica paranoid").is_empty());
    // Diacritics are folded.
    assert_eq!(f.search("cafe"), vec!["Café del Mar"]);
    // FTS syntax in the input is taken literally rather than breaking the query.
    assert_eq!(f.search("ac/dc"), vec!["Hells Bells"]);
    assert_eq!(f.search("\"enter"), vec!["Enter Sandman"]);
    for nasty in [
        "\"",
        "*",
        "(",
        ")",
        "-",
        "a OR",
        "NOT b",
        "title:x",
        "^x",
        "x\"y",
        "''",
        "NEAR(a b)",
        "%",
        "\\",
    ] {
        assert!(
            f.library
                .list(&TabFilter {
                    query: nasty.into(),
                    ..TabFilter::default()
                })
                .is_ok(),
            "{nasty}"
        );
    }
}

#[test]
fn search_follows_metadata_changes_and_deletes() {
    let f = Fixture::new();
    let id = f.add("1.gp", "Original|Band|");
    f.library
        .writer
        .lock()
        .execute("UPDATE tabs SET title = 'Renamed' WHERE id = ?1", [id])
        .unwrap();
    assert!(f.search("original").is_empty());
    assert_eq!(f.search("renamed"), vec!["Renamed"]);
    f.library.delete(id).unwrap();
    assert!(f.search("renamed").is_empty());
}

#[test]
fn filters_favourites_and_tags() {
    let f = Fixture::new();
    let a = f.add("a.gp", "Alpha|X|");
    let b = f.add("b.gp", "Bravo|Y|");
    let c = f.add("c.gp", "Charlie|Z|");
    f.library.set_favourite(b, true).unwrap();
    f.library
        .set_tags(a, &["rock".into(), " Learning ".into(), "".into()])
        .unwrap();
    f.library.set_tags(c, &["Rock".into()]).unwrap();

    let favourites = TabFilter {
        favourites_only: true,
        ..TabFilter::default()
    };
    assert_eq!(f.titles(&favourites), vec!["Bravo"]);

    let rock = TabFilter {
        tag: Some("ROCK".into()),
        ..TabFilter::default()
    };
    assert_eq!(f.titles(&rock), vec!["Alpha", "Charlie"]);
    assert_eq!(f.library.get(a).unwrap().tags, vec!["Learning", "rock"]);
    assert_eq!(f.library.get(c).unwrap().tags, vec!["rock"]);
    assert_eq!(f.library.tags().unwrap(), vec!["Learning", "rock"]);

    // Replacing tags drops ones nobody uses any more.
    f.library.set_tags(a, &["rock".into()]).unwrap();
    assert_eq!(f.library.tags().unwrap(), vec!["rock"]);
    f.library.delete(a).unwrap();
    f.library.set_tags(c, &[]).unwrap();
    assert!(f.library.tags().unwrap().is_empty());

    // Combined filters.
    f.library.set_tags(b, &["live".into()]).unwrap();
    let both = TabFilter {
        favourites_only: true,
        tag: Some("live".into()),
        query: "bra".into(),
        ..TabFilter::default()
    };
    assert_eq!(f.titles(&both), vec!["Bravo"]);

    assert!(matches!(
        f.library.set_favourite(999, true),
        Err(LibraryError::NotFound)
    ));
    assert!(matches!(
        f.library.set_tags(999, &["x".into()]),
        Err(LibraryError::NotFound)
    ));
}

#[test]
fn setlists() {
    let f = Fixture::new();
    let a = f.add("a.gp", "Alpha|X|");
    let b = f.add("b.gp", "Bravo|Y|");
    let c = f.add("c.gp", "Charlie|Z|");
    let gig = f.library.create_setlist(" Gig ").unwrap();
    let empty = f.library.create_setlist("Archive").unwrap();
    f.library.set_setlist_tabs(gig, &[c, a, b, a]).unwrap();

    let in_gig = TabFilter {
        setlist: Some(gig),
        // Ignored: a setlist lists in its own order.
        sort: SortOrder::Title,
        ..TabFilter::default()
    };
    assert_eq!(f.titles(&in_gig), vec!["Charlie", "Alpha", "Bravo"]);
    let gig_search = TabFilter {
        query: "a".into(),
        ..in_gig.clone()
    };
    assert_eq!(f.titles(&gig_search), vec!["Alpha"]);
    assert_eq!(
        f.library.setlists().unwrap(),
        vec![
            Setlist {
                id: empty,
                name: "Archive".into(),
                tab_count: 0
            },
            Setlist {
                id: gig,
                name: "Gig".into(),
                tab_count: 3
            },
        ]
    );

    // Reordering replaces; deleting a tab leaves the others in order.
    f.library.set_setlist_tabs(gig, &[b, c]).unwrap();
    assert_eq!(f.titles(&in_gig), vec!["Bravo", "Charlie"]);
    f.library.delete(b).unwrap();
    assert_eq!(f.titles(&in_gig), vec!["Charlie"]);

    assert!(matches!(
        f.library.set_setlist_tabs(gig, &[999]),
        Err(LibraryError::NotFound)
    ));
    // The failed replace rolled back.
    assert_eq!(f.titles(&in_gig), vec!["Charlie"]);
    assert!(matches!(
        f.library.set_setlist_tabs(999, &[a]),
        Err(LibraryError::NotFound)
    ));

    f.library.delete_setlist(gig).unwrap();
    assert_eq!(f.library.setlists().unwrap().len(), 1);
    assert!(f.titles(&in_gig).is_empty());
    assert!(matches!(
        f.library.delete_setlist(gig),
        Err(LibraryError::NotFound)
    ));
    // Tabs survive their setlist.
    assert_eq!(f.library.list(&TabFilter::default()).unwrap().len(), 2);
}

#[test]
fn sort_orders() {
    let f = Fixture::new();
    let a = f.add("a.gp", "beta|Zed|");
    let b = f.add("b.gp", "Alpha|yak|");
    let c = f.add("c.gp", "gamma||");
    let set = |sql: &str, id: TabId, value: i64| {
        f.library
            .writer
            .lock()
            .execute(sql, params![value, id])
            .unwrap();
    };
    let imported = "UPDATE tabs SET imported_at = ?1 WHERE id = ?2";
    let opened = "UPDATE tabs SET last_opened_at = ?1 WHERE id = ?2";
    set(imported, a, 100);
    set(imported, b, 300);
    set(imported, c, 200);
    set(opened, a, 500);
    set(opened, b, 400);

    let sorted = |sort| {
        f.titles(&TabFilter {
            sort,
            ..TabFilter::default()
        })
    };
    assert_eq!(sorted(SortOrder::Title), vec!["Alpha", "beta", "gamma"]);
    assert_eq!(sorted(SortOrder::Artist), vec!["Alpha", "beta", "gamma"]);
    assert_eq!(
        sorted(SortOrder::RecentlyAdded),
        vec!["Alpha", "gamma", "beta"]
    );
    assert_eq!(
        sorted(SortOrder::RecentlyOpened),
        vec!["beta", "Alpha", "gamma"]
    );
}

#[test]
fn delete_removes_row_and_files() {
    let f = Fixture::new();
    let id = f.add("a.gp", "Alpha|X|");
    let keep = f.add("b.gp", "Bravo|Y|");
    let stored = f.library.file_path(id).unwrap();
    let video = f.library.add_video(id, "dQw4w9WgXcQ", "").unwrap();
    f.library.set_tags(id, &["t".into()]).unwrap();
    f.library
        .set_practice_state(
            id,
            &PracticeState {
                track: 1,
                ..PracticeState::default()
            },
        )
        .unwrap();
    f.library.delete(id).unwrap();

    assert!(!stored.exists());
    assert_eq!(f.files_in("tabs").len(), 1);
    assert_eq!(f.files_in("cache").len(), 1);
    assert!(matches!(f.library.get(id), Err(LibraryError::NotFound)));
    assert!(matches!(
        f.library.file_path(id),
        Err(LibraryError::NotFound)
    ));
    assert!(matches!(f.library.delete(id), Err(LibraryError::NotFound)));
    assert!(f.library.videos(id).unwrap().is_empty());
    assert!(matches!(
        f.library.remove_video(video),
        Err(LibraryError::NotFound)
    ));
    assert!(f.library.tags().unwrap().is_empty());
    assert_eq!(f.library.practice_state(id).unwrap().track, 0);
    assert!(f.library.get(keep).is_ok());

    // Re-importing the same content works again after a delete.
    let again = f.import("a2.gp", "Alpha|X|");
    assert!(!again.duplicate);
    assert!(f.library.file_path(again.id.unwrap()).unwrap().exists());
}

#[test]
fn load_score_uses_the_cache() {
    let f = Fixture::new();
    let id = f.add("a.gp", "Alpha|X|");
    let parses = AtomicUsize::new(0);
    let counting = |bytes: &[u8], ext: Option<&str>| {
        parses.fetch_add(1, Ordering::SeqCst);
        fake_parse(bytes, ext)
    };

    let score = f.library.load_score_with(id, &counting).unwrap();
    assert_eq!(score.title, "Alpha");
    assert_eq!(parses.load(Ordering::SeqCst), 0);
    assert!(f.library.get(id).unwrap().last_opened_at.is_some());

    // A corrupt cache entry is a miss: re-parse and rewrite it.
    let cache = f.files_in("cache").remove(0);
    fs::write(&cache, b"garbage").unwrap();
    assert_eq!(f.library.load_score_with(id, &counting).unwrap(), score);
    assert_eq!(parses.load(Ordering::SeqCst), 1);
    assert_eq!(f.library.load_score_with(id, &counting).unwrap(), score);
    assert_eq!(parses.load(Ordering::SeqCst), 1);

    // So is a truncated one, and a missing one.
    let bytes = fs::read(&cache).unwrap();
    fs::write(&cache, &bytes[..bytes.len() / 2]).unwrap();
    assert_eq!(f.library.load_score_with(id, &counting).unwrap(), score);
    fs::remove_file(&cache).unwrap();
    assert_eq!(f.library.load_score_with(id, &counting).unwrap(), score);
    assert_eq!(parses.load(Ordering::SeqCst), 3);
    assert_eq!(mode(&cache), 0o600);

    assert!(matches!(
        f.library.load_score_with(999, &counting),
        Err(LibraryError::NotFound)
    ));
}

#[test]
fn videos_default_logic() {
    let f = Fixture::new();
    let tab = f.add("a.gp", "Alpha|X|");
    let other = f.add("b.gp", "Bravo|Y|");
    let defaults = |tab| -> Vec<(VideoId, bool)> {
        f.library
            .videos(tab)
            .unwrap()
            .into_iter()
            .map(|v| (v.id, v.is_default))
            .collect()
    };

    let v1 = f.library.add_video(tab, "aaaaaaaaaaa", " Live ").unwrap();
    let v2 = f.library.add_video(tab, "bbbbbbbbbbb", "Studio").unwrap();
    let v3 = f.library.add_video(tab, "ccccccccccc", "").unwrap();
    let w1 = f.library.add_video(other, "ddddddddddd", "").unwrap();
    assert_eq!(defaults(tab), vec![(v1, true), (v2, false), (v3, false)]);
    assert_eq!(defaults(other), vec![(w1, true)]);
    let first = &f.library.videos(tab).unwrap()[0];
    assert_eq!(
        (
            first.tab_id,
            first.youtube_id.as_str(),
            first.label.as_str()
        ),
        (tab, "aaaaaaaaaaa", "Live")
    );
    assert_eq!(first.sync, SyncSpec::default());
    assert!(f.library.get(tab).unwrap().has_video);

    f.library.set_default_video(v3).unwrap();
    assert_eq!(defaults(tab), vec![(v1, false), (v2, false), (v3, true)]);
    assert_eq!(defaults(other), vec![(w1, true)]);

    // Removing a non-default leaves the default alone; removing the default promotes.
    f.library.remove_video(v1).unwrap();
    assert_eq!(defaults(tab), vec![(v2, false), (v3, true)]);
    f.library.remove_video(v3).unwrap();
    assert_eq!(defaults(tab), vec![(v2, true)]);
    f.library.remove_video(v2).unwrap();
    assert!(defaults(tab).is_empty());
    assert!(!f.library.get(tab).unwrap().has_video);

    assert!(matches!(
        f.library.add_video(999, "aaaaaaaaaaa", ""),
        Err(LibraryError::NotFound)
    ));
    assert!(matches!(
        f.library.set_default_video(v1),
        Err(LibraryError::NotFound)
    ));
}

#[test]
fn video_sync_round_trips() {
    let f = Fixture::new();
    let tab = f.add("a.gp", "Alpha|X|");
    let video = f.library.add_video(tab, "aaaaaaaaaaa", "").unwrap();
    for spec in [
        SyncSpec::Offset {
            offset_secs: 3.25,
            stretch: 0.998,
        },
        SyncSpec::Points {
            points: vec![
                SyncPoint {
                    played_bar: 0,
                    video_secs: 1.5,
                },
                SyncPoint {
                    played_bar: 7,
                    video_secs: 19.125,
                },
            ],
        },
        SyncSpec::Points { points: vec![] },
    ] {
        f.library.set_video_sync(video, &spec).unwrap();
        assert_eq!(f.library.videos(tab).unwrap()[0].sync, spec);
    }
    assert!(matches!(
        f.library.set_video_sync(999, &SyncSpec::default()),
        Err(LibraryError::NotFound)
    ));
}

#[test]
fn practice_state() {
    let f = Fixture::new();
    let tab = f.add("a.gp", "Alpha|X|");
    assert_eq!(
        f.library.practice_state(tab).unwrap(),
        PracticeState {
            track: 0,
            speed: 1.0,
            loop_start_bar: None,
            loop_end_bar: None,
            last_position_secs: 0.0,
        }
    );
    let state = PracticeState {
        track: 2,
        speed: 0.75,
        loop_start_bar: Some(4),
        loop_end_bar: Some(7),
        last_position_secs: 42.5,
    };
    f.library.set_practice_state(tab, &state).unwrap();
    assert_eq!(f.library.practice_state(tab).unwrap(), state);
    let cleared = PracticeState {
        loop_start_bar: None,
        loop_end_bar: None,
        ..state.clone()
    };
    f.library.set_practice_state(tab, &cleared).unwrap();
    assert_eq!(f.library.practice_state(tab).unwrap(), cleared);
    assert!(matches!(
        f.library.set_practice_state(999, &state),
        Err(LibraryError::NotFound)
    ));
}

#[test]
fn tuning_labels() {
    let label = |tuning: &[u8]| {
        let mut low_to_high = tuning.to_vec();
        low_to_high.reverse();
        metadata::tuning_label(&score("", "", "", &low_to_high, 1))
    };
    assert_eq!(label(&[40, 45, 50, 55, 59, 64]), "Standard");
    assert_eq!(label(&[38, 45, 50, 55, 59, 64]), "Drop D");
    assert_eq!(label(&[39, 44, 49, 54, 58, 63]), "Eb Standard");
    assert_eq!(label(&[38, 43, 48, 53, 57, 62]), "D Standard");
    assert_eq!(label(&[36, 43, 48, 53, 57, 62]), "Drop C");
    assert_eq!(label(&[38, 43, 50, 55, 59, 62]), "Open G");
    assert_eq!(label(&[38, 45, 50, 55, 57, 62]), "DADGAD");
    assert_eq!(label(&[35, 40, 45, 50, 55, 59, 64]), "Standard");
    assert_eq!(label(&[28, 33, 38, 43]), "Standard");
    // Unrecognised: note names, lowest string first.
    assert_eq!(label(&[37, 42, 47, 52, 56, 63]), "C# F# B E G# D#");
    assert_eq!(label(&[52, 57, 62, 67, 71, 76]), "E A D G B E");
    assert_eq!(label(&[]), "");

    // The first non-percussion track decides.
    let mut s = score("", "", "", &STANDARD, 1);
    s.tracks.insert(
        0,
        Track {
            is_percussion: true,
            tuning: vec![0; 6],
            ..Track::default()
        },
    );
    s.tracks[1].tuning = vec![64, 59, 55, 50, 45, 38];
    assert_eq!(metadata::tuning_label(&s), "Drop D");
    s.tracks.truncate(1);
    assert_eq!(metadata::tuning_label(&s), "");
}

#[test]
fn duration_estimate() {
    // 4/4 at 120: 2 s a bar.
    let mut s = score("", "", "", &STANDARD, 3);
    assert!((metadata::estimate_duration(&s) - 6.0).abs() < 1e-9);
    // An empty bar lasts its time signature; a shorter bar its beats.
    s.tracks[0].bars[1] = Bar::default();
    s.tracks[0].bars[2].voices[0].beats.truncate(2);
    assert!((metadata::estimate_duration(&s) - 5.0).abs() < 1e-9);
    s.initial_tempo = 0.0;
    assert!(metadata::estimate_duration(&s) > 0.0);
    assert_eq!(metadata::estimate_duration(&Score::default()), 0.0);
}

#[test]
fn youtube_ids() {
    let id = Some("dQw4w9WgXcQ".to_string());
    for (input, expected) in [
        ("dQw4w9WgXcQ", id.clone()),
        ("  dQw4w9WgXcQ\n", id.clone()),
        ("a-b_c-d_e-f", Some("a-b_c-d_e-f".into())),
        ("https://www.youtube.com/watch?v=dQw4w9WgXcQ", id.clone()),
        (
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            id.clone(),
        ),
        (
            "https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
            id.clone(),
        ),
        ("http://youtube.com/watch?v=dQw4w9WgXcQ#t=1m", id.clone()),
        ("www.youtube.com/watch?v=dQw4w9WgXcQ", id.clone()),
        ("HTTPS://WWW.YOUTUBE.COM/watch?v=dQw4w9WgXcQ", id.clone()),
        ("https://m.youtube.com/watch?v=dQw4w9WgXcQ", id.clone()),
        (
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RD",
            id.clone(),
        ),
        ("https://youtu.be/dQw4w9WgXcQ", id.clone()),
        ("https://youtu.be/dQw4w9WgXcQ?si=abc&t=10", id.clone()),
        ("youtu.be/dQw4w9WgXcQ", id.clone()),
        ("https://www.youtube.com/shorts/dQw4w9WgXcQ", id.clone()),
        (
            "https://www.youtube.com/shorts/dQw4w9WgXcQ?feature=share",
            id.clone(),
        ),
        (
            "https://www.youtube.com/embed/dQw4w9WgXcQ?start=5",
            id.clone(),
        ),
        (
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            id.clone(),
        ),
        ("https://www.youtube.com/live/dQw4w9WgXcQ", id.clone()),
        // Rejected.
        ("", None),
        ("dQw4w9WgXc", None),
        ("dQw4w9WgXcQQ", None),
        ("dQw4w9WgXc!", None),
        ("dQw4w9WgXc€", None),
        ("https://www.youtube.com/watch?v=short", None),
        ("https://www.youtube.com/watch?vv=dQw4w9WgXcQ", None),
        ("https://www.youtube.com/watch", None),
        ("https://www.youtube.com/", None),
        ("https://www.youtube.com/channel/dQw4w9WgXcQ", None),
        ("https://youtu.be/", None),
        ("https://vimeo.com/dQw4w9WgXcQ", None),
        ("https://evil.com/watch?v=dQw4w9WgXcQ", None),
        ("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ", None),
        ("not a url at all", None),
    ] {
        assert_eq!(parse_youtube_id(input), expected, "{input}");
    }
}

#[test]
fn concurrent_imports() {
    let f = Fixture::new();
    let paths: Vec<PathBuf> = (0..24)
        .map(|i| f.file(&format!("{i}.gp"), &format!("Song {}|Band|", i % 16)))
        .collect();
    let outcomes: Vec<ImportOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(4)
            .map(|chunk| {
                let library = &f.library;
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|p| library.import_with(p, &fake_parse))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        // Readers at the same time.
        for _ in 0..20 {
            f.library.list(&TabFilter::default()).unwrap();
        }
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    assert!(outcomes.iter().all(|o| o.error.is_none() && o.id.is_some()));
    assert_eq!(outcomes.iter().filter(|o| o.duplicate).count(), 8);
    assert_eq!(f.library.list(&TabFilter::default()).unwrap().len(), 16);
    assert_eq!(f.files_in("tabs").len(), 16);
    assert_eq!(f.files_in("cache").len(), 16);
}

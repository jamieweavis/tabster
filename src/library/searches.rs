//! Video searches per tab: queued, finished (with ranked results) or failed. A pending row
//! survives a restart, so a big import's searches carry on next launch.

use rusqlite::{OptionalExtension, params};

use super::{Library, LibraryError, TabId, found, unix_now};
use crate::video_search::{MatchConfidence, VideoMatch};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VideoSearchState {
    /// Queued or running.
    Pending,
    Done,
    Failed,
}

impl VideoSearchState {
    fn as_str(self) -> &'static str {
        match self {
            VideoSearchState::Pending => "pending",
            VideoSearchState::Done => "done",
            VideoSearchState::Failed => "failed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "pending" => VideoSearchState::Pending,
            "done" => VideoSearchState::Done,
            _ => VideoSearchState::Failed,
        }
    }
}

/// A tab's last video search.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct VideoSearch {
    pub tab_id: TabId,
    pub state: VideoSearchState,
    /// What was searched for.
    pub query: String,
    /// Best first.
    pub results: Vec<VideoMatch>,
    /// Why it failed.
    pub error: Option<String>,
    /// The video this search attached by itself.
    pub attached_youtube_id: Option<String>,
    pub dismissed: bool,
    /// Unix seconds.
    pub searched_at: Option<i64>,
}

impl Library {
    /// Queue a search for `tab`. An automatic one (`manual` false) is only queued for a tab with
    /// no videos that has never been searched; returns whether it was queued. A manual one is
    /// always queued, and un-dismisses the tab.
    pub fn queue_video_search(&self, tab: TabId, manual: bool) -> Result<bool, LibraryError> {
        let writer = self.writer.lock();
        let changed = if manual {
            writer.execute(
                "INSERT INTO video_searches (tab_id, state, queued_at) VALUES (?1, 'pending', ?2)
                 ON CONFLICT (tab_id) DO UPDATE
                 SET state = 'pending', dismissed = 0, error = '', queued_at = excluded.queued_at",
                params![tab, unix_now()],
            )?
        } else {
            writer.execute(
                "INSERT INTO video_searches (tab_id, state, queued_at)
                 SELECT ?1, 'pending', ?2
                 WHERE EXISTS (SELECT 1 FROM tabs WHERE id = ?1)
                   AND NOT EXISTS (SELECT 1 FROM videos WHERE tab_id = ?1)
                 ON CONFLICT (tab_id) DO NOTHING",
                params![tab, unix_now()],
            )?
        };
        Ok(changed > 0)
    }

    /// Tabs with a search still pending (e.g. from before a restart), oldest first.
    pub fn pending_video_searches(&self) -> Result<Vec<TabId>, LibraryError> {
        let reader = self.reader.lock();
        let mut statement = reader.prepare(
            "SELECT tab_id FROM video_searches WHERE state = 'pending' ORDER BY queued_at, tab_id",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn video_search(&self, tab: TabId) -> Result<Option<VideoSearch>, LibraryError> {
        let reader = self.reader.lock();
        let row = reader
            .query_row(
                "SELECT state, query, results, error, attached_youtube_id, dismissed, searched_at
                 FROM video_searches WHERE tab_id = ?1",
                [tab],
                |row| {
                    let state: String = row.get(0)?;
                    let results: String = row.get(2)?;
                    let error: String = row.get(3)?;
                    Ok(VideoSearch {
                        tab_id: tab,
                        state: VideoSearchState::parse(&state),
                        query: row.get(1)?,
                        // Results written by another version may not decode; they're only a cache.
                        results: serde_json::from_str(&results).unwrap_or_default(),
                        error: (!error.is_empty()).then_some(error),
                        attached_youtube_id: row.get(4)?,
                        dismissed: row.get(5)?,
                        searched_at: row.get(6)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Record a finished search. A likely match that wasn't attached becomes a suggestion.
    pub fn finish_video_search(
        &self,
        tab: TabId,
        query: &str,
        results: &[VideoMatch],
        attached: Option<&str>,
    ) -> Result<(), LibraryError> {
        let json = serde_json::to_string(results)
            .map_err(|e| LibraryError::Db(format!("encoding search results: {e}")))?;
        let suggested = attached.is_none()
            && results
                .first()
                .is_some_and(|m| m.confidence >= MatchConfidence::Medium);
        self.write_search(
            tab,
            VideoSearchState::Done,
            query,
            &json,
            "",
            attached,
            suggested,
        )
    }

    pub fn fail_video_search(
        &self,
        tab: TabId,
        query: &str,
        error: &str,
    ) -> Result<(), LibraryError> {
        self.write_search(
            tab,
            VideoSearchState::Failed,
            query,
            "[]",
            error,
            None,
            false,
        )
    }

    /// Stop suggesting videos for `tab`; automatic searches won't search it again.
    pub fn dismiss_video_search(&self, tab: TabId) -> Result<(), LibraryError> {
        let changed = self.writer.lock().execute(
            "INSERT INTO video_searches (tab_id, state, dismissed, queued_at)
             VALUES (?1, 'done', 1, ?2)
             ON CONFLICT (tab_id) DO UPDATE SET dismissed = 1",
            params![tab, unix_now()],
        )?;
        found(changed)
    }

    #[allow(clippy::too_many_arguments)]
    fn write_search(
        &self,
        tab: TabId,
        state: VideoSearchState,
        query: &str,
        results: &str,
        error: &str,
        attached: Option<&str>,
        suggested: bool,
    ) -> Result<(), LibraryError> {
        let changed = self.writer.lock().execute(
            "INSERT INTO video_searches
                 (tab_id, state, query, results, error, attached_youtube_id, suggested, queued_at,
                  searched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT (tab_id) DO UPDATE SET
                 state = excluded.state, query = excluded.query, results = excluded.results,
                 error = excluded.error, attached_youtube_id = excluded.attached_youtube_id,
                 suggested = excluded.suggested, searched_at = excluded.searched_at",
            params![
                tab,
                state.as_str(),
                query,
                results,
                error,
                attached,
                suggested,
                unix_now()
            ],
        )?;
        found(changed)
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::super::schema;
    use super::*;
    use crate::video_search::{ChannelBadge, VideoCandidate};

    fn insert_tab(connection: &Connection, title: &str) -> TabId {
        connection
            .execute(
                "INSERT INTO tabs (sha256, format, title, imported_at) VALUES (?1, 'gp', ?1, 0)",
                [title],
            )
            .unwrap();
        connection.last_insert_rowid()
    }

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path()).unwrap();
        (dir, library)
    }

    fn result(id: &str, confidence: MatchConfidence) -> VideoMatch {
        VideoMatch {
            candidate: VideoCandidate {
                youtube_id: id.into(),
                title: "Song".into(),
                channel: "Artist".into(),
                duration_secs: Some(200.0),
                views: None,
                badge: ChannelBadge::None,
                description: String::new(),
                rank: 0,
            },
            score: 0.7,
            confidence,
            label: "Original".into(),
            reasons: vec!["Same length as the tab".into()],
            length_difference_secs: Some(1.0),
            official: false,
        }
    }

    #[test]
    fn migrates_a_v2_library_with_videos() {
        let dir = tempfile::tempdir().unwrap();
        let tab = {
            let connection = Connection::open(dir.path().join("library.sqlite")).unwrap();
            schema::migrate_to(&connection, 2).unwrap();
            let tab = insert_tab(&connection, "Old");
            connection
                .execute(
                    "INSERT INTO videos (tab_id, youtube_id, label, is_default, sync)
                     VALUES (?1, 'dQw4w9WgXcQ', 'Original', 1, '{}')",
                    [tab],
                )
                .unwrap();
            tab
        };
        let library = Library::open(dir.path()).unwrap();
        let videos = library.videos(tab).unwrap();
        assert_eq!(videos[0].youtube_id, "dQw4w9WgXcQ");
        assert_eq!(
            (videos[0].title.as_str(), videos[0].duration_secs),
            ("", None)
        );
        assert_eq!(library.video_search(tab).unwrap(), None);
        let summary = library.get(tab).unwrap();
        assert!(summary.has_video && !summary.finding_video && !summary.video_suggested);
    }

    #[test]
    fn automatic_searches_skip_tabs_with_videos_or_searched_before() {
        let (_dir, library) = library();
        let tab = insert_tab(&library.writer.lock(), "A");
        let with_video = insert_tab(&library.writer.lock(), "B");
        library
            .add_video(with_video, "dQw4w9WgXcQ", "Original")
            .unwrap();

        assert!(library.queue_video_search(tab, false).unwrap());
        assert!(!library.queue_video_search(with_video, false).unwrap());
        assert!(
            !library.queue_video_search(9999, false).unwrap(),
            "no such tab"
        );
        assert_eq!(library.pending_video_searches().unwrap(), [tab]);
        assert!(library.get(tab).unwrap().finding_video);

        library.finish_video_search(tab, "q", &[], None).unwrap();
        assert!(
            !library.queue_video_search(tab, false).unwrap(),
            "searched before"
        );
        assert!(library.pending_video_searches().unwrap().is_empty());

        // Asking always queues, and un-dismisses.
        library.dismiss_video_search(tab).unwrap();
        assert!(library.queue_video_search(tab, true).unwrap());
        assert!(library.queue_video_search(with_video, true).unwrap());
        let search = library.video_search(tab).unwrap().unwrap();
        assert_eq!(search.state, VideoSearchState::Pending);
        assert!(!search.dismissed);
        assert!(matches!(
            library.queue_video_search(9999, true),
            Err(LibraryError::NotFound)
        ));
    }

    #[test]
    fn a_likely_unattached_result_is_a_suggestion() {
        let (_dir, library) = library();
        let tab = insert_tab(&library.writer.lock(), "A");
        let results = [
            result("aaaaaaaaaaa", MatchConfidence::Medium),
            result("bbbbbbbbbbb", MatchConfidence::Low),
        ];
        library
            .finish_video_search(tab, "Artist Song", &results, None)
            .unwrap();
        let search = library.video_search(tab).unwrap().unwrap();
        assert_eq!(search.state, VideoSearchState::Done);
        assert_eq!(search.query, "Artist Song");
        assert_eq!(search.results, results);
        assert!(search.searched_at.is_some());
        assert!(library.get(tab).unwrap().video_suggested);

        // Not once a video is attached, nor for an unlikely best result.
        library.add_video(tab, "aaaaaaaaaaa", "Original").unwrap();
        assert!(!library.get(tab).unwrap().video_suggested);
        let other = insert_tab(&library.writer.lock(), "B");
        library
            .finish_video_search(
                other,
                "q",
                &[result("ccccccccccc", MatchConfidence::Low)],
                None,
            )
            .unwrap();
        assert!(!library.get(other).unwrap().video_suggested);

        // Failure.
        library.fail_video_search(other, "q", "offline").unwrap();
        let failed = library.video_search(other).unwrap().unwrap();
        assert_eq!(
            (failed.state, failed.error.as_deref()),
            (VideoSearchState::Failed, Some("offline"))
        );
        assert!(failed.results.is_empty());
    }

    #[test]
    fn deleting_a_tab_deletes_its_search() {
        let (_dir, library) = library();
        let tab = insert_tab(&library.writer.lock(), "A");
        library.queue_video_search(tab, false).unwrap();
        library.delete(tab).unwrap();
        assert!(library.pending_video_searches().unwrap().is_empty());
        assert!(matches!(
            library.finish_video_search(tab, "q", &[], None),
            Err(LibraryError::NotFound)
        ));
    }
}

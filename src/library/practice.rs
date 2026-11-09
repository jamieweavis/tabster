//! Practice tools: saved loop sections and practice-time history (schema V2).

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use super::{Library, LibraryError, TabId, found, unix_now};

/// A named loop section of a tab.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SavedLoop {
    pub id: i64,
    pub tab_id: TabId,
    pub name: String,
    /// Inclusive master-bar range (0-based).
    pub start_bar: u32,
    pub end_bar: u32,
}

/// Practice time on one tab.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct PracticeStats {
    pub total_secs: f64,
    /// Today and the six days before it (local time).
    pub last_7_days_secs: f64,
    /// Practice sessions (separated by 30 minutes without practice).
    pub sessions: u32,
}

/// A recording this long after the previous one starts a new session.
const SESSION_GAP_SECS: i64 = 30 * 60;
/// One recording never counts for more than this (guards against a stuck clock).
const MAX_RECORD_SECS: f64 = 6.0 * 3600.0;

/// The local day (days since the epoch, in local time) of unix seconds `?N`, in SQL.
macro_rules! local_day {
    ($param:literal) => {
        concat!(
            "(CAST(strftime('%s', ",
            $param,
            ", 'unixepoch', 'localtime') AS INTEGER) / 86400)"
        )
    };
}

impl Library {
    pub fn saved_loops(&self, tab: TabId) -> Result<Vec<SavedLoop>, LibraryError> {
        let reader = self.reader.lock();
        let mut statement = reader.prepare(
            "SELECT id, tab_id, name, start_bar, end_bar FROM saved_loops
             WHERE tab_id = ?1 ORDER BY start_bar, end_bar, id",
        )?;
        let rows = statement.query_map([tab], |row| {
            Ok(SavedLoop {
                id: row.get(0)?,
                tab_id: row.get(1)?,
                name: row.get(2)?,
                start_bar: row.get(3)?,
                end_bar: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Save a loop over master bars `start_bar..=end_bar` (swapped if reversed).
    pub fn save_loop(
        &self,
        tab: TabId,
        name: &str,
        start_bar: u32,
        end_bar: u32,
    ) -> Result<i64, LibraryError> {
        let (start, end) = (start_bar.min(end_bar), start_bar.max(end_bar));
        let writer = self.writer.lock();
        writer.execute(
            "INSERT INTO saved_loops (tab_id, name, start_bar, end_bar, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![tab, name.trim(), start, end, unix_now()],
        )?;
        Ok(writer.last_insert_rowid())
    }

    pub fn rename_loop(&self, id: i64, name: &str) -> Result<(), LibraryError> {
        let changed = self.writer.lock().execute(
            "UPDATE saved_loops SET name = ?2 WHERE id = ?1",
            params![id, name.trim()],
        )?;
        found(changed)
    }

    pub fn delete_loop(&self, id: i64) -> Result<(), LibraryError> {
        let changed = self
            .writer
            .lock()
            .execute("DELETE FROM saved_loops WHERE id = ?1", [id])?;
        found(changed)
    }

    /// Add `secs` of practice on `tab`, now.
    pub fn record_practice(&self, tab: TabId, secs: f64) -> Result<(), LibraryError> {
        self.record_practice_at(tab, secs, unix_now())
    }

    /// Add `secs` of practice on `tab` at unix time `now` (to the local day of `now`).
    pub fn record_practice_at(&self, tab: TabId, secs: f64, now: i64) -> Result<(), LibraryError> {
        if !secs.is_finite() || secs <= 0.0 {
            return Ok(());
        }
        let secs = secs.min(MAX_RECORD_SECS);
        let writer = self.writer.lock();
        let last: Option<i64> = writer
            .query_row(
                "SELECT max(last_at) FROM practice_days WHERE tab_id = ?1",
                [tab],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        // The session started `secs` ago; it's new if nothing was recorded shortly before that.
        let started = now - secs as i64;
        let new_session = last.is_none_or(|last| started - last > SESSION_GAP_SECS);
        writer.execute(
            concat!(
                "INSERT INTO practice_days (tab_id, day, secs, sessions, last_at)
                 VALUES (?1, ",
                local_day!("?3"),
                ", ?2, ?4, ?3)
                 ON CONFLICT (tab_id, day) DO UPDATE SET
                    secs = secs + excluded.secs,
                    sessions = sessions + excluded.sessions,
                    last_at = max(last_at, excluded.last_at)"
            ),
            params![tab, secs, now, i64::from(new_session)],
        )?;
        Ok(())
    }

    pub fn practice_stats(&self, tab: TabId) -> Result<PracticeStats, LibraryError> {
        self.practice_stats_at(tab, unix_now())
    }

    /// Stats as of unix time `now` (which decides what "the last 7 days" are).
    pub fn practice_stats_at(&self, tab: TabId, now: i64) -> Result<PracticeStats, LibraryError> {
        let reader = self.reader.lock();
        let stats = reader.query_row(
            concat!(
                "SELECT coalesce(sum(secs), 0),
                        coalesce(sum(CASE WHEN ",
                local_day!("?2"),
                " - day BETWEEN 0 AND 6 THEN secs END), 0),
                        coalesce(sum(sessions), 0)
                 FROM practice_days WHERE tab_id = ?1"
            ),
            params![tab, now],
            |row| {
                Ok(PracticeStats {
                    total_secs: row.get(0)?,
                    last_7_days_secs: row.get(1)?,
                    sessions: row.get(2)?,
                })
            },
        )?;
        Ok(stats)
    }

    /// Total practice time of every tab that has some.
    pub fn practice_totals(&self) -> Result<HashMap<TabId, f64>, LibraryError> {
        let reader = self.reader.lock();
        let mut statement =
            reader.prepare("SELECT tab_id, sum(secs) FROM practice_days GROUP BY tab_id")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::super::schema;
    use super::*;

    /// 2026-01-15 12:00 UTC: far from DST changes, so day arithmetic holds in any time zone.
    const NOON: i64 = 1_768_478_400;
    const DAY: i64 = 86_400;

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path()).unwrap();
        (dir, library)
    }

    fn insert_tab(connection: &Connection, title: &str) -> TabId {
        connection
            .execute(
                "INSERT INTO tabs (sha256, format, title, imported_at) VALUES (?1, 'gp', ?1, 0)",
                [title],
            )
            .unwrap();
        connection.last_insert_rowid()
    }

    fn tab(library: &Library, title: &str) -> TabId {
        insert_tab(&library.writer.lock(), title)
    }

    #[test]
    fn migrates_a_v1_library() {
        let dir = tempfile::tempdir().unwrap();
        let tab = {
            let connection = Connection::open(dir.path().join("library.sqlite")).unwrap();
            schema::migrate_to(&connection, 1).unwrap();
            let tab = insert_tab(&connection, "Old");
            connection
                .execute(
                    "INSERT INTO practice (tab_id, speed, loop_start_bar, loop_end_bar)
                     VALUES (?1, 0.8, 3, 5)",
                    [tab],
                )
                .unwrap();
            tab
        };
        let library = Library::open(dir.path()).unwrap();
        let version: i64 = library
            .reader
            .lock()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        // Old data survives; the new tables work.
        assert_eq!(library.get(tab).unwrap().title, "Old");
        assert_eq!(
            library.practice_state(tab).unwrap().unwrap().loop_start_bar,
            Some(3)
        );
        assert!(library.saved_loops(tab).unwrap().is_empty());
        library.save_loop(tab, "Intro", 0, 3).unwrap();
        library.record_practice_at(tab, 60.0, NOON).unwrap();
        assert_eq!(
            library.practice_stats_at(tab, NOON).unwrap().total_secs,
            60.0
        );
    }

    #[test]
    fn saved_loop_crud() {
        let (_dir, library) = library();
        let a = tab(&library, "A");
        let b = tab(&library, "B");
        let chorus = library.save_loop(a, "  Chorus ", 12, 15).unwrap();
        let intro = library.save_loop(a, "Intro", 3, 0).unwrap();
        library.save_loop(b, "Solo", 20, 24).unwrap();

        let loops = library.saved_loops(a).unwrap();
        assert_eq!(
            loops,
            vec![
                SavedLoop {
                    id: intro,
                    tab_id: a,
                    name: "Intro".into(),
                    start_bar: 0,
                    end_bar: 3
                },
                SavedLoop {
                    id: chorus,
                    tab_id: a,
                    name: "Chorus".into(),
                    start_bar: 12,
                    end_bar: 15
                },
            ]
        );

        library.rename_loop(chorus, "Chorus 2").unwrap();
        assert_eq!(library.saved_loops(a).unwrap()[1].name, "Chorus 2");
        library.delete_loop(intro).unwrap();
        assert_eq!(library.saved_loops(a).unwrap().len(), 1);
        assert_eq!(library.saved_loops(b).unwrap().len(), 1);

        assert!(matches!(
            library.rename_loop(intro, "x"),
            Err(LibraryError::NotFound)
        ));
        assert!(matches!(
            library.delete_loop(intro),
            Err(LibraryError::NotFound)
        ));
        assert!(matches!(
            library.save_loop(999, "x", 0, 1),
            Err(LibraryError::NotFound)
        ));

        // Deleting the tab deletes its loops and history.
        library.record_practice_at(a, 10.0, NOON).unwrap();
        library
            .writer
            .lock()
            .execute("DELETE FROM tabs WHERE id = ?1", [a])
            .unwrap();
        assert!(library.saved_loops(a).unwrap().is_empty());
        assert_eq!(
            library.practice_stats_at(a, NOON).unwrap(),
            PracticeStats::default()
        );
    }

    #[test]
    fn practice_accumulates_across_days() {
        let (_dir, library) = library();
        let a = tab(&library, "A");
        let b = tab(&library, "B");

        // Day 0: one session of 3 × 30 s recordings, then another after a long break.
        library.record_practice_at(a, 30.0, NOON).unwrap();
        library.record_practice_at(a, 30.0, NOON + 30).unwrap();
        library.record_practice_at(a, 12.5, NOON + 43).unwrap();
        library
            .record_practice_at(a, 30.0, NOON + 3 * 3600)
            .unwrap();
        // Day 3 and day 10.
        library
            .record_practice_at(a, 600.0, NOON + 3 * DAY)
            .unwrap();
        library
            .record_practice_at(a, 120.0, NOON + 10 * DAY)
            .unwrap();
        // Ignored.
        library.record_practice_at(a, 0.0, NOON).unwrap();
        library.record_practice_at(a, -5.0, NOON).unwrap();
        library.record_practice_at(a, f64::NAN, NOON).unwrap();
        // Another tab.
        library.record_practice_at(b, 45.0, NOON).unwrap();

        let at = |now| library.practice_stats_at(a, now).unwrap();
        // As of day 3: days 0–3 are within the week (day 10 is still to come).
        assert_eq!(
            at(NOON + 3 * DAY),
            PracticeStats {
                total_secs: 822.5,
                last_7_days_secs: 702.5,
                sessions: 4,
            }
        );
        // As of day 10: only day 10 itself (days 4–10) is in the last 7 days.
        assert_eq!(
            at(NOON + 10 * DAY),
            PracticeStats {
                total_secs: 822.5,
                last_7_days_secs: 120.0,
                sessions: 4,
            }
        );
        // As of day 9: days 3–9 (day 10 is still to come).
        assert_eq!(at(NOON + 9 * DAY).last_7_days_secs, 600.0);
        assert_eq!(library.practice_stats_at(b, NOON).unwrap().total_secs, 45.0);
        assert_eq!(
            library.practice_stats_at(999, NOON).unwrap(),
            PracticeStats::default()
        );

        let totals = library.practice_totals().unwrap();
        assert_eq!(totals.len(), 2);
        assert_eq!(totals[&a], 822.5);
        assert_eq!(totals[&b], 45.0);

        assert!(matches!(
            library.record_practice_at(999, 10.0, NOON),
            Err(LibraryError::NotFound)
        ));
    }
}

//! Schema and migrations. `PRAGMA user_version` is the number of [`MIGRATIONS`] applied; to
//! change the schema, append a step (never edit a shipped one).

use rusqlite::{Connection, TransactionBehavior};

use super::LibraryError;

/// Each step runs in its own transaction, together with the `user_version` bump.
const MIGRATIONS: &[&str] = &[V1, V2];

const V1: &str = "
CREATE TABLE tabs (
    id INTEGER PRIMARY KEY,
    sha256 TEXT NOT NULL UNIQUE,
    -- File extension of the stored copy (`Format::extension`): gp, gpx, gp5 …
    format TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL DEFAULT '',
    album TEXT NOT NULL DEFAULT '',
    -- Display label, e.g. 'Drop D' or 'C# F# B E G# C#'.
    tuning TEXT NOT NULL DEFAULT '',
    track_count INTEGER NOT NULL DEFAULT 0,
    bar_count INTEGER NOT NULL DEFAULT 0,
    duration_secs REAL NOT NULL DEFAULT 0,
    favourite INTEGER NOT NULL DEFAULT 0,
    -- Unix seconds.
    imported_at INTEGER NOT NULL,
    last_opened_at INTEGER,
    -- Where the file was imported from (informational; the library uses its own copy).
    original_path TEXT NOT NULL DEFAULT ''
);
CREATE INDEX tabs_imported_at ON tabs (imported_at);
CREATE INDEX tabs_last_opened_at ON tabs (last_opened_at);

CREATE VIRTUAL TABLE tabs_fts USING fts5 (
    title, artist, album,
    content = 'tabs', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER tabs_fts_insert AFTER INSERT ON tabs BEGIN
    INSERT INTO tabs_fts (rowid, title, artist, album)
    VALUES (new.id, new.title, new.artist, new.album);
END;
CREATE TRIGGER tabs_fts_delete AFTER DELETE ON tabs BEGIN
    INSERT INTO tabs_fts (tabs_fts, rowid, title, artist, album)
    VALUES ('delete', old.id, old.title, old.artist, old.album);
END;
CREATE TRIGGER tabs_fts_update AFTER UPDATE OF title, artist, album ON tabs BEGIN
    INSERT INTO tabs_fts (tabs_fts, rowid, title, artist, album)
    VALUES ('delete', old.id, old.title, old.artist, old.album);
    INSERT INTO tabs_fts (rowid, title, artist, album)
    VALUES (new.id, new.title, new.artist, new.album);
END;

CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE tab_tags (
    tab_id INTEGER NOT NULL REFERENCES tabs (id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (tab_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX tab_tags_tag ON tab_tags (tag_id);

CREATE TABLE setlists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE setlist_items (
    setlist_id INTEGER NOT NULL REFERENCES setlists (id) ON DELETE CASCADE,
    tab_id INTEGER NOT NULL REFERENCES tabs (id) ON DELETE CASCADE,
    -- Gaps are fine (left by deleted tabs); order is by position.
    position INTEGER NOT NULL,
    PRIMARY KEY (setlist_id, tab_id)
) WITHOUT ROWID;
CREATE INDEX setlist_items_tab ON setlist_items (tab_id);

CREATE TABLE videos (
    id INTEGER PRIMARY KEY,
    tab_id INTEGER NOT NULL REFERENCES tabs (id) ON DELETE CASCADE,
    youtube_id TEXT NOT NULL,
    label TEXT NOT NULL DEFAULT '',
    is_default INTEGER NOT NULL DEFAULT 0,
    -- serde_json of `sync::SyncSpec`.
    sync TEXT NOT NULL
);
CREATE INDEX videos_tab ON videos (tab_id);

CREATE TABLE practice (
    tab_id INTEGER PRIMARY KEY REFERENCES tabs (id) ON DELETE CASCADE,
    track INTEGER NOT NULL DEFAULT 0,
    speed REAL NOT NULL DEFAULT 1.0,
    loop_start_bar INTEGER,
    loop_end_bar INTEGER,
    last_position_secs REAL NOT NULL DEFAULT 0
);
";

/// Practice tools: named loop sections per tab, and practice time per tab per local day.
const V2: &str = "
CREATE TABLE saved_loops (
    id INTEGER PRIMARY KEY,
    tab_id INTEGER NOT NULL REFERENCES tabs (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    -- Inclusive master-bar range (0-based).
    start_bar INTEGER NOT NULL,
    end_bar INTEGER NOT NULL,
    -- Unix seconds.
    created_at INTEGER NOT NULL
);
CREATE INDEX saved_loops_tab ON saved_loops (tab_id);

CREATE TABLE practice_days (
    tab_id INTEGER NOT NULL REFERENCES tabs (id) ON DELETE CASCADE,
    -- Days since 1970-01-01 in local time.
    day INTEGER NOT NULL,
    secs REAL NOT NULL DEFAULT 0,
    -- Sessions that started this day (a gap of 30 minutes starts a new one).
    sessions INTEGER NOT NULL DEFAULT 0,
    -- Unix seconds of the last recording.
    last_at INTEGER NOT NULL,
    PRIMARY KEY (tab_id, day)
) WITHOUT ROWID;
";

/// Bring the database up to the latest schema. Refuses a database from a newer Tabster.
/// The version is read inside each step's write transaction, so two connections opening the
/// same library at once can't apply a step twice.
pub(super) fn migrate(connection: &mut Connection) -> Result<(), LibraryError> {
    loop {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > latest_version() {
            return Err(LibraryError::Db(format!(
                "the library was written by a newer Tabster (schema {version})"
            )));
        }
        let Some(step) = usize::try_from(version)
            .ok()
            .and_then(|v| MIGRATIONS.get(v))
        else {
            return Ok(());
        };
        tx.execute_batch(step)?;
        tx.pragma_update(None, "user_version", version + 1)?;
        tx.commit()?;
    }
}

pub(super) fn latest_version() -> i64 {
    MIGRATIONS.len() as i64
}

/// Apply only the first `version` migrations to a fresh database (tests of upgrades).
#[cfg(test)]
pub(super) fn migrate_to(connection: &Connection, version: usize) -> Result<(), LibraryError> {
    for (v, step) in MIGRATIONS.iter().enumerate().take(version) {
        connection.execute_batch(step)?;
        connection.pragma_update(None, "user_version", v as i64 + 1)?;
    }
    Ok(())
}

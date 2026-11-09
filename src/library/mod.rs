//! The tab library: SQLite catalogue (WAL, migrations via `PRAGMA user_version`, FTS5 search),
//! a content-addressed file store (`<data dir>/tabs/<sha256>.<ext>`), a parsed-score cache
//! (`<data dir>/cache/<sha256>.postcard`, versioned header, decode failure = miss), tags,
//! setlists, attached videos and their sync, and folder watching ([`FolderWatcher`]).
//!
//! Everything on disk is owner-only (0700 directories, 0600 files). Writes take turns on one
//! connection; reads use a second, `query_only` connection, which WAL lets run alongside a write.
//!
//! Frozen contract: the public types and `Library` method signatures. Owner: Phase 1 agent E.
//! The database schema is the agent's to design (see PLAN.md §1 for the intended shape).

use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::formats::{self, Format, ParseError};
use crate::model::Score;
use crate::sync::SyncSpec;

pub type TabId = i64;
pub type VideoId = i64;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("database error: {0}")]
    Db(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("not found")]
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct TabSummary {
    pub id: TabId,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: Format,
    /// e.g. "E A D G B E", "Drop D". Of the first non-percussion track.
    pub tuning: String,
    pub track_count: u32,
    pub bar_count: u32,
    pub duration_secs: f64,
    pub favourite: bool,
    pub has_video: bool,
    pub tags: Vec<String>,
    /// Unix seconds.
    pub imported_at: i64,
    pub last_opened_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Enum)]
pub enum SortOrder {
    #[default]
    Title,
    Artist,
    RecentlyAdded,
    RecentlyOpened,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct TabFilter {
    /// Full-text search over title/artist/album; empty = all.
    pub query: String,
    pub favourites_only: bool,
    pub tag: Option<String>,
    pub setlist: Option<i64>,
    pub sort: SortOrder,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ImportOutcome {
    pub path: String,
    /// The tab's id (also set when the file was a duplicate of an existing tab).
    pub id: Option<TabId>,
    pub duplicate: bool,
    /// Human-readable reason when the import failed.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Video {
    pub id: VideoId,
    pub tab_id: TabId,
    /// The 11-character YouTube video id.
    pub youtube_id: String,
    pub label: String,
    pub is_default: bool,
    pub sync: SyncSpec,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Setlist {
    pub id: i64,
    pub name: String,
    pub tab_count: u32,
}

/// Per-tab practice state, restored when a tab is reopened.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct PracticeState {
    pub track: u32,
    pub speed: f64,
    /// Inclusive master-bar range.
    pub loop_start_bar: Option<u32>,
    pub loop_end_bar: Option<u32>,
    pub last_position_secs: f64,
}

mod metadata;
mod practice;
mod schema;
#[cfg(test)]
mod tests;
pub mod watch;
mod youtube;

pub use practice::{PracticeStats, SavedLoop};
pub use watch::FolderWatcher;

impl From<rusqlite::Error> for LibraryError {
    fn from(error: rusqlite::Error) -> Self {
        match &error {
            rusqlite::Error::QueryReturnedNoRows => LibraryError::NotFound,
            // A row referring to a tab, setlist … that doesn't exist.
            rusqlite::Error::SqliteFailure(e, _)
                if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
            {
                LibraryError::NotFound
            }
            _ => LibraryError::Db(error.to_string()),
        }
    }
}

/// How long a connection waits for another (e.g. another process) to release a lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Bumped whenever the cached form of a score changes incompatibly. The crate version is also
/// part of the header, so every release starts with a fresh cache.
const CACHE_FORMAT: u32 = 2;
const CACHE_MAGIC: &[u8; 4] = b"SGSC";

/// Separates tag names in `group_concat` (`char(31)`, the ASCII unit separator).
const TAG_SEPARATOR: char = '\u{1f}';

/// Columns of a [`TabSummary`], in the order [`summary_from_row`] reads them. `t` is `tabs`.
const SUMMARY_COLUMNS: &str = "t.id, t.title, t.artist, t.album, t.format, t.tuning,
    t.track_count, t.bar_count, t.duration_secs, t.favourite,
    EXISTS (SELECT 1 FROM videos v WHERE v.tab_id = t.id),
    (SELECT group_concat(g.name, char(31)) FROM tab_tags tt JOIN tags g ON g.id = tt.tag_id
      WHERE tt.tab_id = t.id),
    t.imported_at, t.last_opened_at";

type ParseFn<'a> = &'a dyn Fn(&[u8], Option<&str>) -> Result<Score, ParseError>;

pub struct Library {
    data_dir: PathBuf,
    tabs_dir: PathBuf,
    cache_dir: PathBuf,
    /// All writes take turns on this connection. Stored files are written and removed while
    /// it's held, so an import and a delete of the same content can't interleave.
    writer: Mutex<Connection>,
    /// `query_only`; WAL lets it read while a write is in progress.
    reader: Mutex<Connection>,
}

impl Library {
    /// Open (creating and migrating if needed) the library in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self, LibraryError> {
        let tabs_dir = data_dir.join("tabs");
        let cache_dir = data_dir.join("cache");
        for dir in [data_dir, tabs_dir.as_path(), cache_dir.as_path()] {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
            // `recursive` leaves existing directories alone; tighten them too.
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }

        let db_path = data_dir.join("library.sqlite");
        set_aside_if_corrupt(&db_path)?;
        let mut writer = Connection::open(&db_path)?;
        configure(&writer)?;
        writer.pragma_update(None, "journal_mode", "WAL")?;
        writer.pragma_update(None, "synchronous", "NORMAL")?;
        schema::migrate(&mut writer)?;
        // The WAL and shared-memory files appear with the first write; create them now so they
        // get the same permissions.
        writer.execute_batch("BEGIN IMMEDIATE; COMMIT;")?;
        for suffix in ["", "-wal", "-shm"] {
            let file = PathBuf::from(format!("{}{suffix}", db_path.display()));
            if file.exists() {
                fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
            }
        }

        let reader = Connection::open(&db_path)?;
        configure(&reader)?;
        reader.pragma_update(None, "query_only", true)?;

        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            tabs_dir,
            cache_dir,
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Import one file: hash, dedupe, copy into the store, parse, catalogue, cache. Safe to call
    /// from several threads at once.
    pub fn import_file(&self, path: &Path) -> ImportOutcome {
        self.import_with(path, &formats::parse)
    }

    /// [`Library::import_file`] with the parser injected (tests use a fake one).
    fn import_with(&self, path: &Path, parse: ParseFn) -> ImportOutcome {
        let mut outcome = ImportOutcome {
            path: path.to_string_lossy().into_owned(),
            id: None,
            duplicate: false,
            error: None,
        };
        match self.try_import(path, parse) {
            Ok((id, duplicate)) => {
                outcome.id = Some(id);
                outcome.duplicate = duplicate;
            }
            Err(e) => {
                log::info!("import of {} failed: {e}", path.display());
                outcome.error = Some(e.to_string());
            }
        }
        outcome
    }

    /// The tab's id and whether it was already in the library.
    fn try_import(&self, path: &Path, parse: ParseFn) -> Result<(TabId, bool), LibraryError> {
        let bytes = read_limited(path)?;
        let hash = sha256_hex(&bytes);
        let extension = path.extension().and_then(|e| e.to_str());
        let format = formats::detect(&bytes, extension).ok_or(ParseError::UnknownFormat)?;
        if let Some(id) = find_by_hash(&self.reader.lock(), &hash)? {
            return Ok((id, true));
        }

        // Parsing is the slow part: do it without holding any lock.
        let score = parse(&bytes, extension)?;
        let meta = metadata::TabMeta::new(&score, path);
        let stored = self.stored_path(&hash, format.extension());

        let id = {
            let writer = self.writer.lock();
            // Another thread may have imported the same content meanwhile.
            if let Some(id) = find_by_hash(&writer, &hash)? {
                return Ok((id, true));
            }
            write_atomic(&stored, &bytes)?;
            let inserted = writer.execute(
                "INSERT INTO tabs (sha256, format, title, artist, album, tuning, track_count,
                    bar_count, duration_secs, imported_at, original_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    hash,
                    format.extension(),
                    meta.title,
                    meta.artist,
                    meta.album,
                    meta.tuning,
                    meta.track_count,
                    meta.bar_count,
                    meta.duration_secs,
                    unix_now(),
                    path.to_string_lossy(),
                ],
            );
            if let Err(e) = inserted {
                let _ = fs::remove_file(&stored);
                return Err(e.into());
            }
            writer.last_insert_rowid()
        };
        // The cache is only an optimisation: a failure here just means a re-parse on open.
        if let Err(e) = self.write_cache(&hash, &score) {
            log::warn!("couldn't cache the parsed score of {}: {e}", path.display());
        }
        Ok((id, false))
    }

    pub fn list(&self, filter: &TabFilter) -> Result<Vec<TabSummary>, LibraryError> {
        let mut sql = format!("SELECT {SUMMARY_COLUMNS} FROM tabs t");
        let mut args: Vec<Value> = Vec::new();
        if let Some(setlist) = filter.setlist {
            sql.push_str(" JOIN setlist_items si ON si.tab_id = t.id AND si.setlist_id = ?");
            args.push(Value::Integer(setlist));
        }
        sql.push_str(" WHERE 1");
        if let Some(query) = fts_query(&filter.query) {
            sql.push_str(" AND t.id IN (SELECT rowid FROM tabs_fts WHERE tabs_fts MATCH ?)");
            args.push(Value::Text(query));
        }
        if filter.favourites_only {
            sql.push_str(" AND t.favourite = 1");
        }
        if let Some(tag) = &filter.tag {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM tab_tags tt JOIN tags g ON g.id = tt.tag_id
                   WHERE tt.tab_id = t.id AND g.name = ?)",
            );
            args.push(Value::Text(tag.trim().to_string()));
        }
        sql.push_str(" ORDER BY ");
        sql.push_str(if filter.setlist.is_some() {
            "si.position"
        } else {
            match filter.sort {
                SortOrder::Title => "t.title COLLATE NOCASE, t.artist COLLATE NOCASE, t.id",
                // Tabs without an artist go last.
                SortOrder::Artist => {
                    "t.artist = '', t.artist COLLATE NOCASE, t.album COLLATE NOCASE,
                     t.title COLLATE NOCASE, t.id"
                }
                SortOrder::RecentlyAdded => "t.imported_at DESC, t.id DESC",
                SortOrder::RecentlyOpened => {
                    "t.last_opened_at IS NULL, t.last_opened_at DESC, t.id DESC"
                }
            }
        });

        let reader = self.reader.lock();
        let mut statement = reader.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(args), summary_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get(&self, id: TabId) -> Result<TabSummary, LibraryError> {
        let sql = format!("SELECT {SUMMARY_COLUMNS} FROM tabs t WHERE t.id = ?1");
        Ok(self.reader.lock().query_row(&sql, [id], summary_from_row)?)
    }

    /// Load the parsed score (from cache, re-parsing the stored file on a cache miss) and mark
    /// the tab as opened.
    pub fn load_score(&self, id: TabId) -> Result<Score, LibraryError> {
        self.load_score_with(id, &formats::parse)
    }

    /// [`Library::load_score`] with the parser injected (tests use a fake one).
    fn load_score_with(&self, id: TabId, parse: ParseFn) -> Result<Score, LibraryError> {
        let (hash, extension) = self.stored_key(id)?;
        let score = match self.read_cache(&hash) {
            Some(score) => score,
            None => {
                let bytes = fs::read(self.stored_path(&hash, &extension))?;
                let score = parse(&bytes, Some(&extension))?;
                if let Err(e) = self.write_cache(&hash, &score) {
                    log::warn!("couldn't cache the parsed score of tab {id}: {e}");
                }
                score
            }
        };
        self.writer.lock().execute(
            "UPDATE tabs SET last_opened_at = ?2 WHERE id = ?1",
            params![id, unix_now()],
        )?;
        Ok(score)
    }

    /// Path of the stored tab file.
    pub fn file_path(&self, id: TabId) -> Result<PathBuf, LibraryError> {
        let (hash, extension) = self.stored_key(id)?;
        Ok(self.stored_path(&hash, &extension))
    }

    pub fn delete(&self, id: TabId) -> Result<(), LibraryError> {
        let mut writer = self.writer.lock();
        let (hash, extension) = stored_key(&writer, id)?;
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM tabs WHERE id = ?1", [id])?;
        delete_unused_tags(&tx)?;
        tx.commit()?;
        // Rows are unique per hash, so nothing else refers to these files.
        for file in [self.stored_path(&hash, &extension), self.cache_path(&hash)] {
            match fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    log::warn!("couldn't remove {}: {e}", file.display());
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn set_favourite(&self, id: TabId, favourite: bool) -> Result<(), LibraryError> {
        let changed = self.writer.lock().execute(
            "UPDATE tabs SET favourite = ?2 WHERE id = ?1",
            params![id, favourite],
        )?;
        found(changed)
    }

    /// Replace the tab's tags. Names are trimmed; blank ones are dropped, and names differing
    /// only in ASCII case are the same tag.
    pub fn set_tags(&self, id: TabId, tags: &[String]) -> Result<(), LibraryError> {
        let mut writer = self.writer.lock();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.query_row("SELECT 1 FROM tabs WHERE id = ?1", [id], |_| Ok(()))?;
        tx.execute("DELETE FROM tab_tags WHERE tab_id = ?1", [id])?;
        for tag in tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            tx.execute(
                "INSERT INTO tags (name) VALUES (?1) ON CONFLICT (name) DO NOTHING",
                [tag],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO tab_tags (tab_id, tag_id)
                 SELECT ?1, id FROM tags WHERE name = ?2",
                params![id, tag],
            )?;
        }
        delete_unused_tags(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// All tags in use, alphabetical.
    pub fn tags(&self) -> Result<Vec<String>, LibraryError> {
        let reader = self.reader.lock();
        let mut statement = reader.prepare(
            "SELECT name FROM tags WHERE id IN (SELECT tag_id FROM tab_tags)
             ORDER BY name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn setlists(&self) -> Result<Vec<Setlist>, LibraryError> {
        let reader = self.reader.lock();
        let mut statement = reader.prepare(
            "SELECT s.id, s.name, (SELECT count(*) FROM setlist_items i WHERE i.setlist_id = s.id)
             FROM setlists s ORDER BY s.name COLLATE NOCASE, s.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Setlist {
                id: row.get(0)?,
                name: row.get(1)?,
                tab_count: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn create_setlist(&self, name: &str) -> Result<i64, LibraryError> {
        let writer = self.writer.lock();
        writer.execute(
            "INSERT INTO setlists (name, created_at) VALUES (?1, ?2)",
            params![name.trim(), unix_now()],
        )?;
        Ok(writer.last_insert_rowid())
    }

    pub fn rename_setlist(&self, id: i64, name: &str) -> Result<(), LibraryError> {
        let changed = self.writer.lock().execute(
            "UPDATE setlists SET name = ?1 WHERE id = ?2",
            params![name.trim(), id],
        )?;
        found(changed)
    }

    pub fn delete_setlist(&self, id: i64) -> Result<(), LibraryError> {
        let changed = self
            .writer
            .lock()
            .execute("DELETE FROM setlists WHERE id = ?1", [id])?;
        found(changed)
    }

    /// Replace the setlist's contents with `tabs`, in order. A tab listed twice keeps its first
    /// position.
    pub fn set_setlist_tabs(&self, id: i64, tabs: &[TabId]) -> Result<(), LibraryError> {
        let mut writer = self.writer.lock();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.query_row("SELECT 1 FROM setlists WHERE id = ?1", [id], |_| Ok(()))?;
        tx.execute("DELETE FROM setlist_items WHERE setlist_id = ?1", [id])?;
        {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO setlist_items (setlist_id, tab_id, position)
                 VALUES (?1, ?2, ?3)",
            )?;
            for (position, tab) in tabs.iter().enumerate() {
                insert.execute(params![id, tab, position as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn videos(&self, tab: TabId) -> Result<Vec<Video>, LibraryError> {
        let reader = self.reader.lock();
        let mut statement = reader.prepare(
            "SELECT id, tab_id, youtube_id, label, is_default, sync FROM videos
             WHERE tab_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([tab], video_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Attach a video. The first video attached to a tab becomes its default.
    pub fn add_video(
        &self,
        tab: TabId,
        youtube_id: &str,
        label: &str,
    ) -> Result<VideoId, LibraryError> {
        let sync = sync_json(&SyncSpec::default())?;
        let writer = self.writer.lock();
        // One statement, so the "first video" check and the insert are atomic.
        writer.execute(
            "INSERT INTO videos (tab_id, youtube_id, label, is_default, sync)
             VALUES (?1, ?2, ?3, NOT EXISTS (SELECT 1 FROM videos WHERE tab_id = ?1), ?4)",
            params![tab, youtube_id, label.trim(), sync],
        )?;
        Ok(writer.last_insert_rowid())
    }

    /// Removing the default video promotes the oldest remaining one.
    pub fn remove_video(&self, id: VideoId) -> Result<(), LibraryError> {
        let mut writer = self.writer.lock();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (tab, was_default): (TabId, bool) = tx.query_row(
            "SELECT tab_id, is_default FROM videos WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        tx.execute("DELETE FROM videos WHERE id = ?1", [id])?;
        if was_default {
            tx.execute(
                "UPDATE videos SET is_default = 1
                 WHERE id = (SELECT min(id) FROM videos WHERE tab_id = ?1)",
                [tab],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn set_default_video(&self, id: VideoId) -> Result<(), LibraryError> {
        let mut writer = self.writer.lock();
        let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let tab: TabId = tx.query_row("SELECT tab_id FROM videos WHERE id = ?1", [id], |row| {
            row.get(0)
        })?;
        tx.execute(
            "UPDATE videos SET is_default = (id = ?1) WHERE tab_id = ?2",
            params![id, tab],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_video_sync(&self, id: VideoId, sync: &SyncSpec) -> Result<(), LibraryError> {
        let changed = self.writer.lock().execute(
            "UPDATE videos SET sync = ?2 WHERE id = ?1",
            params![id, sync_json(sync)?],
        )?;
        found(changed)
    }

    /// The tab's saved practice state; `None` if it has never been saved (a tab not yet opened).
    pub fn practice_state(&self, tab: TabId) -> Result<Option<PracticeState>, LibraryError> {
        Ok(self
            .reader
            .lock()
            .query_row(
                "SELECT track, speed, loop_start_bar, loop_end_bar, last_position_secs
                 FROM practice WHERE tab_id = ?1",
                [tab],
                |row| {
                    Ok(PracticeState {
                        track: row.get(0)?,
                        speed: row.get(1)?,
                        loop_start_bar: row.get(2)?,
                        loop_end_bar: row.get(3)?,
                        last_position_secs: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_practice_state(
        &self,
        tab: TabId,
        state: &PracticeState,
    ) -> Result<(), LibraryError> {
        self.writer.lock().execute(
            "INSERT INTO practice
                (tab_id, track, speed, loop_start_bar, loop_end_bar, last_position_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (tab_id) DO UPDATE SET
                track = excluded.track,
                speed = excluded.speed,
                loop_start_bar = excluded.loop_start_bar,
                loop_end_bar = excluded.loop_end_bar,
                last_position_secs = excluded.last_position_secs",
            params![
                tab,
                state.track,
                state.speed,
                state.loop_start_bar,
                state.loop_end_bar,
                state.last_position_secs,
            ],
        )?;
        Ok(())
    }

    // -- File store and parse cache ------------------------------------------------------------

    /// The stored file's hash and extension.
    fn stored_key(&self, id: TabId) -> Result<(String, String), LibraryError> {
        stored_key(&self.reader.lock(), id)
    }

    fn stored_path(&self, hash: &str, extension: &str) -> PathBuf {
        self.tabs_dir.join(format!("{hash}.{extension}"))
    }

    fn cache_path(&self, hash: &str) -> PathBuf {
        self.cache_dir.join(format!("{hash}.postcard"))
    }

    /// The cached score, or `None` if it's missing, from another version, or unreadable.
    fn read_cache(&self, hash: &str) -> Option<Score> {
        let bytes = fs::read(self.cache_path(hash)).ok()?;
        let body = bytes.strip_prefix(CACHE_MAGIC.as_slice())?;
        let ((format, version, score), rest): ((u32, String, Score), _) =
            postcard::take_from_bytes(body).ok()?;
        (format == CACHE_FORMAT && version == env!("CARGO_PKG_VERSION") && rest.is_empty())
            .then_some(score)
    }

    fn write_cache(&self, hash: &str, score: &Score) -> Result<(), LibraryError> {
        let header = (CACHE_FORMAT, env!("CARGO_PKG_VERSION"), score);
        let bytes = postcard::to_extend(&header, CACHE_MAGIC.to_vec())
            .map_err(|e| std::io::Error::other(format!("encoding the score: {e}")))?;
        write_atomic(&self.cache_path(hash), &bytes)?;
        Ok(())
    }
}

/// Extract the 11-character video id from a YouTube URL (watch, youtu.be, shorts, embed,
/// music.youtube.com) or a bare id.
pub fn parse_youtube_id(input: &str) -> Option<String> {
    youtube::parse_youtube_id(input)
}

/// Tab files are at most a few MB (GP8 files with embedded audio a few tens); refuse anything
/// bigger rather than reading it all into memory.
const MAX_TAB_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Read a file to import, refusing ones over [`MAX_TAB_FILE_BYTES`] (checked while reading, so a
/// file that grows meanwhile can't sneak past).
fn read_limited(path: &Path) -> Result<Vec<u8>, LibraryError> {
    use std::io::Read;
    // Opening a FIFO or device would block or never end.
    if !fs::metadata(path)?.is_file() {
        return Err(LibraryError::Io(std::io::Error::other(
            "not a regular file",
        )));
    }
    let file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    let mut bytes = Vec::with_capacity(size.min(MAX_TAB_FILE_BYTES) as usize);
    file.take(MAX_TAB_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_TAB_FILE_BYTES {
        return Err(LibraryError::Io(std::io::Error::other(format!(
            "file is larger than {} MB",
            MAX_TAB_FILE_BYTES / (1024 * 1024)
        ))));
    }
    Ok(bytes)
}

/// If the database at `db_path` exists but isn't a readable SQLite database (or fails SQLite's
/// quick integrity check), move it and its WAL/shared-memory files aside as
/// `library.sqlite.corrupt-<unix time>…` so the library starts fresh instead of failing to open
/// forever. The stored tab files are left alone. A database that's merely locked or unreadable
/// isn't touched (opening then reports the error).
fn set_aside_if_corrupt(db_path: &Path) -> Result<(), LibraryError> {
    if !db_path.exists() {
        return Ok(());
    }
    let corrupt = match Connection::open(db_path).and_then(|c| {
        c.busy_timeout(BUSY_TIMEOUT)?;
        c.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
    }) {
        Ok(result) => result != "ok",
        Err(rusqlite::Error::SqliteFailure(e, _)) => matches!(
            e.code,
            rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt
        ),
        Err(_) => false,
    };
    if !corrupt {
        return Ok(());
    }
    let aside = format!("{}.corrupt-{}", db_path.display(), unix_now());
    log::warn!(
        "the library database {} is corrupt; moving it to {aside} and starting a new one",
        db_path.display()
    );
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", db_path.display()));
        match fs::rename(&from, format!("{aside}{suffix}")) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

fn configure(connection: &Connection) -> Result<(), LibraryError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", true)?;
    Ok(())
}

fn find_by_hash(connection: &Connection, hash: &str) -> Result<Option<TabId>, LibraryError> {
    Ok(connection
        .query_row("SELECT id FROM tabs WHERE sha256 = ?1", [hash], |row| {
            row.get(0)
        })
        .optional()?)
}

fn stored_key(connection: &Connection, id: TabId) -> Result<(String, String), LibraryError> {
    Ok(connection.query_row(
        "SELECT sha256, format FROM tabs WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

fn delete_unused_tags(connection: &Connection) -> Result<(), LibraryError> {
    connection.execute(
        "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM tab_tags)",
        [],
    )?;
    Ok(())
}

/// `NotFound` when a statement touched no rows.
fn found(changed: usize) -> Result<(), LibraryError> {
    if changed == 0 {
        Err(LibraryError::NotFound)
    } else {
        Ok(())
    }
}

fn summary_from_row(row: &Row) -> rusqlite::Result<TabSummary> {
    let format: String = row.get(4)?;
    let tags: Option<String> = row.get(11)?;
    let mut tags: Vec<String> = tags
        .map(|t| t.split(TAG_SEPARATOR).map(str::to_string).collect())
        .unwrap_or_default();
    tags.sort_by_key(|t| t.to_lowercase());
    Ok(TabSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        format: Format::from_extension(&format).unwrap_or(Format::Gp7),
        tuning: row.get(5)?,
        track_count: row.get(6)?,
        bar_count: row.get(7)?,
        duration_secs: row.get(8)?,
        favourite: row.get(9)?,
        has_video: row.get(10)?,
        tags,
        imported_at: row.get(12)?,
        last_opened_at: row.get(13)?,
    })
}

fn video_from_row(row: &Row) -> rusqlite::Result<Video> {
    let sync: String = row.get(5)?;
    Ok(Video {
        id: row.get(0)?,
        tab_id: row.get(1)?,
        youtube_id: row.get(2)?,
        label: row.get(3)?,
        is_default: row.get(4)?,
        sync: serde_json::from_str(&sync).unwrap_or_default(),
    })
}

fn sync_json(sync: &SyncSpec) -> Result<String, LibraryError> {
    serde_json::to_string(sync).map_err(|e| LibraryError::Db(format!("encoding sync: {e}")))
}

/// An FTS5 query matching rows that contain a word starting with each word of `text`, or
/// `None` to match everything. Each word is quoted, so FTS syntax in the input is literal.
fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split_whitespace()
        .map(|word| word.replace('"', ""))
        // Pure punctuation tokenises to nothing, which can't be a prefix query.
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .map(|word| format!("\"{word}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes).iter() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Write to a uniquely named temp file beside `path`, then rename it into place, so a
/// half-written file is never seen. Owner-only.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = fs::write(&tmp, bytes)
        .and_then(|()| fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)))
        .and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

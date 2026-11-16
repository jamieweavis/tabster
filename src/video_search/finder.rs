//! The background video finder: one thread working through a queue of tabs, one search at a time
//! and no faster than one request per [`VideoFinder::new`]'s interval, so a big import doesn't
//! hammer YouTube. A confident match is attached when the tab has no video yet; anything else is
//! kept as suggestions. The thread exits when the queue is empty and starts again on demand.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::client::Searcher;
use super::{MatchConfidence, SearchError, Song, VideoCandidate, VideoMatch, find, matching};
use crate::library::{Library, LibraryError, TabId, VideoDetails, VideoId};

/// What the finder needs from its owner.
pub(crate) trait FinderHost: Send + Sync + 'static {
    fn library(&self) -> &Library;
    fn finder(&self) -> &VideoFinder;
    /// A tab's search finished (or failed). Called on the finder's thread.
    fn search_finished(&self, tab: TabId);
}

pub(crate) struct VideoFinder {
    queue: Mutex<Queue>,
    searcher: Throttled,
    auto_attach: AtomicBool,
    shutdown: AtomicBool,
}

#[derive(Default)]
struct Queue {
    tabs: VecDeque<TabId>,
    running: bool,
}

/// After YouTube says "too many requests", wait this long, doubling up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);

impl VideoFinder {
    pub fn new(searcher: Arc<dyn Searcher>, interval: Duration) -> Self {
        Self {
            queue: Mutex::new(Queue::default()),
            searcher: Throttled {
                inner: searcher,
                interval,
                last: Mutex::new(None),
            },
            auto_attach: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn set_auto_attach(&self, on: bool) {
        self.auto_attach.store(on, Ordering::Relaxed);
    }

    /// Queue `tabs` (already marked pending in the library). `first` puts them ahead of the
    /// queue (the user asked); otherwise they go at the back.
    pub fn enqueue<H: FinderHost>(&self, host: &Arc<H>, tabs: &[TabId], first: bool) {
        if tabs.is_empty() || self.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let mut queue = self.queue.lock();
        queue.tabs.retain(|t| !tabs.contains(t));
        if first {
            for &tab in tabs.iter().rev() {
                queue.tabs.push_front(tab);
            }
        } else {
            queue.tabs.extend(tabs);
        }
        if !queue.running {
            let weak = Arc::downgrade(host);
            let spawned = thread::Builder::new()
                .name("tabster-video-finder".into())
                .spawn(move || run(weak));
            match spawned {
                Ok(_) => queue.running = true,
                Err(e) => log::error!("couldn't start the video finder: {e}"),
            }
        }
    }

    /// Stop after the current search. Queued tabs stay pending in the library.
    pub fn shut_down(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Search right now with the user's own words (blocking), ranking against `song`.
    pub fn search(&self, song: &Song, query: &str) -> Result<Vec<VideoMatch>, SearchError> {
        let query = query.trim();
        if query.is_empty() || query == song.query() {
            return find(&self.searcher, song);
        }
        let mut ranked = matching::rank(song, &self.searcher.search(query)?);
        ranked.truncate(super::MAX_RESULTS);
        Ok(ranked)
    }

    /// The next tab to search, or `None` (and the thread marked stopped) when there's none.
    fn next(&self) -> Option<TabId> {
        let mut queue = self.queue.lock();
        let next = if self.shutdown.load(Ordering::Relaxed) {
            None
        } else {
            queue.tabs.pop_front()
        };
        if next.is_none() {
            queue.running = false;
        }
        next
    }

    fn retry_later(&self, tab: TabId) {
        self.queue.lock().tabs.push_front(tab);
    }
}

fn run<H: FinderHost>(weak: Weak<H>) {
    let mut backoff = FIRST_BACKOFF;
    loop {
        let Some(host) = weak.upgrade() else { return };
        let Some(tab) = host.finder().next() else {
            return;
        };
        if let Err(SearchError::RateLimited) = search_tab(&*host, tab) {
            log::warn!(
                "YouTube is limiting searches; pausing for {}s",
                backoff.as_secs()
            );
            host.finder().retry_later(tab);
            drop(host);
            if !pause(&weak, backoff) {
                return;
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
            continue;
        }
        backoff = FIRST_BACKOFF;
        host.search_finished(tab);
    }
}

/// Sleep for `duration`, waking early (false) if the host goes away or shuts down.
fn pause<H: FinderHost>(weak: &Weak<H>, duration: Duration) -> bool {
    let until = Instant::now() + duration;
    loop {
        let now = Instant::now();
        if now >= until {
            return true;
        }
        thread::sleep((until - now).min(Duration::from_millis(500)));
        match weak.upgrade() {
            Some(host) if !host.finder().shutdown.load(Ordering::Relaxed) => {}
            _ => return false,
        }
    }
}

/// Search for one tab and record the outcome. Only a rate limit is returned as an error (the
/// tab is then retried); other failures are recorded on the tab.
fn search_tab<H: FinderHost>(host: &H, tab: TabId) -> Result<(), SearchError> {
    let library = host.library();
    let summary = match library.get(tab) {
        Ok(summary) => summary,
        // Deleted while queued.
        Err(LibraryError::NotFound) => return Ok(()),
        Err(e) => {
            log::error!("video search for tab {tab}: couldn't read it: {e}");
            return Ok(());
        }
    };
    let song = Song::new(&summary.title, &summary.artist, summary.duration_secs, "");
    let query = song.query();
    let finder = host.finder();
    let outcome = match find(&finder.searcher, &song) {
        Ok(results) => {
            let attached = if finder.auto_attach.load(Ordering::Relaxed) {
                attach_best(library, tab, &results)
            } else {
                None
            };
            log::info!(
                "video search for tab {tab}: {} result(s), best {:?}{}",
                results.len(),
                results.first().map(|m| m.confidence),
                if attached.is_some() { ", attached" } else { "" }
            );
            library.finish_video_search(tab, &query, &results, attached.as_deref())
        }
        Err(SearchError::RateLimited) => return Err(SearchError::RateLimited),
        Err(e) => {
            log::warn!("video search for tab {tab} failed: {e}");
            library.fail_video_search(tab, &query, &e.to_string())
        }
    };
    match outcome {
        Ok(()) | Err(LibraryError::NotFound) => {}
        Err(e) => log::error!("couldn't save the video search for tab {tab}: {e}"),
    }
    Ok(())
}

/// Attach the best result if it's a confident match and the tab still has no video; returns its
/// YouTube id.
fn attach_best(library: &Library, tab: TabId, results: &[VideoMatch]) -> Option<String> {
    let best = results
        .first()
        .filter(|m| m.confidence == MatchConfidence::High)?;
    if !library.videos(tab).ok()?.is_empty() {
        return None;
    }
    match attach(library, tab, &best.candidate, &best.label) {
        Ok(_) => Some(best.candidate.youtube_id.clone()),
        Err(e) => {
            log::error!("couldn't attach the video found for tab {tab}: {e}");
            None
        }
    }
}

/// Attach a search result to a tab, keeping its YouTube details.
pub(crate) fn attach(
    library: &Library,
    tab: TabId,
    candidate: &VideoCandidate,
    label: &str,
) -> Result<VideoId, LibraryError> {
    let details = VideoDetails {
        title: candidate.title.clone(),
        channel: candidate.channel.clone(),
        duration_secs: candidate.duration_secs,
    };
    library.add_video_with_details(tab, &candidate.youtube_id, label, &details)
}

/// Spaces requests at least `interval` apart, whoever makes them.
struct Throttled {
    inner: Arc<dyn Searcher>,
    interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl Searcher for Throttled {
    fn search(&self, query: &str) -> Result<Vec<VideoCandidate>, SearchError> {
        {
            // Held while waiting, so callers queue up behind each other.
            let mut last = self.last.lock();
            if let Some(at) = *last {
                let wait = self.interval.saturating_sub(at.elapsed());
                if !wait.is_zero() {
                    thread::sleep(wait);
                }
            }
            *last = Some(Instant::now());
        }
        self.inner.search(query)
    }
}

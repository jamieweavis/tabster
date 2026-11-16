//! Fetching search results from YouTube.
//!
//! Uses the web client's own search endpoint (`youtubei/v1/search`, JSON, no API key) and falls
//! back to the HTML results page, whose embedded `ytInitialData` holds the same data. Only the
//! search text is sent: no cookies, no account.

use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use super::{SearchError, VideoCandidate, parse};

/// Something that can search YouTube; tests provide canned results.
pub trait Searcher: Send + Sync {
    fn search(&self, query: &str) -> Result<Vec<VideoCandidate>, SearchError>;
}

pub struct YouTubeClient {
    agent: Agent,
}

const SEARCH_URL: &str = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false";
const RESULTS_URL: &str = "https://www.youtube.com/results";
/// The web client version sent with searches. YouTube accepts old versions for a long time; bump
/// it if searches start failing.
const CLIENT_VERSION: &str = "2.20251101.01.00";
/// Search filter "Type: Video" (no channels, playlists or Shorts shelves).
const VIDEOS_ONLY: &str = "EgIQAQ%3D%3D";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                          (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
/// Search responses are a few hundred KB; anything far bigger isn't one.
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

impl YouTubeClient {
    pub fn new() -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let agent = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self { agent }
    }

    fn search_api(&self, query: &str) -> Result<Vec<VideoCandidate>, SearchError> {
        let body = serde_json::json!({
            "context": { "client": { "clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": "en" } },
            "query": query,
            "params": VIDEOS_ONLY,
        });
        let response = self
            .agent
            .post(SEARCH_URL)
            .header("Content-Type", "application/json")
            .header("Origin", "https://www.youtube.com")
            .header("X-YouTube-Client-Name", "1")
            .header("X-YouTube-Client-Version", CLIENT_VERSION)
            .send(body.to_string());
        parse::search_response(&read(response)?)
    }

    fn search_page(&self, query: &str) -> Result<Vec<VideoCandidate>, SearchError> {
        let response = self
            .agent
            .get(RESULTS_URL)
            .query("search_query", query)
            .query("sp", "EgIQAQ==")
            .query("hl", "en")
            .header("Accept-Language", "en")
            .call();
        parse::results_page(&read(response)?)
    }
}

impl Default for YouTubeClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Searcher for YouTubeClient {
    fn search(&self, query: &str) -> Result<Vec<VideoCandidate>, SearchError> {
        match self.search_api(query) {
            Ok(videos) => Ok(videos),
            // Not worth a second request.
            Err(e @ (SearchError::RateLimited | SearchError::Network(_))) => Err(e),
            Err(SearchError::Unrecognised) => {
                log::info!("the search API's response wasn't understood; trying the results page");
                self.search_page(query)
            }
        }
    }
}

fn read(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<String, SearchError> {
    let mut response = response.map_err(|e| SearchError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    if status == 429 {
        return Err(SearchError::RateLimited);
    }
    if !(200..300).contains(&status) {
        log::warn!("YouTube search returned HTTP {status}");
        return Err(if status >= 500 {
            SearchError::Network(format!("HTTP {status}"))
        } else {
            SearchError::Unrecognised
        });
    }
    response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .map_err(|e| SearchError::Network(e.to_string()))
}

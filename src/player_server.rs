//! A tiny HTTP server on `127.0.0.1` serving the YouTube player page, for when WKWebView's
//! `loadHTMLString` + `baseURL` route hits YouTube's error 153 (no Referer). The page's bytes
//! come from the shell (it ships the HTML in its bundle); the server only serves them at
//! `/player.html` and answers 404 to everything else.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use parking_lot::RwLock;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

pub const PAGE_PATH: &str = "/player.html";

/// The page to serve, shared with the server thread. Empty until the shell provides it.
pub type Page = Arc<RwLock<Arc<[u8]>>>;

/// Runs until dropped.
pub struct PlayerServer {
    server: Arc<Server>,
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlayerServer {
    /// Bind an ephemeral port on the loopback interface and start serving `page`.
    pub fn start(page: Page) -> io::Result<Self> {
        let server = Server::http("127.0.0.1:0").map_err(io::Error::other)?;
        let port = server
            .server_addr()
            .to_ip()
            .map(|addr| addr.port())
            .ok_or_else(|| io::Error::other("player server isn't on an IP socket"))?;
        let server = Arc::new(server);
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let server = Arc::clone(&server);
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("tabster-player-server".into())
                .spawn(move || serve(&server, &stop, &page))?
        };
        log::info!("player server listening on 127.0.0.1:{port}");
        Ok(Self {
            server,
            port,
            stop,
            thread: Some(thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://localhost:<port>/player.html`. The listener is bound to `127.0.0.1` only, but the
    /// URL must name `localhost`: YouTube's embed rejects IP-literal origins such as
    /// `http://127.0.0.1:<port>` with error 150, while `localhost` works. (WebKit falls back
    /// from `::1` to `127.0.0.1` when resolving `localhost`.) A query string is fine, e.g.
    /// `/player.html?v=<id>`.
    pub fn url(&self) -> String {
        format!("http://localhost:{}{PAGE_PATH}", self.port)
    }
}

impl Drop for PlayerServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(server: &Server, stop: &AtomicBool, page: &Page) {
    loop {
        match server.recv() {
            Ok(request) => respond(request, page),
            Err(_) if stop.load(Ordering::SeqCst) => return,
            Err(e) => log::warn!("player server: {e}"),
        }
    }
}

fn respond(request: Request, page: &Page) {
    let loopback = request
        .remote_addr()
        .is_some_and(|addr: &SocketAddr| addr.ip().is_loopback());
    if !loopback {
        let _ = request.respond(Response::empty(StatusCode(403)));
        return;
    }
    let path = request.url().split(['?', '#']).next().unwrap_or_default();
    let method_ok = matches!(request.method(), Method::Get | Method::Head);
    let body = page.read().clone();
    if path != PAGE_PATH || !method_ok || body.is_empty() {
        let _ = request.respond(Response::empty(StatusCode(404)));
        return;
    }
    let response = Response::from_data(body.to_vec())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"));
    let _ = request.respond(response);
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::*;

    fn get(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    #[test]
    fn serves_the_page_and_nothing_else() {
        let page: Page = Arc::new(RwLock::new(Arc::from(&b""[..])));
        let server = PlayerServer::start(page.clone()).unwrap();
        assert_eq!(
            server.url(),
            format!("http://localhost:{}/player.html", server.port())
        );

        // Not provided yet.
        assert!(get(server.port(), PAGE_PATH).starts_with("HTTP/1.1 404"));

        *page.write() = Arc::from("<html>player ✓</html>".as_bytes());
        let response = get(server.port(), "/player.html?v=abc");
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let lower = response.to_ascii_lowercase();
        assert!(lower.contains("content-type: text/html; charset=utf-8"));
        assert!(lower.contains("cache-control: no-store"));
        assert!(response.ends_with("<html>player ✓</html>"));

        assert!(get(server.port(), "/").starts_with("HTTP/1.1 404"));
        assert!(get(server.port(), "/etc/passwd").starts_with("HTTP/1.1 404"));

        let port = server.port();
        drop(server);
        assert!(
            TcpStream::connect(("127.0.0.1", port))
                .and_then(|mut s| {
                    s.write_all(b"GET /player.html HTTP/1.1\r\n\r\n")?;
                    let mut buf = String::new();
                    s.read_to_string(&mut buf)?;
                    Ok(buf)
                })
                .map_or(true, |r| !r.contains("player")),
            "still serving after drop"
        );
    }
}

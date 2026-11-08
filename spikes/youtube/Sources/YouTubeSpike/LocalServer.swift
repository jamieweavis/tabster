import Foundation
import Network

/// Minimal HTTP/1.1 server on 127.0.0.1:<random port> that serves one HTML page at `/player.html`.
/// Stand-in for the Rust core's `tiny_http` player_server.
final class LocalServer: @unchecked Sendable {
    private let listener: NWListener
    private let html: Data
    private let queue = DispatchQueue(label: "spike.localserver")

    init(html: String) throws {
        self.html = Data(html.utf8)
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        params.allowLocalEndpointReuse = true
        listener = try NWListener(using: params)
    }

    /// Starts listening and returns the bound port.
    func start() async throws -> UInt16 {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<UInt16, Error>) in
            let once = OnceFlag()
            listener.stateUpdateHandler = { [listener] state in
                switch state {
                case .ready:
                    if once.fire() { cont.resume(returning: listener.port?.rawValue ?? 0) }
                case .failed(let err):
                    if once.fire() { cont.resume(throwing: err) }
                default: break
                }
            }
            listener.newConnectionHandler = { [weak self] conn in self?.serve(conn) }
            listener.start(queue: queue)
        }
    }

    private func serve(_ conn: NWConnection) {
        conn.start(queue: queue)
        conn.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) { [html] data, _, _, _ in
            let request = data.flatMap { String(data: $0, encoding: .utf8) } ?? ""
            let line = request.split(separator: "\r\n").first.map(String.init) ?? ""
            let path = line.split(separator: " ").dropFirst().first.map(String.init) ?? "/"
            let route = path.split(separator: "?").first.map(String.init) ?? path
            Log.emit(["type": "http", "request": line])
            let (status, body, ctype): (String, Data, String) =
                (route == "/player.html" || route == "/")
                ? ("200 OK", html, "text/html; charset=utf-8")
                : ("404 Not Found", Data("not found".utf8), "text/plain")
            var head = "HTTP/1.1 \(status)\r\nContent-Type: \(ctype)\r\nContent-Length: \(body.count)\r\n"
            head += "Cache-Control: no-store\r\nReferrer-Policy: strict-origin-when-cross-origin\r\nConnection: close\r\n\r\n"
            conn.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in conn.cancel() })
        }
    }

    func stop() { listener.cancel() }
}

final class OnceFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var fired = false
    func fire() -> Bool { lock.withLock { defer { fired = true }; return !fired } }
}

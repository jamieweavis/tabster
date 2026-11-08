import Foundation
import QuartzCore

/// JSON-lines output on stdout. `ts` = seconds since launch (CACurrentMediaTime base).
enum Log {
    nonisolated(unsafe) static var start = CACurrentMediaTime()
    private static let lock = NSLock()

    static func emit(_ fields: [String: Any]) {
        var f = fields
        f["ts"] = (CACurrentMediaTime() - start).rounded(places: 4)
        let data = (try? JSONSerialization.data(withJSONObject: sanitize(f), options: [.sortedKeys]))
            ?? Data("{\"type\":\"logError\"}".utf8)
        lock.withLock {
            FileHandle.standardOutput.write(data + Data("\n".utf8))
        }
    }

    private static func sanitize(_ v: Any) -> Any {
        switch v {
        case let d as [String: Any]: return d.mapValues(sanitize)
        case let a as [Any]: return a.map(sanitize)
        case let x as Double: return x.isFinite ? x : NSNull()
        case is NSNull, is String, is Int, is Bool, is NSNumber: return v
        default: return String(describing: v)
        }
    }
}

extension Double {
    func rounded(places: Int) -> Double {
        let m = pow(10, Double(places))
        return (self * m).rounded() / m
    }
}

import Foundation

enum IOSLocalExecInvariantViolationLog {
    static let event = "sand.local_exec_daemon.invariant_violation"

    static func line(name: String) -> String {
        let object: [String: Any] = ["event": event, "name": name]
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
              let json = String(data: data, encoding: .utf8)
        else {
            return "{\"event\":\"\(event)\",\"name\":\"invalid\"}\n"
        }
        return json + "\n"
    }

    static func write(name: String, writer: (String) -> Void) {
        writer(line(name: name))
    }
}

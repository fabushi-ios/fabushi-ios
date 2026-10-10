import Foundation

enum TimeZonePolicy {
    static func isValidIANA(_ identifier: String) -> Bool { !identifier.isEmpty && TimeZone(identifier: identifier) != nil }

    static func formatUTCOffset(at date: Date, timeZone identifier: String) -> String? {
        guard let zone = TimeZone(identifier: identifier) else { return nil }
        let seconds = zone.secondsFromGMT(for: date), sign = seconds < 0 ? "-" : "+"
        let absolute = abs(seconds), hours = absolute / 3_600, minutes = (absolute % 3_600) / 60
        return minutes == 0 ? "UTC\(sign)\(hours)" : "UTC\(sign)\(hours):\(String(format: "%02d", minutes))"
    }

    static func renderSystemPrompt(timeZone identifier: String?, now: Date = Date()) -> String {
        guard let identifier, !identifier.isEmpty else { return "" }
        let zone = formatUTCOffset(at: now, timeZone: identifier).map { "\(identifier) (currently \($0))" } ?? identifier
        return """
        ## Time
        Your box and tools run on a UTC clock, but the user lives in \(zone). So any time you report to them — a git or gh timestamp, a file's mtime, a log line, "finished at", a schedule — is a UTC value: convert it to the user's zone and label it clearly (a short tag like "PT" is enough) rather than parroting the raw UTC time back.
        """
    }
}

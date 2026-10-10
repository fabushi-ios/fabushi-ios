import Foundation

enum RetryAfter {
    static func parseMilliseconds(_ raw: String?, now: Date = Date()) -> Int? {
        guard let trimmed = raw?.trimmingCharacters(in: .whitespacesAndNewlines), !trimmed.isEmpty else { return nil }
        if let seconds = Double(trimmed), seconds.isFinite {
            return seconds <= 0 ? 0 : Int((seconds * 1_000).rounded())
        }
        let locale = Locale(identifier: "en_US_POSIX")
        for format in ["EEE, dd MMM yyyy HH:mm:ss zzz", "EEEE, dd-MMM-yy HH:mm:ss zzz", "EEE MMM d HH:mm:ss yyyy"] {
            let formatter = DateFormatter()
            formatter.locale = locale
            formatter.timeZone = TimeZone(secondsFromGMT: 0)
            formatter.dateFormat = format
            if let date = formatter.date(from: trimmed) {
                return max(0, Int((date.timeIntervalSince(now) * 1_000).rounded()))
            }
        }
        return nil
    }
}

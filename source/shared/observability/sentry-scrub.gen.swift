import Foundation

private let SAND_SENTRY_REDACTED_PATH = "<REDACTED: user-file-path>"
private let SAND_SENTRY_REDACTED_URL = "<REDACTED: url>"
private let SAND_SENTRY_REDACTED_EXCEPTION = "<REDACTED: exception-message>"
private let SAND_SENTRY_MAX_EXCEPTIONS = 8
private let SAND_SENTRY_MAX_FRAMES = 100
private let SAND_SENTRY_MAX_THREADS = 16
private let SAND_SENTRY_MAX_COMPONENT_STACK_LINES = 64
private let SAND_SENTRY_SEVERITY_LEVELS: Set<String> = ["fatal", "error", "warning", "log", "info", "debug"]
private let SAND_SENTRY_SESSION_STATUSES: Set<String> = ["ok", "exited", "crashed", "abnormal"]
private let SAND_SENTRY_FATAL_TAGS: Set<String> = [
    "app_flavor",
    "event.environment",
    "event.origin",
    "event.process",
    "exit.reason",
    "crash.kind",
    "sand.failure_code",
    "sand.process",
]

private let sandSentryBoundedCode = try! NSRegularExpression(pattern: #"^[A-Za-z0-9._:@-]+$"#)
private let sandSentryOpaqueID = try! NSRegularExpression(pattern: #"^[A-Za-z0-9._:@|\-]+$"#)
private let sandSentryEmail = try! NSRegularExpression(pattern: #"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}"#)
private let sandSentrySecret = try! NSRegularExpression(
    pattern: #"(github_pat_[A-Za-z0-9_]{20,}|gh[psuro]_[A-Za-z0-9]{20,}|xox[pbar]-[A-Za-z0-9-]+|AIza[A-Za-z0-9_\\\-]{30,}|(?:key|token|sig|secret|signature|password|passwd|pwd)[^A-Za-z0-9])"#,
    options: [.caseInsensitive]
)

private func sandSentryMatches(_ regex: NSRegularExpression, _ value: String) -> Bool {
    regex.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) != nil
}

private func sandSentryBounded(_ value: Any?, maxLength: Int = 128) -> String? {
    guard let value = value as? String, !value.isEmpty, value.count <= maxLength else { return nil }
    guard sandSentryMatches(sandSentryBoundedCode, value) else { return nil }
    guard sandSentryEmail.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) == nil else { return nil }
    guard sandSentrySecret.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) == nil else { return nil }
    return value
}

func isSandSentryBoundedTagValue(_ value: Any?) -> Bool {
    sandSentryBounded(value) != nil
}

private func sandSentryBoundedID(_ value: Any?) -> Any? {
    if let number = value as? NSNumber {
        let double = number.doubleValue
        return double.isFinite ? number : nil
    }
    guard let value = value as? String, !value.isEmpty, value.count <= 128 else { return nil }
    guard sandSentryMatches(sandSentryOpaqueID, value) else { return nil }
    guard sandSentryEmail.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) == nil else { return nil }
    guard sandSentrySecret.firstMatch(in: value, range: NSRange(value.startIndex..., in: value)) == nil else { return nil }
    return value
}

private func sandSentryScrubURL(_ value: Any?) -> String? {
    guard let value = value as? String else { return nil }
    if value == SAND_SENTRY_REDACTED_URL { return value }
    guard let components = URLComponents(string: value),
          components.scheme == "app",
          components.host == nil || components.host == "",
          components.user == nil,
          components.password == nil else {
        return SAND_SENTRY_REDACTED_URL
    }
    let path = components.percentEncodedPath
    guard !path.isEmpty, path.count <= 512 else { return SAND_SENTRY_REDACTED_URL }
    let candidate = "app://\(path)"
    return candidate.range(of: #"^app:///[A-Za-z0-9._/\-]+$"#, options: .regularExpression) != nil
        ? candidate
        : SAND_SENTRY_REDACTED_URL
}

private func sandSentryFrameFilename(_ value: Any?) -> String? {
    guard let value = value as? String else { return nil }
    if value.count <= 512,
       value.range(of: #"^node:[A-Za-z0-9._/\-]+$"#, options: .regularExpression) != nil {
        return value
    }
    let appURL = sandSentryScrubURL(value)
    return appURL == SAND_SENTRY_REDACTED_URL ? SAND_SENTRY_REDACTED_PATH : appURL
}

private func sandSentryScrubFrame(_ value: Any) -> [String: Any]? {
    guard let value = value as? [String: Any] else { return nil }
    var result: [String: Any] = [:]
    if let filename = sandSentryFrameFilename(value["filename"]) {
        result["filename"] = filename
        if filename != SAND_SENTRY_REDACTED_PATH {
            if let function = sandSentryBounded(value["function"]) { result["function"] = function }
            if let module = sandSentryBounded(value["module"]) { result["module"] = module }
        }
    }
    for key in ["platform", "instruction_addr", "addr_mode", "debug_id"] {
        if let code = sandSentryBounded(value[key]) { result[key] = code }
    }
    for key in ["lineno", "colno"] {
        if let number = value[key] as? NSNumber, number.doubleValue.isFinite { result[key] = number }
    }
    if let inApp = value["in_app"] as? Bool { result["in_app"] = inApp }
    return result
}

private func sandSentryScrubStacktrace(_ value: Any?) -> [String: Any]? {
    guard let value = value as? [String: Any], let frames = value["frames"] as? [Any] else { return nil }
    return ["frames": frames.prefix(SAND_SENTRY_MAX_FRAMES).compactMap(sandSentryScrubFrame)]
}

private func sandSentryScrubException(_ value: Any?, tier: SandSentryPrivacyTier) -> [String: Any]? {
    guard let value = value as? [String: Any], let values = value["values"] as? [Any] else { return nil }
    let projected: [[String: Any]] = values.prefix(SAND_SENTRY_MAX_EXCEPTIONS).compactMap { item in
        guard let item = item as? [String: Any] else { return nil }
        var output: [String: Any] = [:]
        if let type = sandSentryBounded(item["type"]) {
            output["type"] = type
        } else if item["type"] is String {
            output["type"] = "<REDACTED: exception-type>"
        }
        guard tier != .fatalMetadata else { return output }
        if item["value"] is String { output["value"] = SAND_SENTRY_REDACTED_EXCEPTION }
        if let threadID = sandSentryBoundedID(item["thread_id"]) { output["thread_id"] = threadID }
        if let stack = sandSentryScrubStacktrace(item["stacktrace"]) { output["stacktrace"] = stack }
        if let mechanism = item["mechanism"] as? [String: Any] {
            var clean: [String: Any] = [:]
            if let type = sandSentryBounded(mechanism["type"]) { clean["type"] = type }
            if let handled = mechanism["handled"] as? Bool { clean["handled"] = handled }
            if let synthetic = mechanism["synthetic"] as? Bool { clean["synthetic"] = synthetic }
            if let group = mechanism["is_exception_group"] as? Bool { clean["is_exception_group"] = group }
            for key in ["exception_id", "parent_id"] {
                if let number = mechanism[key] as? NSNumber, number.doubleValue.isFinite { clean[key] = number }
            }
            if !clean.isEmpty { output["mechanism"] = clean }
        }
        return output
    }
    return projected.isEmpty ? nil : ["values": projected]
}

private func sandSentryScrubTags(_ value: Any?, tier: SandSentryPrivacyTier) -> [String: Any]? {
    guard let value = value as? [String: Any] else { return nil }
    var result: [String: Any] = [:]
    for (key, raw) in value {
        guard sandSentryBounded(key) != nil else { continue }
        if tier == .fatalMetadata && !SAND_SENTRY_FATAL_TAGS.contains(key) { continue }
        if let string = raw as? String, let safe = sandSentryBounded(string) {
            result[key] = safe
        } else if tier != .fatalMetadata, raw is Bool || raw is NSNumber || raw is NSNull {
            result[key] = raw
        }
    }
    return result.isEmpty ? nil : result
}

private func sandSentryScrubRequest(_ value: Any?) -> [String: Any]? {
    guard let value = value as? [String: Any] else { return nil }
    var result: [String: Any] = [:]
    if let method = sandSentryBounded(value["method"]) { result["method"] = method }
    if let url = sandSentryScrubURL(value["url"]) { result["url"] = url }
    return result.isEmpty ? nil : result
}

private func sandSentryScrubUser(_ value: Any?) -> [String: Any]? {
    guard let value = value as? [String: Any], let id = sandSentryBoundedID(value["id"]) else { return nil }
    return ["id": id]
}

private func sandSentryFiniteNumber(_ value: Any?) -> NSNumber? {
    guard let number = value as? NSNumber, number.doubleValue.isFinite else { return nil }
    return number
}

private func sandSentryNonNegativeNumber(_ value: Any?) -> NSNumber? {
    guard let number = sandSentryFiniteNumber(value), number.doubleValue >= 0 else { return nil }
    return number
}

private func sandSentryProjectCodes(_ source: [String: Any], keys: [String]) -> [String: Any] {
    var result: [String: Any] = [:]
    for key in keys {
        if let value = sandSentryBounded(source[key]) { result[key] = value }
    }
    return result
}

private func sandSentryProjectSDK(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    var result = sandSentryProjectCodes(source, keys: ["name", "version"])
    if let integrations = source["integrations"] as? [Any] {
        result["integrations"] = integrations.prefix(64).compactMap { sandSentryBounded($0) }
    }
    if let packages = source["packages"] as? [Any] {
        result["packages"] = packages.prefix(64).compactMap { candidate -> [String: String]? in
            guard let candidate = candidate as? [String: Any],
                  let name = sandSentryBounded(candidate["name"]),
                  let version = sandSentryBounded(candidate["version"]) else { return nil }
            return ["name": name, "version": version]
        }
    }
    return result.isEmpty ? nil : result
}

private func sandSentryProjectAppContext(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    var result = sandSentryProjectCodes(source, keys: [
        "app_name", "app_start_time", "app_version", "app_identifier", "build_type", "app_arch",
    ])
    for key in ["app_memory", "free_memory"] {
        if let number = sandSentryFiniteNumber(source[key]) { result[key] = number }
    }
    return result.isEmpty ? nil : result
}

private func sandSentryProjectOSContext(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    let result = sandSentryProjectCodes(source, keys: ["name", "version", "build", "kernel_version"])
    return result.isEmpty ? nil : result
}

private func sandSentryProjectRuntimeContext(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    let result = sandSentryProjectCodes(source, keys: ["name", "type", "version"])
    return result.isEmpty ? nil : result
}

private func sandSentryProjectDeviceContext(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    var result = sandSentryProjectCodes(source, keys: ["family", "arch", "screen_resolution", "orientation"])
    for key in [
        "screen_height_pixels", "screen_width_pixels", "screen_density", "screen_dpi",
        "memory_size", "free_memory", "usable_memory", "storage_size", "free_storage",
        "processor_count", "processor_frequency",
    ] {
        if let number = sandSentryFiniteNumber(source[key]) { result[key] = number }
    }
    for key in ["online", "charging", "low_memory", "simulator"] {
        if let value = source[key] as? Bool { result[key] = value }
    }
    return result.isEmpty ? nil : result
}

private func sandSentryProjectElectronContext(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    var result: [String: Any] = [:]
    if let crashedURL = sandSentryScrubURL(source["crashed_url"]) { result["crashed_url"] = crashedURL }
    if let details = source["details"] as? [String: Any] {
        var projected = sandSentryProjectCodes(details, keys: ["reason", "serviceName", "name", "type"])
        if let exitCode = sandSentryFiniteNumber(details["exitCode"]) { projected["exitCode"] = exitCode }
        if !projected.isEmpty { result["details"] = projected }
    }
    return result.isEmpty ? nil : result
}

private func sandSentryProjectReactContext(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any],
          let stack = source["componentStack"] as? String else { return nil }
    let pattern = #"^\s*(in|at)\s+([A-Za-z0-9$_.]{1,128})(?:\s|\(|$)"#
    guard let regex = try? NSRegularExpression(pattern: pattern) else { return nil }
    let lines = stack.split(whereSeparator: { $0.isNewline }).prefix(SAND_SENTRY_MAX_COMPONENT_STACK_LINES).compactMap { raw -> String? in
        let line = String(raw)
        guard let match = regex.firstMatch(in: line, range: NSRange(line.startIndex..., in: line)),
              let verbRange = Range(match.range(at: 1), in: line),
              let nameRange = Range(match.range(at: 2), in: line) else { return nil }
        return "\(line[verbRange]) \(line[nameRange])"
    }
    return lines.isEmpty ? nil : ["componentStack": lines.joined(separator: "\n")]
}

private func sandSentryProjectCultureContext(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    var result = sandSentryProjectCodes(source, keys: ["calendar", "locale"])
    if let timezone = source["timezone"] as? String, TimeZone(identifier: timezone) != nil {
        result["timezone"] = timezone
    }
    if let value = source["is_24_hour_format"] as? Bool { result["is_24_hour_format"] = value }
    return result.isEmpty ? nil : result
}

private func sandSentryScrubContexts(_ value: Any?, tier: SandSentryPrivacyTier) -> [String: Any]? {
    guard let source = value as? [String: Any] else { return nil }
    var result: [String: Any] = [:]
    if let app = sandSentryProjectAppContext(source["app"]) { result["app"] = app }
    if let os = sandSentryProjectOSContext(source["os"]) { result["os"] = os }
    if tier == .fatalMetadata { return result.isEmpty ? nil : result }
    for key in ["runtime", "browser", "chrome", "node"] {
        if let runtime = sandSentryProjectRuntimeContext(source[key]) { result[key] = runtime }
    }
    if let device = sandSentryProjectDeviceContext(source["device"]) { result["device"] = device }
    if let electron = sandSentryProjectElectronContext(source["electron"]) { result["electron"] = electron }
    if let react = sandSentryProjectReactContext(source["react"]) { result["react"] = react }
    if let culture = sandSentryProjectCultureContext(source["culture"]) { result["culture"] = culture }
    return result.isEmpty ? nil : result
}

private func sandSentryScrubThreads(_ value: Any?) -> [String: Any]? {
    guard let source = value as? [String: Any], let values = source["values"] as? [Any] else { return nil }
    let projected: [[String: Any]] = values.prefix(SAND_SENTRY_MAX_THREADS).compactMap { candidate in
        guard let candidate = candidate as? [String: Any] else { return nil }
        var thread: [String: Any] = [:]
        if let id = sandSentryBoundedID(candidate["id"]) { thread["id"] = id }
        for key in ["main", "crashed", "current"] {
            if let value = candidate[key] as? Bool { thread[key] = value }
        }
        if let stack = sandSentryScrubStacktrace(candidate["stacktrace"]) { thread["stacktrace"] = stack }
        return thread
    }
    return projected.isEmpty ? nil : ["values": projected]
}

private func sandSentryProjectEvent(_ event: [String: Any], tier: SandSentryPrivacyTier) -> [String: Any]? {
    if tier == .full { return event }
    if tier == .fatalMetadata && event["level"] as? String != "fatal" { return nil }

    var result: [String: Any] = [:]
    if tier == .fatalMetadata { result["level"] = "fatal" }
    if let eventID = sandSentryBounded(event["event_id"]) { result["event_id"] = eventID }
    for key in ["timestamp", "start_timestamp"] {
        if let value = sandSentryFiniteNumber(event[key]) { result[key] = value }
    }
    if tier == .scrubbed,
       let level = event["level"] as? String,
       SAND_SENTRY_SEVERITY_LEVELS.contains(level) {
        result["level"] = level
    }
    if let platform = sandSentryBounded(event["platform"]) { result["platform"] = platform }
    if let release = sandSentryBounded(event["release"]) { result["release"] = release }
    if let dist = sandSentryBounded(event["dist"], maxLength: 64) { result["dist"] = dist }
    if let environment = sandSentryBounded(event["environment"], maxLength: tier == .scrubbed ? 64 : 128) {
        result["environment"] = environment
    }

    if tier == .scrubbed {
        if let sdk = sandSentryProjectSDK(event["sdk"]) { result["sdk"] = sdk }
        if let request = sandSentryScrubRequest(event["request"]) { result["request"] = request }
        if let user = sandSentryScrubUser(event["user"]) { result["user"] = user }
        if let threads = sandSentryScrubThreads(event["threads"]) { result["threads"] = threads }
    }
    if let exception = sandSentryScrubException(event["exception"], tier: tier) { result["exception"] = exception }
    if let contexts = sandSentryScrubContexts(event["contexts"], tier: tier) { result["contexts"] = contexts }
    if let tags = sandSentryScrubTags(event["tags"], tier: tier) { result["tags"] = tags }
    return result
}

private func sandSentryProjectSession(_ session: [String: Any]) -> [String: Any]? {
    var result: [String: Any] = [:]
    if let value = session["init"] as? Bool { result["init"] = value }
    if let sid = sandSentryBounded(session["sid"]) { result["sid"] = sid }
    for key in ["timestamp", "started"] {
        if let value = session[key] as? String, let safe = sandSentryBounded(value, maxLength: 64) { result[key] = safe }
    }
    if let duration = sandSentryFiniteNumber(session["duration"]) { result["duration"] = duration }
    if let status = session["status"] as? String, SAND_SENTRY_SESSION_STATUSES.contains(status) { result["status"] = status }
    if let errors = sandSentryFiniteNumber(session["errors"]) { result["errors"] = errors }
    if let attrs = session["attrs"] as? [String: Any] {
        var projected: [String: Any] = [:]
        if let release = sandSentryBounded(attrs["release"]) { projected["release"] = release }
        if let environment = sandSentryBounded(attrs["environment"], maxLength: 64) { projected["environment"] = environment }
        if !projected.isEmpty { result["attrs"] = projected }
    }
    return result.isEmpty ? nil : result
}

private func sandSentryProjectSessionAggregates(_ payload: [String: Any]) -> [String: Any]? {
    guard let aggregates = payload["aggregates"] as? [Any] else { return nil }
    var result: [String: Any] = [:]
    if let attrs = payload["attrs"] as? [String: Any] {
        var projected: [String: Any] = [:]
        if let release = sandSentryBounded(attrs["release"]) { projected["release"] = release }
        if let environment = sandSentryBounded(attrs["environment"], maxLength: 64) { projected["environment"] = environment }
        if !projected.isEmpty { result["attrs"] = projected }
    }
    result["aggregates"] = aggregates.prefix(100).compactMap { candidate -> [String: Any]? in
        guard let candidate = candidate as? [String: Any] else { return nil }
        var aggregate: [String: Any] = [:]
        if let started = candidate["started"] as? String, let safe = sandSentryBounded(started, maxLength: 64) {
            aggregate["started"] = safe
        }
        for key in ["exited", "errored", "crashed"] {
            if let value = sandSentryFiniteNumber(candidate[key]) { aggregate[key] = value }
        }
        return aggregate
    }
    return result
}

private func sandSentryProjectClientReport(_ payload: [String: Any]) -> [String: Any]? {
    guard let timestamp = sandSentryFiniteNumber(payload["timestamp"]),
          let events = payload["discarded_events"] as? [Any] else { return nil }
    let projected: [[String: Any]] = events.prefix(100).compactMap { candidate in
        guard let candidate = candidate as? [String: Any],
              let reason = sandSentryBounded(candidate["reason"]),
              let category = sandSentryBounded(candidate["category"]),
              let quantity = sandSentryNonNegativeNumber(candidate["quantity"]) else { return nil }
        return ["reason": reason, "category": category, "quantity": quantity]
    }
    return ["timestamp": timestamp, "discarded_events": projected]
}

func projectSandSentryEnvelope(
    _ envelope: SandSentryEnvelope,
    tier: SandSentryPrivacyTier
) -> SandSentryEnvelope? {
    if tier == .full { return envelope }

    var header: [String: Any] = [:]
    if let eventID = sandSentryBounded(envelope.header["event_id"]) { header["event_id"] = eventID }
    if let sentAt = sandSentryBounded(envelope.header["sent_at"], maxLength: 64) { header["sent_at"] = sentAt }

    var items: [SandSentryItem] = []
    for item in envelope.items {
        let type = item.header["type"] as? String
        switch type {
        case "event":
            guard let event = item.payload as? [String: Any],
                  let projected = sandSentryProjectEvent(event, tier: tier) else { continue }
            items.append(.init(header: ["type": "event"], payload: projected))
        case "session":
            guard tier == .scrubbed,
                  let session = item.payload as? [String: Any],
                  let projected = sandSentryProjectSession(session) else { continue }
            items.append(.init(header: ["type": "session"], payload: projected))
        case "sessions":
            guard tier == .scrubbed,
                  let payload = item.payload as? [String: Any],
                  let projected = sandSentryProjectSessionAggregates(payload) else { continue }
            items.append(.init(header: ["type": "sessions"], payload: projected))
        case "client_report":
            guard tier == .scrubbed,
                  let payload = item.payload as? [String: Any],
                  let projected = sandSentryProjectClientReport(payload) else { continue }
            items.append(.init(header: ["type": "client_report"], payload: projected))
        default:
            continue
        }
    }
    return items.isEmpty ? nil : SandSentryEnvelope(header: header, items: items)
}

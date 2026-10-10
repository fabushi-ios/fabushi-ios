import Foundation

let FEATURE_FLAG_OVERRIDES_FILENAME = "sand-feature-flag-overrides.json"
let FEATURE_FLAG_OVERRIDE_TTL_MS: Int64 = 24 * 60 * 60 * 1_000

func isFlagName(_ name: String) -> Bool {
    BUNDLED_FEATURE_FLAGS[name] != nil
}

private struct FeatureFlagOverrideEntry: Codable, Equatable, Sendable {
    let value: Bool
    let expiresAtMs: Int64
}

private struct FeatureFlagOverrideFile: Codable {
    let overrides: [String: FeatureFlagOverrideEntry]
}

final class SandFeatureFlagOverrideStore: @unchecked Sendable {
    private let lock = NSLock()
    private let getCacheDir: () -> String
    private let now: () -> Int64
    private var overrides: [String: FeatureFlagOverrideEntry] = [:]

    init(
        getCacheDir: @escaping () -> String,
        now: @escaping () -> Int64 = { Int64(Date().timeIntervalSince1970 * 1_000) }
    ) {
        self.getCacheDir = getCacheDir
        self.now = now
    }

    func getOverridesPath() -> String {
        URL(fileURLWithPath: getCacheDir()).appendingPathComponent(FEATURE_FLAG_OVERRIDES_FILENAME).path
    }

    func hydrateFromDisk() {
        do {
            let path = getOverridesPath()
            guard FileManager.default.fileExists(atPath: path) else { return }
            let data = try Data(contentsOf: URL(fileURLWithPath: path))
            let parsed = try JSONDecoder().decode(FeatureFlagOverrideFile.self, from: data)
            let current = now()
            let valid = parsed.overrides.filter { isFlagName($0.key) && $0.value.expiresAtMs > current }
            lock.lock(); overrides = valid; lock.unlock()
        } catch {
            reportExperimentsDiagnostic(.init(kind: "overrides_load_failed", metadata: ["errorClass": errorLogTag(error)]))
        }
    }

    func persist() {
        do {
            let snapshot: [String: FeatureFlagOverrideEntry]
            lock.lock(); snapshot = overrides; lock.unlock()
            let data = try JSONEncoder().encode(FeatureFlagOverrideFile(overrides: snapshot))
            try writeFileAtomic(targetPath: getOverridesPath(), data: data)
        } catch {
            reportExperimentsDiagnostic(.init(kind: "overrides_persist_failed", metadata: ["errorClass": errorLogTag(error)]))
        }
    }

    func read(_ name: String) -> Bool? {
        lock.lock(); defer { lock.unlock() }
        guard let entry = overrides[name] else { return nil }
        if entry.expiresAtMs <= now() {
            overrides.removeValue(forKey: name)
            return nil
        }
        return entry.value
    }

    func activeOverrides() -> [String: Bool] {
        let current = now()
        lock.lock(); defer { lock.unlock() }
        return overrides.compactMapValues { $0.expiresAtMs > current ? $0.value : nil }
    }

    var size: Int {
        lock.lock(); defer { lock.unlock() }
        return overrides.count
    }

    @discardableResult
    func set(_ name: String, value: Bool) -> Bool {
        guard isFlagName(name) else { return false }
        lock.lock()
        overrides[name] = .init(value: value, expiresAtMs: now() + FEATURE_FLAG_OVERRIDE_TTL_MS)
        lock.unlock()
        return true
    }

    @discardableResult
    func clear(_ name: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return overrides.removeValue(forKey: name) != nil
    }

    func clearAll() {
        lock.lock(); overrides.removeAll(); lock.unlock()
    }

    func setAllToBundledDefaults() {
        let expiry = now() + FEATURE_FLAG_OVERRIDE_TTL_MS
        lock.lock()
        overrides = BUNDLED_FEATURE_FLAGS.mapValues { .init(value: $0.defaultValue, expiresAtMs: expiry) }
        lock.unlock()
    }

    func replaceAll(_ replacements: [String: Bool]) {
        let expiry = now() + FEATURE_FLAG_OVERRIDE_TTL_MS
        let valid = replacements.filter { isFlagName($0.key) }
            .mapValues { FeatureFlagOverrideEntry(value: $0, expiresAtMs: expiry) }
        lock.lock(); overrides = valid; lock.unlock()
    }
}

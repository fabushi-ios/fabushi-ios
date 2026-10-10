import Foundation

@MainActor
final class IOSDeepLinkController {
    static let pendingLimit = 16
    static let dedupeWindow: TimeInterval = 2

    private let dispatch: @MainActor (ParsedFabushiDeepLink) -> Void
    private let requestActivation: @MainActor () -> Void
    private let log: @MainActor (String) -> Void
    private let now: () -> Date
    private var rendererReady = false
    private var pending: [ParsedFabushiDeepLink] = []
    private var recentlyAccepted: [String: Date] = [:]

    init(
        dispatch: @escaping @MainActor (ParsedFabushiDeepLink) -> Void,
        requestActivation: @escaping @MainActor () -> Void = {},
        log: @escaping @MainActor (String) -> Void = { _ in },
        now: @escaping () -> Date = Date.init
    ) {
        self.dispatch = dispatch
        self.requestActivation = requestActivation
        self.log = log
        self.now = now
    }

    @discardableResult
    func handleCandidate(_ raw: String, origin: String) -> Bool {
        guard let parsed = FabushiDeepLinkParser.parse(raw) else {
            log("deep-link: ignored invalid candidate from \(origin)")
            return false
        }

        let canonical = parsed.canonicalURL.absoluteString
        pruneRecent()
        guard !pending.contains(where: { $0.canonicalURL.absoluteString == canonical }),
              recentlyAccepted[canonical] == nil
        else {
            log("deep-link: deduped \(canonical) from \(origin)")
            return false
        }
        guard rendererReady || pending.count < Self.pendingLimit else {
            log("deep-link: dropped \(canonical) from \(origin) (pending queue full)")
            return false
        }

        recentlyAccepted[canonical] = now()
        requestActivation()
        if rendererReady {
            dispatch(parsed)
        } else {
            pending.append(parsed)
        }
        return true
    }

    func markReady() {
        rendererReady = true
        guard !pending.isEmpty else { return }
        let queued = pending
        pending.removeAll(keepingCapacity: true)
        queued.forEach(dispatch)
    }

    func markNotReady() {
        rendererReady = false
        let pendingKeys = Set(pending.map { $0.canonicalURL.absoluteString })
        recentlyAccepted = recentlyAccepted.filter { pendingKeys.contains($0.key) }
    }

    var hasPendingActivation: Bool { !pending.isEmpty }

    private func pruneRecent() {
        let current = now()
        recentlyAccepted = recentlyAccepted.filter {
            current.timeIntervalSince($0.value) <= Self.dedupeWindow
        }
    }
}

import Foundation

final class CursorMarketplaceLogoRegistry: @unchecked Sendable {
    static let shared = CursorMarketplaceLogoRegistry()

    private let lock = NSLock()
    private var knownLogoUrls: Set<String> = []

    func rememberPluginLogoUrl(_ url: String) {
        lock.lock()
        knownLogoUrls.insert(url)
        lock.unlock()
    }

    func isKnownPluginLogoUrl(_ url: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return knownLogoUrls.contains(url)
    }

    func resetForTesting() {
        lock.lock()
        knownLogoUrls.removeAll()
        lock.unlock()
    }
}

func rememberPluginLogoUrl(_ url: String) {
    CursorMarketplaceLogoRegistry.shared.rememberPluginLogoUrl(url)
}

func isKnownPluginLogoUrl(_ url: String) -> Bool {
    CursorMarketplaceLogoRegistry.shared.isKnownPluginLogoUrl(url)
}

import Foundation

let LOGO_MAX_BYTES = 512 * 1024
let LOGO_FETCH_CONCURRENCY = 6

private actor MarketplaceLogoFetchGate {
    private var active = 0
    private var waiters: [CheckedContinuation<Void, Never>] = []

    func acquire() async {
        if active < LOGO_FETCH_CONCURRENCY {
            active += 1
            return
        }
        await withCheckedContinuation { continuation in
            waiters.append(continuation)
        }
    }

    func release() {
        if waiters.isEmpty {
            active = max(0, active - 1)
        } else {
            waiters.removeFirst().resume()
        }
    }
}

private actor MarketplaceLogoCache {
    private enum Entry {
        case value(String)
        case missing
    }
    private var values: [String: Entry] = [:]

    func get(_ url: String) -> (found: Bool, value: String?) {
        guard let entry = values[url] else { return (false, nil) }
        switch entry {
        case .value(let value): return (true, value)
        case .missing: return (true, nil)
        }
    }

    func put(_ url: String, _ value: String?) {
        values[url] = value.map(Entry.value) ?? .missing
    }

    func clear() {
        values.removeAll()
    }
}

private let marketplaceLogoFetchGate = MarketplaceLogoFetchGate()
private let marketplaceLogoCache = MarketplaceLogoCache()

typealias MarketplaceLogoFetch = @Sendable (URL, TimeInterval) async throws -> (Data, HTTPURLResponse)

func withLogoFetchSlot<T: Sendable>(
    _ run: @Sendable () async throws -> T
) async rethrows -> T {
    await marketplaceLogoFetchGate.acquire()
    do {
        let value = try await run()
        await marketplaceLogoFetchGate.release()
        return value
    } catch {
        await marketplaceLogoFetchGate.release()
        throw error
    }
}

func resolvePluginLogo(
    _ url: String,
    isKnown: @Sendable (String) -> Bool = { isKnownPluginLogoUrl($0) },
    fetch: @escaping MarketplaceLogoFetch = { url, timeoutSeconds in
        var request = URLRequest(url: url)
        request.timeoutInterval = timeoutSeconds
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        return (data, http)
    }
) async -> String? {
    let cached = await marketplaceLogoCache.get(url)
    if cached.found { return cached.value }

    guard isKnown(url),
          let parsed = URL(string: url),
          parsed.scheme?.lowercased() == "https" else {
        await marketplaceLogoCache.put(url, nil)
        return nil
    }

    let value: String?
    do {
        value = try await withLogoFetchSlot {
            let (data, response) = try await fetch(
                parsed,
                TimeInterval(CURSOR_MARKETPLACE_REQUEST_TIMEOUT_MS) / 1_000
            )
            return responseToImageDataUrl(
                response,
                body: data,
                maxBytes: LOGO_MAX_BYTES
            )
        }
    } catch {
        value = nil
    }
    await marketplaceLogoCache.put(url, value)
    return value
}

func clearPluginLogoCacheForTesting() async {
    await marketplaceLogoCache.clear()
}

import XCTest
@testable import Fabushi

private actor InMemoryPersistenceFiles: ClientPersistenceFiles {
    var storage: [String: String] = [:]
    var directories = Set<String>()

    nonisolated func joinPath(_ dir: String, _ name: String) -> String { "\(dir)/\(name)" }
    func ensureDir(_ dir: String) async throws { directories.insert(dir) }
    func listFiles(_ dir: String) async throws -> [String] {
        let prefix = "\(dir)/"
        return storage.keys.filter { $0.hasPrefix(prefix) }.map { String($0.dropFirst(prefix.count)) }
    }
    func readTextFile(_ path: String) async throws -> String {
        guard let value = storage[path] else { throw CocoaError(.fileReadNoSuchFile) }
        return value
    }
    func writeTextFile(_ path: String, data: String) async throws { storage[path] = data }
    func rename(_ from: String, to: String) async throws {
        guard let value = storage.removeValue(forKey: from) else { throw CocoaError(.fileReadNoSuchFile) }
        storage[to] = value
    }
    func removeFile(_ path: String) async throws { storage.removeValue(forKey: path) }
    func fileSize(_ path: String) async throws -> Int? { storage[path]?.lengthOfBytes(using: .utf8) }
}

final class SharedChannelsPersistenceParityTests: XCTestCase {
    func testBoundedErrorBrandingAndAttachmentSchemes() {
        XCTAssertEqual(brandedErrno("ENOENT"), "ENOENT")
        XCTAssertEqual(brandedErrno("WHATEVER"), "E_OTHER")
        XCTAssertEqual(brandedId("abc:123"), "abc:123")
        XCTAssertNil(brandedId("contains space"))
        XCTAssertTrue(isValidAttachmentUrl("https://example.com/a.png"))
        XCTAssertTrue(isValidAttachmentUrl("file:///tmp/a.png"))
        XCTAssertFalse(isValidAttachmentUrl("http://example.com/a.png"))
    }

    func testPersistenceFilenameRoundTripAndNamespaceBoundary() {
        let key = "sand.sidebar.state"
        let encoded = encodeClientPersistenceFileName(key)
        XCTAssertEqual(decodeClientPersistenceFileName(encoded), key)
        XCTAssertEqual(clientPersistenceFileNameFor(key), encoded)
        XCTAssertNil(clientPersistenceFileNameFor("other.sidebar.state"))
        XCTAssertNil(decodeClientPersistenceFileName("invalid!.blob"))
    }

    func testPersistenceStoreCapsAtomicWriteAndMigrationMarker() async throws {
        let files = InMemoryPersistenceFiles()
        let store = SandClientPersistenceStore(dir: "/store", files: files, maxValueBytes: 8, maxTotalBytes: 12)
        try await store.write("sand.a", value: "1234")
        let storedA = try await store.read("sand.a")
        XCTAssertEqual(storedA, "1234")

        do {
            try await store.write("sand.big", value: "123456789")
            XCTFail("per-key cap must fail")
        } catch is ClientPersistenceCapError {}

        let migrated = await store.migrateFromLocalStorage([
            (key: "sand.b", value: "5678"),
            (key: "other.skip", value: "ignored"),
        ])
        XCTAssertTrue(migrated)
        let keys = await store.listKeys(prefix: "sand.")
        XCTAssertEqual(keys, ["sand.a", "sand.b"])
        let hasMarker = await store.hasCompletedOneShotMigration()
        XCTAssertTrue(hasMarker)
    }

    func testChannelOutboundAndWakeFormatting() {
        XCTAssertEqual(
            buildChannelOutboundMessage(.text(content: "hello", images: [])),
            .text("hello")
        )
        XCTAssertEqual(
            buildChannelOutboundMessage(.attachment(url: "", alt: "caption")),
            .text("caption")
        )
        let wake = buildChannelInboundWakePrompt([
            .init(address: .init(platform: "slack", chat: "C1"), sender: "A", text: "Hi")
        ])
        XCTAssertTrue(wake.contains(CHANNEL_INBOUND_WAKE_CUE))
        XCTAssertTrue(wake.contains("slack:C1"))
        XCTAssertTrue(humanizeChannelDeliveryFailure("bad", rawMessage: "not a valid channel address").contains("isn't a valid"))
    }

    func testWebauthnOriginClassificationAndWireConstants() {
        XCTAssertEqual(sandWebAuthnOriginClass("https://cursor.com"), .cursorCom)
        XCTAssertEqual(sandWebAuthnOriginClass("https://foo.cursor.com"), .subdomain)
        XCTAssertEqual(sandWebAuthnOriginClass("https://example.com"), .external)
        XCTAssertEqual(GATEWAY_WEBAUTHN_REQUESTS_PATH, "/webauthn/requests")
        XCTAssertEqual(SAND_WEBAUTHN_HEARTBEAT_INTERVAL_MS, 10_000)
    }

    func testPrReviewDefaultsAndPersistenceChannels() {
        XCTAssertNil(NO_SAND_PR_REVIEW_PREFERENCES.user)
        XCTAssertNil(NO_SAND_PR_REVIEW_PREFERENCES.team)
        XCTAssertEqual(ClientPersistenceChannels.read, "sand:client-persistence-read")
        XCTAssertEqual(ClientPersistenceChannels.migrate, "sand:client-persistence-migrate")
    }

    func testTeamRulesResolverPreservesLastCompleteSnapshot() async {
        actor State {
            var step = 0
            func next() -> Int { defer { step += 1 }; return step }
        }
        let state = State()
        let resolver = SandTeamRulesResolver<String>(
            load: {
                switch await state.next() {
                case 0: return .rules(["r1"])
                default: return .incomplete
                }
            },
            reportLoadFailure: { _ in XCTFail("load should not fail") }
        )
        let initial = await resolver.resolveRules()
        XCTAssertEqual(initial, ["r1"])
        await resolver.refresh()
        let afterIncompleteRefresh = await resolver.resolveRules()
        XCTAssertEqual(afterIncompleteRefresh, ["r1"])
    }
}

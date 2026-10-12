import XCTest
@testable import Fabushi

@MainActor
private final class IOSMachineIDSecretStoreStub: IOSMachineIDSecretStore {
    var reads: [String?]
    private(set) var writes: [(String, String)] = []
    private(set) var waitCount = 0

    init(reads: [String?]) {
        self.reads = reads
    }

    func readSecret(_ key: String) async throws -> String? {
        guard !reads.isEmpty else { return nil }
        return reads.removeFirst()
    }

    func writeSecret(_ key: String, value: String) async throws {
        writes.append((key, value))
    }

    func waitForEncryptedStorage() async throws {
        waitCount += 1
    }
}

final class IOSMainPolicyParityTests: XCTestCase {
    func testDownloadPathPolicyPreservesSafeBasenamesAndAbsoluteOverride() {
        XCTAssertEqual(
            resolveDefaultDownloadDirectory(
                configuredDirectory: " relative/downloads ",
                osDownloadsDirectory: "/var/mobile/Downloads"
            ),
            "/var/mobile/Downloads"
        )
        XCTAssertEqual(
            resolveDefaultDownloadDirectory(
                configuredDirectory: " /private/fabushi ",
                osDownloadsDirectory: "/var/mobile/Downloads"
            ),
            "/private/fabushi"
        )
        XCTAssertEqual(
            resolveDefaultDownloadPath(
                configuredDirectory: nil,
                osDownloadsDirectory: "/var/mobile/Downloads",
                fileName: "../payload.zip"
            ),
            "/var/mobile/Downloads/payload.zip"
        )
        XCTAssertEqual(
            resolveSuggestedDownloadName(
                sourcePath: "/tmp/photo.png",
                suggestedName: "../avatar.PNG"
            ),
            "avatar.PNG"
        )
        XCTAssertEqual(
            resolveSuggestedDownloadName(
                sourcePath: "/tmp/photo.png",
                suggestedName: "avatar.jpg"
            ),
            "photo.png"
        )
        XCTAssertEqual(
            resolveSuggestedDownloadName(
                sourcePath: "/tmp/archive.bin",
                suggestedName: "archive"
            ),
            "archive"
        )
    }

    @MainActor
    func testMachineIDResolverDoubleChecksAfterEncryptedStorageSettles() async throws {
        let store = IOSMachineIDSecretStoreStub(reads: [nil, "settled-id"])
        let resolver = IOSMachineIDResolver(
            secrets: store,
            createID: { XCTFail("must not generate after storage settles"); return "unexpected" }
        )

        let settledID = try await resolver.getOrCreate()
        XCTAssertEqual(settledID, "settled-id")
        XCTAssertEqual(store.waitCount, 1)
        XCTAssertTrue(store.writes.isEmpty)
    }

    @MainActor
    func testMachineIDResolverPersistsGeneratedIDWhenStillMissing() async throws {
        let store = IOSMachineIDSecretStoreStub(reads: [nil, nil])
        let resolver = IOSMachineIDResolver(secrets: store, createID: { "generated-id" })

        let generatedID = try await resolver.getOrCreate()
        XCTAssertEqual(generatedID, "generated-id")
        XCTAssertEqual(store.waitCount, 1)
        XCTAssertEqual(store.writes.count, 1)
        XCTAssertEqual(store.writes.first?.0, iosMachineIDSecretKey)
        XCTAssertEqual(store.writes.first?.1, "generated-id")
    }
}

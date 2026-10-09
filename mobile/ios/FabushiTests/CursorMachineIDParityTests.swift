import XCTest
@testable import Fabushi

@MainActor
final class CursorMachineIDParityTests: XCTestCase {
    private final class Store: IOSMachineIDSecretStore {
        var value: String?
        var readCount = 0
        var waitCount = 0
        var writes: [(String, String)] = []
        var valueAfterWait: String?

        func readSecret(_ key: String) async throws -> String? {
            XCTAssertEqual(key, iosMachineIDSecretKey)
            readCount += 1
            return value
        }

        func writeSecret(_ key: String, value: String) async throws {
            XCTAssertEqual(key, iosMachineIDSecretKey)
            writes.append((key, value))
            self.value = value
        }

        func waitForEncryptedStorage() async throws {
            waitCount += 1
            if let valueAfterWait {
                value = valueAfterWait
            }
        }
    }

    func testExistingMachineIDReturnsWithoutWaitingOrWriting() async throws {
        let store = Store()
        store.value = "existing"
        let resolver = IOSMachineIDResolver(secrets: store, createID: { "new" })

        let resolved = try await resolver.getOrCreate()
        XCTAssertEqual(resolved, "existing")
        XCTAssertEqual(store.readCount, 1)
        XCTAssertEqual(store.waitCount, 0)
        XCTAssertTrue(store.writes.isEmpty)
    }

    func testStorageSettlementGetsSecondReadBeforeCreatingID() async throws {
        let store = Store()
        store.valueAfterWait = "settled"
        let resolver = IOSMachineIDResolver(secrets: store, createID: { "new" })

        let resolved = try await resolver.getOrCreate()
        XCTAssertEqual(resolved, "settled")
        XCTAssertEqual(store.readCount, 2)
        XCTAssertEqual(store.waitCount, 1)
        XCTAssertTrue(store.writes.isEmpty)
    }

    func testMissingMachineIDCreatesAndPersistsExactlyOnce() async throws {
        let store = Store()
        var createCount = 0
        let resolver = IOSMachineIDResolver(secrets: store, createID: {
            createCount += 1
            return "generated-id"
        })

        let resolved = try await resolver.getOrCreate()
        XCTAssertEqual(resolved, "generated-id")
        XCTAssertEqual(createCount, 1)
        XCTAssertEqual(store.waitCount, 1)
        XCTAssertEqual(store.writes.count, 1)
        XCTAssertEqual(store.writes.first?.0, iosMachineIDSecretKey)
        XCTAssertEqual(store.writes.first?.1, "generated-id")

        let resolvedAgain = try await resolver.getOrCreate()
        XCTAssertEqual(resolvedAgain, "generated-id")
        XCTAssertEqual(createCount, 1)
        XCTAssertEqual(store.writes.count, 1)
    }
}

#if canImport(XCTest)
import XCTest
@testable import Fabushi

final class KeychainSessionStoreTests: XCTestCase {

    func testMultiAccountKeychainPersistenceSwitchCapAndCleanup() throws {
        let store = KeychainSessionStore(
            serviceNamespace: "com.ombhrum.fabushi.tests.\(UUID().uuidString)"
        )
        defer {
            try? store.clearAllSessions()
            try? store.deleteStoragePassphrase()
        }

        // 1. Empty initial state
        XCTAssertNil(try store.activeSession())
        XCTAssertTrue(try store.listSessions().isEmpty)

        // 2. Save 2 accounts & verify active account switching
        let s1 = StoredCloudSession(
            userId: 101,
            username: "alice",
            token: "tok_alice_secret",
            sessionId: "sess_101",
            serverURL: "http://34.46.77.133:8080"
        )
        let s2 = StoredCloudSession(
            userId: 202,
            username: "bob",
            token: "tok_bob_secret",
            sessionId: "sess_202",
            serverURL: "http://34.46.77.133:8080"
        )
        try store.saveSession(s1, makeActive: true)
        try store.saveSession(s2, makeActive: false)

        XCTAssertEqual(try store.activeSession()?.userId, 101)
        XCTAssertEqual(try store.listSessions().count, 2)

        let switched = try store.switchActiveAccount(userId: 202)
        XCTAssertEqual(switched?.username, "bob")
        XCTAssertEqual(try store.activeSession()?.userId, 202)

        // 3. Enforce maxAccounts == 6 cap
        for extra in 1...6 {
            let uid = UInt64(1000 + extra)
            try store.saveSession(
                StoredCloudSession(
                    userId: uid,
                    username: "user_\(uid)",
                    token: "tok_\(uid)",
                    sessionId: "sess_\(uid)",
                    serverURL: "http://34.46.77.133:8080"
                ),
                makeActive: true
            )
        }
        let capped = try store.listSessions()
        XCTAssertEqual(capped.count, KeychainSessionStore.maxAccounts)
        XCTAssertEqual(try store.activeSession()?.userId, 1006)

        // 4. Remove active account -> falls back cleanly
        try store.removeSession(userId: 1006)
        XCTAssertEqual(try store.listSessions().count, 5)
        XCTAssertNotNil(try store.activeSession())

        // 5. 32-byte storage passphrase generation & idempotent reload
        let p1 = try store.loadOrCreateStoragePassphrase()
        let p2 = try store.loadOrCreateStoragePassphrase()
        XCTAssertEqual(p1, p2)
        XCTAssertEqual(Data(base64Encoded: p1)?.count, 32)
    }
}

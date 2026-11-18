#if canImport(XCTest)
import SwiftUI
import XCTest
@testable import Fabushi

final class HostEventStreamTests: XCTestCase {

    func testRealtimePushEventStreamAndScenePhaseLifecycle() async throws {
        let sandbox = FileManager.default.temporaryDirectory
            .appendingPathComponent("HostEventStream_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }

        let client = HostBridgeClient(
            keychain: KeychainSessionStore(serviceNamespace: "test.events.\(UUID().uuidString)")
        )
        try await client.initialize(appDataDirectory: sandbox, isTestMode: true)

        let stream = client.eventStream
        let received = EventCollector()
        let reader = Task {
            for await ev in stream {
                received.append(ev)
                if received.count >= 4 {
                    break
                }
            }
        }

        // 1. Send message -> triggers `newMessage` push event
        struct SendParams: Codable, Sendable {
            let chat_id: Int64
            let sender_id: UInt64
            let text: String
        }
        struct SendResp: Decodable, Sendable { let id: Int64 }
        let sent: SendResp = try await client.call(
            "messages.send",
            params: SendParams(chat_id: 501, sender_id: 7, text: "Realtime push hello")
        )
        XCTAssertGreaterThan(sent.id, 0)

        // 2. Trigger <60ms 3-frame VAD barge-in -> triggers `vadBargeIn` push event
        struct TalkStartParams: Codable, Sendable {
            let peer_id: String
            let tts_playing: Bool
        }
        struct TalkStartResp: Decodable, Sendable { let active: Bool }
        let _: TalkStartResp = try await client.call(
            "talk.start",
            params: TalkStartParams(peer_id: "hermes", tts_playing: true)
        )

        struct PcmParams: Codable, Sendable { let pcm_i16: [Int16] }
        struct PcmResp: Decodable, Sendable { let barge_in: Bool }
        let loudFrame = [Int16](repeating: 18_000, count: 320)
        _ = try await client.call("talk.feed_pcm", params: PcmParams(pcm_i16: loudFrame)) as PcmResp
        _ = try await client.call("talk.feed_pcm", params: PcmParams(pcm_i16: loudFrame)) as PcmResp
        let third: PcmResp = try await client.call("talk.feed_pcm", params: PcmParams(pcm_i16: loudFrame))
        XCTAssertTrue(third.barge_in)

        // 3. SceneLifecycleCoordinator `.background` & `.active` -> emits `connectionStateChanged`
        let coordinator = await MainActor.run { SceneLifecycleCoordinator(hostClient: client) }
        await coordinator.handleScenePhaseChange(.background)
        let bgSuspended = await MainActor.run { coordinator.isBackgroundSuspended }
        XCTAssertTrue(bgSuspended)

        await coordinator.handleScenePhaseChange(.active)
        let fgSuspended = await MainActor.run { coordinator.isBackgroundSuspended }
        XCTAssertFalse(fgSuspended)

        _ = await reader.result
        let snapshot = received.all()
        XCTAssertTrue(snapshot.contains {
            if case .newMessage(_, let cid, _, let txt, _) = $0 {
                return cid == 501 && txt == "Realtime push hello"
            }
            return false
        })
        XCTAssertTrue(snapshot.contains {
            if case .vadBargeIn(_, let cnt) = $0 {
                return cnt >= 1
            }
            return false
        })
        XCTAssertTrue(snapshot.contains {
            if case .connectionStateChanged(let st) = $0 {
                return st == "suspended_background"
            }
            return false
        })

        await client.shutdown()
    }
}

private final class EventCollector: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [HostRealtimeEvent] = []

    var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return items.count
    }

    func append(_ ev: HostRealtimeEvent) {
        lock.lock()
        items.append(ev)
        lock.unlock()
    }

    func all() -> [HostRealtimeEvent] {
        lock.lock()
        defer { lock.unlock() }
        return items
    }
}

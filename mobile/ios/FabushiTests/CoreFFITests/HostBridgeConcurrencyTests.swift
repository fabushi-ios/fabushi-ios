#if canImport(XCTest)
import XCTest
@testable import Fabushi

final class HostBridgeConcurrencyTests: XCTestCase {

    func test120ConcurrentMultiDomainDispatchesWithoutMainActorStall() async throws {
        let sandbox = FileManager.default.temporaryDirectory
            .appendingPathComponent("HostBridgeConcurrency_\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }

        let client = HostBridgeClient(
            keychain: KeychainSessionStore(serviceNamespace: "test.concurrency.\(UUID().uuidString)")
        )
        try await client.initialize(appDataDirectory: sandbox, isTestMode: true)

        struct WaveformEncodeParams: Codable, Sendable { let samples: [Float] }
        struct WaveformEncodeResp: Decodable, Sendable {
            let waveform_b64: String
            let byte_count: Int
        }
        struct HermesCmdListResp: Decodable, Sendable { let count: Int }
        struct MagpiePresetsResp: Decodable, Sendable { let preset_count: Int }
        struct DharmaWasmResp: Decodable, Sendable {
            let wasm_bytes: Int
            let magic_valid: Bool
        }
        struct SendParams: Codable, Sendable {
            let chat_id: Int64
            let sender_id: UInt64
            let client_nonce: UInt64
            let text: String
        }
        struct SendResp: Decodable, Sendable {
            let id: Int64
            let duplicate: Bool?
        }

        // Verify @MainActor responsiveness while 120 parallel FFI calls execute
        let mainActorLatencyMs = LockedBox<Double>(0)
        let probeTask = Task { @MainActor in
            let t0 = ContinuousClock.now
            try? await Task.sleep(for: .milliseconds(5))
            let elapsed = t0.duration(to: ContinuousClock.now)
            let ms = Double(elapsed.components.seconds) * 1000.0
                + Double(elapsed.components.attoseconds) / 1e15
            mainActorLatencyMs.set(ms)
        }

        let dedupResults = try await withThrowingTaskGroup(of: SendResp?.self) { group -> [SendResp] in
            for idx in 0..<120 {
                group.addTask {
                    switch idx % 6 {
                    case 0:
                        let pong = try await client.ping(nonce: UInt64(idx))
                        XCTAssertEqual(pong.nonce, UInt64(idx))
                        return nil
                    case 1:
                        let wf: WaveformEncodeResp = try await client.call(
                            "media.encode_waveform",
                            params: WaveformEncodeParams(samples: [0.1, 0.6, 0.9, 0.4])
                        )
                        XCTAssertGreaterThan(wf.byte_count, 0)
                        return nil
                    case 2:
                        let cmds: HermesCmdListResp = try await client.call("hermes.commands_list")
                        XCTAssertGreaterThanOrEqual(cmds.count, 50)
                        return nil
                    case 3:
                        let mp: MagpiePresetsResp = try await client.call("magpie.presets_list")
                        XCTAssertGreaterThanOrEqual(mp.preset_count, 40)
                        return nil
                    case 4:
                        let wasm: DharmaWasmResp = try await client.call("fabushi_miniapp.bundle_wasm_base64")
                        XCTAssertTrue(wasm.magic_valid)
                        XCTAssertGreaterThan(wasm.wasm_bytes, 1024)
                        return nil
                    default:
                        // Shared client_nonce 888_888 across 20 concurrent tasks -> atomic deduplication!
                        let sent: SendResp = try await client.call(
                            "messages.send",
                            params: SendParams(
                                chat_id: 7070,
                                sender_id: 42,
                                client_nonce: 888_888,
                                text: "Burst dedup \(idx)"
                            )
                        )
                        return sent
                    }
                }
            }

            var collected: [SendResp] = []
            for try await item in group {
                if let item {
                    collected.append(item)
                }
            }
            return collected
        }

        await probeTask.value
        XCTAssertEqual(dedupResults.count, 20)
        let uniqueIds = Set(dedupResults.map(\.id))
        XCTAssertEqual(uniqueIds.count, 1, "All 20 concurrent sends with same client_nonce must resolve to 1 message ID")
        let leaderCount = dedupResults.filter { $0.duplicate != true }.count
        XCTAssertEqual(leaderCount, 1)
        XCTAssertLessThan(mainActorLatencyMs.get(), 250.0)

        await client.shutdown()
    }
}

private final class LockedBox<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: T
    init(_ value: T) { self.value = value }
    func set(_ newValue: T) {
        lock.lock()
        value = newValue
        lock.unlock()
    }
    func get() -> T {
        lock.lock()
        defer { lock.unlock() }
        return value
    }
}

import AVFoundation
import CallKit
import Foundation
@preconcurrency import PushKit

internal struct HumanCallPushDescriptor: Equatable, Sendable {
    let callId: String
    let generation: Int
    let displayName: String
    let hasVideo: Bool

    static func parse(_ payload: [AnyHashable: Any]) -> HumanCallPushDescriptor? {
        guard
            let callId = payload["callId"] as? String,
            !callId.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return nil
        }
        let generation = (payload["generation"] as? NSNumber)?.intValue ?? 0
        guard generation >= 0 else { return nil }
        let displayName = (payload["displayName"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return HumanCallPushDescriptor(
            callId: callId,
            generation: generation,
            displayName: displayName?.isEmpty == false ? displayName! : "Fabushi 通话",
            hasVideo: (payload["hasVideo"] as? Bool) ?? false
        )
    }
}

internal enum HumanCallSystemAction: Equatable, Sendable {
    case incoming(HumanCallPushDescriptor)
    case answer(HumanCallPushDescriptor)
    case end(HumanCallPushDescriptor)
}

internal enum HumanCallSystemCoordinatorError: LocalizedError {
    case runtimeUnavailable

    var errorDescription: String? {
        "Human Call runtime is unavailable."
    }
}

private struct HumanCallSystemUncheckedTransfer<Value>: @unchecked Sendable {
    let value: Value
}

/// Native iOS owner for system-call presentation and VoIP wake delivery.
///
/// Host remains the source of truth for call state. This adapter only projects
/// incoming pushes to CallKit and forwards user actions back to the canonical
/// Coordinator/Host call-session transition contract.
@MainActor
internal final class HumanCallSystemCoordinator: NSObject,
    @preconcurrency PKPushRegistryDelegate,
    @preconcurrency CXProviderDelegate
{
    typealias ActionHandler = @MainActor (HumanCallSystemAction) async throws -> Void

    static let shared = HumanCallSystemCoordinator()

    static let voIPTokenDefaultsKey = "fabushi.human-call.voip-token"
    static let voIPTokenDidChange = Notification.Name("fabushi.human-call.voip-token-did-change")

    private let provider: CXProvider
    private var pushRegistry: PKPushRegistry?
    private var actionHandler: ActionHandler?
    private var callsByUUID: [UUID: HumanCallPushDescriptor] = [:]
    private var uuidsByCallID: [String: UUID] = [:]

    override private init() {
        let configuration = CXProviderConfiguration(localizedName: "Fabushi")
        configuration.supportsVideo = true
        configuration.maximumCallsPerCallGroup = 1
        configuration.maximumCallGroups = 1
        configuration.supportedHandleTypes = [.generic]
        configuration.includesCallsInRecents = true
        provider = CXProvider(configuration: configuration)
        super.init()
        provider.setDelegate(self, queue: .main)
    }

    func start() {
        guard pushRegistry == nil else { return }
        let registry = PKPushRegistry(queue: .main)
        registry.delegate = self
        registry.desiredPushTypes = [.voIP]
        pushRegistry = registry
    }

    func bind(actionHandler: @escaping ActionHandler) {
        self.actionHandler = actionHandler
    }

    func unbind() {
        actionHandler = nil
    }

    nonisolated static func hexadecimalToken(_ data: Data) -> String {
        data.map { String(format: "%02x", $0) }.joined()
    }

    nonisolated func pushRegistry(
        _ registry: PKPushRegistry,
        didUpdate pushCredentials: PKPushCredentials,
        for type: PKPushType
    ) {
        guard type == .voIP else { return }
        let token = Self.hexadecimalToken(pushCredentials.token)
        Task { @MainActor in
            UserDefaults.standard.set(token, forKey: Self.voIPTokenDefaultsKey)
            NotificationCenter.default.post(name: Self.voIPTokenDidChange, object: token)
        }
    }

    nonisolated func pushRegistry(
        _ registry: PKPushRegistry,
        didInvalidatePushTokenFor type: PKPushType
    ) {
        guard type == .voIP else { return }
        Task { @MainActor in
            UserDefaults.standard.removeObject(forKey: Self.voIPTokenDefaultsKey)
            NotificationCenter.default.post(name: Self.voIPTokenDidChange, object: nil)
        }
    }

    nonisolated func pushRegistry(
        _ registry: PKPushRegistry,
        didReceiveIncomingPushWith payload: PKPushPayload,
        for type: PKPushType,
        completion: @escaping () -> Void
    ) {
        guard type == .voIP, let descriptor = HumanCallPushDescriptor.parse(payload.dictionaryPayload) else {
            completion()
            return
        }
        let completionTransfer = HumanCallSystemUncheckedTransfer(value: completion)
        Task { @MainActor [weak self, descriptor, completionTransfer] in
            guard let self else {
                completionTransfer.value()
                return
            }
            self.reportIncoming(descriptor, completion: completionTransfer)
        }
    }

    private func reportIncoming(
        _ descriptor: HumanCallPushDescriptor,
        completion: HumanCallSystemUncheckedTransfer<() -> Void>
    ) {
        let uuid = UUID(uuidString: descriptor.callId)
            ?? uuidsByCallID[descriptor.callId]
            ?? UUID()
        callsByUUID[uuid] = descriptor
        uuidsByCallID[descriptor.callId] = uuid

        let update = CXCallUpdate()
        update.localizedCallerName = descriptor.displayName
        update.remoteHandle = CXHandle(type: .generic, value: descriptor.displayName)
        update.hasVideo = descriptor.hasVideo
        update.supportsHolding = false
        update.supportsGrouping = false
        update.supportsUngrouping = false
        update.supportsDTMF = false

        provider.reportNewIncomingCall(with: uuid, update: update) { [weak self] error in
            let succeeded = error == nil
            Task { @MainActor [weak self, descriptor, completion] in
                defer { completion.value() }
                guard succeeded, let self else { return }
                try? await self.actionHandler?(.incoming(descriptor))
            }
        }
    }

    nonisolated func providerDidReset(_ provider: CXProvider) {
        Task { @MainActor [weak self] in
            self?.callsByUUID.removeAll()
            self?.uuidsByCallID.removeAll()
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
        let transfer = HumanCallSystemUncheckedTransfer(value: action)
        Task { @MainActor [weak self, transfer] in
            guard let self, let descriptor = self.callsByUUID[transfer.value.callUUID] else {
                transfer.value.fail()
                return
            }
            do {
                guard let actionHandler = self.actionHandler else {
                    throw HumanCallSystemCoordinatorError.runtimeUnavailable
                }
                try await actionHandler(.answer(descriptor))
                transfer.value.fulfill()
            } catch {
                transfer.value.fail()
            }
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
        let transfer = HumanCallSystemUncheckedTransfer(value: action)
        Task { @MainActor [weak self, transfer] in
            guard let self, let descriptor = self.callsByUUID[transfer.value.callUUID] else {
                transfer.value.fulfill()
                return
            }
            do {
                if let actionHandler = self.actionHandler {
                    try await actionHandler(.end(descriptor))
                }
                transfer.value.fulfill()
                self.callsByUUID.removeValue(forKey: transfer.value.callUUID)
                self.uuidsByCallID.removeValue(forKey: descriptor.callId)
            } catch {
                transfer.value.fail()
            }
        }
    }

    nonisolated func provider(
        _ provider: CXProvider,
        didActivate audioSession: AVAudioSession
    ) {}

    nonisolated func provider(
        _ provider: CXProvider,
        didDeactivate audioSession: AVAudioSession
    ) {}
}

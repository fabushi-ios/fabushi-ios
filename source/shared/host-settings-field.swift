import Foundation

struct HostSettingsPort<Settings, Value> {
    let isReadable: @MainActor () -> Bool
    let read: @MainActor () async throws -> Settings
    let write: @MainActor (Value) async throws -> Settings?
    let value: @MainActor (Settings) -> Value?
}

struct HostSettingsMirror<Value> {
    let read: @MainActor () -> Value?
    let write: @MainActor (Value) -> Void
    let clear: (@MainActor () -> Void)?

    init(
        read: @escaping @MainActor () -> Value?,
        write: @escaping @MainActor (Value) -> Void,
        clear: (@MainActor () -> Void)? = nil
    ) {
        self.read = read
        self.write = write
        self.clear = clear
    }
}

enum BoxSettingsApplyResult<Value: Equatable>: Equatable {
    case unreachable
    case persisted(Value?)
}

enum BoxSettingsAbsorbResult: String, Equatable {
    case none
    case backfilled
    case repainted
    case cleared
}

/// Swift adaptation of Grok's generic BoxSettingsField.
///
/// Key-path updates are represented by typed write and value closures so the
/// iOS boundary remains type-safe without dictionary-shaped partial settings.
@MainActor
final class BoxSettingsField<Settings, Value: Equatable> {
    private let writes = WriteEpoch()
    private var answeredThisSession = false
    private var sentToBox: Value?

    let port: HostSettingsPort<Settings, Value>
    let mirror: HostSettingsMirror<Value>?

    init(
        port: HostSettingsPort<Settings, Value>,
        mirror: HostSettingsMirror<Value>? = nil
    ) {
        self.port = port
        self.mirror = mirror
    }

    func reconcile() async -> Value? {
        guard let mirror else {
            guard let settings = await readOrNil() else { return nil }
            return boxValue(settings)
        }

        let readAt = writes.snapshot()
        guard let settings = await readOrNil(), !writes.isStale(readAt) else {
            return mirror.read()
        }
        return resolve(settings)
    }

    func apply(_ value: Value) async -> BoxSettingsApplyResult<Value> {
        mirror?.write(value)
        answeredThisSession = true
        let settle = writes.begin()
        defer { settle() }

        do {
            guard let echo = try await port.write(value) else {
                return .unreachable
            }
            sentToBox = value
            return .persisted(boxValue(echo))
        } catch {
            return .unreachable
        }
    }

    func absorbFromBox() async -> BoxSettingsAbsorbResult {
        guard let mirror else { return .none }
        let readAt = writes.snapshot()
        guard let settings = await readOrNil(), !writes.isStale(readAt) else {
            return .none
        }

        guard let onBox = boxValue(settings) else {
            return await giveUnwrittenBoxThisSessionsAnswer()
        }

        let local = mirror.read()
        guard onBox != local else { return .none }
        mirror.write(onBox)
        return local == nil ? .backfilled : .repainted
    }

    func abandonInFlight() {
        writes.reset()
        answeredThisSession = false
        sentToBox = nil
    }

    private func giveUnwrittenBoxThisSessionsAnswer() async -> BoxSettingsAbsorbResult {
        guard let mirror else { return .none }
        if !answeredThisSession {
            guard mirror.read() != nil, let clear = mirror.clear else { return .none }
            clear()
            return .cleared
        }

        if let local = mirror.read(), local != sentToBox {
            _ = await apply(local)
        }
        return .none
    }

    private func readOrNil() async -> Settings? {
        guard port.isReadable() else { return nil }
        return try? await port.read()
    }

    private func resolve(_ settings: Settings) -> Value? {
        if !answeredThisSession {
            return boxValue(settings)
        }
        return boxValue(settings) ?? mirror?.read()
    }

    private func boxValue(_ settings: Settings) -> Value? {
        port.value(settings)
    }
}

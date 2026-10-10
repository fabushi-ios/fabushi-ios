import Foundation

private final class PackageContextKeyToken {}

private final class PackageContextStoredValue {
    let value: Any

    init<Value>(_ value: Value) {
        self.value = value as Any
    }
}

struct PackageContextKey<Value>: @unchecked Sendable {
    fileprivate let token: PackageContextKeyToken
    let defaultValue: Value

    init(defaultValue: Value) {
        token = PackageContextKeyToken()
        self.defaultValue = defaultValue
    }
}

final class PackageContextCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var storedReason: String?
    private var callbacks: [UUID: @Sendable (String?) -> Void] = [:]

    var cancelled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return storedReason != nil
    }

    var reason: String? {
        lock.lock()
        defer { lock.unlock() }
        return storedReason
    }

    func cancel(_ reason: String? = nil) {
        let callbacksToRun: [@Sendable (String?) -> Void]
        let normalized = reason ?? "cancelled"
        lock.lock()
        if storedReason != nil {
            lock.unlock()
            return
        }
        storedReason = normalized
        callbacksToRun = Array(callbacks.values)
        callbacks.removeAll()
        lock.unlock()
        for callback in callbacksToRun {
            callback(normalized)
        }
    }

    @discardableResult
    func observe(_ callback: @escaping @Sendable (String?) -> Void) -> @Sendable () -> Void {
        let id = UUID()
        lock.lock()
        if let storedReason {
            lock.unlock()
            callback(storedReason)
            return {}
        }
        callbacks[id] = callback
        lock.unlock()
        return { [weak self] in
            self?.lock.lock()
            self?.callbacks.removeValue(forKey: id)
            self?.lock.unlock()
        }
    }
}

/// Native typed context tree used by iOS package policies.
///
/// Values inherit through parent contexts. Cancellation normally propagates
/// downward; detached contexts intentionally keep value/name ancestry while
/// starting a fresh cancellation root, matching Grok's detached semantics.
final class PackageContext {
    private let parent: PackageContext?
    private let cancellation: PackageContextCancellation
    private let values: [ObjectIdentifier: PackageContextStoredValue]
    let name: String?

    private init(
        parent: PackageContext?,
        cancellation: PackageContextCancellation,
        values: [ObjectIdentifier: PackageContextStoredValue],
        name: String?
    ) {
        self.parent = parent
        self.cancellation = cancellation
        self.values = values
        self.name = name
    }

    static func root() -> PackageContext {
        PackageContext(
            parent: nil,
            cancellation: PackageContextCancellation(),
            values: [:],
            name: nil
        )
    }

    var cancelled: Bool { cancellation.cancelled }
    var reason: String? { cancellation.reason }

    func get<Value>(_ key: PackageContextKey<Value>) -> Value {
        let identifier = ObjectIdentifier(key.token)
        if let stored = values[identifier] {
            guard let value = stored.value as? Value else {
                preconditionFailure("PackageContext key was read with a mismatched value type")
            }
            return value
        }
        return parent?.get(key) ?? key.defaultValue
    }

    func with<Value>(_ key: PackageContextKey<Value>, value: Value) -> PackageContext {
        var nextValues = values
        nextValues[ObjectIdentifier(key.token)] = PackageContextStoredValue(value)
        return PackageContext(
            parent: self,
            cancellation: cancellation,
            values: nextValues,
            name: nil
        )
    }

    func withCancel() -> (PackageContext, @Sendable (String?) -> Void) {
        let childCancellation = PackageContextCancellation()
        let unsubscribe = cancellation.observe { reason in
            childCancellation.cancel(reason)
        }
        _ = childCancellation.observe { _ in
            unsubscribe()
        }
        let child = PackageContext(
            parent: self,
            cancellation: childCancellation,
            values: [:],
            name: nil
        )
        return (child, { reason in childCancellation.cancel(reason) })
    }

    func withTimeout(milliseconds: Int) -> PackageContext {
        let (child, cancel) = withCancel()
        let bounded = max(0, milliseconds)
        let task = Task {
            do {
                try await Task.sleep(for: .milliseconds(bounded))
                cancel("context deadline exceeded")
            } catch {
                // Child cancellation cancels the timer task; no second reason.
            }
        }
        _ = child.cancellation.observe { _ in task.cancel() }
        return child
    }

    func withDeadline(_ deadline: Date) -> PackageContext {
        let milliseconds = Int((deadline.timeIntervalSinceNow * 1_000).rounded(.up))
        if milliseconds <= 0 {
            let (child, cancel) = withCancel()
            cancel("context deadline exceeded")
            return child
        }
        return withTimeout(milliseconds: milliseconds)
    }

    func withTimeoutAndCancel(milliseconds: Int) -> (PackageContext, @Sendable (String?) -> Void) {
        let (cancelContext, cancel) = withCancel()
        return (cancelContext.withTimeout(milliseconds: milliseconds), cancel)
    }

    func withName(_ name: String) -> PackageContext {
        PackageContext(
            parent: self,
            cancellation: cancellation,
            values: [:],
            name: name
        )
    }

    func withDetached() -> PackageContext {
        PackageContext(
            parent: self,
            cancellation: PackageContextCancellation(),
            values: [:],
            name: nil
        )
    }

    func getParent() -> PackageContext? { parent }

    func getPath() -> [String] {
        var result: [String] = []
        var current: PackageContext? = self
        while let context = current {
            if let name = context.name, !name.isEmpty {
                result.insert(name, at: 0)
            }
            current = context.parent
        }
        return result
    }
}

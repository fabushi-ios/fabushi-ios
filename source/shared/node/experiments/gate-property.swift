import Foundation

final class MutableGateProperty: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Bool
    private var listeners: [UUID: (Bool) -> Void] = [:]

    init(_ value: Bool) { self.value = value }

    func get() -> Bool {
        lock.lock(); defer { lock.unlock() }
        return value
    }

    @discardableResult
    func subscribe(_ listener: @escaping (Bool) -> Void) -> () -> Void {
        let id = UUID()
        lock.lock(); listeners[id] = listener; lock.unlock()
        return { [weak self] in
            guard let self else { return }
            self.lock.lock(); self.listeners.removeValue(forKey: id); self.lock.unlock()
        }
    }

    func set(_ next: Bool) {
        let snapshot: [(Bool) -> Void]
        lock.lock()
        if value == next { lock.unlock(); return }
        value = next
        snapshot = Array(listeners.values)
        lock.unlock()

        for listener in snapshot {
            listener(next)
        }
    }

    func clearListeners() {
        lock.lock(); listeners.removeAll(); lock.unlock()
    }
}

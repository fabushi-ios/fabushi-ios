import Foundation

protocol SandDelayClock: Sendable {
    func sleep(milliseconds: Int) async
}

struct RealSandDelayClock: SandDelayClock {
    func sleep(milliseconds: Int) async {
        let safe = max(0, milliseconds)
        guard safe > 0 else { await Task.yield(); return }
        try? await Task.sleep(nanoseconds: UInt64(safe) * 1_000_000)
    }
}

func delay(_ ms: Int) async {
    await delayWith(RealSandDelayClock(), ms: ms)
}

func delayWith(_ clock: any SandDelayClock, ms: Int) async {
    if Task.isCancelled { return }
    await clock.sleep(milliseconds: max(0, ms))
}

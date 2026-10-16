import Foundation

final class WriteEpoch {
    private var pending = 0
    private var epoch = 0
    private var generation = 0

    func begin() -> () -> Void {
        epoch += 1
        pending += 1
        let startingGeneration = generation
        var settled = false
        return { [weak self] in
            guard let self, !settled, startingGeneration == self.generation else { return }
            settled = true
            self.pending -= 1
            if self.pending == 0 { self.epoch += 1 }
        }
    }

    func snapshot() -> Int { epoch }

    func isStale(_ snapshotAtStart: Int) -> Bool {
        pending > 0 || epoch != snapshotAtStart
    }

    func reset() {
        generation += 1
        pending = 0
        epoch += 1
    }
}

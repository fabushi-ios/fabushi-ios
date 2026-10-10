import Foundation

struct IOSParsedUpdateVersion: Equatable, Sendable {
    let release: [Int]
    let prerelease: [String]
}

enum IOSUpdateVersion {
    static func parse(_ version: String) -> IOSParsedUpdateVersion? {
        let pieces = version.split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
        let releaseParts = pieces[0].split(separator: ".", omittingEmptySubsequences: false)
        guard releaseParts.count == 3 else { return nil }
        var release: [Int] = []
        for component in releaseParts {
            guard !component.isEmpty,
                  component.allSatisfy(\.isNumber),
                  let value = Int(component)
            else { return nil }
            release.append(value)
        }
        let prerelease: [String]
        if pieces.count == 2 {
            guard !pieces[1].isEmpty else { return nil }
            prerelease = pieces[1].split(separator: ".", omittingEmptySubsequences: false).map(String.init)
            guard prerelease.allSatisfy({ !$0.isEmpty }) else { return nil }
        } else {
            prerelease = []
        }
        return .init(release: release, prerelease: prerelease)
    }

    static func compare(_ lhs: String, _ rhs: String) -> Int? {
        guard let left = parse(lhs), let right = parse(rhs) else { return nil }
        for index in 0..<3 where left.release[index] != right.release[index] {
            return left.release[index] < right.release[index] ? -1 : 1
        }

        if left.prerelease.isEmpty && right.prerelease.isEmpty { return 0 }
        if left.prerelease.isEmpty { return 1 }
        if right.prerelease.isEmpty { return -1 }

        for index in 0..<min(left.prerelease.count, right.prerelease.count) {
            let a = left.prerelease[index]
            let b = right.prerelease[index]
            if a == b { continue }
            let aNumber = Int(a)
            let bNumber = Int(b)
            if let aNumber, let bNumber { return aNumber < bNumber ? -1 : 1 }
            if aNumber != nil { return -1 }
            if bNumber != nil { return 1 }
            return a < b ? -1 : 1
        }
        if left.prerelease.count == right.prerelease.count { return 0 }
        return left.prerelease.count < right.prerelease.count ? -1 : 1
    }

    static func isNewer(_ candidate: String, than current: String) -> Bool {
        compare(candidate, current) == 1
    }
}

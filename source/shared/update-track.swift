import Foundation

enum FabushiUpdateTrack: String, CaseIterable, Sendable { case stable, nightly, dogfood }

struct FabushiReleaseTrackGate: Equatable, Sendable {
    let managedTrack: FabushiUpdateTrack?
    let unlockInternalTracks: Bool
}

enum UpdateTrackPolicy {
    static let nightlyDisabled = true

    static func selectable(unlockInternalTracks: Bool) -> [FabushiUpdateTrack] {
        FabushiUpdateTrack.allCases.filter {
            !(nightlyDisabled && $0 == .nightly) && ($0 != .dogfood || unlockInternalTracks)
        }
    }

    static func available(unlockInternalTracks: Bool, effectiveTrack: FabushiUpdateTrack) -> [FabushiUpdateTrack] {
        let allowed = Set(selectable(unlockInternalTracks: unlockInternalTracks))
        return FabushiUpdateTrack.allCases.filter { allowed.contains($0) || $0 == effectiveTrack }
    }

    static func coerceToEnabled(_ track: FabushiUpdateTrack) -> FabushiUpdateTrack { track == .nightly ? .stable : track }

    static func managedTrack(_ value: String?) -> FabushiUpdateTrack? {
        guard let value, let track = FabushiUpdateTrack(rawValue: value.trimmingCharacters(in: .whitespacesAndNewlines)),
              coerceToEnabled(track) == track
        else { return nil }
        return track
    }

    static func releaseGate(releaseTrack: String?, unlockInternalTracks: Bool) -> FabushiReleaseTrackGate {
        .init(managedTrack: managedTrack(releaseTrack), unlockInternalTracks: unlockInternalTracks)
    }

    static func effectiveTrack(managed: FabushiUpdateTrack?, userOverride: FabushiUpdateTrack?, buildDefault: FabushiUpdateTrack?) -> FabushiUpdateTrack {
        coerceToEnabled(managed ?? userOverride ?? buildDefault ?? .stable)
    }
}

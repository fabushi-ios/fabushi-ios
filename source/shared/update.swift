import Foundation

let SAND_UPDATE_TRACKS = ["stable", "nightly", "dogfood"]

func isSandUpdateTrack(_ value: String?) -> Bool {
    guard let value else { return false }
    return SAND_UPDATE_TRACKS.contains(value)
}

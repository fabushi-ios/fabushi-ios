import Foundation

@MainActor
enum IOSVNCPreloadEntrypoint {
    static func install() -> IOSVNCPreloadRuntime {
        IOSVNCPreloadRuntime()
    }
}

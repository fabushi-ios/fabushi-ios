import Foundation

@MainActor
enum IOSPrimaryPreloadEntrypoint {
    static func install(main: IOSMainRuntime) -> IOSPreloadBridge {
        IOSPreloadBridge(main: main)
    }
}

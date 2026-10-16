import SwiftUI

@main
struct FabushiApp: App {
    @UIApplicationDelegateAdaptor(FabushiAppDelegate.self)
    private var appDelegate

    var body: some Scene {
        WindowGroup {
            FabushiSceneRoot()
        }
    }
}

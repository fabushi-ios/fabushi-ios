import UIKit

@MainActor
final class FabushiAppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        #if DEBUG
        if ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil {
            return true
        }
        #endif
        HumanCallSystemCoordinator.shared.start()
        return true
    }

    func application(
        _ application: UIApplication,
        handleEventsForBackgroundURLSession identifier: String,
        completionHandler: @escaping () -> Void
    ) {
        IOSBackgroundTransferService.shared.handleEvents(
            forBackgroundURLSession: identifier,
            completionHandler: completionHandler
        )
    }
}

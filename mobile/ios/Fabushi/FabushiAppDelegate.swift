import UIKit
import UserNotifications

final class FabushiAgentNotificationDelegate: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    static let shared = FabushiAgentNotificationDelegate()

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        []
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let userInfo = response.notification.request.content.userInfo
        guard let agentId = userInfo[IOSAgentNotificationController.agentIdUserInfoKey] as? String,
              let accountScope = userInfo[IOSAgentNotificationController.accountScopeUserInfoKey] as? String,
              !agentId.isEmpty,
              !accountScope.isEmpty
        else { return }
        NotificationCenter.default.post(
            name: .fabushiOpenAgentFromNotification,
            object: nil,
            userInfo: [
                IOSAgentNotificationController.agentIdUserInfoKey: agentId,
                IOSAgentNotificationController.accountScopeUserInfoKey: accountScope,
            ]
        )
    }
}

@MainActor
final class FabushiAppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = FabushiAgentNotificationDelegate.shared
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

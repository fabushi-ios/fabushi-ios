import Foundation

/// Narrow launch policy for XCTest's app-hosted unit-test process.
///
/// Release builds ignore this policy at the scene boundary. In DEBUG, the
/// product runtime may be bypassed only for the explicit CI unit-test marker
/// or for an XCTest host that is injecting the FabushiTests bundle. UI tests
/// launch the application as a product process and therefore do not match the
/// hosted-unit-test signature.
enum IOSUnitTestHostPolicy {
    static let explicitEnvironmentKey = "FABUSHI_UNIT_TEST_HOST"

    static func shouldBypassProductRuntime(
        environment: [String: String]
    ) -> Bool {
        if environment[explicitEnvironmentKey] == "1" {
            return true
        }

        guard let testBundlePath = environment["XCTestBundlePath"],
              testBundlePath.hasSuffix("/FabushiTests.xctest"),
              let injectedHost = environment["XCInjectBundleInto"],
              !injectedHost.isEmpty
        else {
            return false
        }

        return true
    }
}

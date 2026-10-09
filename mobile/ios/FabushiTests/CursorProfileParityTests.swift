import Foundation
import XCTest
@testable import Fabushi

private func profileCredentials() -> AccountMcpCredentials {
    AccountMcpCredentials(
        getAccessToken: { _ in "profile-token" },
        getMachineId: { "profile-machine" }
    )
}

private func jsonResponse(_ request: URLRequest, _ object: [String: Any]) throws -> (Data, URLResponse) {
    (
        try JSONSerialization.data(withJSONObject: object),
        HTTPURLResponse(
            url: request.url!,
            statusCode: 200,
            httpVersion: "HTTP/2",
            headerFields: ["Content-Type": "application/json"]
        )!
    )
}

final class CursorProfileParityTests: XCTestCase {
    func testUpdateNameSplitsFirstTokenFromRemainingName() async throws {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(request.url?.path, "/aiserver.v1.DashboardService/UpdateUserName")
                XCTAssertEqual(request.timeoutInterval, 10)
                let body = try JSONSerialization.jsonObject(with: request.httpBody!) as? [String: String]
                XCTAssertEqual(body?["firstName"], "Ada")
                XCTAssertEqual(body?["lastName"], "Lovelace Byron")
                return try jsonResponse(request, [:])
            }
        )
        try await client.updateCursorAccountName("  Ada   Lovelace Byron ")
    }

    func testWeeklyUsagePreservesIncludedAndOnDemandSemantics() async throws {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                switch request.url?.lastPathComponent {
                case "GetSandUsageStatus":
                    return try jsonResponse(request, [
                        "usagePercent": 42.5,
                        "nextResetTimestampUtc": "2026-10-10T00:00:00Z",
                        "usesPooledEnterpriseAllowance": false,
                        "hasNonZeroIncludedLimit": true,
                    ])
                case "GetCurrentPeriodUsage":
                    return try jsonResponse(request, [
                        "spendLimitUsage": [
                            "individualUsed": 500,
                            "individualLimit": 1000,
                        ],
                    ])
                default:
                    XCTFail("Unexpected weekly usage method")
                    return try jsonResponse(request, [:])
                }
            }
        )
        let usage = await client.getCursorWeeklyUsage()
        XCTAssertEqual(usage?["percentUsed"] as? Double, 42.5)
        XCTAssertEqual(usage?["hasNonZeroIncludedLimit"] as? Bool, true)
        let onDemand = usage?["onDemand"] as? [String: Any]
        XCTAssertEqual(onDemand?["usedCents"] as? Double, 500)
        XCTAssertEqual(onDemand?["limitCents"] as? Double, 1000)
        XCTAssertNotNil(usage?["nextResetMs"] as? Int64)
    }

    func testPrivacyModeDefaultsFailClosedAndDisablesForTrainingModes() async throws {
        let allowed = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                try jsonResponse(request, [
                    "privacyMode": "PRIVACY_MODE_USAGE_DATA_TRAINING_ALLOWED",
                ])
            }
        )
        XCTAssertFalse(await allowed.getCursorPrivacyModeEnabled())

        let noStorage = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                try jsonResponse(request, [
                    "privacyMode": "PRIVACY_MODE_NO_STORAGE",
                ])
            }
        )
        XCTAssertTrue(await noStorage.getCursorPrivacyModeEnabled())
    }

    func testUsageSummaryAndDashboardActionUseDesktopTimeoutAndProjection() async throws {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(request.timeoutInterval, 15)
                switch request.url?.lastPathComponent {
                case "GetSandUsageStatus":
                    return try jsonResponse(request, [
                        "usagePercent": 25,
                        "hasAvailableUsage": true,
                        "hasNonZeroIncludedLimit": true,
                        "sandTrialCancelable": false,
                        "upgradeRecommendation": [
                            "disabled": false,
                            "cta": [
                                "label": "Ask for more",
                                "dashboardAction": [
                                    "action": "requestLimitIncrease",
                                    "args": ["source": "usage"],
                                    "successMessage": "Requested",
                                ],
                            ],
                        ],
                    ])
                case "GetCurrentPeriodUsage":
                    return try jsonResponse(request, [
                        "billingCycleEnd": "1791590400000",
                        "spendLimitUsage": [
                            "individualUsed": 200,
                            "individualLimit": 800,
                        ],
                    ])
                case "GetTeams":
                    return try jsonResponse(request, [
                        "teams": [["id": 8, "isEnterprise": true]],
                    ])
                case "GetSandTrialClaimStatus":
                    return try jsonResponse(request, [
                        "status": "SAND_TRIAL_CLAIM_STATUS_GRANTED",
                    ])
                case "ClientAction":
                    let body = try JSONSerialization.jsonObject(with: request.httpBody!) as? [String: Any]
                    XCTAssertEqual(body?["action"] as? String, "requestLimitIncrease")
                    return try jsonResponse(request, [
                        "success": true,
                        "infoMessage": "Done",
                    ])
                default:
                    XCTFail("Unexpected usage method")
                    return try jsonResponse(request, [:])
                }
            }
        )

        let summary = try await client.getCursorUsageSummary()
        XCTAssertEqual(summary["isEnterprise"] as? Bool, true)
        XCTAssertEqual(summary["hasEndedSandTrial"] as? Bool, true)
        let cta = summary["upgradeCta"] as? [String: Any]
        XCTAssertEqual(cta?["label"] as? String, "Ask for more")

        let action = try await client.invokeCursorDashboardAction(
            action: "requestLimitIncrease",
            args: ["source": "usage"]
        )
        XCTAssertEqual(action["ok"] as? Bool, true)
        XCTAssertEqual(action["message"] as? String, "Done")
    }

    func testCancelTrialReturnsStableResultInsteadOfThrowing() async {
        let client = IOSCursorDashboardClient(
            credentials: profileCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { _ in
                throw IOSCursorDashboardError(message: "cancel failed")
            }
        )
        let result = await client.cancelCursorSandTrial()
        XCTAssertEqual(result["ok"] as? Bool, false)
        XCTAssertEqual(result["message"] as? String, "cancel failed")
    }
}

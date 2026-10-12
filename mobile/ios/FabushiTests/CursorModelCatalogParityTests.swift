import Foundation
import XCTest
@testable import Fabushi

private func modelCatalogCredentials() -> AccountMcpCredentials {
    AccountMcpCredentials(
        getAccessToken: { _ in "model-token" },
        getMachineId: { "model-machine-id" }
    )
}

final class CursorModelCatalogParityTests: XCTestCase {
    func testAvailableModelsUsesCanonicalAiServiceUserScopeAndPreservesProtoJSON() async throws {
        let client = IOSCursorDashboardClient(
            credentials: modelCatalogCredentials(),
            backendURL: URL(string: "https://backend.example.test")!,
            requestExecutor: { request in
                XCTAssertEqual(
                    request.url?.path,
                    "/aiserver.v1.AiService/AvailableModels"
                )
                XCTAssertEqual(request.timeoutInterval, 12)
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Content-Type"),
                    "application/json"
                )
                XCTAssertEqual(
                    request.value(forHTTPHeaderField: "Connect-Protocol-Version"),
                    "1"
                )
                let body = try XCTUnwrap(request.httpBody)
                let object = try XCTUnwrap(
                    JSONSerialization.jsonObject(with: body) as? [String: Any]
                )
                XCTAssertEqual(
                    object["scope"] as? String,
                    "AVAILABLE_MODELS_SCOPE_USER_AVAILABLE"
                )
                XCTAssertEqual(object["useModelParameters"] as? Bool, true)

                let response: [String: Any] = [
                    "models": [
                        [
                            "name": "claude-sonnet",
                            "displayName": "Claude Sonnet",
                        ],
                    ],
                    "modelNames": ["claude-sonnet"],
                    "useModelParameters": true,
                ]
                return (
                    try JSONSerialization.data(withJSONObject: response),
                    HTTPURLResponse(
                        url: request.url!,
                        statusCode: 200,
                        httpVersion: "HTTP/2",
                        headerFields: ["Content-Type": "application/json"]
                    )!
                )
            }
        )

        let result = try await client.getAvailableModels()
        XCTAssertEqual(result["modelNames"] as? [String], ["claude-sonnet"])
        XCTAssertEqual(result["useModelParameters"] as? Bool, true)
        let models = try XCTUnwrap(result["models"] as? [[String: Any]])
        XCTAssertEqual(models.first?["name"] as? String, "claude-sonnet")
    }

    func testShippingCoordinatorInterceptsModelCatalogBeforeHostAndFencesStaleAccount() throws {
        let repositoryRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(
            contentsOf: repositoryRoot.appendingPathComponent(
                "source/mahayana-agent-coordinator/coordinator-mcp-manager.swift"
            ),
            encoding: .utf8
        )

        XCTAssertTrue(source.contains(#"case "getAvailableModels":"#))
        XCTAssertTrue(source.contains("let models = try await dashboard.getAvailableModels()"))
        XCTAssertTrue(source.contains("guard routedScopeToken() == scope"))
        XCTAssertTrue(source.contains("latestAuth.authId == auth.authId"))
        XCTAssertTrue(source.contains("latestAuth.freshness == auth.freshness"))
        XCTAssertTrue(source.contains("try Task.checkCancellation()"))
    }
}

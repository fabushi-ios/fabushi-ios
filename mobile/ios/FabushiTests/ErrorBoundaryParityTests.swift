import XCTest
@testable import Fabushi

@MainActor
final class ErrorBoundaryParityTests: XCTestCase {
    private enum StubError: Error, LocalizedError {
        case initialization
        case root
        case clipboard

        var errorDescription: String? {
            switch self {
            case .initialization: return "runtime construction failed"
            case .root: return "root failed"
            case .clipboard: return "clipboard unavailable"
            }
        }
    }

    func testConstructionFailureShowsErrorAndNeverRendersProductionRenderer() {
        let lifecycle = FabushiRuntimeLifecycle {
            throw StubError.initialization
        }

        lifecycle.loadIfNeeded()

        guard case .failed(let failure) = lifecycle.phase else {
            return XCTFail("Expected native root error state")
        }
        XCTAssertEqual(lifecycle.constructionAttempts, 1)
        XCTAssertFalse(lifecycle.rendersProductionRenderer)
        XCTAssertEqual(failure.title, FabushiErrorBoundaryAccessibility.titleLabel)
        XCTAssertEqual(failure.detail, FabushiErrorBoundaryAccessibility.detailLabel)
        XCTAssertTrue(failure.diagnostics.contains("runtime-initialization"))
        XCTAssertTrue(failure.diagnostics.contains("runtime construction failed"))
    }

    func testRetryReusesSingleLifecycleFactoryAndRebuildsOnlyRuntimeOwner() {
        var attempts = 0
        let lifecycle = FabushiRuntimeLifecycle {
            attempts += 1
            throw StubError.initialization
        }

        lifecycle.loadIfNeeded()
        lifecycle.retry()

        XCTAssertEqual(attempts, 2)
        XCTAssertEqual(lifecycle.constructionAttempts, 2)
        XCTAssertFalse(lifecycle.rendersProductionRenderer)
        guard case .failed = lifecycle.phase else {
            return XCTFail("Retry failure must remain on the same root error surface")
        }
    }

    func testRootFailureCannotFallThroughToProductionRenderer() {
        let lifecycle = FabushiRuntimeLifecycle {
            throw StubError.initialization
        }

        lifecycle.presentRootFailure(StubError.root, context: "renderer-root")

        XCTAssertFalse(lifecycle.rendersProductionRenderer)
        guard case .failed(let failure) = lifecycle.phase else {
            return XCTFail("Expected failed lifecycle state")
        }
        XCTAssertTrue(failure.diagnostics.contains("renderer-root"))
        XCTAssertTrue(failure.diagnostics.contains("root failed"))
    }

    func testCopyDiagnosticsReportsSuccessWithoutMutatingErrorLifecycle() {
        var copied = ""
        let ok = copyFabushiDiagnostics("diagnostic payload") { value in
            copied = value
        }

        XCTAssertTrue(ok)
        XCTAssertEqual(copied, "diagnostic payload")
    }

    func testCopyDiagnosticsFailureIsContainedAndLeavesErrorSurfaceRecoverable() {
        let lifecycle = FabushiRuntimeLifecycle {
            throw StubError.initialization
        }
        lifecycle.loadIfNeeded()

        let ok = copyFabushiDiagnostics("diagnostic payload") { _ in
            throw StubError.clipboard
        }

        XCTAssertFalse(ok)
        XCTAssertFalse(lifecycle.rendersProductionRenderer)
        guard case .failed = lifecycle.phase else {
            return XCTFail("Clipboard failure must not replace the root error state")
        }
    }

    func testAccessibilityContractExposesStableErrorActions() {
        XCTAssertEqual(FabushiErrorBoundaryAccessibility.surface, "fabushi-root-error-surface")
        XCTAssertEqual(FabushiErrorBoundaryAccessibility.title, "fabushi-root-error-title")
        XCTAssertEqual(FabushiErrorBoundaryAccessibility.detail, "fabushi-root-error-detail")
        XCTAssertEqual(FabushiErrorBoundaryAccessibility.retry, "fabushi-root-error-retry")
        XCTAssertEqual(
            FabushiErrorBoundaryAccessibility.copyDiagnostics,
            "fabushi-root-error-copy-diagnostics"
        )
        XCTAssertEqual(FabushiErrorBoundaryAccessibility.retryLabel, "Retry")
        XCTAssertEqual(FabushiErrorBoundaryAccessibility.copyLabel, "Copy diagnostics")
    }
}

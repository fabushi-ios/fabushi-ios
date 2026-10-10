import Foundation
import XCTest
@testable import Fabushi

private func cloudAgentAppendVarint(_ value: UInt64, to data: inout Data) {
    var value = value
    repeat {
        var byte = UInt8(value & 0x7f)
        value >>= 7
        if value != 0 { byte |= 0x80 }
        data.append(byte)
    } while value != 0
}

private func cloudAgentVarint(_ field: Int, _ value: UInt64) -> Data {
    var data = Data()
    cloudAgentAppendVarint(UInt64(field << 3), to: &data)
    cloudAgentAppendVarint(value, to: &data)
    return data
}

private func cloudAgentBytes(_ field: Int, _ body: Data) -> Data {
    var data = Data()
    cloudAgentAppendVarint(UInt64((field << 3) | 2), to: &data)
    cloudAgentAppendVarint(UInt64(body.count), to: &data)
    data.append(body)
    return data
}

private func cloudAgentString(_ field: Int, _ value: String) -> Data {
    cloudAgentBytes(field, Data(value.utf8))
}

final class CursorCloudAgentInfoParityTests: XCTestCase {
    func testDecodesAuthoritativeComposerBranchPRAndDiffStats() throws {
        var composer = Data()
        composer.append(cloudAgentString(5, "Refactor auth"))
        composer.append(cloudAgentString(6, "cursor/auth-refactor"))
        composer.append(cloudAgentVarint(12, 2))
        composer.append(cloudAgentString(22, "https://github.com/acme/repo/pull/42"))
        composer.append(cloudAgentVarint(25, 123))
        composer.append(cloudAgentVarint(26, 17))
        composer.append(cloudAgentVarint(27, 9))
        composer.append(cloudAgentVarint(43, 1))

        var prompt = Data()
        prompt.append(cloudAgentString(1, "Refactor the auth flow"))

        var pr = Data()
        pr.append(cloudAgentString(1, "cursor/auth-refactor"))
        pr.append(cloudAgentVarint(4, 42))
        pr.append(cloudAgentVarint(5, 2))
        pr.append(cloudAgentString(6, "https://github.com/acme/repo/pull/42"))

        var detailed = Data()
        detailed.append(cloudAgentBytes(1, composer))
        detailed.append(cloudAgentBytes(4, prompt))
        detailed.append(cloudAgentVarint(5, 2))
        detailed.append(cloudAgentString(10, "Implemented the requested changes"))
        detailed.append(cloudAgentBytes(20, pr))

        let info = try IOSCursorDashboardClient.decodeBackgroundComposerInfoForTests(
            cloudAgentBytes(1, detailed)
        )
        XCTAssertEqual(info.status, 2)
        XCTAssertEqual(info.name, "Refactor auth")
        XCTAssertEqual(info.prompt, "Refactor the auth flow")
        XCTAssertEqual(info.summary, "Implemented the requested changes")
        XCTAssertEqual(info.branchName, "cursor/auth-refactor")
        XCTAssertEqual(info.filesChanged, 9)
        XCTAssertEqual(info.linesAdded, 123)
        XCTAssertEqual(info.linesRemoved, 17)
        XCTAssertEqual(info.prURL, "https://github.com/acme/repo/pull/42")
        XCTAssertEqual(info.prState, "draft")
        XCTAssertEqual(info.prNumber, 42)
    }

    func testFallsBackToPRBranchAndPRURLWhenComposerFieldsAreEmpty() throws {
        var composer = Data()
        composer.append(cloudAgentVarint(12, 4))

        var pr = Data()
        pr.append(cloudAgentString(1, "cursor/fallback"))
        pr.append(cloudAgentVarint(4, 7))
        pr.append(cloudAgentVarint(5, 3))
        pr.append(cloudAgentString(6, "https://github.com/acme/repo/pull/7"))

        var detailed = Data()
        detailed.append(cloudAgentBytes(1, composer))
        detailed.append(cloudAgentVarint(5, 4))
        detailed.append(cloudAgentBytes(20, pr))

        let info = try IOSCursorDashboardClient.decodeBackgroundComposerInfoForTests(
            cloudAgentBytes(1, detailed)
        )
        XCTAssertEqual(info.branchName, "cursor/fallback")
        XCTAssertEqual(info.prURL, "https://github.com/acme/repo/pull/7")
        XCTAssertEqual(info.prState, "merged")
        XCTAssertEqual(info.prNumber, 7)
    }
}

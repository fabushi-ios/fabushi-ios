import XCTest
@testable import Fabushi

final class NativeSpreadsheetPreviewTests: XCTestCase {
    func testTableKindMatchesDesktopExtensionsAndMimeTypes() {
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: "text/csv", fileName: nil), .delimited(","))
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: "text/tab-separated-values", fileName: nil), .delimited("\t"))
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: "application/vnd.ms-excel", fileName: nil), .workbook)
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", fileName: nil), .workbook)
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: nil, fileName: "book.xlsx"), .workbook)
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: nil, fileName: "legacy.XLS"), .workbook)
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: nil, fileName: "rows.csv"), .delimited(","))
        XCTAssertEqual(nativeSpreadsheetKind(mimeType: nil, fileName: "rows.tsv"), .delimited("\t"))
        XCTAssertNil(nativeSpreadsheetKind(mimeType: "text/plain", fileName: "notes.txt"))
    }

    func testDelimitedParserPreservesQuotedFieldsAndNewlines() {
        let source = "name,note,count\nalpha,\"hello, world\",1\nbeta,\"line one\nline two\",2\ngamma,\"quote \"\"inside\"\"\",3"
        let sheet = NativeDelimitedSpreadsheet.parse(Data(source.utf8), delimiter: ",")
        XCTAssertEqual(sheet.totalRows, 4)
        XCTAssertEqual(sheet.displayedRowCount, 3)
        XCTAssertEqual(sheet.rows[1], ["alpha", "hello, world", "1"])
        XCTAssertEqual(sheet.rows[2], ["beta", "line one\nline two", "2"])
        XCTAssertEqual(sheet.rows[3], ["gamma", "quote \"inside\"", "3"])
    }

    func testDelimitedParserCapsStoredRowsButRetainsTotalCount() {
        let source = (0..<2_105).map { "row\($0),value\($0)" }.joined(separator: "\n")
        let sheet = NativeDelimitedSpreadsheet.parse(Data(source.utf8), delimiter: ",")
        XCTAssertEqual(sheet.rows.count, nativeSpreadsheetMaxRows)
        XCTAssertEqual(sheet.totalRows, 2_105)
    }

    func testSpreadsheetPreviewUsesDesktopByteAndRenderBounds() {
        XCTAssertEqual(nativeSpreadsheetPreviewByteCap, 25 * 1024 * 1024)
        XCTAssertEqual(nativeSpreadsheetMaxRows, 2_000)
        XCTAssertEqual(nativeSpreadsheetRenderRows, 200)
        XCTAssertEqual(nativeSpreadsheetRenderColumns, 200)
    }
}

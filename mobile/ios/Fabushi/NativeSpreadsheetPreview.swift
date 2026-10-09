import QuickLook
import SwiftUI
import UIKit

let nativeSpreadsheetPreviewByteCap = 25 * 1024 * 1024
let nativeSpreadsheetMaxRows = 2_000
let nativeSpreadsheetRenderRows = 200
let nativeSpreadsheetRenderColumns = 200

enum NativeSpreadsheetKind: Equatable {
    case delimited(Character)
    case workbook
}

internal func nativeSpreadsheetKind(mimeType: String?, fileName: String?) -> NativeSpreadsheetKind? {
    let mime = (mimeType ?? "").split(separator: ";", maxSplits: 1).first?
        .trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    switch mime {
    case "text/csv": return .delimited(",")
    case "text/tab-separated-values": return .delimited("\t")
    case "application/vnd.ms-excel",
         "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet":
        return .workbook
    default: break
    }
    guard let fileName else { return nil }
    switch URL(fileURLWithPath: fileName).pathExtension.lowercased() {
    case "csv": return .delimited(",")
    case "tsv": return .delimited("\t")
    case "xls", "xlsx": return .workbook
    default: return nil
    }
}

internal func isNativeSpreadsheetAttachment(mimeType: String?, fileName: String?) -> Bool {
    nativeSpreadsheetKind(mimeType: mimeType, fileName: fileName) != nil
}

struct NativeDelimitedSpreadsheet: Equatable {
    let rows: [[String]]
    let totalRows: Int

    var displayedRowCount: Int { max(0, totalRows - (rows.isEmpty ? 0 : 1)) }

    static func parse(_ data: Data, delimiter: Character, maxRows: Int = nativeSpreadsheetMaxRows) -> Self {
        let text = String(decoding: data, as: UTF8.self)
        let rows = parseRows(text, delimiter: delimiter, maxRows: maxRows)
        return .init(rows: rows, totalRows: rows.count < maxRows ? rows.count : countRows(text))
    }

    private static func parseRows(_ text: String, delimiter: Character, maxRows: Int) -> [[String]] {
        guard maxRows > 0 else { return [] }
        let characters = Array(text)
        var rows: [[String]] = []
        var row: [String] = []
        var field = ""
        var quoted = false
        var hasContent = false
        var index = 0
        func pushField() { row.append(field); field = "" }
        func pushRow() { pushField(); rows.append(row); row = []; hasContent = false }

        while index < characters.count && rows.count < maxRows {
            let character = characters[index]
            if quoted {
                if character == "\"" {
                    if index + 1 < characters.count, characters[index + 1] == "\"" {
                        field.append("\""); index += 1
                    } else {
                        quoted = false
                    }
                } else {
                    field.append(character)
                }
                hasContent = true
                index += 1
                continue
            }
            if character == "\"" && field.isEmpty {
                quoted = true; hasContent = true; index += 1; continue
            }
            if character == delimiter {
                pushField(); hasContent = true; index += 1; continue
            }
            if character == "\n" || character == "\r" {
                if character == "\r", index + 1 < characters.count, characters[index + 1] == "\n" { index += 1 }
                pushRow(); index += 1; continue
            }
            field.append(character)
            if !character.isWhitespace { hasContent = true }
            index += 1
        }
        if rows.count < maxRows && (hasContent || !field.isEmpty || !row.isEmpty) { pushRow() }
        return rows
    }

    private static func countRows(_ text: String) -> Int {
        let characters = Array(text)
        var rows = 0
        var quoted = false
        var hasContent = false
        var index = 0
        while index < characters.count {
            let character = characters[index]
            if character == "\"" {
                if quoted, index + 1 < characters.count, characters[index + 1] == "\"" { index += 1 }
                else { quoted.toggle() }
                hasContent = true
                index += 1
                continue
            }
            if !quoted && (character == "\n" || character == "\r") {
                if character == "\r", index + 1 < characters.count, characters[index + 1] == "\n" { index += 1 }
                rows += 1
                hasContent = false
                index += 1
                continue
            }
            if !character.isWhitespace { hasContent = true }
            index += 1
        }
        return hasContent ? rows + 1 : rows
    }
}

private final class NativeWorkbookPreviewDataSource: NSObject, QLPreviewControllerDataSource {
    let url: URL
    init(url: URL) { self.url = url }
    func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
    func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> any QLPreviewItem {
        url as NSURL
    }
}

private struct NativeWorkbookPreview: UIViewControllerRepresentable {
    let url: URL
    func makeCoordinator() -> NativeWorkbookPreviewDataSource { .init(url: url) }
    func makeUIViewController(context: Context) -> QLPreviewController {
        let controller = QLPreviewController()
        controller.dataSource = context.coordinator
        return controller
    }
    func updateUIViewController(_ controller: QLPreviewController, context: Context) {}
}

private struct NativeDelimitedSpreadsheetPreview: View {
    let sheet: NativeDelimitedSpreadsheet
    @State private var selectedCell: (row: Int, column: Int)?

    private var header: [String] { sheet.rows.first ?? [] }
    private var visibleRows: [[String]] { Array(sheet.rows.dropFirst().prefix(nativeSpreadsheetRenderRows)) }
    private var columnCount: Int {
        min(nativeSpreadsheetRenderColumns, sheet.rows.reduce(0) { max($0, $1.count) })
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("\(sheet.displayedRowCount) \(sheet.displayedRowCount == 1 ? "row" : "rows")")
                    .font(.caption).foregroundStyle(.secondary)
                Spacer()
                if sheet.totalRows > nativeSpreadsheetRenderRows + 1 {
                    Text("预览前 \(nativeSpreadsheetRenderRows) 行").font(.caption).foregroundStyle(.secondary)
                }
            }.padding(.horizontal, 12).padding(.vertical, 8)

            if visibleRows.isEmpty {
                ContentUnavailableView("This sheet is empty", systemImage: "tablecells")
            } else {
                ScrollView([.horizontal, .vertical]) {
                    VStack(alignment: .leading, spacing: 0) {
                        spreadsheetRow(values: header, row: -1, isHeader: true)
                        ForEach(Array(visibleRows.enumerated()), id: \.offset) { offset, values in
                            spreadsheetRow(values: values, row: offset, isHeader: false)
                        }
                    }.padding(.bottom, 12)
                }
            }

            if let selectedCell {
                Divider()
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text(selectedCellLabel(selectedCell)).font(.caption.weight(.semibold))
                        Spacer()
                        Button { self.selectedCell = nil } label: { Image(systemName: "xmark") }
                            .buttonStyle(.plain).accessibilityLabel("Close cell detail")
                    }
                    ScrollView {
                        Text(cellValue(selectedCell)).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }.frame(maxHeight: 140)
                }.padding(12).background(.thinMaterial)
            }
        }.background(Color(uiColor: .systemBackground))
    }

    @ViewBuilder
    private func spreadsheetRow(values: [String], row: Int, isHeader: Bool) -> some View {
        HStack(spacing: 0) {
            Group {
                if isHeader { Color.clear } else {
                    Text("\(row + 1)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                }
            }
            .frame(width: 44, height: 34)
            .overlay(
                Rectangle().stroke(
                    Color(uiColor: .separator).opacity(0.45),
                    lineWidth: 0.5
                )
            )

            ForEach(0..<columnCount, id: \.self) { column in
                let value = column < values.count ? values[column] : ""
                Button {
                    if value.isEmpty { selectedCell = nil }
                    else if selectedCell?.row == row && selectedCell?.column == column { selectedCell = nil }
                    else { selectedCell = (row, column) }
                } label: {
                    Text(value).font(isHeader ? .caption.weight(.semibold) : .caption)
                        .foregroundStyle(.primary).lineLimit(1).truncationMode(.tail)
                        .frame(width: 132, height: 34, alignment: .leading)
                        .padding(.horizontal, 6).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .overlay(
                Rectangle().stroke(
                    Color(uiColor: .separator).opacity(0.45),
                    lineWidth: 0.5
                )
            )
                .accessibilityLabel(isHeader ? (value.isEmpty ? "Column \(column + 1)" : value) : "\(headerValue(column)) row \(row + 1)")
                .accessibilityValue(value)
            }
        }
    }

    private func headerValue(_ column: Int) -> String {
        column < header.count && !header[column].isEmpty ? header[column] : "Column \(column + 1)"
    }
    private func cellValue(_ cell: (row: Int, column: Int)) -> String {
        let sourceRow = cell.row == -1 ? 0 : cell.row + 1
        guard sourceRow >= 0, sourceRow < sheet.rows.count, cell.column >= 0,
              cell.column < sheet.rows[sourceRow].count else { return "" }
        return sheet.rows[sourceRow][cell.column]
    }
    private func selectedCellLabel(_ cell: (row: Int, column: Int)) -> String {
        "\(headerValue(cell.column)) · \(cell.row == -1 ? "header" : "row \(cell.row + 1)")"
    }
}

struct NativeSpreadsheetPreview: View {
    let data: Data
    let url: URL
    let fileName: String?
    let mimeType: String?

    var body: some View {
        switch nativeSpreadsheetKind(mimeType: mimeType, fileName: fileName) {
        case .delimited(let delimiter):
            NativeDelimitedSpreadsheetPreview(sheet: .parse(data, delimiter: delimiter))
        case .workbook:
            NativeWorkbookPreview(url: url).ignoresSafeArea(edges: .bottom)
        case nil:
            ContentUnavailableView("File unavailable", systemImage: "doc")
        }
    }
}

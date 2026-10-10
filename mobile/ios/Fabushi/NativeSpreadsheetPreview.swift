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

struct NativeWorkbookSheet: Codable, Equatable, Sendable {
    let name: String
    let rows: [[String]]
    let totalRows: Int
}

struct NativeWorkbookSpreadsheet: Codable, Equatable, Sendable {
    let sheets: [NativeWorkbookSheet]

    enum ProjectionError: LocalizedError, Equatable {
        case invalidResponse
        case parserFailure(String)

        var errorDescription: String? {
            switch self {
            case .invalidResponse:
                return "Workbook parser returned an invalid response."
            case .parserFailure(let message):
                return message
            }
        }
    }

    private struct Envelope: Decodable {
        let ok: Bool
        let result: NativeWorkbookSpreadsheet?
        let error: String?
    }

    static func decodeEnvelope(_ data: Data) throws -> NativeWorkbookSpreadsheet {
        let envelope: Envelope
        do {
            envelope = try JSONDecoder().decode(Envelope.self, from: data)
        } catch {
            throw ProjectionError.invalidResponse
        }
        guard envelope.ok, let workbook = envelope.result else {
            throw ProjectionError.parserFailure(envelope.error ?? "Workbook could not be parsed.")
        }
        guard !workbook.sheets.isEmpty else {
            throw ProjectionError.parserFailure("Workbook contains no worksheets.")
        }
        return workbook
    }

    static func parse(
        url: URL,
        maxBytes: Int = nativeSpreadsheetPreviewByteCap,
        maxRows: Int = nativeSpreadsheetMaxRows
    ) throws -> NativeWorkbookSpreadsheet {
        guard maxBytes > 0, maxRows > 0 else {
            throw ProjectionError.parserFailure("Workbook preview bounds are invalid.")
        }
        let pointer = url.path.withCString { path in
            mahayana_spreadsheet_parse_file(path, maxBytes, maxRows)
        }
        guard let pointer else { throw ProjectionError.invalidResponse }
        defer { mahayana_app_host_free_string(pointer) }
        let json = String(cString: pointer)
        guard let data = json.data(using: .utf8) else { throw ProjectionError.invalidResponse }
        return try decodeEnvelope(data)
    }
}

private struct NativeSpreadsheetTablePreview: View {
    let rows: [[String]]
    let totalRows: Int
    @State private var selectedCell: (row: Int, column: Int)?

    private var displayedRowCount: Int { max(0, totalRows - (rows.isEmpty ? 0 : 1)) }
    private var header: [String] { rows.first ?? [] }
    private var visibleRows: [[String]] { Array(rows.dropFirst().prefix(nativeSpreadsheetRenderRows)) }
    private var columnCount: Int {
        min(nativeSpreadsheetRenderColumns, rows.reduce(0) { max($0, $1.count) })
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("\(displayedRowCount) \(displayedRowCount == 1 ? "row" : "rows")")
                    .font(.caption).foregroundStyle(.secondary)
                Spacer()
                if totalRows > nativeSpreadsheetRenderRows + 1 {
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
        guard sourceRow >= 0, sourceRow < rows.count, cell.column >= 0,
              cell.column < rows[sourceRow].count else { return "" }
        return rows[sourceRow][cell.column]
    }

    private func selectedCellLabel(_ cell: (row: Int, column: Int)) -> String {
        "\(headerValue(cell.column)) · \(cell.row == -1 ? "header" : "row \(cell.row + 1)")"
    }
}

private struct NativeWorkbookPreview: View {
    enum State: Equatable {
        case loading
        case ready(NativeWorkbookSpreadsheet)
        case failed(String)
    }

    let url: URL
    @State private var state: State = .loading
    @State private var selectedSheet = 0

    var body: some View {
        Group {
            switch state {
            case .loading:
                ProgressView("Loading workbook…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            case .failed(let message):
                ContentUnavailableView(
                    "Couldn't read this spreadsheet",
                    systemImage: "tablecells.badge.ellipsis",
                    description: Text(message)
                )
            case .ready(let workbook):
                let safeIndex = min(max(0, selectedSheet), max(0, workbook.sheets.count - 1))
                let sheet = workbook.sheets[safeIndex]
                VStack(spacing: 0) {
                    if workbook.sheets.count > 1 {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 8) {
                                ForEach(Array(workbook.sheets.enumerated()), id: \.offset) { index, item in
                                    Button(item.name) { selectedSheet = index }
                                        .buttonStyle(index == safeIndex ? .borderedProminent : .bordered)
                                        .accessibilityAddTraits(index == safeIndex ? .isSelected : [])
                                }
                            }
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                        }
                        Divider()
                    }
                    NativeSpreadsheetTablePreview(rows: sheet.rows, totalRows: sheet.totalRows)
                        .id("\(url.path)#\(safeIndex)")
                }
            }
        }
        .task(id: url) {
            state = .loading
            selectedSheet = 0
            do {
                let workbook = try await Task.detached(priority: .userInitiated) {
                    try NativeWorkbookSpreadsheet.parse(url: url)
                }.value
                try Task.checkCancellation()
                state = .ready(workbook)
            } catch is CancellationError {
                return
            } catch {
                guard !Task.isCancelled else { return }
                state = .failed(error.localizedDescription)
            }
        }
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
            let sheet = NativeDelimitedSpreadsheet.parse(data, delimiter: delimiter)
            NativeSpreadsheetTablePreview(rows: sheet.rows, totalRows: sheet.totalRows)
        case .workbook:
            NativeWorkbookPreview(url: url)
        case nil:
            ContentUnavailableView("File unavailable", systemImage: "doc")
        }
    }
}

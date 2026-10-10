import Foundation
import SwiftUI

internal enum MobileTerminalOutputStatus: String, Equatable, Sendable {
    case idle
    case running
    case exited
    case error
}

internal struct MobileTerminalOutputSnapshot: Equatable, Sendable {
    let sessionId: String
    let command: String
    let cwd: String?
    let output: String
    let status: MobileTerminalOutputStatus
    let exitCode: Double?
}

internal enum MobileTerminalOutputModel {
    static func normalizeOutput(_ value: Any?) -> String {
        guard let text = value as? String else { return "" }
        return text
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
    }

    static func project(_ value: Any?) -> MobileTerminalOutputSnapshot? {
        guard let root = record(value) else { return nil }
        let metadata = record(root["metadata"])
        let currentCommand = record(root["currentCommand"])
            ?? record(metadata?["currentCommand"])
        let command = nonEmptyString(root["command"])
            ?? nonEmptyString(currentCommand?["command"])

        let numericId = finiteNumber(root["terminalInstanceId"])
            ?? finiteNumber(root["terminal_instance_id"])
        let sessionId = nonEmptyString(root["sessionId"])
            ?? numericId.map(numberString)
            ?? nonEmptyString(root["terminalInstancePath"])
            ?? nonEmptyString(root["terminal_instance_path"])
        guard let command, let sessionId else { return nil }

        let exitCode = finiteNumber(root["exitCode"] ?? root["exit_code"])
        let running = (root["status"] as? String) == "running"
            || (root["isRunning"] as? Bool) == true
            || (root["isRunningInBackground"] as? Bool) == true
        let failed = (root["status"] as? String) == "error"
            || (root["rejected"] as? Bool) == true
            || (exitCode != nil && exitCode != 0)

        let status: MobileTerminalOutputStatus
        if running {
            status = .running
        } else if failed {
            status = .error
        } else if (root["status"] as? String) == "idle" {
            status = .idle
        } else {
            status = .exited
        }

        return .init(
            sessionId: sessionId,
            command: command,
            cwd: nonEmptyString(root["cwd"])
                ?? nonEmptyString(root["cwdFull"])
                ?? metadata.flatMap { nonEmptyString($0["cwd"]) },
            output: normalizeOutput(
                root["outputRaw"]
                    ?? root["output_raw"]
                    ?? root["output"]
                    ?? root["contents"]
            ),
            status: status,
            exitCode: exitCode
        )
    }

    private static func record(_ value: Any?) -> [String: Any]? {
        value as? [String: Any]
    }

    private static func nonEmptyString(_ value: Any?) -> String? {
        guard let value = value as? String, !value.isEmpty else { return nil }
        return value
    }

    private static func finiteNumber(_ value: Any?) -> Double? {
        guard let number = value as? NSNumber,
              CFGetTypeID(number) != CFBooleanGetTypeID()
        else { return nil }
        let value = number.doubleValue
        return value.isFinite ? value : nil
    }

    private static func numberString(_ value: Double) -> String {
        if value.rounded() == value {
            return String(Int64(value))
        }
        return String(value)
    }
}

/// Native read-only terminal result leaf. Desktop main currently exposes the
/// same leaf contract without a terminal coordinator or mounted execution UI;
/// iOS deliberately does not invent mutation/execution controls.
internal struct MobileTerminalOutputPanel: View {
    let accessibilityLabel: String
    let snapshot: MobileTerminalOutputSnapshot

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(snapshot.command)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
                Spacer(minLength: 8)
                Text(snapshot.status.rawValue)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityLabel(snapshot.status.rawValue)
            }
            if let cwd = snapshot.cwd {
                Text(cwd)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            ScrollView([.vertical, .horizontal]) {
                Text(snapshot.output)
                    .font(.system(.body, design: .monospaced))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("mobile-terminal-output-log")
            }
            .frame(maxHeight: 480)
        }
        .padding(12)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(accessibilityLabel)
        .accessibilityIdentifier("mobile-terminal-output-\(snapshot.sessionId)")
    }
}

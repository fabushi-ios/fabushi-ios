import CryptoKit
import Foundation
import SwiftUI
import UIKit
import WebKit

private let webMcpOriginHost = "fabushi.ombhrum.com"
private let localWebMcpOriginHost = "miniapp.local.fabushi.invalid"
private let generatedMiniAppPreviewOriginHost = "preview.miniapp.local.fabushi.invalid"
private let webMcpMessageHandler = "fabushiWebMcp"
let miniAppWebMcpHostCallTimeoutMilliseconds = 15_000

func generatedMiniAppPreviewDigest(_ html: String) -> String {
    SHA256.hash(data: Data(html.utf8))
        .map { String(format: "%02x", $0) }
        .joined()
}

func miniAppSourceGenerationKey(
    accountKey: String,
    pluginId: String,
    previewDigest: String?
) -> String {
    let accountDigest = SHA256.hash(data: Data(accountKey.utf8))
        .map { String(format: "%02x", $0) }
        .joined()
    if let previewDigest {
        return "preview:\(accountDigest):\(pluginId):\(previewDigest)"
    }
    return "installed:\(accountDigest):\(pluginId)"
}

func generatedMiniAppReviewMatchesPreview(
    _ review: LocalMiniAppDraftReview,
    html: String
) -> Bool {
    guard review.files.count == 1,
          let file = review.files.first,
          file.path == "index.html",
          file.mime == "text/html"
    else { return false }
    let bytes = Data(html.utf8)
    return file.bytes == UInt64(bytes.count)
        && file.sha256 == generatedMiniAppPreviewDigest(html)
}

func isTrustedWebMcpBridgeHost(_ host: String?) -> Bool {
    guard let host else { return false }
    return host == webMcpOriginHost || host == localWebMcpOriginHost
}

func isTrustedWebMcpBridgeURL(_ url: URL?, localRootURL: URL? = nil) -> Bool {
    guard let url else { return false }
    if url.scheme == "https" {
        return isTrustedWebMcpBridgeHost(url.host)
    }
    guard url.isFileURL, let localRootURL else { return false }
    let root = localRootURL.standardizedFileURL
    let candidate = url.standardizedFileURL
    let rootPath = root.path.hasSuffix("/") ? root.path : root.path + "/"
    return candidate.path.hasPrefix(rootPath) && candidate.path != root.path
}

struct MiniAppWebMcpBridgeSession: Equatable {
    let pluginInstanceId: String
    let nonce: String
    let grants: Set<String>
    let generation: UInt64

    init(
        pluginInstanceId: String,
        nonce: String,
        grants: Set<String>,
        generation: UInt64 = 1
    ) {
        self.pluginInstanceId = pluginInstanceId
        self.nonce = nonce
        self.grants = grants
        self.generation = generation
    }

    static func fresh(plugin: MarketplacePlugin, generation: UInt64 = 1) -> Self {
        Self(
            pluginInstanceId: "\(plugin.pluginId):\(UUID().uuidString)",
            nonce: UUID().uuidString.replacingOccurrences(of: "-", with: ""),
            grants: Set(plugin.tools.map(\.name)),
            generation: generation
        )
    }

    func allows(
        pluginInstanceId: String,
        nonce: String,
        generation: UInt64,
        toolName: String
    ) -> Bool {
        self.pluginInstanceId == pluginInstanceId
            && self.nonce == nonce
            && self.generation == generation
            && grants.contains(toolName)
    }

    func allows(pluginInstanceId: String, nonce: String, toolName: String) -> Bool {
        allows(
            pluginInstanceId: pluginInstanceId,
            nonce: nonce,
            generation: generation,
            toolName: toolName
        )
    }
}


indirect enum MiniAppCommandValue: Equatable, Sendable {
    case string(String)
    case number(Double)
    case integer(Int64)
    case boolean(Bool)
    case array([MiniAppCommandValue])
    case object([String: MiniAppCommandValue])
    case null

    static func fromJSON(_ value: Any, schema: [String: Any]? = nil) -> MiniAppCommandValue? {
        if value is NSNull { return .null }
        if let value = value as? String { return .string(value) }
        if let value = value as? Bool { return .boolean(value) }
        if let value = value as? Int {
            return schema?["type"] as? String == "number"
                ? .number(Double(value))
                : .integer(Int64(value))
        }
        if let value = value as? Int64 {
            return schema?["type"] as? String == "number"
                ? .number(Double(value))
                : .integer(value)
        }
        if let value = value as? NSNumber {
            let number = value.doubleValue
            if schema?["type"] as? String == "integer", number.rounded() == number {
                return .integer(Int64(number))
            }
            return .number(number)
        }
        if let values = value as? [Any] {
            let itemSchema = schema?["items"] as? [String: Any]
            var result: [MiniAppCommandValue] = []
            for rawValue in values {
                guard let item = fromJSON(rawValue, schema: itemSchema) else { return nil }
                result.append(item)
            }
            return .array(result)
        }
        if let values = value as? [String: Any] {
            let properties = schema?["properties"] as? [String: Any] ?? [:]
            var result: [String: MiniAppCommandValue] = [:]
            for (key, rawValue) in values {
                let childSchema = properties[key] as? [String: Any]
                guard let childValue = fromJSON(rawValue, schema: childSchema) else { return nil }
                result[key] = childValue
            }
            return .object(result)
        }
        return nil
    }

    var jsonObject: Any {
        switch self {
        case .string(let value): return value
        case .number(let value): return value
        case .integer(let value): return value
        case .boolean(let value): return value
        case .array(let values): return values.map { $0.jsonObject }
        case .object(let values): return values.mapValues { $0.jsonObject }
        case .null: return NSNull()
        }
    }

    var displayText: String {
        switch self {
        case .string(let value):
            return value
        case .number(let value):
            return String(value)
        case .integer(let value):
            return String(value)
        case .boolean(let value):
            return value ? "true" : "false"
        case .null:
            return "null"
        case .array, .object:
            guard JSONSerialization.isValidJSONObject(jsonObject),
                  let data = try? JSONSerialization.data(withJSONObject: jsonObject, options: [.sortedKeys]),
                  let text = String(data: data, encoding: .utf8)
            else { return "" }
            return text
        }
    }
}

enum MiniAppParsedToolCommand: Equatable {
    case call(tool: MiniAppToolContract, arguments: [String: MiniAppCommandValue])
    case form(tool: MiniAppToolContract, initial: [String: MiniAppCommandValue])
    case text(String)
}

enum MiniAppToolCommandError: LocalizedError, Equatable {
    case unknownTool(String)

    var errorDescription: String? {
        switch self {
        case .unknownTool(let name):
            return "当前插件没有 /\(name) Tool"
        }
    }
}

func miniAppSchemaDefaults(_ schema: [String: Any]?) -> MiniAppCommandValue? {
    guard let schema else { return nil }
    if schema.keys.contains("default") {
        return MiniAppCommandValue.fromJSON(schema["default"] ?? NSNull(), schema: schema)
    }
    if schema["type"] as? String == "object" {
        let properties = schema["properties"] as? [String: Any] ?? [:]
        var result: [String: MiniAppCommandValue] = [:]
        for (key, rawChild) in properties {
            guard let childSchema = rawChild as? [String: Any],
                  let childDefault = miniAppSchemaDefaults(childSchema)
            else { continue }
            result[key] = childDefault
        }
        return .object(result)
    }
    if schema["type"] as? String == "array" { return .array([]) }
    if schema["type"] as? String == "boolean" { return .boolean(false) }
    return nil
}

func validateMiniAppSchemaValue(
    _ schema: [String: Any]?,
    value: MiniAppCommandValue?,
    path: String = "$"
) -> [String] {
    guard let schema else { return [] }
    var errors: [String] = []

    if let candidates = schema["enum"] as? [Any] {
        let matches = candidates.contains { candidate in
            MiniAppCommandValue.fromJSON(candidate, schema: schema) == value
        }
        if !matches {
            return ["\(path) 必须是枚举中的一个值"]
        }
    }

    let type = schema["type"] as? String
    if type == "object" || schema["properties"] != nil {
        guard let value, case .object(let object) = value else {
            return ["\(path) 必须是对象"]
        }
        for required in schema["required"] as? [String] ?? [] {
            let child = object[required]
            if child == nil || child == .null || child == .string("") {
                errors.append("\(path).\(required) 是必填项")
            }
        }
        let properties = schema["properties"] as? [String: Any] ?? [:]
        if schema["additionalProperties"] as? Bool == false {
            for key in object.keys where properties[key] == nil {
                errors.append("\(path).\(key) 不允许")
            }
        }
        for (key, rawChild) in properties {
            guard let child = object[key],
                  let childSchema = rawChild as? [String: Any]
            else { continue }
            errors.append(contentsOf: validateMiniAppSchemaValue(
                childSchema,
                value: child,
                path: "\(path).\(key)"
            ))
        }
    } else if type == "array" {
        guard let value, case .array(let values) = value else {
            return ["\(path) 必须是数组"]
        }
        let itemSchema = schema["items"] as? [String: Any]
        for (index, item) in values.enumerated() {
            errors.append(contentsOf: validateMiniAppSchemaValue(
                itemSchema,
                value: item,
                path: "\(path)[\(index)]"
            ))
        }
    } else if type == "string" {
        guard let value, case .string(let text) = value else {
            return ["\(path) 必须是字符串"]
        }
        if let minLength = schema["minLength"] as? Int, text.count < minLength {
            errors.append("\(path) 长度不能小于 \(minLength)")
        }
        if let maxLength = schema["maxLength"] as? Int, text.count > maxLength {
            errors.append("\(path) 长度不能超过 \(maxLength)")
        }
    } else if type == "boolean" {
        guard let value, case .boolean = value else { return ["\(path) 必须是布尔值"] }
    } else if type == "number" {
        guard let value else { return ["\(path) 必须是数字"] }
        switch value {
        case .number, .integer:
            break
        default:
            return ["\(path) 必须是数字"]
        }
    } else if type == "integer" {
        guard let value else { return ["\(path) 必须是整数"] }
        switch value {
        case .integer:
            break
        case .number(let number) where number.rounded() == number:
            break
        default:
            return ["\(path) 必须是整数"]
        }
    }
    return errors
}

func miniAppEffectiveApproval(catalog: String, page: String) -> String {
    func rank(_ value: String) -> Int {
        switch value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "none":
            return 0
        case "destructive":
            return 2
        default:
            return 1
        }
    }

    let strongest = max(rank(catalog), rank(page))
    if strongest >= 2 { return "destructive" }
    if strongest >= 1 { return "required" }
    return "none"
}

func parseMiniAppToolCommand(
    _ input: String,
    tools: [MiniAppToolContract]
) throws -> MiniAppParsedToolCommand {
    guard input.hasPrefix("/") else { return .text(input) }

    let first = input.index(after: input.startIndex)
    let separator = input[first...].firstIndex(where: { $0.isWhitespace })
    let nameEnd = separator ?? input.endIndex
    let name = String(input[first..<nameEnd])
    let remainder: String
    if let separator {
        let remainderStart = input.index(after: separator)
        remainder = String(input[remainderStart..<input.endIndex])
    } else {
        remainder = ""
    }

    guard let tool = tools.first(where: { $0.name == name }) else {
        throw MiniAppToolCommandError.unknownTool(name)
    }
    let properties = tool.inputSchemaObject?["properties"] as? [String: Any] ?? [:]
    if properties.isEmpty {
        return .call(tool: tool, arguments: [:])
    }
    if properties.count == 1,
       let only = properties.first,
       let fieldSchema = only.value as? [String: Any],
       fieldSchema["type"] as? String == "string" {
        return .call(tool: tool, arguments: [only.key: .string(remainder)])
    }
    if let defaults = miniAppSchemaDefaults(tool.inputSchemaObject),
       case .object(let initial) = defaults {
        return .form(tool: tool, initial: initial)
    }
    return .form(tool: tool, initial: [:])
}

private struct MiniAppSchemaFieldDefinition: Identifiable {
    let name: String
    let schema: [String: Any]
    let required: Bool
    var id: String { name }
}

private struct MiniAppPendingToolCall: Identifiable {
    let id = UUID()
    let tool: MiniAppToolContract
    let arguments: [String: MiniAppCommandValue]
}

private func miniAppSchemaEnumOptions(_ schema: [String: Any]) -> [MiniAppCommandValue] {
    (schema["enum"] as? [Any] ?? []).compactMap {
        MiniAppCommandValue.fromJSON($0, schema: schema)
    }
}

private func miniAppCommandValue(
    schema: [String: Any],
    raw: String
) -> MiniAppCommandValue {
    let enumOptions = miniAppSchemaEnumOptions(schema)
    if let match = enumOptions.first(where: { $0.displayText == raw }) {
        return match
    }
    switch schema["type"] as? String {
    case "number":
        return Double(raw).map(MiniAppCommandValue.number) ?? .string(raw)
    case "integer":
        return Int64(raw).map(MiniAppCommandValue.integer) ?? .string(raw)
    case "array", "object":
        guard let data = raw.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let value = MiniAppCommandValue.fromJSON(object, schema: schema)
        else { return .string(raw) }
        return value
    default:
        return .string(raw)
    }
}

private struct MiniAppToolCommandPanel: View {
    let plugin: MarketplacePlugin
    let model: MarketplaceModel

    @State private var input = ""
    @State private var activeFormTool: MiniAppToolContract?
    @State private var formValues: [String: MiniAppCommandValue] = [:]
    @State private var message: String?
    @State private var running = false
    @State private var pendingCall: MiniAppPendingToolCall?

    private var fields: [MiniAppSchemaFieldDefinition] {
        guard let tool = activeFormTool,
              let properties = tool.inputSchemaObject?["properties"] as? [String: Any]
        else { return [] }
        let required = Set(tool.inputSchemaObject?["required"] as? [String] ?? [])
        return properties.compactMap { key, rawSchema in
            guard let schema = rawSchema as? [String: Any] else { return nil }
            return MiniAppSchemaFieldDefinition(
                name: key,
                schema: schema,
                required: required.contains(key)
            )
        }
        .sorted { $0.name < $1.name }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                TextField("/tool 参数", text: $input)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("miniapp-tool-command-input")
                Button("运行") {
                    submitInput()
                }
                .buttonStyle(.bordered)
                .disabled(running || input.isEmpty)
                .accessibilityIdentifier("miniapp-tool-command-run")
            }

            if let tool = activeFormTool {
                VStack(alignment: .leading, spacing: 8) {
                    Text(tool.title ?? "/\(tool.name)")
                        .font(.subheadline.weight(.semibold))
                    if !tool.description.isEmpty {
                        Text(tool.description)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    ForEach(fields) { field in
                        fieldEditor(field)
                    }
                    Button("执行 /\(tool.name)") {
                        submitForm(tool)
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(running)
                    .accessibilityIdentifier("miniapp-tool-command-form-submit")
                }
                .padding(10)
                .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 10))
                .accessibilityIdentifier("miniapp-tool-command-form")
            }

            if let message {
                Text(message)
                    .font(.caption)
                    .foregroundColor(
                        message.contains("失败") || message.contains("必须") || message.contains("没有")
                            ? .red
                            : .secondary
                    )
                    .accessibilityIdentifier("miniapp-tool-command-status")
            }
        }
        .alert(item: $pendingCall) { call in
            let destructive = call.tool.approval == "destructive"
            return Alert(
                title: Text("允许调用 \(call.tool.name)？"),
                message: Text(destructive ? "该操作可能产生破坏性修改。" : "该操作会修改小程序或后台状态。"),
                primaryButton: .cancel(Text("取消")),
                secondaryButton: .default(Text("允许")) {
                    Task { await execute(call.tool, arguments: call.arguments) }
                }
            )
        }
    }

    @ViewBuilder
    private func fieldEditor(_ field: MiniAppSchemaFieldDefinition) -> some View {
        let label = field.name + (field.required ? " *" : "")
        let enumOptions = miniAppSchemaEnumOptions(field.schema)
        VStack(alignment: .leading, spacing: 4) {
            if !enumOptions.isEmpty {
                Picker(label, selection: textBinding(field)) {
                    Text("请选择").tag("")
                    ForEach(Array(enumOptions.enumerated()), id: \.offset) { _, option in
                        Text(option.displayText).tag(option.displayText)
                    }
                }
                .pickerStyle(.menu)
            } else if field.schema["type"] as? String == "boolean" {
                Toggle(label, isOn: boolBinding(field))
            } else {
                Text(label)
                    .font(.caption.weight(.medium))
                TextField(
                    field.schema["description"] as? String ?? label,
                    text: textBinding(field),
                    axis: .vertical
                )
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .textFieldStyle(.roundedBorder)
            }
            if let description = field.schema["description"] as? String, !description.isEmpty {
                Text(description)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityIdentifier("miniapp-tool-command-field-\(field.name)")
    }

    private func textBinding(_ field: MiniAppSchemaFieldDefinition) -> Binding<String> {
        Binding(
            get: { formValues[field.name]?.displayText ?? "" },
            set: { raw in
                if raw.isEmpty, field.schema["enum"] != nil {
                    formValues.removeValue(forKey: field.name)
                } else if raw.isEmpty, field.schema["type"] as? String != "string" {
                    formValues.removeValue(forKey: field.name)
                } else {
                    formValues[field.name] = miniAppCommandValue(schema: field.schema, raw: raw)
                }
            }
        )
    }

    private func boolBinding(_ field: MiniAppSchemaFieldDefinition) -> Binding<Bool> {
        Binding(
            get: {
                guard case .boolean(let value) = formValues[field.name] else { return false }
                return value
            },
            set: { formValues[field.name] = .boolean($0) }
        )
    }

    private func submitInput() {
        do {
            switch try parseMiniAppToolCommand(input, tools: plugin.tools) {
            case .text:
                activeFormTool = nil
                formValues = [:]
                message = "请输入以 / 开头的 Tool 命令"
            case .call(let tool, let arguments):
                activeFormTool = nil
                formValues = [:]
                requestExecution(tool, arguments: arguments)
            case .form(let tool, let initial):
                activeFormTool = tool
                formValues = initial
                message = "请填写 /\(tool.name) 参数"
            }
        } catch {
            activeFormTool = nil
            formValues = [:]
            message = error.localizedDescription
        }
    }

    private func submitForm(_ tool: MiniAppToolContract) {
        requestExecution(tool, arguments: formValues)
    }

    private func requestExecution(
        _ tool: MiniAppToolContract,
        arguments: [String: MiniAppCommandValue]
    ) {
        let errors = validateMiniAppSchemaValue(
            tool.inputSchemaObject,
            value: .object(arguments)
        )
        guard errors.isEmpty else {
            message = errors.joined(separator: "\n")
            return
        }
        if tool.approval == "none" {
            Task { await execute(tool, arguments: arguments) }
        } else {
            pendingCall = MiniAppPendingToolCall(tool: tool, arguments: arguments)
        }
    }

    private func execute(
        _ tool: MiniAppToolContract,
        arguments: [String: MiniAppCommandValue]
    ) async {
        running = true
        defer { running = false }
        do {
            let payload = arguments.mapValues { $0.jsonObject }
            _ = try await model.callActiveWebMcpTool(
                pluginId: plugin.pluginId,
                name: tool.name,
                arguments: payload
            )
            message = "已执行 /\(tool.name)"
        } catch {
            message = "/\(tool.name) 执行失败：\(error.localizedDescription)"
        }
    }
}

struct MiniAppWebMcpSurface: View {
    let plugin: MarketplacePlugin
    let model: MarketplaceModel
    let localHtmlOverride: String?
    let generatedDraftReviewOverride: LocalMiniAppDraftReview?
    let onClose: (() -> Void)?

    init(
        plugin: MarketplacePlugin,
        model: MarketplaceModel,
        localHtmlOverride: String? = nil,
        generatedDraftReviewOverride: LocalMiniAppDraftReview? = nil,
        onClose: (() -> Void)? = nil
    ) {
        self.plugin = plugin
        self.model = model
        self.localHtmlOverride = localHtmlOverride
        self.generatedDraftReviewOverride = generatedDraftReviewOverride
        self.onClose = onClose
    }

    @Environment(\.dismiss) private var dismiss
    @State private var status = "正在解析本地 WebMCP…"
    @State private var localHtml: String?
    @State private var localSource: LocalMiniAppSource?
    @State private var webMcpPlugin: MarketplacePlugin?
    @State private var sourceResolved = false
    @State private var generatedDraftReview: LocalMiniAppDraftReview?
    @State private var generatedInstallBusy = false
    @State private var generatedInstallMessage: String?
    @State private var generatedInstallTask: Task<Void, Never>?
    @State private var generatedInstallCompleted = false

    private var previewDigest: String? {
        localHtmlOverride.map(generatedMiniAppPreviewDigest)
    }

    private var sourceGenerationKey: String {
        miniAppSourceGenerationKey(
            accountKey: model.settingsNoticeAccountKey,
            pluginId: plugin.pluginId,
            previewDigest: previewDigest
        )
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Button("返回") {
                    if let onClose {
                        onClose()
                    } else {
                        dismiss()
                    }
                }
                .accessibilityIdentifier("miniapp-webmcp-close")

                VStack(alignment: .leading, spacing: 2) {
                    Text(plugin.displayName).font(.headline)
                    Text(status)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("miniapp-webmcp-surface")
                }
                Spacer()
            }
            .padding(12)

            if plugin.pluginId == GlobalDharmaCommerceModel.miniAppId {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Image(systemName: model.globalDharmaCommerce.accessAllowed ? "checkmark.seal.fill" : "lock.fill")
                            .foregroundStyle(model.globalDharmaCommerce.accessAllowed ? .green : .secondary)
                        Text(model.globalDharmaCommerce.message)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Spacer()
                    }
                    .accessibilityIdentifier("global-dharma-entitlement-status")

                    VStack(alignment: .leading, spacing: 8) {
                        if model.globalDharmaCommerce.accessAllowed {
                            Label("本地转经轮权限有效", systemImage: "checkmark.seal")
                                .font(.subheadline.weight(.semibold))
                                .accessibilityIdentifier("global-dharma-entitlement-allowed")
                        } else {
                            HStack(spacing: 10) {
                                Button("\(model.globalDharmaCommerce.monthlyPriceLabel) 开通") {
                                    Task { await model.globalDharmaCommerce.purchaseMonthly() }
                                }
                                .buttonStyle(.bordered)
                                .disabled(!model.globalDharmaCommerce.canBuyMonthly)
                                .accessibilityIdentifier("global-dharma-buy-monthly")

                                Button("\(model.globalDharmaCommerce.lifetimePriceLabel) 买断") {
                                    Task { await model.globalDharmaCommerce.purchaseLifetime() }
                                }
                                .buttonStyle(.borderedProminent)
                                .disabled(!model.globalDharmaCommerce.canBuyLifetime)
                                .accessibilityIdentifier("global-dharma-buy-lifetime")
                            }
                        }

                        Button("恢复购买") {
                            Task { await model.globalDharmaCommerce.restorePurchases() }
                        }
                        .buttonStyle(.bordered)
                        .disabled(model.globalDharmaCommerce.busy)
                        .accessibilityIdentifier("global-dharma-restore-purchase")
                    }
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 10)
            }

            if sourceResolved, let webMcpPlugin {
                VStack(spacing: 0) {
                    if localHtmlOverride == nil {
                        MiniAppToolCommandPanel(plugin: webMcpPlugin, model: model)
                            .padding(.horizontal, 12)
                            .padding(.bottom, 10)

                        Divider()
                    }

                    if localHtmlOverride != nil {
                        generatedMiniAppReviewPanel
                            .padding(.horizontal, 12)
                            .padding(.bottom, 10)
                        Divider()
                    }

                    if MarketplaceModel.requiresVerifiedLocalMiniAppPackage(pluginId: webMcpPlugin.pluginId),
                       localHtmlOverride == nil,
                       localSource == nil
                    {
                        VStack(spacing: 10) {
                            Image(systemName: "exclamationmark.shield")
                                .font(.title2)
                            Text("全球法布施本地安装包不可用")
                                .font(.headline)
                            Text("已停止 Hosted WebMCP 回退。请重新安装或修复已验证的 .fabapp 包；免费离线功能不会切换到远程运行时。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .multilineTextAlignment(.center)
                        }
                        .padding(24)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .accessibilityIdentifier("global-dharma-local-package-required")
                    } else {
                        MiniAppWebView(
                            plugin: webMcpPlugin,
                            model: model,
                            localHtml: localHtml,
                            localSource: localSource,
                            dataStoreIdentifier: localHtmlOverride == nil
                                ? model.miniAppWebsiteDataStoreIdentifier(pluginId: webMcpPlugin.pluginId)
                                : nil,
                            onRuntimeRevoked: {
                                if let onClose {
                                    onClose()
                                } else {
                                    dismiss()
                                }
                            },
                            sourceResolved: true,
                            status: $status
                        )
                        .id(
                            previewDigest.map { "miniapp-preview-\($0)" }
                                ?? model.miniAppWebsiteDataStoreIdentifier(pluginId: webMcpPlugin.pluginId)?.uuidString
                                ?? "miniapp-ephemeral-logged-out"
                        )
                    }
                }
            } else {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .accessibilityIdentifier("miniapp-webmcp-resolving")
            }
        }
        .accessibilityElement(children: .contain)
        .task(id: sourceGenerationKey) {
            // Opening a free/offline Mini App must never preflight paid
            // entitlement. Protected capability execution, purchase and
            // restore own their network checks.
            let resolvedPlugin = await model.webMcpPlugin(for: plugin)
            webMcpPlugin = resolvedPlugin
            if let localHtmlOverride {
                localSource = nil
                generatedDraftReview = nil
                generatedInstallMessage = nil
                generatedInstallCompleted = false
                if let controlledReview = generatedDraftReviewOverride {
                    if controlledReview.transaction.reservedAppId == plugin.pluginId,
                       generatedMiniAppReviewMatchesPreview(controlledReview, html: localHtmlOverride)
                    {
                        localHtml = localHtmlOverride
                        generatedDraftReview = controlledReview
                    } else {
                        localHtml = nil
                        generatedInstallMessage = "受控 Agent 草稿摘要与预览字节不一致，已拒绝预览和安装。"
                    }
                } else {
                    let hardened = hardenGeneratedMiniAppDocument(localHtmlOverride)
                    localHtml = hardened
                    do {
                        let transaction = try await model.beginGeneratedMiniAppDraft()
                        generatedDraftReview = try await model.prepareGeneratedMiniAppDraft(
                            transaction,
                            displayName: plugin.displayName,
                            requestedCapabilities: [],
                            files: [
                                "index.html": LocalMiniAppDraftFile(
                                    mime: "text/html",
                                    data: Data(hardened.utf8)
                                )
                            ]
                        )
                    } catch {
                        generatedInstallMessage = "草稿准备失败：\(error.localizedDescription)"
                    }
                }
            } else {
                localHtml = nil
                localSource = await model.loadLocalMiniAppSource(plugin: resolvedPlugin)
            }
            sourceResolved = true
            status = (localHtml == nil && localSource == nil)
                ? "正在加载 Hosted WebMCP…"
                : "正在加载本地 WebMCP…"
        }
        .onDisappear {
            generatedInstallTask?.cancel()
            generatedInstallTask = nil
            guard !generatedInstallCompleted, let review = generatedDraftReview else { return }
            Task {
                try? await model.cancelGeneratedMiniAppDraft(review)
            }
        }
    }

    @ViewBuilder
    private var generatedMiniAppReviewPanel: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Label("一次性安全预览", systemImage: "eye")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                if let review = generatedDraftReview {
                    Text(ByteCountFormatter.string(
                        fromByteCount: Int64(review.totalBytes),
                        countStyle: .file
                    ))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
                }
            }

            if let review = generatedDraftReview {
                Text("安装前请核对源文件、权限和大小。预览不会继承已安装 App 的 Cookie、存储或 Native/MCP 权限。")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                DisclosureGroup("查看将安装的源文件") {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(review.files, id: \.path) { file in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(file.path)
                                    .font(.caption.monospaced())
                                Text("\(file.mime) • \(file.bytes) bytes • \(file.sha256.prefix(16))…")
                                    .font(.caption2.monospaced())
                                    .foregroundStyle(.secondary)
                            }
                        }
                        if let source = localHtml {
                            Divider()
                            ScrollView {
                                Text(source)
                                    .font(.caption2.monospaced())
                                    .textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            .frame(maxHeight: 180)
                            .accessibilityIdentifier("miniapp-generated-source-review")
                        }
                    }
                }

                VStack(alignment: .leading, spacing: 3) {
                    Text("权限")
                        .font(.caption.weight(.semibold))
                    if review.requestedCapabilities.isEmpty {
                        Text("无额外 Host / Shell / MCP / Network 权限")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(review.requestedCapabilities, id: \.self) { capability in
                            Text("• \(capability)")
                                .font(.caption.monospaced())
                        }
                    }
                }

                Button {
                    generatedInstallTask?.cancel()
                    generatedInstallTask = Task { await installGeneratedDraft(review) }
                } label: {
                    if generatedInstallBusy {
                        ProgressView()
                    } else {
                        Label("明确安装此版本", systemImage: "square.and.arrow.down")
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(generatedInstallBusy)
                .accessibilityIdentifier("miniapp-generated-explicit-install")
            } else if generatedInstallMessage == nil {
                ProgressView("正在生成不可变草稿摘要…")
                    .font(.caption)
            }

            if let generatedInstallMessage {
                Text(generatedInstallMessage)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("miniapp-generated-install-status")
            }
        }
        .padding(10)
        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 12))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("miniapp-generated-review")
    }

    @MainActor
    private func installGeneratedDraft(_ review: LocalMiniAppDraftReview) async {
        guard !generatedInstallBusy,
              let preview = localHtml,
              generatedMiniAppReviewMatchesPreview(review, html: preview)
        else {
            generatedInstallMessage = "草稿摘要已变化，必须重新生成并再次明确批准。"
            return
        }
        let accountKey = model.settingsNoticeAccountKey
        generatedInstallBusy = true
        defer { generatedInstallBusy = false }
        do {
            try Task.checkCancellation()
            let approval = try await model.approveGeneratedMiniAppDraft(review)
            try Task.checkCancellation()
            guard accountKey == model.settingsNoticeAccountKey,
                  generatedDraftReview?.transaction.transactionId == review.transaction.transactionId,
                  generatedDraftReview?.draftDigest == review.draftDigest,
                  let currentPreview = localHtml,
                  generatedMiniAppReviewMatchesPreview(review, html: currentPreview)
            else {
                throw CancellationError()
            }
            let signature = try await model.signApprovedGeneratedMiniApp(approval)
            try Task.checkCancellation()
            guard accountKey == model.settingsNoticeAccountKey,
                  generatedDraftReview?.draftDigest == review.draftDigest
            else {
                throw CancellationError()
            }
            let installed = try await model.installApprovedGeneratedMiniApp(
                signatureBase64URL: signature,
                approval: approval
            )
            try Task.checkCancellation()
            generatedInstallCompleted = true
            generatedInstallMessage = "已安装 \(installed.pluginId) \(installed.version)。"
        } catch is CancellationError {
            try? await model.cancelGeneratedMiniAppDraft(review)
            generatedInstallMessage = "已取消，未安装此草稿。"
        } catch {
            generatedInstallMessage = "安装失败：\(error.localizedDescription)"
        }
    }
}

private struct MiniAppWebView: UIViewRepresentable {
    let plugin: MarketplacePlugin
    let model: MarketplaceModel
    let localHtml: String?
    let localSource: LocalMiniAppSource?
    let dataStoreIdentifier: UUID?
    let onRuntimeRevoked: () -> Void
    let sourceResolved: Bool
    @Binding var status: String

    func makeCoordinator() -> Coordinator {
        Coordinator(
            plugin: plugin,
            model: model,
            status: $status,
            onRuntimeRevoked: onRuntimeRevoked
        )
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        if let dataStoreIdentifier {
            configuration.websiteDataStore = WKWebsiteDataStore(forIdentifier: dataStoreIdentifier)
        } else {
            configuration.websiteDataStore = .nonPersistent()
        }
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.userContentController.add(context.coordinator, name: webMcpMessageHandler)
        configuration.userContentController.add(
            context.coordinator,
            name: miniAppNativeContextMenuHandler
        )
        configuration.userContentController.addUserScript(
            WKUserScript(
                source: miniAppNativeContextMenuJavaScript,
                injectionTime: .atDocumentEnd,
                forMainFrameOnly: true
            )
        )

        let webView = WKWebView(frame: .zero, configuration: configuration)
        webView.accessibilityIdentifier = "miniapp-webmcp-webview"
        webView.navigationDelegate = context.coordinator
        webView.allowsBackForwardNavigationGestures = false
        webView.isInspectable = false
        context.coordinator.webView = webView
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        guard sourceResolved else { return }

        if let localSource {
            let key = "local-package:\(plugin.pluginId):\(localSource.version):\(localSource.artifactSha256)"
            guard context.coordinator.loadedSourceKey != key else { return }
            context.coordinator.loadedSourceKey = key
            let bridgeSession = context.coordinator.prepareLocalBridgeSession()
            context.coordinator.activeLocalRootURL = localSource.rootURL
            context.coordinator.activeLocalSource = localSource
            webView.configuration.userContentController.removeAllUserScripts()
            webView.configuration.userContentController.addUserScript(
                WKUserScript(
                    source: miniAppNativeContextMenuJavaScript,
                    injectionTime: .atDocumentEnd,
                    forMainFrameOnly: true
                )
            )
            webView.configuration.userContentController.addUserScript(
                WKUserScript(
                    source: webMcpBootstrapJavaScript(plugin: plugin, bridgeSession: bridgeSession),
                    injectionTime: .atDocumentStart,
                    forMainFrameOnly: true
                )
            )
            webView.loadFileURL(
                localSource.entryURL,
                allowingReadAccessTo: localSource.rootURL
            )
            return
        }

        if let localHtml {
            let digest = generatedMiniAppPreviewDigest(localHtml)
            let key = "local-preview:\(plugin.pluginId):\(digest)"
            guard context.coordinator.loadedSourceKey != key else { return }
            context.coordinator.loadedSourceKey = key
            context.coordinator.activeLocalRootURL = nil
            context.coordinator.activeLocalSource = nil
            context.coordinator.previewMode = true
            context.coordinator.disposeLocalBridgeSession()
            webView.configuration.userContentController.removeAllUserScripts()
            webView.configuration.userContentController.addUserScript(
                WKUserScript(
                    source: miniAppNativeContextMenuJavaScript,
                    injectionTime: .atDocumentEnd,
                    forMainFrameOnly: true
                )
            )
            let baseURL = URL(
                string: "https://\(generatedMiniAppPreviewOriginHost)/drafts/\(digest)/"
            )
            webView.loadHTMLString(localHtml, baseURL: baseURL)
            return
        }

        context.coordinator.previewMode = false

        let key = "hosted:\(plugin.pluginId)"
        guard context.coordinator.loadedSourceKey != key else { return }
        context.coordinator.loadedSourceKey = key
        context.coordinator.activeLocalRootURL = nil
        context.coordinator.activeLocalSource = nil
        webView.configuration.userContentController.removeAllUserScripts()
        webView.configuration.userContentController.addUserScript(
            WKUserScript(
                source: miniAppNativeContextMenuJavaScript,
                injectionTime: .atDocumentEnd,
                forMainFrameOnly: true
            )
        )
        _ = context.coordinator.prepareLocalBridgeSession()
        var components = URLComponents()
        components.scheme = "https"
        components.host = webMcpOriginHost
        components.path = "/miniapps/\(plugin.pluginId)/"
        if let url = components.url {
            webView.load(URLRequest(url: url))
        }
    }

    static func dismantleUIView(_ webView: WKWebView, coordinator: Coordinator) {
        webView.stopLoading()
        webView.navigationDelegate = nil
        webView.configuration.userContentController.removeScriptMessageHandler(forName: webMcpMessageHandler)
        webView.configuration.userContentController.removeScriptMessageHandler(
            forName: miniAppNativeContextMenuHandler
        )
        coordinator.disposeLocalBridgeSession()
        coordinator.activeLocalRootURL = nil
        coordinator.activeLocalSource = nil
        webView.configuration.userContentController.removeAllUserScripts()
        webView.loadHTMLString("", baseURL: nil)
        coordinator.webView = nil
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        let plugin: MarketplacePlugin
        let model: MarketplaceModel
        @Binding private var status: String
        weak var webView: WKWebView?
        private let nativeContextActions = MiniAppNativeContextActions()
        var previewMode = false
        var loadedSourceKey: String?
        var activeLocalRootURL: URL?
        var activeLocalSource: LocalMiniAppSource?
        private let toolByName: [String: MiniAppToolContract]
        private var activeBridgeSession: MiniAppWebMcpBridgeSession?
        private var activeBridgeAccountKey: String?
        private var activeRegistrySnapshot: ActiveMiniAppWebMcpSessionRegistry.Snapshot?
        private var bridgeGeneration: UInt64 = 0
        private var pendingRequests: [String: Task<Void, Never>] = [:]
        private let onRuntimeRevoked: () -> Void

        init(
            plugin: MarketplacePlugin,
            model: MarketplaceModel,
            status: Binding<String>,
            onRuntimeRevoked: @escaping () -> Void
        ) {
            self.plugin = plugin
            self.model = model
            self.toolByName = Dictionary(uniqueKeysWithValues: plugin.tools.map { ($0.name, $0) })
            self.onRuntimeRevoked = onRuntimeRevoked
            _status = status
        }

        func prepareLocalBridgeSession() -> MiniAppWebMcpBridgeSession {
            disposeLocalBridgeSession()
            bridgeGeneration = bridgeGeneration == UInt64.max ? 1 : bridgeGeneration + 1
            activeBridgeAccountKey = model.settingsNoticeAccountKey.trimmingCharacters(in: .whitespacesAndNewlines)
            let session = MiniAppWebMcpBridgeSession.fresh(
                plugin: plugin,
                generation: bridgeGeneration
            )
            activeBridgeSession = session
            return session
        }

        func disposeLocalBridgeSession() {
            if let snapshot = activeRegistrySnapshot {
                ActiveMiniAppWebMcpSessionRegistry.shared.unregister(expectedSnapshot: snapshot)
            }
            activeRegistrySnapshot = nil
            for task in pendingRequests.values { task.cancel() }
            pendingRequests.removeAll()
            activeBridgeSession = nil
            activeBridgeAccountKey = nil
        }

        func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation?) {
            status = "正在加载 WebMCP…"
        }

        private func failActiveBridge(_ message: String) {
            disposeLocalBridgeSession()
            loadedSourceKey = nil
            status = message
        }

        func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
            guard self.webView === webView else { return }
            failActiveBridge("WebMCP 页面进程已终止，当前 Tool 会话已失效")
        }

        func webView(
            _ webView: WKWebView,
            didFail navigation: WKNavigation?,
            withError error: Error
        ) {
            guard self.webView === webView else { return }
            failActiveBridge("WebMCP 页面加载失败，当前 Tool 会话已失效：\(error.localizedDescription)")
        }

        func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation?,
            withError error: Error
        ) {
            guard self.webView === webView else { return }
            failActiveBridge("WebMCP 页面加载失败，当前 Tool 会话已失效：\(error.localizedDescription)")
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation?) {
            guard isTrustedWebMcpBridgeURL(webView.url, localRootURL: activeLocalRootURL) else {
                disposeLocalBridgeSession()
                status = "WebMCP 页面已打开"
                return
            }

            if webView.url?.host == webMcpOriginHost {
                guard let session = activeBridgeSession else {
                    status = "WebMCP 页面已打开"
                    return
                }
                let bootstrap = webMcpBootstrapJavaScript(plugin: plugin, bridgeSession: session)
                webView.evaluateJavaScript(bootstrap) { [weak self, weak webView] _, error in
                    guard let self, let webView else { return }
                    guard error == nil,
                          self.activeBridgeSession == session,
                          webView.url?.host == webMcpOriginHost
                    else {
                        self.status = "WebMCP 页面已打开"
                        return
                    }
                    self.probeBridge(in: webView, local: false)
                }
                return
            }

            probeBridge(in: webView, local: true)
        }

        private func probeBridge(in webView: WKWebView, local: Bool) {
            let probe = """
            (() => {
              const tools = window.__fabushiWebMcp?.list?.() || [];
              return JSON.stringify({ ready: tools.length > 0, tools: tools.map((tool) => tool.name) });
            })()
            """
            webView.evaluateJavaScript(probe) { [weak self, weak webView] value, _ in
                guard let self else { return }
                let result = value as? String ?? ""
                guard result.contains("\"ready\":true") else {
                    self.status = "WebMCP 页面已打开"
                    return
                }
                self.status = local ? "本地 WebMCP 已连接" : "WebMCP 已连接"
            }
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor navigationAction: WKNavigationAction,
            decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void
        ) {
            if previewMode {
                let url = navigationAction.request.url
                let allowed = url?.scheme == "https"
                    && url?.host == generatedMiniAppPreviewOriginHost
                decisionHandler(allowed ? .allow : .cancel)
                return
            }
            guard isTrustedWebMcpBridgeURL(
                navigationAction.request.url,
                localRootURL: activeLocalRootURL
            ) else {
                decisionHandler(.cancel)
                return
            }
            decisionHandler(.allow)
        }

        func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
            if message.name == miniAppNativeContextMenuHandler,
               let webView,
               message.frameInfo.isMainFrame,
               let body = message.body as? [String: Any] {
                let currentURL = webView.url
                let trusted = previewMode
                    ? (currentURL?.scheme == "https"
                        && currentURL?.host == generatedMiniAppPreviewOriginHost)
                    : isTrustedWebMcpBridgeURL(
                        currentURL,
                        localRootURL: activeLocalRootURL
                    )
                if trusted {
                    nativeContextActions.present(payload: body, from: webView)
                }
                return
            }

            guard message.name == webMcpMessageHandler,
                  let webView,
                  isTrustedWebMcpBridgeURL(webView.url, localRootURL: activeLocalRootURL),
                  let body = message.body as? [String: Any],
                  let pluginInstanceId = body["pluginInstanceId"] as? String,
                  let nonce = body["nonce"] as? String,
                  let session = activeBridgeSession,
                  session.pluginInstanceId == pluginInstanceId,
                  session.nonce == nonce,
                  (body["generation"] as? NSNumber)?.uint64Value == session.generation,
                  message.frameInfo.isMainFrame
            else { return }

            if body["kind"] as? String == "ready" {
                probeBridge(in: webView, local: webView.url?.host == localWebMcpOriginHost)
                return
            }

            if body["kind"] as? String == "dispose" {
                disposeLocalBridgeSession()
                return
            }

            if body["kind"] as? String == "pageTools" {
                registerPageTools(
                    body["tools"] as? [[String: Any]] ?? [],
                    session: session,
                    webView: webView
                )
                return
            }

            guard body["kind"] as? String == "call",
                  let requestId = body["requestId"] as? String,
                  !requestId.isEmpty,
                  pendingRequests[requestId] == nil,
                  let name = body["name"] as? String,
                  session.allows(
                      pluginInstanceId: pluginInstanceId,
                      nonce: nonce,
                      generation: session.generation,
                      toolName: name
                  ),
                  let tool = toolByName[name],
                  let input = body["input"] as? [String: Any]
            else { return }

            let task = Task { @MainActor [weak self, weak webView] in
                guard let self, let webView else { return }
                defer { self.pendingRequests.removeValue(forKey: requestId) }
                do {
                    try Task.checkCancellation()
                    guard self.activeBridgeSession == session else { throw CancellationError() }
                    if tool.approval != "none",
                       !MarketplaceModel.requiresActiveLocalWebMcpSession(
                           pluginId: self.plugin.pluginId
                       )
                    {
                        guard await self.requestApproval(tool) else {
                            self.resolve(
                                webView: webView,
                                session: session,
                                requestId: requestId,
                                payload: ["ok": false, "error": "用户取消了 WebMCP Tool 调用"]
                            )
                            return
                        }
                    }
                    try Task.checkCancellation()
                    guard self.activeBridgeSession == session else { throw CancellationError() }

                    let result: Any
                    if MarketplaceModel.requiresActiveLocalWebMcpSession(
                        pluginId: self.plugin.pluginId
                    ) {
                        // Page-originated calls for offline/live Mini Apps must
                        // traverse the same Host-owned active-session boundary
                        // as Bot-originated calls. This prevents a local page
                        // from silently switching to the remote official MCP
                        // runtime and keeps protected entitlement enforcement
                        // at one trusted boundary.
                        result = try await self.model.callActiveWebMcpTool(
                            pluginId: self.plugin.pluginId,
                            name: name,
                            arguments: input
                        )
                    } else {
                        result = try await self.model.callWebMcpTool(
                            pluginId: self.plugin.pluginId,
                            name: name,
                            arguments: input
                        )
                    }

                    try Task.checkCancellation()
                    guard self.activeBridgeSession == session else { throw CancellationError() }
                    self.resolve(
                        webView: webView,
                        session: session,
                        requestId: requestId,
                        payload: ["ok": true, "result": result]
                    )
                } catch is CancellationError {
                    self.resolve(
                        webView: webView,
                        session: session,
                        requestId: requestId,
                        payload: ["ok": false, "error": "MCP App bridge disposed"]
                    )
                } catch {
                    self.resolve(
                        webView: webView,
                        session: session,
                        requestId: requestId,
                        payload: ["ok": false, "error": error.localizedDescription]
                    )
                }
            }
            pendingRequests[requestId] = task
        }

        private func registerPageTools(
            _ rawTools: [[String: Any]],
            session: MiniAppWebMcpBridgeSession,
            webView: WKWebView
        ) {
            guard rawTools.count <= 64,
                  activeBridgeSession == session,
                  let accountKey = activeBridgeAccountKey,
                  !accountKey.isEmpty,
                  accountKey == model.settingsNoticeAccountKey.trimmingCharacters(in: .whitespacesAndNewlines),
                  let source = activeLocalSource,
                  isTrustedWebMcpBridgeURL(webView.url, localRootURL: source.rootURL)
            else { return }

            let tools = rawTools
                .compactMap { MarketplaceModel.webMcpToolContract(from: $0) }
                .compactMap { pageTool -> MiniAppToolContract? in
                    guard session.grants.contains(pageTool.name),
                          let catalogTool = toolByName[pageTool.name]
                    else { return nil }

                    // The live page supplies the implementation schema, but it
                    // cannot widen a reviewed catalog schema or weaken the
                    // Host-owned approval class.
                    if let catalogSchema = catalogTool.inputSchemaJSON,
                       catalogSchema != pageTool.inputSchemaJSON {
                        return nil
                    }
                    return MiniAppToolContract(
                        name: pageTool.name,
                        description: pageTool.description,
                        approval: miniAppEffectiveApproval(
                            catalog: catalogTool.approval,
                            page: pageTool.approval
                        ),
                        title: pageTool.title,
                        inputSchemaJSON: pageTool.inputSchemaJSON
                    )
                }
            guard !tools.isEmpty else { return }

            activeRegistrySnapshot = ActiveMiniAppWebMcpSessionRegistry.shared.register(
                accountKey: accountKey,
                pluginId: plugin.pluginId,
                version: source.version,
                artifactSha256: source.artifactSha256,
                pluginInstanceId: session.pluginInstanceId,
                nonce: session.nonce,
                generation: session.generation,
                tools: tools,
                revoke: { [weak self, weak webView] in
                    guard let self, self.activeBridgeSession == session else { return }
                    if let webView, self.webView === webView {
                        webView.stopLoading()
                        webView.loadHTMLString("", baseURL: nil)
                    }
                    self.disposeLocalBridgeSession()
                    self.loadedSourceKey = nil
                    self.activeLocalRootURL = nil
                    self.activeLocalSource = nil
                    self.status = "Mini App 运行实例已关闭"
                    self.onRuntimeRevoked()
                }
            ) { [weak self, weak webView] name, arguments in
                guard let self, let webView,
                      let effectiveTool = tools.first(where: { $0.name == name })
                else { throw CancellationError() }
                return try await self.invokePageTool(
                    webView: webView,
                    session: session,
                    source: source,
                    tool: effectiveTool,
                    arguments: arguments
                )
            }
            status = "本地 WebMCP 已连接 · 页面注册 \(tools.count) 个实时 Tool"
        }

        private func invokePageTool(
            webView: WKWebView,
            session: MiniAppWebMcpBridgeSession,
            source: LocalMiniAppSource,
            tool: MiniAppToolContract,
            arguments: [String: Any]
        ) async throws -> Any {
            guard activeBridgeSession == session,
                  activeLocalSource == source,
                  isTrustedWebMcpBridgeURL(webView.url, localRootURL: source.rootURL),
                  session.grants.contains(tool.name),
                  JSONSerialization.isValidJSONObject(arguments)
            else { throw CancellationError() }

            if tool.approval != "none" {
                guard await requestApproval(tool) else {
                    throw MiniAppWebMcpApprovalDenied()
                }
                guard activeBridgeSession == session,
                      activeLocalSource == source
                else { throw CancellationError() }
            }

            let result = try await webView.callAsyncJavaScript(
                """
                const bridge = window.__fabushiWebMcp;
                if (!bridge || typeof bridge.invokeLocalFromHost !== 'function') {
                  throw new Error('Active page has no Host-callable WebMCP implementation');
                }
                return await bridge.invokeLocalFromHost(
                  name,
                  input,
                  pluginInstanceId,
                  nonce,
                  generation
                );
                """,
                arguments: [
                    "name": tool.name,
                    "input": arguments,
                    "pluginInstanceId": session.pluginInstanceId,
                    "nonce": session.nonce,
                    "generation": NSNumber(value: session.generation),
                ],
                in: nil,
                contentWorld: .page
            )

            guard activeBridgeSession == session,
                  activeLocalSource == source
            else { throw CancellationError() }
            return result as Any
        }

        private func requestApproval(_ tool: MiniAppToolContract) async -> Bool {
            guard let presenter = topViewController() else { return false }
            let warning = tool.approval == "destructive"
                ? "该操作可能产生破坏性修改。"
                : "该操作会修改小程序或后台状态。"
            return await withCheckedContinuation { continuation in
                let alert = UIAlertController(
                    title: "允许 WebMCP 调用 \(tool.name)？",
                    message: "\(tool.description)\n\n\(warning)",
                    preferredStyle: .alert
                )
                alert.addAction(UIAlertAction(title: "取消", style: .cancel) { _ in
                    continuation.resume(returning: false)
                })
                alert.addAction(UIAlertAction(title: "允许", style: .default) { _ in
                    continuation.resume(returning: true)
                })
                presenter.present(alert, animated: true)
            }
        }

        private func topViewController() -> UIViewController? {
            let scene = UIApplication.shared.connectedScenes
                .compactMap { $0 as? UIWindowScene }
                .first { $0.activationState == .foregroundActive }
            var controller = scene?.windows.first(where: \.isKeyWindow)?.rootViewController
            while let presented = controller?.presentedViewController {
                controller = presented
            }
            return controller
        }

        private func resolve(
            webView: WKWebView,
            session: MiniAppWebMcpBridgeSession,
            requestId: String,
            payload: [String: Any]
        ) {
            guard activeBridgeSession == session,
                  JSONSerialization.isValidJSONObject(payload),
                  let data = try? JSONSerialization.data(withJSONObject: payload),
                  let json = String(data: data, encoding: .utf8),
                  let requestData = try? JSONEncoder().encode(requestId),
                  let requestJson = String(data: requestData, encoding: .utf8)
            else { return }
            let instanceJson = jsonString(session.pluginInstanceId)
            let nonceJson = jsonString(session.nonce)
            webView.evaluateJavaScript(
                "window.__fabushiNativeResolve?.(\(requestJson),\(instanceJson),\(nonceJson),\(json));"
            )
        }
    }
}

func hardenGeneratedMiniAppDocument(_ html: String) -> String {
    let policy = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'; media-src data:; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'\">"
    if let range = html.range(of: "<head>", options: .caseInsensitive) {
        var result = html
        result.insert(contentsOf: policy, at: range.upperBound)
        return result
    }
    if let range = html.range(of: "<html>", options: .caseInsensitive) {
        var result = html
        result.insert(contentsOf: "<head>\(policy)</head>", at: range.upperBound)
        return result
    }
    return "<!doctype html><html><head>\(policy)</head><body>\(html)</body></html>"
}

func webMcpToolDefinitions(plugin: MarketplacePlugin) -> [[String: Any]] {
    plugin.tools.map { tool in
        [
            "name": tool.name,
            "title": tool.title ?? tool.name,
            "description": tool.description,
            "readOnlyHint": tool.approval == "none",
            "inputSchema": tool.inputSchemaObject ?? [
                "type": "object",
                "properties": [String: Any](),
            ],
        ] as [String: Any]
    }
}

func webMcpBootstrapJavaScript(
    plugin: MarketplacePlugin,
    bridgeSession: MiniAppWebMcpBridgeSession
) -> String {
    let definitions = webMcpToolDefinitions(plugin: plugin)
    let data = (try? JSONSerialization.data(withJSONObject: definitions)) ?? Data("[]".utf8)
    let toolsJson = String(data: data, encoding: .utf8) ?? "[]"
    return """
    (function(){
      const definitions=\(toolsJson);
      const pluginInstanceId=\(jsonString(bridgeSession.pluginInstanceId));
      const nonce=\(jsonString(bridgeSession.nonce));
      const grants=new Set(\(jsonStringArray(Array(bridgeSession.grants).sorted())));
      const generation=\(bridgeSession.generation);
      const hostCallTimeoutMs=\(miniAppWebMcpHostCallTimeoutMilliseconds);
      const hostTools=new Map();const pageTools=new Map();const controllers=[];const activePageCalls=new Set();const pending=new Map();let sequence=0;let disposed=false;
      function rejectPending(reason){for(const task of pending.values())task.reject(new Error(reason));pending.clear();}
      window.__fabushiNativeResolve=(requestId,responseInstanceId,responseNonce,payload)=>{if(disposed||responseInstanceId!==pluginInstanceId||responseNonce!==nonce)return;const task=pending.get(requestId);if(!task)return;pending.delete(requestId);if(payload&&payload.ok)task.resolve(payload.result);else task.reject(new Error(payload?.error||'WebMCP runtime call failed'));};
      function post(message){window.webkit.messageHandlers.\(webMcpMessageHandler).postMessage({...message,pluginInstanceId,nonce,generation});}
      function callNative(name,input){if(disposed)return Promise.reject(new Error('MCP App bridge disposed'));if(!grants.has(name))return Promise.reject(new Error('MCP App bridge capability not granted: '+name));return new Promise((resolve,reject)=>{const requestId='webmcp-'+Date.now()+'-'+(++sequence);pending.set(requestId,{resolve,reject});post({kind:'call',requestId,name,input:input||{}});});}
      function publicTool(tool){const copy={...tool};delete copy.execute;return copy;}
      function registerHost(item){if(!grants.has(item.name))return;const tool={name:item.name,title:item.title||item.name,description:item.description||item.name,inputSchema:item.inputSchema||{type:'object',properties:{}},annotations:{readOnlyHint:item.readOnlyHint===true},execute:(input)=>callNative(item.name,input)};hostTools.set(tool.name,tool);if(document.modelContext&&typeof document.modelContext.registerTool==='function'){const controller=new AbortController();controllers.push(controller);Promise.resolve(document.modelContext.registerTool(tool,{signal:controller.signal})).catch(()=>{});}}
      function publishPageTools(){post({kind:'pageTools',tools:Array.from(pageTools.values()).map(publicTool)});}
      function registerLocalTool(definition,execute){if(disposed)throw new Error('MCP App bridge disposed');if(!definition||typeof definition.name!=='string'||!/^[A-Za-z0-9_.-]{1,128}$/.test(definition.name))throw new Error('Invalid local WebMCP tool name');if(typeof execute!=='function')throw new Error('Local WebMCP tool requires an execute function');if(!grants.has(definition.name))throw new Error('MCP App bridge capability not granted: '+definition.name);const tool={name:definition.name,title:definition.title||definition.name,description:definition.description||definition.name,inputSchema:definition.inputSchema||{type:'object',properties:{}},annotations:definition.annotations||{readOnlyHint:false},execute};pageTools.set(tool.name,tool);publishPageTools();return ()=>{pageTools.delete(tool.name);publishPageTools();};}
      async function invokeLocalFromHost(name,input,expectedInstanceId,expectedNonce,expectedGeneration){if(disposed||expectedInstanceId!==pluginInstanceId||expectedNonce!==nonce||Number(expectedGeneration)!==generation)throw new Error('Stale Mini App WebMCP session');const tool=pageTools.get(name);if(!tool)throw new Error('Active page did not register local WebMCP tool: '+name);const controller=new AbortController();activePageCalls.add(controller);const timer=setTimeout(()=>controller.abort(),hostCallTimeoutMs);try{return await Promise.race([Promise.resolve(tool.execute(input||{},{signal:controller.signal})),new Promise((_,reject)=>controller.signal.addEventListener('abort',()=>reject(new Error(disposed?'MCP App bridge disposed':'WebMCP Tool timed out or cancelled')),{once:true}))]);}finally{clearTimeout(timer);activePageCalls.delete(controller);}}
      for(const item of definitions)registerHost(item);
      Object.defineProperty(window,'__fabushiWebMcp',{configurable:true,value:{version:2,list:()=>Array.from(hostTools.values()).map(publicTool),call:async(name,input={})=>{const tool=hostTools.get(name);if(!tool)throw new Error('Unknown WebMCP tool: '+name);return tool.execute(input);},registerLocalTool,listLocal:()=>Array.from(pageTools.values()).map(publicTool),invokeLocalFromHost}});
      post({kind:'ready'});
      window.addEventListener('pagehide',()=>{if(disposed)return;disposed=true;for(const controller of controllers)controller.abort();for(const controller of activePageCalls)controller.abort();activePageCalls.clear();post({kind:'dispose'});rejectPending('MCP App bridge disposed');pageTools.clear();},{once:true});
      window.dispatchEvent(new CustomEvent('fabushi:webmcp-ready',{detail:{pluginId:\(jsonString(plugin.pluginId)),pluginInstanceId,generation,grants:Array.from(grants),tools:Array.from(hostTools.keys())}}));
    })();
    """
}

private func injectLocalWebMcp(
    _ html: String,
    plugin: MarketplacePlugin,
    bridgeSession: MiniAppWebMcpBridgeSession
) -> String {
    let bootstrap = "<script>\(webMcpBootstrapJavaScript(plugin: plugin, bridgeSession: bridgeSession))</script>"
    if let range = html.range(of: "</head>", options: .caseInsensitive) {
        var result = html
        result.insert(contentsOf: bootstrap, at: range.lowerBound)
        return result
    }
    return bootstrap + html
}

private func jsonString(_ value: String) -> String {
    guard let data = try? JSONEncoder().encode(value) else { return "\"\"" }
    return String(data: data, encoding: .utf8) ?? "\"\""
}

private func jsonStringArray(_ values: [String]) -> String {
    guard let data = try? JSONEncoder().encode(values) else { return "[]" }
    return String(data: data, encoding: .utf8) ?? "[]"
}

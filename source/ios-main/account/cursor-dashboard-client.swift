import Foundation

struct IOSCursorDashboardError: Error, LocalizedError, Sendable {
    let message: String
    let httpStatus: Int?
    let retryAfterMilliseconds: Int?

    init(
        message: String,
        httpStatus: Int? = nil,
        retryAfterMilliseconds: Int? = nil
    ) {
        self.message = message
        self.httpStatus = httpStatus
        self.retryAfterMilliseconds = retryAfterMilliseconds
    }

    var errorDescription: String? { message }
}

func iosCursorRetryAfterMilliseconds(_ value: String?) -> Int? {
    guard let raw = value?.trimmingCharacters(in: .whitespacesAndNewlines),
          !raw.isEmpty,
          let seconds = Double(raw),
          seconds.isFinite,
          seconds >= 0
    else {
        return nil
    }
    let milliseconds = seconds * 1_000
    guard milliseconds < Double(Int.max) else { return Int.max }
    return Int(milliseconds)
}

struct IOSCursorSandBoxBrokerError: Error, LocalizedError, Equatable, Sendable {
    enum Kind: Equatable, Sendable {
        case noStorage
        case blocked(retryAfterMilliseconds: Int?)
        case updateRequired
        case accessDenied
        case http(status: Int)
    }

    let kind: Kind
    let message: String

    var errorDescription: String? { message }

    var deniesCachedDescriptor: Bool {
        switch kind {
        case .noStorage, .accessDenied: true
        default: false
        }
    }
}


indirect enum IOSCursorAutoReviewJSONValue: Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case object([String: IOSCursorAutoReviewJSONValue])
    case array([IOSCursorAutoReviewJSONValue])
}

struct IOSCursorAutoReviewConversationMessage: Equatable, Sendable {
    let role: String
    let content: String
}

struct IOSCursorAutoReviewClassifierRequest: Equatable, Sendable {
    let parentConversationId: String?
    let action: String
    let arguments: [String: IOSCursorAutoReviewJSONValue]
    let conversationContext: [IOSCursorAutoReviewConversationMessage]
    let attemptIndex: UInt32
    let mode: String

    init(
        parentConversationId: String?,
        action: String,
        arguments: [String: IOSCursorAutoReviewJSONValue] = [:],
        conversationContext: [IOSCursorAutoReviewConversationMessage] = [],
        attemptIndex: UInt32 = 0,
        mode: String = "enforce"
    ) {
        self.parentConversationId = parentConversationId
        self.action = action
        self.arguments = arguments
        self.conversationContext = conversationContext
        self.attemptIndex = attemptIndex
        self.mode = mode
    }
}

enum IOSCursorAutoReviewDecision: Equatable, Sendable {
    case unspecified
    case allow
    case block
    case unknown(Int32)
}

struct IOSCursorAutoReviewClassifierSuccess: Equatable, Sendable {
    let decision: IOSCursorAutoReviewDecision
    let blockReason: String?
    let proposedAllowRule: String?
}

enum IOSCursorAutoReviewClassifierResult: Equatable, Sendable {
    case success(IOSCursorAutoReviewClassifierSuccess)
    case backendError(message: String, failureReason: String?, retryable: Bool?)
    case missing
}

private enum IOSCursorDashboardProto {
    private enum WireValue {
        case varint(UInt64)
        case bytes(Data)
    }

    private struct Reader {
        private let bytes: [UInt8]
        private var index = 0

        init(_ data: Data) {
            bytes = Array(data)
        }

        mutating func readFields() throws -> [(Int, WireValue)] {
            var fields: [(Int, WireValue)] = []
            while index < bytes.count {
                let key = try readVarint()
                let field = Int(key >> 3)
                guard field > 0 else {
                    throw IOSCursorDashboardError(message: "Dashboard protobuf contained field zero.")
                }
                switch Int(key & 0x07) {
                case 0:
                    fields.append((field, .varint(try readVarint())))
                case 1:
                    try skip(8)
                case 2:
                    let length = try readLength()
                    let end = index + length
                    guard end <= bytes.count else {
                        throw IOSCursorDashboardError(message: "Dashboard protobuf length exceeded the response body.")
                    }
                    fields.append((field, .bytes(Data(bytes[index..<end]))))
                    index = end
                case 5:
                    try skip(4)
                default:
                    throw IOSCursorDashboardError(message: "Dashboard protobuf used an unsupported wire type.")
                }
            }
            return fields
        }

        private mutating func readVarint() throws -> UInt64 {
            var value: UInt64 = 0
            var shift: UInt64 = 0
            while index < bytes.count, shift < 64 {
                let byte = bytes[index]
                index += 1
                value |= UInt64(byte & 0x7f) << shift
                if byte & 0x80 == 0 {
                    return value
                }
                shift += 7
            }
            throw IOSCursorDashboardError(message: "Dashboard protobuf contained an invalid varint.")
        }

        private mutating func readLength() throws -> Int {
            let value = try readVarint()
            guard value <= UInt64(Int.max) else {
                throw IOSCursorDashboardError(message: "Dashboard protobuf length overflowed this platform.")
            }
            return Int(value)
        }

        private mutating func skip(_ count: Int) throws {
            guard count >= 0, index + count <= bytes.count else {
                throw IOSCursorDashboardError(message: "Dashboard protobuf ended unexpectedly.")
            }
            index += count
        }
    }

    static func recreateSandBoxRequest(preserveData: Bool, force: Bool) -> Data {
        var data = Data()
        appendVarintField(1, preserveData ? 1 : 0, to: &data)
        appendVarintField(2, force ? 1 : 0, to: &data)
        return data
    }

    static func forceRecreateSandBoxRequest() -> Data {
        Data()
    }

    static func ensureSandBoxRequest() -> Data {
        Data()
    }

    static func watchSandBoxMigrationRequest(
        fromOffsetKey: String,
        includeFinished: Bool
    ) -> Data {
        var data = Data()
        if !fromOffsetKey.isEmpty {
            appendBytesField(1, Data(fromOffsetKey.utf8), to: &data)
        }
        appendVarintField(2, includeFinished ? 1 : 0, to: &data)
        return data
    }

    static func decodeSandBoxRunStateResponse(_ data: Data) throws -> IOSCursorSandBoxRunState {
        var reader = Reader(data)
        var imageUpdateAvailable = false
        for (field, value) in try reader.readFields() {
            if field == 2, case .varint(let raw) = value {
                imageUpdateAvailable = raw != 0
            }
        }
        return .init(imageUpdateAvailable: imageUpdateAvailable)
    }

    static func decodeRecreateSandBoxResponse(_ data: Data) throws -> IOSCursorSandBoxRecreateResult {
        var reader = Reader(data)
        var started = false
        var reason = ""
        var operationId = ""
        for (field, value) in try reader.readFields() {
            switch (field, value) {
            case (1, .varint(let value)):
                started = value != 0
            case (2, .bytes(let bytes)):
                reason = String(data: bytes, encoding: .utf8) ?? ""
            case (3, .bytes(let bytes)):
                operationId = String(data: bytes, encoding: .utf8) ?? ""
            default:
                break
            }
        }
        return .init(started: started, reason: reason, operationId: operationId)
    }

    static func decodeEnsureSandBoxResponse(_ data: Data) throws -> IOSCursorSandBoxConnection {
        var reader = Reader(data)
        var networkToken = ""
        var execDaemonAuthToken = ""
        var execDaemonURL = ""
        var vncURL = ""
        var gatewayURL = ""
        var gatewayToken = ""
        var forkVNCBaseURL = ""

        for (field, value) in try reader.readFields() {
            guard case .bytes(let bytes) = value else { continue }
            let decoded = String(data: bytes, encoding: .utf8) ?? ""
            switch field {
            case 4: networkToken = decoded
            case 5: execDaemonAuthToken = decoded
            case 6: execDaemonURL = decoded
            case 7: vncURL = decoded
            case 10: gatewayURL = decoded
            case 11: gatewayToken = decoded
            case 12: forkVNCBaseURL = decoded
            default: break
            }
        }

        guard !execDaemonAuthToken.isEmpty else {
            throw IOSCursorDashboardError(message: "EnsureSandBox response was missing exec daemon auth token.")
        }
        guard let daemonURL = URL(string: execDaemonURL),
              let scheme = daemonURL.scheme?.lowercased(),
              (scheme == "http" || scheme == "https"),
              daemonURL.host != nil
        else {
            throw IOSCursorDashboardError(message: "EnsureSandBox response contained an invalid exec daemon URL.")
        }

        return .init(
            networkToken: networkToken,
            execDaemonAuthToken: execDaemonAuthToken,
            execDaemonURL: daemonURL.absoluteString,
            vncURL: vncURL,
            gatewayURL: gatewayURL,
            gatewayToken: gatewayToken,
            forkVNCBaseURL: forkVNCBaseURL
        )
    }

    static func decodeSandBoxMigrationEvent(_ data: Data) throws -> IOSCursorSandBoxMigrationEvent {
        var reader = Reader(data)
        var phase: Int32 = 0
        var detail = ""
        var atMs: Int64 = 0
        var offsetKey = ""
        var operationId = ""
        for (field, value) in try reader.readFields() {
            switch (field, value) {
            case (1, .varint(let value)):
                phase = Int32(truncatingIfNeeded: value)
            case (2, .bytes(let bytes)):
                detail = String(data: bytes, encoding: .utf8) ?? ""
            case (3, .varint(let value)):
                atMs = Int64(bitPattern: value)
            case (4, .bytes(let bytes)):
                offsetKey = String(data: bytes, encoding: .utf8) ?? ""
            case (5, .bytes(let bytes)):
                operationId = String(data: bytes, encoding: .utf8) ?? ""
            default:
                break
            }
        }
        return .init(
            phase: phase,
            detail: detail,
            atMs: atMs,
            offsetKey: offsetKey,
            operationId: operationId
        )
    }

    static func getTeamsRequest(activeOnly: Bool) -> Data {
        var data = Data()
        appendVarintField(1, activeOnly ? 1 : 0, to: &data)
        return data
    }

    static func transcribeAudioRequest(
        audio: Data,
        mimeType: String,
        language: String
    ) -> Data {
        var data = Data()
        appendBytesField(1, audio, to: &data)
        appendBytesField(2, Data(mimeType.utf8), to: &data)
        appendBytesField(3, Data(language.utf8), to: &data)
        return data
    }

    static func decodeTranscribeAudioResponse(_ data: Data) throws -> IOSCursorTranscriptionResult {
        var reader = Reader(data)
        var text = ""
        var transcriptionTimeMs: Int64 = 0
        for (field, value) in try reader.readFields() {
            switch (field, value) {
            case (1, .bytes(let bytes)):
                text = String(data: bytes, encoding: .utf8) ?? ""
            case (2, .varint(let raw)):
                transcriptionTimeMs = Int64(bitPattern: raw)
            default:
                break
            }
        }
        return .init(text: text, transcriptionTimeMs: transcriptionTimeMs)
    }

    static func prReviewUserSettingsRequest() -> Data {
        Data()
    }

    static func teamAdminSettingsRequest() -> Data {
        Data()
    }

    static func decodeLocalToolPermissionCeiling(
        _ data: Data
    ) throws -> SandLocalToolPermission? {
        var reader = Reader(data)
        for (field, value) in try reader.readFields() where field == 60 {
            guard case .bytes(let controls) = value,
                  let raw = try firstVarintField(1, in: controls)
            else { continue }
            switch raw {
            case 1: return "never"
            case 2: return "ask"
            case 3: return "always"
            default: return nil
            }
        }
        return nil
    }

    static func decodePrReviewUserDestination(_ data: Data) throws -> SandPrReviewDestination? {
        var reader = Reader(data)
        for (field, value) in try reader.readFields() where field == 6 {
            if case .varint(let raw) = value {
                return prReviewDestination(raw)
            }
        }
        return nil
    }

    static func decodePrReviewTeamDestination(_ data: Data) throws -> SandPrReviewDestination? {
        var reader = Reader(data)
        var pullRequestPreferences: Data?
        var backgroundAgentSettings: Data?
        for (field, value) in try reader.readFields() {
            switch (field, value) {
            case (37, .bytes(let bytes)):
                pullRequestPreferences = bytes
            case (7, .bytes(let bytes)):
                backgroundAgentSettings = bytes
            default:
                break
            }
        }
        if let pullRequestPreferences,
           let raw = try firstVarintField(1, in: pullRequestPreferences),
           let destination = prReviewDestination(raw) {
            return destination
        }
        if let backgroundAgentSettings,
           let raw = try firstVarintField(5, in: backgroundAgentSettings) {
            return prReviewDestination(raw)
        }
        return nil
    }

    private static func firstVarintField(_ target: Int, in data: Data) throws -> UInt64? {
        var reader = Reader(data)
        for (field, value) in try reader.readFields() where field == target {
            if case .varint(let raw) = value {
                return raw
            }
        }
        return nil
    }

    private static func prReviewDestination(_ raw: UInt64) -> SandPrReviewDestination? {
        switch raw {
        case 1: .github
        case 2: .graphite
        case 3: .reviewCursor
        default: nil
        }
    }

    static func publishPluginRequest(
        teamId: Int32,
        name: String,
        displayName: String,
        description: String,
        pluginTarGz: Data
    ) throws -> Data {
        guard teamId >= 0 else {
            throw IOSCursorDashboardError(message: "Skill publish team id must be non-negative.")
        }
        var data = Data()
        appendVarintField(1, UInt64(teamId), to: &data)
        appendBytesField(2, Data(name.utf8), to: &data)
        appendBytesField(3, Data(displayName.utf8), to: &data)
        appendBytesField(4, Data(description.utf8), to: &data)
        appendBytesField(5, pluginTarGz, to: &data)
        return data
    }

    static func unpublishPluginRequest(pluginId: Int64, teamId: Int32) throws -> Data {
        guard pluginId >= 0, teamId >= 0 else {
            throw IOSCursorDashboardError(message: "Skill unpublish ids must be non-negative.")
        }
        var data = Data()
        appendVarintField(1, UInt64(pluginId), to: &data)
        appendVarintField(2, UInt64(teamId), to: &data)
        return data
    }

    static func getBackgroundComposerInfoRequest(bcId: String) throws -> Data {
        let bcId = bcId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !bcId.isEmpty else {
            throw IOSCursorDashboardError(message: "Background composer id is required.")
        }
        var data = Data()
        appendBytesField(1, Data(bcId.utf8), to: &data)
        appendVarintField(2, 0, to: &data)
        appendVarintField(3, 1, to: &data)
        return data
    }

    static func decodeBackgroundComposerInfo(_ data: Data) throws -> IOSCloudAgentComposerInfo {
        var response = Reader(data)
        var detailedMessage: Data?
        for (field, value) in try response.readFields() where field == 1 {
            if case .bytes(let bytes) = value {
                detailedMessage = bytes
                break
            }
        }
        guard let detailedMessage else {
            throw IOSCursorDashboardError(message: "Background composer response was missing composer details.")
        }

        var detailed = Reader(detailedMessage)
        var composerMessage: Data?
        var promptMessage: Data?
        var detailedStatus: Int32?
        var summary: String?
        var permanentErrorMessage: Data?
        var prMessages: [Data] = []
        for (field, value) in try detailed.readFields() {
            switch (field, value) {
            case (1, .bytes(let bytes)):
                composerMessage = bytes
            case (4, .bytes(let bytes)):
                promptMessage = bytes
            case (5, .varint(let value)):
                detailedStatus = Int32(truncatingIfNeeded: value)
            case (10, .bytes(let bytes)):
                summary = String(data: bytes, encoding: .utf8)
            case (16, .bytes(let bytes)):
                permanentErrorMessage = bytes
            case (20, .bytes(let bytes)):
                prMessages.append(bytes)
            default:
                break
            }
        }

        var composerStatus: Int32?
        var name: String?
        var branchName: String?
        var prURL: String?
        var composerPrState: String?
        var filesChanged: Int?
        var linesAdded: Int?
        var linesRemoved: Int?
        if let composerMessage {
            var composer = Reader(composerMessage)
            for (field, value) in try composer.readFields() {
                switch (field, value) {
                case (5, .bytes(let bytes)):
                    name = nonemptyUTF8(bytes)
                case (6, .bytes(let bytes)):
                    branchName = nonemptyUTF8(bytes)
                case (12, .varint(let value)):
                    composerStatus = Int32(truncatingIfNeeded: value)
                case (22, .bytes(let bytes)):
                    prURL = nonemptyUTF8(bytes)
                case (25, .varint(let value)):
                    linesAdded = nonnegativeInt32(value)
                case (26, .varint(let value)):
                    linesRemoved = nonnegativeInt32(value)
                case (27, .varint(let value)):
                    filesChanged = nonnegativeInt32(value)
                case (43, .varint(let value)):
                    composerPrState = cloudAgentPRState(Int32(truncatingIfNeeded: value))
                default:
                    break
                }
            }
        }

        var prompt: String?
        if let promptMessage {
            var promptReader = Reader(promptMessage)
            for (field, value) in try promptReader.readFields() where field == 1 {
                if case .bytes(let bytes) = value {
                    prompt = nonemptyUTF8(bytes)
                    break
                }
            }
        }

        struct DecodedPR {
            let branchName: String?
            let number: Int32?
            let state: String?
            let url: String?
        }
        var prs: [DecodedPR] = []
        for message in prMessages {
            var reader = Reader(message)
            var prBranch: String?
            var number: Int32?
            var state: String?
            var url: String?
            for (field, value) in try reader.readFields() {
                switch (field, value) {
                case (1, .bytes(let bytes)):
                    prBranch = nonemptyUTF8(bytes)
                case (4, .varint(let value)):
                    number = Int32(truncatingIfNeeded: value)
                case (5, .varint(let value)):
                    state = cloudAgentPRState(Int32(truncatingIfNeeded: value))
                case (6, .bytes(let bytes)):
                    url = nonemptyUTF8(bytes)
                default:
                    break
                }
            }
            prs.append(.init(branchName: prBranch, number: number, state: state, url: url))
        }

        let primaryPR: DecodedPR? = {
            if let prURL {
                return prs.first(where: { $0.url == prURL })
            }
            return prs.first(where: { $0.number != nil || $0.state != nil || $0.url != nil })
        }()
        if branchName == nil {
            branchName = prs.compactMap(\.branchName).first
        }
        if prURL == nil {
            prURL = primaryPR?.url
        }

        var permanentError: String?
        if let permanentErrorMessage {
            var errorEnvelope = Reader(permanentErrorMessage)
            var customErrorMessage: Data?
            for (field, value) in try errorEnvelope.readFields() where field == 2 {
                if case .bytes(let bytes) = value {
                    customErrorMessage = bytes
                    break
                }
            }
            if let customErrorMessage {
                var customError = Reader(customErrorMessage)
                var title: String?
                var detail: String?
                for (field, value) in try customError.readFields() {
                    switch (field, value) {
                    case (1, .bytes(let bytes)):
                        title = String(data: bytes, encoding: .utf8)
                    case (2, .bytes(let bytes)):
                        detail = String(data: bytes, encoding: .utf8)
                    default:
                        break
                    }
                }
                let joined = [title, detail]
                    .compactMap { $0?.trimmingCharacters(in: .whitespacesAndNewlines) }
                    .filter { !$0.isEmpty }
                    .joined(separator: ": ")
                permanentError = joined.isEmpty ? nil : joined
            }
        }

        guard let status = composerStatus ?? detailedStatus else {
            throw IOSCursorDashboardError(message: "Background composer response was missing status.")
        }
        let resolvedPrState = primaryPR?.state ?? composerPrState ?? (prURL == nil ? "none" : "unknown")
        return .init(
            status: status,
            name: name,
            prompt: prompt,
            summary: summary,
            permanentError: permanentError,
            branchName: branchName,
            filesChanged: filesChanged,
            linesAdded: linesAdded,
            linesRemoved: linesRemoved,
            prURL: prURL,
            prState: resolvedPrState,
            prNumber: primaryPR?.number
        )
    }

    static func startBackgroundComposerRequest(_ input: IOSCloudAgentLaunchInput) throws -> Data {
        let bcId = input.bcId.trimmingCharacters(in: .whitespacesAndNewlines)
        let snapshot = input.snapshotNameOrId.trimmingCharacters(in: .whitespacesAndNewlines)
        let repoURL = input.repoURL.trimmingCharacters(in: .whitespacesAndNewlines)
        let prompt = input.prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !bcId.isEmpty, !snapshot.isEmpty, !repoURL.isEmpty, !prompt.isEmpty else {
            throw IOSCursorDashboardError(message: "Cloud agent launch requires bcId, repository and prompt.")
        }

        var userMessage = Data()
        appendBytesField(1, Data(prompt.utf8), to: &userMessage)
        appendBytesField(2, Data(UUID().uuidString.lowercased().utf8), to: &userMessage)
        if !input.images.isEmpty {
            var selectedContext = Data()
            for image in input.images {
                guard image.mimeType.hasPrefix("image/"), !image.data.isEmpty else {
                    throw IOSCursorDashboardError(message: "Cloud agent image payload is invalid.")
                }
                var selectedImage = Data()
                appendBytesField(8, image.data, to: &selectedImage)
                if !image.path.isEmpty {
                    appendBytesField(3, Data(image.path.utf8), to: &selectedImage)
                }
                appendBytesField(7, Data(image.mimeType.utf8), to: &selectedImage)
                appendBytesField(1, selectedImage, to: &selectedContext)
            }
            appendBytesField(3, selectedContext, to: &userMessage)
        }
        appendVarintField(4, 1, to: &userMessage)

        var userMessageAction = Data()
        appendBytesField(1, userMessage, to: &userMessageAction)
        appendVarintField(3, 1, to: &userMessageAction)
        var conversationAction = Data()
        appendBytesField(1, userMessageAction, to: &conversationAction)

        var startingPoint = Data()
        appendBytesField(1, Data(repoURL.utf8), to: &startingPoint)
        if let startingRef = input.startingRef?.trimmingCharacters(in: .whitespacesAndNewlines),
           !startingRef.isEmpty {
            appendBytesField(2, Data(startingRef.utf8), to: &startingPoint)
        }
        if !input.environmentRepos.isEmpty {
            var repoConfig = Data()
            for repo in input.environmentRepos {
                var entry = Data()
                appendBytesField(1, Data(repo.repoURL.utf8), to: &entry)
                if let node = repo.scmRepoNodeId, !node.isEmpty {
                    appendBytesField(2, Data(node.utf8), to: &entry)
                }
                if let enterprise = repo.gitEnterpriseUUID, !enterprise.isEmpty {
                    appendBytesField(3, Data(enterprise.utf8), to: &entry)
                }
                appendBytesField(1, entry, to: &repoConfig)
            }
            appendBytesField(11, repoConfig, to: &startingPoint)
        }
        if let name = input.environmentName?.trimmingCharacters(in: .whitespacesAndNewlines),
           !name.isEmpty {
            appendBytesField(12, Data(name.utf8), to: &startingPoint)
        }
        if let publicId = input.environmentPublicId?.trimmingCharacters(in: .whitespacesAndNewlines),
           !publicId.isEmpty {
            appendBytesField(15, Data(publicId.utf8), to: &startingPoint)
        }

        var data = Data()
        appendBytesField(1, Data(snapshot.utf8), to: &data)
        appendBytesField(5, Data(), to: &data)
        appendBytesField(6, Data("/workspace".utf8), to: &data)
        appendVarintField(9, 1, to: &data)
        appendVarintField(10, 1, to: &data)
        appendBytesField(11, startingPoint, to: &data)
        appendBytesField(14, Data(repoURL.utf8), to: &data)
        appendBytesField(22, Data(bcId.utf8), to: &data)
        appendVarintField(23, 33, to: &data)
        appendVarintField(26, 1, to: &data)
        if let startingRef = input.startingRef?.trimmingCharacters(in: .whitespacesAndNewlines),
           !startingRef.isEmpty {
            appendBytesField(31, Data(startingRef.utf8), to: &data)
        }
        appendVarintField(37, 1, to: &data)
        appendVarintField(41, 1, to: &data)
        appendBytesField(44, conversationAction, to: &data)
        if let title = input.title?.trimmingCharacters(in: .whitespacesAndNewlines),
           !title.isEmpty {
            appendBytesField(50, Data(title.utf8), to: &data)
        }
        if let usePrivateWorker = input.usePrivateWorker {
            appendVarintField(55, usePrivateWorker ? 1 : 0, to: &data)
        }
        if let requestedModel = try cloudAgentRequestedModel(
            modelId: input.modelId,
            modelParams: input.modelParams
        ) {
            appendBytesField(58, requestedModel, to: &data)
        }
        for label in input.labels {
            var wire = Data()
            appendBytesField(1, Data(label.key.utf8), to: &wire)
            appendBytesField(2, Data(label.value.utf8), to: &wire)
            appendBytesField(59, wire, to: &data)
        }
        if let teamId = input.teamId {
            guard teamId >= 0 else {
                throw IOSCursorDashboardError(message: "Cloud agent team id must be non-negative.")
            }
            appendVarintField(98, UInt64(teamId), to: &data)
        }
        return data
    }

    static func decodeStartBackgroundComposerResponse(
        _ data: Data,
        fallbackBcId: String
    ) throws -> IOSCloudAgentLaunchResult {
        var response = Reader(data)
        var composer: Data?
        var initialRunId: String?
        for (field, value) in try response.readFields() {
            switch (field, value) {
            case (1, .bytes(let bytes)): composer = bytes
            case (3, .bytes(let bytes)): initialRunId = nonemptyUTF8(bytes)
            default: break
            }
        }
        var bcId = fallbackBcId
        if let composer {
            var nested = Reader(composer)
            for (field, value) in try nested.readFields() where field == 1 {
                if case .bytes(let bytes) = value, let decoded = nonemptyUTF8(bytes) {
                    bcId = decoded
                    break
                }
            }
        }
        return .init(
            bcId: bcId,
            url: "https://cursor.com/agents/\(bcId)",
            initialRunId: initialRunId
        )
    }

    static func listBackgroundComposersRequest(limit: Int, includeArchived: Bool) -> Data {
        var data = Data()
        appendVarintField(1, UInt64(max(1, min(limit, Int(Int32.max)))), to: &data)
        if includeArchived {
            appendVarintField(10, 1, to: &data)
        }
        return data
    }

    static func decodeBackgroundComposerList(_ data: Data) throws -> [IOSCloudAgentSummary] {
        var response = Reader(data)
        var output: [IOSCloudAgentSummary] = []
        for (field, value) in try response.readFields() where field == 1 {
            guard case .bytes(let message) = value else { continue }
            var nested = Reader(message)
            var bcId = ""
            var name = ""
            var branchName = ""
            var prURL = ""
            var isArchived = false
            var status: Int32 = 0
            for (nestedField, nestedValue) in try nested.readFields() {
                switch (nestedField, nestedValue) {
                case (1, .bytes(let bytes)): bcId = String(data: bytes, encoding: .utf8) ?? ""
                case (5, .bytes(let bytes)): name = String(data: bytes, encoding: .utf8) ?? ""
                case (6, .bytes(let bytes)): branchName = String(data: bytes, encoding: .utf8) ?? ""
                case (9, .varint(let raw)): isArchived = raw != 0
                case (12, .varint(let raw)): status = Int32(truncatingIfNeeded: raw)
                case (22, .bytes(let bytes)): prURL = String(data: bytes, encoding: .utf8) ?? ""
                default: break
                }
            }
            if !bcId.isEmpty {
                output.append(.init(
                    bcId: bcId,
                    name: name,
                    status: status,
                    branchName: branchName,
                    prURL: prURL,
                    isArchived: isArchived
                ))
            }
        }
        return output
    }

    static func backgroundComposerIdRequest(_ bcId: String) throws -> Data {
        let bcId = bcId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !bcId.isEmpty else {
            throw IOSCursorDashboardError(message: "Background composer id is required.")
        }
        var data = Data()
        appendBytesField(1, Data(bcId.utf8), to: &data)
        return data
    }

    static func followupBackgroundComposerRequest(_ input: IOSCloudAgentReplyInput) throws -> Data {
        let bcId = input.bcId.trimmingCharacters(in: .whitespacesAndNewlines)
        let prompt = input.prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !bcId.isEmpty, !prompt.isEmpty else {
            throw IOSCursorDashboardError(message: "Cloud agent follow-up requires agent id and prompt.")
        }

        var userMessage = Data()
        appendBytesField(1, Data(prompt.utf8), to: &userMessage)
        appendBytesField(2, Data(UUID().uuidString.lowercased().utf8), to: &userMessage)
        if !input.images.isEmpty {
            var selectedContext = Data()
            for image in input.images {
                guard image.mimeType.hasPrefix("image/"), !image.data.isEmpty else {
                    throw IOSCursorDashboardError(message: "Cloud agent image payload is invalid.")
                }
                var selectedImage = Data()
                appendBytesField(8, image.data, to: &selectedImage)
                if !image.path.isEmpty { appendBytesField(3, Data(image.path.utf8), to: &selectedImage) }
                appendBytesField(7, Data(image.mimeType.utf8), to: &selectedImage)
                appendBytesField(1, selectedImage, to: &selectedContext)
            }
            appendBytesField(3, selectedContext, to: &userMessage)
        }
        appendVarintField(4, 1, to: &userMessage)
        var userMessageAction = Data()
        appendBytesField(1, userMessage, to: &userMessageAction)
        appendVarintField(3, 1, to: &userMessageAction)
        var conversationAction = Data()
        appendBytesField(1, userMessageAction, to: &conversationAction)

        var data = Data()
        appendBytesField(1, Data(bcId.utf8), to: &data)
        appendVarintField(3, input.interrupt ? 1 : 0, to: &data)
        appendVarintField(7, 33, to: &data)
        appendBytesField(10, conversationAction, to: &data)
        if let requestedModel = try cloudAgentRequestedModel(
            modelId: input.modelId,
            modelParams: input.modelParams
        ) {
            appendBytesField(15, requestedModel, to: &data)
        }
        return data
    }

    static func decodeFollowupRunId(_ data: Data) throws -> String {
        var reader = Reader(data)
        for (field, value) in try reader.readFields() where field == 1 {
            if case .bytes(let bytes) = value, let runId = nonemptyUTF8(bytes) {
                return runId
            }
        }
        throw IOSCursorDashboardError(message: "Cloud agent follow-up response was missing run id.")
    }

    static func pauseBackgroundComposerRequest(_ bcId: String) throws -> Data {
        var data = try backgroundComposerIdRequest(bcId)
        appendVarintField(3, 33, to: &data)
        return data
    }

    static func renameBackgroundComposerRequest(_ bcId: String, title: String) throws -> Data {
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty else {
            throw IOSCursorDashboardError(message: "Cloud agent rename requires a title.")
        }
        var data = try backgroundComposerIdRequest(bcId)
        appendBytesField(2, Data(title.utf8), to: &data)
        return data
    }

    static func archiveBackgroundComposerRequest(_ bcId: String, archived: Bool) throws -> Data {
        var data = try backgroundComposerIdRequest(bcId)
        appendVarintField(2, archived ? 0 : 1, to: &data)
        appendVarintField(4, 33, to: &data)
        return data
    }

    static func decodeBackgroundComposerArtifacts(_ data: Data) throws -> [IOSCloudAgentArtifact] {
        var reader = Reader(data)
        var artifacts: [IOSCloudAgentArtifact] = []
        for (field, value) in try reader.readFields() where field == 1 {
            guard case .bytes(let message) = value else { continue }
            var nested = Reader(message)
            var path = ""
            var size: UInt64 = 0
            for (nestedField, nestedValue) in try nested.readFields() {
                switch (nestedField, nestedValue) {
                case (1, .bytes(let bytes)): path = String(data: bytes, encoding: .utf8) ?? ""
                case (2, .varint(let raw)): size = raw
                default: break
                }
            }
            if !path.isEmpty { artifacts.append(.init(path: path, sizeBytes: size)) }
        }
        return artifacts
    }

    static func decodeBackgroundComposerConversation(_ data: Data) throws -> [Data] {
        var reader = Reader(data)
        return try reader.readFields().compactMap { field, value in
            guard field == 1, case .bytes(let bytes) = value else { return nil }
            return bytes
        }
    }

    static func optimizedDiffRequest(_ bcId: String) throws -> Data {
        var data = try backgroundComposerIdRequest(bcId)
        appendVarintField(2, 1, to: &data)
        return data
    }

    static func decodeOptimizedDiff(_ data: Data) throws -> [IOSCloudAgentFileChange] {
        var response = Reader(data)
        var diffMessage: Data?
        for (field, value) in try response.readFields() where field == 1 {
            if case .bytes(let bytes) = value { diffMessage = bytes; break }
        }
        guard let diffMessage else { return [] }
        var diff = Reader(diffMessage)
        var output: [IOSCloudAgentFileChange] = []
        for (field, value) in try diff.readFields() where field == 1 {
            guard case .bytes(let message) = value else { continue }
            var file = Reader(message)
            var from = ""
            var to = ""
            var added: Int32 = 0
            var removed: Int32 = 0
            for (nestedField, nestedValue) in try file.readFields() {
                switch (nestedField, nestedValue) {
                case (1, .bytes(let bytes)): from = String(data: bytes, encoding: .utf8) ?? ""
                case (2, .bytes(let bytes)): to = String(data: bytes, encoding: .utf8) ?? ""
                case (4, .varint(let raw)): added = Int32(truncatingIfNeeded: raw)
                case (5, .varint(let raw)): removed = Int32(truncatingIfNeeded: raw)
                default: break
                }
            }
            let path = (to == "/dev/null" || to.isEmpty) ? from : to
            if !path.isEmpty, path != "/dev/null", output.count < 300 {
                output.append(.init(path: path, added: Int(added), removed: Int(removed)))
            }
        }
        return output
    }

    static func getEnvironmentRequest(publicId: String) throws -> Data {
        let publicId = publicId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !publicId.isEmpty else {
            throw IOSCursorDashboardError(message: "Saved environment id is required.")
        }
        var data = Data()
        appendBytesField(1, Data(publicId.utf8), to: &data)
        return data
    }

    static func listEnvironmentsRequest(limit: Int) -> Data {
        var data = Data()
        appendVarintField(1, UInt64(max(1, min(limit, Int(Int32.max)))), to: &data)
        appendVarintField(4, 1, to: &data)
        return data
    }

    static func decodeSavedEnvironment(_ data: Data) throws -> IOSCloudAgentSavedEnvironment? {
        var response = Reader(data)
        for (field, value) in try response.readFields() where field == 1 {
            guard case .bytes(let message) = value else { continue }
            return try decodeSavedEnvironmentMessage(message)
        }
        return nil
    }

    static func decodeSavedEnvironmentList(_ data: Data) throws -> [IOSCloudAgentSavedEnvironment] {
        var response = Reader(data)
        var output: [IOSCloudAgentSavedEnvironment] = []
        for (field, value) in try response.readFields() where field == 1 {
            guard case .bytes(let message) = value else { continue }
            output.append(try decodeSavedEnvironmentMessage(message))
        }
        return output
    }

    private static func decodeSavedEnvironmentMessage(_ message: Data) throws -> IOSCloudAgentSavedEnvironment {
        var reader = Reader(message)
        var publicId = ""
        var name = ""
        var repoConfig: Data?
        for (field, value) in try reader.readFields() {
            switch (field, value) {
            case (2, .bytes(let bytes)): publicId = String(data: bytes, encoding: .utf8) ?? ""
            case (3, .bytes(let bytes)): name = String(data: bytes, encoding: .utf8) ?? ""
            case (7, .bytes(let bytes)): repoConfig = bytes
            default: break
            }
        }
        var repos: [IOSCloudAgentEnvironmentRepo] = []
        if let repoConfig {
            var config = Reader(repoConfig)
            for (field, value) in try config.readFields() where field == 1 {
                guard case .bytes(let entry) = value else { continue }
                var repo = Reader(entry)
                var repoURL = ""
                var node: String?
                var enterprise: String?
                for (repoField, repoValue) in try repo.readFields() {
                    switch (repoField, repoValue) {
                    case (1, .bytes(let bytes)): repoURL = String(data: bytes, encoding: .utf8) ?? ""
                    case (2, .bytes(let bytes)): node = nonemptyUTF8(bytes)
                    case (3, .bytes(let bytes)): enterprise = nonemptyUTF8(bytes)
                    default: break
                    }
                }
                if !repoURL.isEmpty {
                    repos.append(.init(repoURL: repoURL, scmRepoNodeId: node, gitEnterpriseUUID: enterprise))
                }
            }
        }
        return .init(publicId: publicId, name: name, repos: repos)
    }

    static func decodeCloudAgentsDisabledByTeamAdmin(_ data: Data) throws -> Bool {
        var response = Reader(data)
        var settings: Data?
        for (field, value) in try response.readFields() where field == 7 {
            if case .bytes(let bytes) = value { settings = bytes; break }
        }
        guard let settings else { return false }
        var nested = Reader(settings)
        for (field, value) in try nested.readFields() where field == 24 {
            if case .varint(let raw) = value { return raw != 0 }
        }
        return false
    }

    private static func cloudAgentRequestedModel(
        modelId: String?,
        modelParams: [String: String]
    ) throws -> Data? {
        let modelId = modelId?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if modelId.isEmpty {
            guard modelParams.isEmpty else {
                throw IOSCursorDashboardError(message: "'model' is required when 'model_params' are provided.")
            }
            return nil
        }
        var model = Data()
        appendBytesField(1, Data(modelId.utf8), to: &model)
        appendVarintField(2, 1, to: &model)
        for key in modelParams.keys.sorted() {
            guard let value = modelParams[key] else { continue }
            var parameter = Data()
            appendBytesField(1, Data(key.utf8), to: &parameter)
            appendBytesField(2, Data(value.utf8), to: &parameter)
            appendBytesField(3, parameter, to: &model)
        }
        return model
    }

    private static func nonemptyUTF8(_ bytes: Data) -> String? {
        guard let value = String(data: bytes, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines),
              !value.isEmpty else { return nil }
        return value
    }

    private static func nonnegativeInt32(_ value: UInt64) -> Int? {
        let signed = Int32(truncatingIfNeeded: value)
        return signed >= 0 ? Int(signed) : nil
    }

    private static func cloudAgentPRState(_ value: Int32) -> String? {
        switch value {
        case 1: "open"
        case 2: "draft"
        case 3: "merged"
        case 4: "closed"
        default: nil
        }
    }

    static func decodeTeams(_ data: Data) throws -> [IOSCursorSkillPublishTeam] {
        var reader = Reader(data)
        var teams: [IOSCursorSkillPublishTeam] = []
        for (field, value) in try reader.readFields() where field == 1 {
            guard case .bytes(let message) = value else { continue }
            var nested = Reader(message)
            var name = ""
            var id: Int32 = 0
            var isDirectMember = false
            for (nestedField, nestedValue) in try nested.readFields() {
                switch (nestedField, nestedValue) {
                case (1, .bytes(let bytes)):
                    name = String(data: bytes, encoding: .utf8) ?? ""
                case (2, .varint(let value)):
                    id = Int32(truncatingIfNeeded: value)
                case (36, .varint(let value)):
                    isDirectMember = value != 0
                default:
                    break
                }
            }
            teams.append(.init(teamId: id, name: name, isDirectMember: isDirectMember))
        }
        return teams
    }


    static func autoReviewClassifierRequest(
        _ request: IOSCursorAutoReviewClassifierRequest
    ) throws -> Data {
        let action = request.action.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !action.isEmpty else {
            throw IOSCursorDashboardError(message: "Auto-review classifier action is required.")
        }
        let mode = request.mode.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !mode.isEmpty else {
            throw IOSCursorDashboardError(message: "Auto-review classifier mode is required.")
        }

        var args = Data()
        if let parentConversationId = request.parentConversationId?
            .trimmingCharacters(in: .whitespacesAndNewlines),
           !parentConversationId.isEmpty {
            appendBytesField(2, Data(parentConversationId.utf8), to: &args)
        }

        var target = Data()
        appendBytesField(1, Data(action.utf8), to: &target)
        if !request.arguments.isEmpty {
            appendBytesField(2, try encodeProtoStruct(request.arguments), to: &target)
        }
        appendBytesField(3, target, to: &args)

        for message in request.conversationContext {
            var encoded = Data()
            let role = message.role.trimmingCharacters(in: .whitespacesAndNewlines)
            if !role.isEmpty {
                appendBytesField(1, Data(role.utf8), to: &encoded)
            }
            if !message.content.isEmpty {
                appendBytesField(2, Data(message.content.utf8), to: &encoded)
            }
            appendBytesField(4, encoded, to: &args)
        }

        var data = Data()
        appendBytesField(1, args, to: &data)
        appendVarintField(2, UInt64(request.attemptIndex), to: &data)
        appendBytesField(3, Data(mode.utf8), to: &data)
        return data
    }

    static func decodeAutoReviewClassifierResponse(
        _ data: Data
    ) throws -> IOSCursorAutoReviewClassifierResult {
        guard !data.isEmpty else { return .missing }
        var outer = Reader(data)
        guard let resultData = try outer.readFields().compactMap({ field, value -> Data? in
            guard field == 1, case .bytes(let bytes) = value else { return nil }
            return bytes
        }).first else {
            return .missing
        }

        var result = Reader(resultData)
        for (field, value) in try result.readFields() {
            guard case .bytes(let bytes) = value else { continue }
            if field == 1 {
                var success = Reader(bytes)
                var decision = IOSCursorAutoReviewDecision.unspecified
                var blockReason: String?
                var proposedAllowRule: String?
                for (successField, successValue) in try success.readFields() {
                    switch (successField, successValue) {
                    case (1, .varint(let raw)):
                        let value = Int32(truncatingIfNeeded: raw)
                        switch value {
                        case 0: decision = .unspecified
                        case 1: decision = .allow
                        case 2: decision = .block
                        default: decision = .unknown(value)
                        }
                    case (2, .bytes(let raw)):
                        let value = String(data: raw, encoding: .utf8)?
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                        blockReason = value?.isEmpty == false ? value : nil
                    case (3, .bytes(let raw)):
                        let value = String(data: raw, encoding: .utf8)?
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                        proposedAllowRule = value?.isEmpty == false ? value : nil
                    default:
                        break
                    }
                }
                return .success(.init(
                    decision: decision,
                    blockReason: blockReason,
                    proposedAllowRule: proposedAllowRule
                ))
            }
            if field == 2 {
                var backend = Reader(bytes)
                var message = ""
                for (errorField, errorValue) in try backend.readFields() {
                    if errorField == 1, case .bytes(let raw) = errorValue {
                        message = String(data: raw, encoding: .utf8) ?? ""
                    }
                }
                let marker = "\n\nSmartModeClassifierFailureMetadata:"
                var visibleMessage = message
                var failureReason: String?
                var retryable: Bool?
                if let range = message.range(of: marker, options: .backwards) {
                    let rawMetadata = String(message[range.upperBound...])
                    visibleMessage = String(message[..<range.lowerBound])
                    if let json = rawMetadata.data(using: .utf8),
                       let object = try? JSONSerialization.jsonObject(with: json) as? [String: Any] {
                        failureReason = (object["failureReason"] as? String)?
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                        retryable = object["retryable"] as? Bool
                    }
                }
                return .backendError(
                    message: visibleMessage.trimmingCharacters(in: .whitespacesAndNewlines),
                    failureReason: failureReason?.isEmpty == false ? failureReason : nil,
                    retryable: retryable
                )
            }
        }
        return .missing
    }

    private static func encodeProtoStruct(
        _ object: [String: IOSCursorAutoReviewJSONValue]
    ) throws -> Data {
        var data = Data()
        for key in object.keys.sorted() {
            guard let value = object[key] else { continue }
            var entry = Data()
            appendBytesField(1, Data(key.utf8), to: &entry)
            appendBytesField(2, try encodeProtoValue(value), to: &entry)
            appendBytesField(1, entry, to: &data)
        }
        return data
    }

    private static func encodeProtoList(
        _ values: [IOSCursorAutoReviewJSONValue]
    ) throws -> Data {
        var data = Data()
        for value in values {
            appendBytesField(1, try encodeProtoValue(value), to: &data)
        }
        return data
    }

    private static func encodeProtoValue(
        _ value: IOSCursorAutoReviewJSONValue
    ) throws -> Data {
        var data = Data()
        switch value {
        case .null:
            appendVarintField(1, 0, to: &data)
        case .number(let number):
            guard number.isFinite else {
                throw IOSCursorDashboardError(message: "Auto-review classifier arguments contain a non-finite number.")
            }
            appendFixed64Field(2, number.bitPattern, to: &data)
        case .string(let string):
            appendBytesField(3, Data(string.utf8), to: &data)
        case .bool(let bool):
            appendVarintField(4, bool ? 1 : 0, to: &data)
        case .object(let object):
            appendBytesField(5, try encodeProtoStruct(object), to: &data)
        case .array(let values):
            appendBytesField(6, try encodeProtoList(values), to: &data)
        }
        return data
    }

    static func decodePublishedSkill(_ data: Data) throws -> IOSCursorPublishedSkill {
        var reader = Reader(data)
        var pluginId: Int64 = 0
        var commitSha = ""
        for (field, value) in try reader.readFields() {
            switch (field, value) {
            case (1, .varint(let value)):
                pluginId = Int64(bitPattern: value)
            case (3, .bytes(let bytes)):
                commitSha = String(data: bytes, encoding: .utf8) ?? ""
            default:
                break
            }
        }
        guard pluginId > 0, !commitSha.isEmpty else {
            throw IOSCursorDashboardError(message: "Dashboard publish response was missing plugin identity or commit SHA.")
        }
        return .init(pluginId: String(pluginId), commitSha: commitSha)
    }

    private static func appendVarintField(_ field: Int, _ value: UInt64, to data: inout Data) {
        appendVarint(UInt64(field << 3), to: &data)
        appendVarint(value, to: &data)
    }

    private static func appendBytesField(_ field: Int, _ value: Data, to data: inout Data) {
        appendVarint(UInt64((field << 3) | 2), to: &data)
        appendVarint(UInt64(value.count), to: &data)
        data.append(value)
    }

    private static func appendFixed64Field(_ field: Int, _ value: UInt64, to data: inout Data) {
        appendVarint(UInt64((field << 3) | 1), to: &data)
        var littleEndian = value.littleEndian
        withUnsafeBytes(of: &littleEndian) { bytes in
            data.append(contentsOf: bytes)
        }
    }

    private static func appendVarint(_ value: UInt64, to data: inout Data) {
        var value = value
        repeat {
            var byte = UInt8(value & 0x7f)
            value >>= 7
            if value != 0 { byte |= 0x80 }
            data.append(byte)
        } while value != 0
    }
}

struct IOSCursorSkillPublishTeam: Equatable, Sendable {
    let teamId: Int32
    let name: String
    let isDirectMember: Bool
}

struct IOSCursorPublishedSkill: Equatable, Sendable {
    let pluginId: String
    let commitSha: String
}

struct IOSCloudAgentComposerInfo: Equatable, Sendable {
    let status: Int32
    let name: String?
    let prompt: String?
    let summary: String?
    let permanentError: String?
    let branchName: String?
    let filesChanged: Int?
    let linesAdded: Int?
    let linesRemoved: Int?
    let prURL: String?
    let prState: String?
    let prNumber: Int32?

    var isActive: Bool { status == 1 || status == 4 }
    var isError: Bool { status == 3 || status == 5 || permanentError != nil }
}

struct IOSCloudAgentImageInput: Equatable, Sendable {
    let data: Data
    let path: String
    let mimeType: String
}

struct IOSCloudAgentWorkerLabel: Equatable, Sendable {
    let key: String
    let value: String
}

struct IOSCloudAgentEnvironmentRepo: Equatable, Sendable {
    let repoURL: String
    let scmRepoNodeId: String?
    let gitEnterpriseUUID: String?
}

struct IOSCloudAgentSavedEnvironment: Equatable, Sendable {
    let publicId: String
    let name: String
    let repos: [IOSCloudAgentEnvironmentRepo]
}

struct IOSCloudAgentLaunchInput: Equatable, Sendable {
    let bcId: String
    let snapshotNameOrId: String
    let repoURL: String
    let startingRef: String?
    let prompt: String
    let images: [IOSCloudAgentImageInput]
    let title: String?
    let usePrivateWorker: Bool?
    let labels: [IOSCloudAgentWorkerLabel]
    let teamId: Int32?
    let environmentName: String?
    let environmentPublicId: String?
    let environmentRepos: [IOSCloudAgentEnvironmentRepo]
    let modelId: String?
    let modelParams: [String: String]
}

struct IOSCloudAgentReplyInput: Equatable, Sendable {
    let bcId: String
    let prompt: String
    let images: [IOSCloudAgentImageInput]
    let interrupt: Bool
    let modelId: String?
    let modelParams: [String: String]
}

struct IOSCloudAgentLaunchResult: Equatable, Sendable {
    let bcId: String
    let url: String
    let initialRunId: String?
}

struct IOSCloudAgentSummary: Equatable, Sendable {
    let bcId: String
    let name: String
    let status: Int32
    let branchName: String
    let prURL: String
    let isArchived: Bool
}

struct IOSCloudAgentArtifact: Equatable, Sendable {
    let path: String
    let sizeBytes: UInt64
}

struct IOSCloudAgentFileChange: Equatable, Sendable {
    let path: String
    let added: Int
    let removed: Int
}

struct IOSCursorTranscriptionResult: Equatable, Sendable {
    let text: String
    let transcriptionTimeMs: Int64
}

struct IOSCursorSandBoxRunState: Equatable, Sendable {
    let imageUpdateAvailable: Bool
}

struct IOSCursorSandBoxRecreateResult: Equatable, Sendable {
    let started: Bool
    let reason: String
    let operationId: String
}

struct IOSCursorSandBoxConnection: Codable, Equatable, Sendable {
    let networkToken: String
    let execDaemonAuthToken: String
    let execDaemonURL: String
    let vncURL: String
    let gatewayURL: String
    let gatewayToken: String
    let forkVNCBaseURL: String
}

struct IOSCursorSandBoxMigrationEvent: Equatable, Sendable {
    let phase: Int32
    let detail: String
    let atMs: Int64
    let offsetKey: String
    let operationId: String

    var phaseName: String? {
        switch phase {
        case 1: "backing-up"
        case 2: "creating"
        case 3: "moving"
        case 4: "cleaning-up"
        case 5: "wiping"
        case 6: "done"
        case 7: "failed"
        default: nil
        }
    }

    var isTerminal: Bool { phase == 6 || phase == 7 }
}

struct IOSCursorConnectEnvelope: Equatable, Sendable {
    let flags: UInt8
    let payload: Data

    var isEndStream: Bool { flags & 0x02 != 0 }
}

struct IOSCursorConnectEnvelopeDecoder: Sendable {
    private var buffer: [UInt8] = []

    mutating func append(_ byte: UInt8) throws -> [IOSCursorConnectEnvelope] {
        buffer.append(byte)
        return try drain()
    }

    mutating func append(_ data: Data) throws -> [IOSCursorConnectEnvelope] {
        buffer.append(contentsOf: data)
        return try drain()
    }

    private mutating func drain() throws -> [IOSCursorConnectEnvelope] {
        var envelopes: [IOSCursorConnectEnvelope] = []
        while buffer.count >= 5 {
            let flags = buffer[0]
            guard flags & 0xfc == 0 else {
                throw IOSCursorDashboardError(message: "Connect stream used reserved envelope flags.")
            }
            guard flags & 0x01 == 0 else {
                throw IOSCursorDashboardError(message: "Compressed Connect migration envelopes are unsupported.")
            }
            let length =
                (UInt32(buffer[1]) << 24) |
                (UInt32(buffer[2]) << 16) |
                (UInt32(buffer[3]) << 8) |
                UInt32(buffer[4])
            let total = 5 + Int(length)
            guard buffer.count >= total else { break }
            envelopes.append(.init(
                flags: flags,
                payload: Data(buffer[5..<total])
            ))
            buffer.removeFirst(total)
        }
        return envelopes
    }

    func requireEmptyAtEOF() throws {
        guard buffer.isEmpty else {
            throw IOSCursorDashboardError(message: "Connect migration stream ended inside an envelope.")
        }
    }
}

struct IOSCursorDashboardJSON: @unchecked Sendable {
    let value: [String: Any]
}

final class IOSCursorDashboardClient: @unchecked Sendable, AccountMcpClient, DashboardMcpExecClient, SandMarketplaceListingClient, ProductAnalyticsClient {
    static func decodeSandBoxRunStateForTests(_ data: Data) throws -> IOSCursorSandBoxRunState {
        try IOSCursorDashboardProto.decodeSandBoxRunStateResponse(data)
    }

    static func decodeEnsureSandBoxConnectionForTests(_ data: Data) throws -> IOSCursorSandBoxConnection {
        try IOSCursorDashboardProto.decodeEnsureSandBoxResponse(data)
    }

    static func ensureSandBoxRequestForTests() -> Data {
        IOSCursorDashboardProto.ensureSandBoxRequest()
    }

    static func decodePrReviewUserDestinationForTests(_ data: Data) throws -> SandPrReviewDestination? {
        try IOSCursorDashboardProto.decodePrReviewUserDestination(data)
    }

    static func decodePrReviewTeamDestinationForTests(_ data: Data) throws -> SandPrReviewDestination? {
        try IOSCursorDashboardProto.decodePrReviewTeamDestination(data)
    }

    static func decodeBackgroundComposerInfoForTests(_ data: Data) throws -> IOSCloudAgentComposerInfo {
        try IOSCursorDashboardProto.decodeBackgroundComposerInfo(data)
    }

    static func startBackgroundComposerRequestForTests(
        _ input: IOSCloudAgentLaunchInput
    ) throws -> Data {
        try IOSCursorDashboardProto.startBackgroundComposerRequest(input)
    }

    static func autoReviewClassifierRequestForTests(
        _ request: IOSCursorAutoReviewClassifierRequest
    ) throws -> Data {
        try IOSCursorDashboardProto.autoReviewClassifierRequest(request)
    }

    static func decodeAutoReviewClassifierResponseForTests(
        _ data: Data
    ) throws -> IOSCursorAutoReviewClassifierResult {
        try IOSCursorDashboardProto.decodeAutoReviewClassifierResponse(data)
    }

    typealias RequestExecutor = @Sendable (URLRequest) async throws -> (Data, URLResponse)
    typealias BackendUpdateRequirementSink = @MainActor (Bool) -> Void

    private let credentials: AccountMcpCredentials
    private let backendURL: URL
    private let session: URLSession
    private let requestExecutor: RequestExecutor?
    private let backendUpdateRequirementSink: BackendUpdateRequirementSink
    private let avatarCache = NSCache<NSString, NSString>()

    init(
        credentials: AccountMcpCredentials,
        backendURL: URL = URL(string: getConfiguredBackendUrl())!,
        session: URLSession = .shared,
        requestExecutor: RequestExecutor? = nil,
        backendUpdateRequirementSink: @escaping BackendUpdateRequirementSink = { _ in }
    ) {
        self.credentials = credentials
        self.backendURL = backendURL
        self.session = session
        self.requestExecutor = requestExecutor
        self.backendUpdateRequirementSink = backendUpdateRequirementSink
    }

    func classifySandAutoReview(
        _ request: IOSCursorAutoReviewClassifierRequest,
        timeoutMs: Int = 10_000
    ) async throws -> IOSCursorAutoReviewClassifierResult {
        try Task.checkCancellation()
        let body = try IOSCursorDashboardProto.autoReviewClassifierRequest(request)
        let data = try await protoRPC(
            "ClassifySandAutoReview",
            body: body,
            timeoutMs: min(max(1, timeoutMs), 10_000)
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeAutoReviewClassifierResponse(data)
    }

    func trackEvents(_ events: [ProductAnalyticsEvent]) async throws {
        try Task.checkCancellation()
        guard !events.isEmpty else { return }
        _ = try await rpc(
            "TrackEvents",
            service: "AnalyticsService",
            body: try Self.productAnalyticsRequestBody(events),
            timeoutMs: 2_500
        )
    }

    func automationServiceUnary(
        method: String,
        bodyBase64: String,
        timeoutMs: Int = 30_000
    ) async throws -> String {
        let allowed = Set([
            "ListSandAutomations",
            "CreateSandAutomation",
            "UpdateSandAutomation",
            "DeleteSandAutomation",
        ])
        guard allowed.contains(method) else {
            throw IOSCursorDashboardError(
                message: "Unsupported AutomationsService method \(method)."
            )
        }
        guard let body = Data(base64Encoded: bodyBase64) else {
            throw IOSCursorDashboardError(
                message: "AutomationsService request body is not valid base64."
            )
        }
        try Task.checkCancellation()
        let data = try await protoRPC(
            method,
            service: "AutomationsService",
            body: body,
            timeoutMs: min(max(1, timeoutMs), 30_000)
        )
        try Task.checkCancellation()
        return data.base64EncodedString()
    }

    func registerAutomationRelay(
        slackChannels: [String],
        githubRepos: [String],
        githubKinds: [String],
        timeoutMs: Int = 30_000
    ) async throws -> [String: Any] {
        try Task.checkCancellation()
        let body: [String: Any] = [
            "slackChannels": Array(Set(slackChannels)).sorted(),
            "githubRepos": Array(Set(githubRepos.map { $0.lowercased() })).sorted(),
            "githubKinds": Array(Set(githubKinds)).sorted(),
        ]
        let result = try await automationBackendPost(
            path: "/sand/listener-subscriptions",
            body: body,
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return result
    }

    func drainAutomationRelayEvents(
        ackIds: [String],
        timeoutMs: Int = 30_000
    ) async throws -> [String: Any] {
        try Task.checkCancellation()
        let result = try await automationBackendPost(
            path: "/sand/listener-events/poll",
            body: ["ackIds": Array(Set(ackIds)).sorted()],
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return result
    }

    func pollAutomationFires(
        ackRunUuids: [String],
        timeoutMs: Int = 30_000
    ) async throws -> [String: Any] {
        try Task.checkCancellation()
        let result = try await automationBackendPost(
            path: "/sand/automation-events/poll",
            body: ["ackRunUuids": Array(Set(ackRunUuids)).sorted()],
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return result
    }

    func completeAutomationRun(
        runUuid: String,
        status: String,
        errorMessage: String?,
        timeoutMs: Int = 30_000
    ) async throws {
        try Task.checkCancellation()
        var body: [String: Any] = [
            "runUuid": runUuid,
            "status": status,
        ]
        body["errorMessage"] = errorMessage ?? NSNull()
        do {
            _ = try await automationBackendPost(
                path: "/sand/automation-runs/complete",
                body: body,
                timeoutMs: timeoutMs
            )
        } catch let error as IOSCursorDashboardError {
            // Desktop treats a missing or already-completed run as terminally reported.
            // Preserve that idempotency so completion retries cannot loop forever.
            guard error.httpStatus == 404 || error.httpStatus == 409 else {
                throw error
            }
        }
        try Task.checkCancellation()
    }

    static func productAnalyticsRequestBodyForTests(
        _ events: [ProductAnalyticsEvent]
    ) throws -> [String: Any] {
        try productAnalyticsRequestBody(events)
    }

    private static func productAnalyticsRequestBody(
        _ events: [ProductAnalyticsEvent]
    ) throws -> [String: Any] {
        let rows = try events.map { event -> [String: Any] in
            var eventData: [String: Any] = [:]
            for (key, value) in event.props {
                switch value {
                case .string(let string):
                    eventData[key] = ["stringValue": string]
                case .bool(let bool):
                    eventData[key] = ["boolValue": bool]
                case .int(let integer):
                    eventData[key] = ["intValue": String(integer)]
                case .double(let double):
                    guard double.isFinite else {
                        throw IOSCursorDashboardError(
                            message: "Analytics event contains a non-finite double."
                        )
                    }
                    eventData[key] = ["doubleValue": double]
                }
            }
            let timestampMilliseconds = Int64(
                (event.timestamp.timeIntervalSince1970 * 1_000).rounded()
            )
            return [
                "eventName": event.eventName,
                "eventData": eventData,
                "timestamp": String(timestampMilliseconds),
            ]
        }
        return ["events": rows]
    }

    private func perform(_ request: URLRequest) async throws -> (Data, URLResponse) {
        if let requestExecutor {
            return try await requestExecutor(request)
        }
        return try await session.data(for: request)
    }

    private func automationBackendPost(
        path: String,
        body: [String: Any],
        timeoutMs: Int
    ) async throws -> [String: Any] {
        guard path.hasPrefix("/sand/") else {
            throw IOSCursorDashboardError(message: "Automation backend path must remain inside /sand/.")
        }
        let headers = try await cursorHeaders()
        guard let url = URL(string: path, relativeTo: backendURL)?.absoluteURL else {
            throw IOSCursorDashboardError(message: "Automation backend URL was invalid.")
        }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = TimeInterval(min(max(1, timeoutMs), 30_000)) / 1_000
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        for (name, value) in headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.httpBody = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
        let (data, response) = try await perform(request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorDashboardError(message: "Automation backend returned no HTTP response.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let detail = String(data: data, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            throw IOSCursorDashboardError(
                message: detail?.isEmpty == false
                    ? String(detail!.prefix(512))
                    : "Automation backend \(path) failed with HTTP \(http.statusCode).",
                httpStatus: http.statusCode
            )
        }
        if data.isEmpty { return [:] }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw IOSCursorDashboardError(message: "Automation backend \(path) returned invalid JSON.")
        }
        return object
    }

    private func rpc(
        _ method: String,
        service: String = "DashboardService",
        body: [String: Any],
        timeoutMs: Int
    ) async throws -> [String: Any] {
        let backend = backendURL.absoluteString
        let headers = try await createSandInferenceHeaders(
            backendUrl: backend,
            getAccessToken: { value in
                try await self.credentials.getAccessToken(value)
            },
            getMachineId: credentials.getMachineId,
            resolveGhostMode: { _ in "true" }
        )
        let url = backendURL
            .appendingPathComponent("aiserver.v1.\(service)")
            .appendingPathComponent(method)
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = TimeInterval(max(1, timeoutMs)) / 1_000
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        for (name, value) in headers.headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await perform(request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorDashboardError(message: "Dashboard RPC returned no HTTP response.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let errorBody = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            let detail = errorBody?["message"] as? String
            throw IOSCursorDashboardError(
                message: detail?.isEmpty == false
                    ? detail!
                    : "Dashboard RPC \(method) failed with HTTP \(http.statusCode).",
                httpStatus: http.statusCode,
                retryAfterMilliseconds: iosCursorRetryAfterMilliseconds(
                    http.value(forHTTPHeaderField: "retry-after")
                )
            )
        }
        if data.isEmpty { return [:] }
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw IOSCursorDashboardError(message: "Dashboard RPC \(method) returned invalid JSON.")
        }
        return object
    }


    private func protoRPC(
        _ method: String,
        service: String = "DashboardService",
        body: Data,
        timeoutMs: Int
    ) async throws -> Data {
        let backend = backendURL.absoluteString
        let headers = try await createSandInferenceHeaders(
            backendUrl: backend,
            getAccessToken: { value in
                try await self.credentials.getAccessToken(value)
            },
            getMachineId: credentials.getMachineId,
            resolveGhostMode: { _ in "true" }
        )
        let url = backendURL
            .appendingPathComponent("aiserver.v1.\(service)")
            .appendingPathComponent(method)
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = TimeInterval(max(1, timeoutMs)) / 1_000
        request.setValue("application/proto", forHTTPHeaderField: "Content-Type")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        for (name, value) in headers.headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.httpBody = body
        let (data, response) = try await perform(request)
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorDashboardError(message: "Dashboard Connect RPC returned no HTTP response.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let detail = String(data: data, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .prefix(512)
            let suffix = detail.map { $0.isEmpty ? "" : ": \($0)" } ?? ""
            throw IOSCursorDashboardError(
                message: "Dashboard Connect RPC \(method) failed with HTTP \(http.statusCode)\(suffix)",
                httpStatus: http.statusCode,
                retryAfterMilliseconds: iosCursorRetryAfterMilliseconds(
                    http.value(forHTTPHeaderField: "retry-after")
                )
            )
        }
        return data
    }

    private func cursorHeaders() async throws -> [String: String] {
        try await createSandInferenceHeaders(
            backendUrl: backendURL.absoluteString,
            getAccessToken: { value in
                try await self.credentials.getAccessToken(value)
            },
            getMachineId: credentials.getMachineId,
            resolveGhostMode: { _ in "true" }
        ).headers
    }

    private func grokBotRequest(
        method: String,
        body: Data,
        contentType: String,
        timeoutMs: Int
    ) async throws -> URLRequest {
        let url = backendURL
            .appendingPathComponent("aiserver.v1.GrokBotService")
            .appendingPathComponent(method)
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = TimeInterval(max(1, timeoutMs)) / 1_000
        request.setValue(contentType, forHTTPHeaderField: "Content-Type")
        request.setValue("1", forHTTPHeaderField: "Connect-Protocol-Version")
        for (name, value) in try await cursorHeaders() {
            request.setValue(value, forHTTPHeaderField: name)
        }
        request.httpBody = body
        return request
    }

    private func validateConnectHTTP(
        _ response: URLResponse,
        method: String,
        responseBody: Data? = nil
    ) throws {
        guard let http = response as? HTTPURLResponse else {
            throw IOSCursorDashboardError(message: "GrokBot Connect RPC returned no HTTP response.")
        }
        guard (200..<300).contains(http.statusCode) else {
            let hint = http.value(forHTTPHeaderField: "x-automation-failure-hint")
            let retryAfterMilliseconds = http.value(forHTTPHeaderField: "retry-after")
                .flatMap(Double.init)
                .map { Int(min(max($0, 0), 15 * 60) * 1_000) }
            let bodyDetail = responseBody
                .flatMap { String(data: $0, encoding: .utf8) }?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let detail = bodyDetail?.isEmpty == false ? String(bodyDetail!.prefix(512)) : nil

            let kind: IOSCursorSandBoxBrokerError.Kind
            let message: String
            switch hint {
            case CLOUD_AGENT_STORAGE_DISABLED:
                kind = .noStorage
                message = GATEWAY_NO_STORAGE_MESSAGE_MARKER
            case SAND_BOX_BLOCKED:
                kind = .blocked(retryAfterMilliseconds: retryAfterMilliseconds)
                message = detail ?? encodeSandBoxBlockedMessage(.init(
                    reason: "",
                    title: "",
                    detail: ""
                ))
            case "SAND_CLIENT_UPDATE_REQUIRED":
                kind = .updateRequired
                message = detail ?? "Sand client update is required."
            default:
                if http.statusCode == 401 || http.statusCode == 403 {
                    kind = .accessDenied
                    message = GATEWAY_ACCESS_DENIED_MESSAGE_MARKER
                } else {
                    kind = .http(status: http.statusCode)
                    message = detail ?? "GrokBot Connect RPC \(method) failed with HTTP \(http.statusCode)."
                }
            }
            throw IOSCursorSandBoxBrokerError(kind: kind, message: message)
        }
    }

    private func connectEnvelope(_ message: Data, flags: UInt8 = 0) throws -> Data {
        guard message.count <= Int(UInt32.max) else {
            throw IOSCursorDashboardError(message: "Connect request envelope is too large.")
        }
        let length = UInt32(message.count)
        return Data([
            flags,
            UInt8((length >> 24) & 0xff),
            UInt8((length >> 16) & 0xff),
            UInt8((length >> 8) & 0xff),
            UInt8(length & 0xff),
        ]) + message
    }

    private func consumeMigrationEnvelope(
        _ envelope: IOSCursorConnectEnvelope,
        continuation: AsyncThrowingStream<IOSCursorSandBoxMigrationEvent, Error>.Continuation
    ) throws -> Bool {
        if envelope.isEndStream {
            guard let object = try JSONSerialization.jsonObject(with: envelope.payload) as? [String: Any] else {
                throw IOSCursorDashboardError(message: "Connect migration end-stream frame was invalid JSON.")
            }
            if let error = object["error"] as? [String: Any] {
                let code = error["code"] as? String ?? "unknown"
                let message = error["message"] as? String ?? "migration stream failed"
                throw IOSCursorDashboardError(message: "GrokBot migration stream \(code): \(message)")
            }
            return true
        }
        continuation.yield(try IOSCursorDashboardProto.decodeSandBoxMigrationEvent(envelope.payload))
        return false
    }

    func getSandBoxRunState(
        timeoutMs: Int = 10_000
    ) async throws -> IOSCursorSandBoxRunState {
        let request = try await grokBotRequest(
            method: "GetSandBoxRunState",
            body: Data(),
            contentType: "application/proto",
            timeoutMs: timeoutMs
        )
        let (data, response) = try await perform(request)
        try validateConnectHTTP(response, method: "GetSandBoxRunState")
        return try IOSCursorDashboardProto.decodeSandBoxRunStateResponse(data)
    }

    func ensureSandBox(
        timeoutMs: Int = 30_000
    ) async throws -> IOSCursorSandBoxConnection {
        let request = try await grokBotRequest(
            method: "EnsureSandBox",
            body: IOSCursorDashboardProto.ensureSandBoxRequest(),
            contentType: "application/proto",
            timeoutMs: timeoutMs
        )
        let (data, response) = try await perform(request)
        do {
            try validateConnectHTTP(
                response,
                method: "EnsureSandBox",
                responseBody: data
            )
            await backendUpdateRequirementSink(false)
            return try IOSCursorDashboardProto.decodeEnsureSandBoxResponse(data)
        } catch let error as IOSCursorSandBoxBrokerError {
            if case .updateRequired = error.kind {
                await backendUpdateRequirementSink(true)
            }
            throw error
        }
    }

    func recreateSandBox(
        preserveData: Bool,
        force: Bool,
        timeoutMs: Int = 30_000
    ) async throws -> IOSCursorSandBoxRecreateResult {
        let request = try await grokBotRequest(
            method: "RecreateSandBox",
            body: IOSCursorDashboardProto.recreateSandBoxRequest(
                preserveData: preserveData,
                force: force
            ),
            contentType: "application/proto",
            timeoutMs: timeoutMs
        )
        let (data, response) = try await perform(request)
        try validateConnectHTTP(response, method: "RecreateSandBox")
        return try IOSCursorDashboardProto.decodeRecreateSandBoxResponse(data)
    }

    func forceRecreateSandBox(
        timeoutMs: Int = 30_000
    ) async throws -> IOSCursorSandBoxRecreateResult {
        let request = try await grokBotRequest(
            method: "ForceRecreateSandBox",
            body: IOSCursorDashboardProto.forceRecreateSandBoxRequest(),
            contentType: "application/proto",
            timeoutMs: timeoutMs
        )
        let (data, response) = try await perform(request)
        try validateConnectHTTP(response, method: "ForceRecreateSandBox")
        return try IOSCursorDashboardProto.decodeRecreateSandBoxResponse(data)
    }

    func watchSandBoxMigration(
        fromOffsetKey: String,
        includeFinished: Bool = true,
        timeoutMs: Int = 86_400_000
    ) -> AsyncThrowingStream<IOSCursorSandBoxMigrationEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task { [self] in
                do {
                    let payload = IOSCursorDashboardProto.watchSandBoxMigrationRequest(
                        fromOffsetKey: fromOffsetKey,
                        includeFinished: includeFinished
                    )
                    let request = try await grokBotRequest(
                        method: "WatchSandBoxMigration",
                        body: try connectEnvelope(payload),
                        contentType: "application/connect+proto",
                        timeoutMs: timeoutMs
                    )
                    var decoder = IOSCursorConnectEnvelopeDecoder()
                    var sawEndStream = false

                    if let requestExecutor {
                        let (data, response) = try await requestExecutor(request)
                        try validateConnectHTTP(response, method: "WatchSandBoxMigration")
                        for envelope in try decoder.append(data) {
                            if try consumeMigrationEnvelope(envelope, continuation: continuation) {
                                sawEndStream = true
                                break
                            }
                        }
                    } else {
                        let (bytes, response) = try await session.bytes(for: request)
                        try validateConnectHTTP(response, method: "WatchSandBoxMigration")
                        for try await byte in bytes {
                            try Task.checkCancellation()
                            for envelope in try decoder.append(byte) {
                                if try consumeMigrationEnvelope(envelope, continuation: continuation) {
                                    sawEndStream = true
                                    break
                                }
                            }
                            if sawEndStream { break }
                        }
                    }
                    try decoder.requireEmptyAtEOF()
                    guard sawEndStream else {
                        throw IOSCursorDashboardError(message: "GrokBot migration stream ended without EndStream.")
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { @Sendable _ in
                task.cancel()
            }
        }
    }

    func getBackgroundComposerInfo(
        bcId: String,
        timeoutMs: Int = 30_000
    ) async throws -> IOSCloudAgentComposerInfo {
        let response = try await protoRPC(
            "GetBackgroundComposerInfo",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.getBackgroundComposerInfoRequest(bcId: bcId),
            timeoutMs: timeoutMs
        )
        return try IOSCursorDashboardProto.decodeBackgroundComposerInfo(response)
    }

    func startBackgroundComposer(
        _ input: IOSCloudAgentLaunchInput,
        timeoutMs: Int = 30_000
    ) async throws -> IOSCloudAgentLaunchResult {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "StartBackgroundComposerFromSnapshot",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.startBackgroundComposerRequest(input),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeStartBackgroundComposerResponse(
            response,
            fallbackBcId: input.bcId
        )
    }

    func listBackgroundComposers(
        limit: Int = 20,
        includeArchived: Bool = false,
        timeoutMs: Int = 30_000
    ) async throws -> [IOSCloudAgentSummary] {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "ListBackgroundComposers",
            service: "BackgroundComposerService",
            body: IOSCursorDashboardProto.listBackgroundComposersRequest(
                limit: limit,
                includeArchived: includeArchived
            ),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        let values = try IOSCursorDashboardProto.decodeBackgroundComposerList(response)
        return includeArchived ? values : values.filter { !$0.isArchived }
    }

    func addBackgroundComposerFollowup(
        _ input: IOSCloudAgentReplyInput,
        timeoutMs: Int = 30_000
    ) async throws -> String {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "AddAsyncFollowupBackgroundComposer",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.followupBackgroundComposerRequest(input),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeFollowupRunId(response)
    }

    func pauseBackgroundComposer(
        bcId: String,
        timeoutMs: Int = 30_000
    ) async throws {
        try Task.checkCancellation()
        _ = try await protoRPC(
            "PauseBackgroundComposer",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.pauseBackgroundComposerRequest(bcId),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
    }

    func renameBackgroundComposer(
        bcId: String,
        title: String,
        timeoutMs: Int = 30_000
    ) async throws {
        try Task.checkCancellation()
        _ = try await protoRPC(
            "RenameBackgroundComposer",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.renameBackgroundComposerRequest(bcId, title: title),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
    }

    func setBackgroundComposerArchived(
        bcId: String,
        archived: Bool,
        timeoutMs: Int = 30_000
    ) async throws {
        try Task.checkCancellation()
        _ = try await protoRPC(
            "ArchiveBackgroundComposer",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.archiveBackgroundComposerRequest(
                bcId,
                archived: archived
            ),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
    }

    func deleteBackgroundComposer(
        bcId: String,
        timeoutMs: Int = 30_000
    ) async throws {
        try Task.checkCancellation()
        _ = try await protoRPC(
            "DeleteBackgroundComposer",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.backgroundComposerIdRequest(bcId),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
    }

    func listBackgroundComposerArtifacts(
        bcId: String,
        timeoutMs: Int = 30_000
    ) async throws -> [IOSCloudAgentArtifact] {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "ListBackgroundComposerArtifacts",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.backgroundComposerIdRequest(bcId),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeBackgroundComposerArtifacts(response)
    }

    func getBackgroundComposerConversation(
        bcId: String,
        timeoutMs: Int = 30_000
    ) async throws -> [Data] {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "GetBackgroundComposerConversation",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.backgroundComposerIdRequest(bcId),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeBackgroundComposerConversation(response)
    }

    func getBackgroundComposerDiff(
        bcId: String,
        timeoutMs: Int = 30_000
    ) async throws -> [IOSCloudAgentFileChange] {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "GetOptimizedDiffDetails",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.optimizedDiffRequest(bcId),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeOptimizedDiff(response)
    }

    func getCloudAgentSavedEnvironment(
        publicId: String,
        timeoutMs: Int = 30_000
    ) async throws -> IOSCloudAgentSavedEnvironment? {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "GetEnvironment",
            service: "BackgroundComposerService",
            body: try IOSCursorDashboardProto.getEnvironmentRequest(publicId: publicId),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeSavedEnvironment(response)
    }

    func listCloudAgentSavedEnvironments(
        limit: Int = 500,
        timeoutMs: Int = 30_000
    ) async throws -> [IOSCloudAgentSavedEnvironment] {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "ListEnvironments",
            service: "BackgroundComposerService",
            body: IOSCursorDashboardProto.listEnvironmentsRequest(limit: limit),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeSavedEnvironmentList(response)
    }

    func cloudAgentsDisabledByTeamAdmin(
        timeoutMs: Int = 10_000
    ) async throws -> Bool {
        try Task.checkCancellation()
        let response = try await protoRPC(
            "GetTeamAdminSettingsOrEmptyIfNotInTeam",
            body: IOSCursorDashboardProto.teamAdminSettingsRequest(),
            timeoutMs: timeoutMs
        )
        try Task.checkCancellation()
        return try IOSCursorDashboardProto.decodeCloudAgentsDisabledByTeamAdmin(response)
    }

    func getSandAccessStatus(
        timeoutMs: Int = IOS_SAND_ACCESS_REQUEST_TIMEOUT_MS
    ) async throws -> SandAccessState {
        let response = try await rpc(
            "GetSandAccessStatus",
            body: [:],
            timeoutMs: timeoutMs
        )
        func wireInt(_ value: Any?) -> Int {
            if let number = value as? NSNumber,
               CFGetTypeID(number) != CFBooleanGetTypeID() {
                return number.intValue
            }
            if let value = value as? String, let parsed = Int(value) {
                return parsed
            }
            return 0
        }
        return iosSandAccessFromWire(
            state: wireInt(response["state"]),
            blockReason: wireInt(response["blockReason"])
        )
    }

    func getCursorAuthProfile(timeoutMs: Int = 10_000) async -> IOSCursorAuthProfile? {
        do {
            async let meTask = rpc("GetMe", body: [:], timeoutMs: timeoutMs)
            async let membershipTask = isAnysphereUser(timeoutMs: timeoutMs)
            let (me, isAnysphereUser) = try await (meTask, membershipTask)
            func nonEmpty(_ value: Any?) -> String? {
                guard let value = value as? String else { return nil }
                let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
                return trimmed.isEmpty ? nil : trimmed
            }
            let displayName = [
                nonEmpty(me["firstName"]),
                nonEmpty(me["lastName"]),
            ]
            .compactMap { $0 }
            .joined(separator: " ")
            return .init(
                displayName: displayName.isEmpty ? nil : displayName,
                email: nonEmpty(me["email"]),
                profilePictureUrl: nonEmpty(me["profilePictureUrl"]),
                isAnysphereUser: isAnysphereUser
            )
        } catch {
            return nil
        }
    }

    func isAnysphereUser(timeoutMs: Int = 10_000) async throws -> Bool {
        let response = try await rpc("GetTeams", body: [:], timeoutMs: timeoutMs)
        let teams = response["teams"] as? [[String: Any]] ?? []
        return teams.contains { team in
            let id: Int
            if let value = team["id"] as? NSNumber {
                id = value.intValue
            } else if let value = team["id"] as? String, let parsed = Int(value) {
                id = parsed
            } else {
                id = -1
            }
            let seats: Int
            if let value = team["seats"] as? NSNumber {
                seats = value.intValue
            } else if let value = team["seats"] as? String, let parsed = Int(value) {
                seats = parsed
            } else {
                seats = 0
            }
            return id == 1
                && team["hasBilling"] as? Bool == true
                && seats > 0
        }
    }

    func getSkillPublishTeams(timeoutMs: Int = 10_000) async throws -> [IOSCursorSkillPublishTeam] {
        let response = try await protoRPC(
            "GetTeams",
            body: IOSCursorDashboardProto.getTeamsRequest(activeOnly: true),
            timeoutMs: timeoutMs
        )
        return try IOSCursorDashboardProto.decodeTeams(response)
    }

    func getAvailableModelsJSON(
        timeoutMs: Int = 12_000
    ) async throws -> IOSCursorDashboardJSON {
        IOSCursorDashboardJSON(value: try await rpc(
            "AvailableModels",
            service: "AiService",
            body: [
                "scope": "AVAILABLE_MODELS_SCOPE_USER_AVAILABLE",
                "useModelParameters": true,
            ],
            timeoutMs: timeoutMs
        ))
    }

    func getAvailableModels(
        timeoutMs: Int = 12_000
    ) async throws -> [String: Any] {
        try await getAvailableModelsJSON(timeoutMs: timeoutMs).value
    }

    func transcribeAudio(
        audio: Data,
        mimeType: String,
        language: String? = nil,
        timeoutMs: Int = 60_000
    ) async throws -> IOSCursorTranscriptionResult {
        guard !audio.isEmpty else {
            throw IOSCursorDashboardError(message: "Cannot transcribe empty audio.")
        }
        let normalizedMimeType = mimeType
            .split(separator: ";", maxSplits: 1, omittingEmptySubsequences: false)
            .first
            .map(String.init)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? mimeType
        let normalizedLanguage = language?.isEmpty == false ? language! : "en-US"
        let response = try await protoRPC(
            "TranscribeAudio",
            service: "AiService",
            body: IOSCursorDashboardProto.transcribeAudioRequest(
                audio: audio,
                mimeType: normalizedMimeType,
                language: normalizedLanguage
            ),
            timeoutMs: timeoutMs
        )
        return try IOSCursorDashboardProto.decodeTranscribeAudioResponse(response)
    }

    func getCursorAvatarDataURL(
        authId: String?,
        timeoutMs: Int = 10_000
    ) async -> String? {
        let profile = try? await rpc("GetMe", body: [:], timeoutMs: timeoutMs)
        let preferred = (profile?["profilePictureUrl"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if let preferred, !preferred.isEmpty,
           let value = await fetchAvatarDataURL(preferred, timeoutMs: timeoutMs) {
            return value
        }
        guard let authId,
              let separator = authId.firstIndex(of: "|"),
              separator != authId.startIndex,
              separator != authId.index(before: authId.endIndex),
              String(authId[..<separator]) == "github"
        else { return nil }
        let subject = String(authId[authId.index(after: separator)...])
        guard !subject.isEmpty, subject.allSatisfy(\.isNumber) else { return nil }
        var components = URLComponents()
        components.scheme = "https"
        components.host = "avatars.githubusercontent.com"
        components.path = "/u/\(subject)"
        components.queryItems = [
            .init(name: "v", value: "4"),
            .init(name: "s", value: "192"),
        ]
        guard let url = components.url else { return nil }
        return await fetchAvatarDataURL(url.absoluteString, timeoutMs: timeoutMs)
    }

    private func fetchAvatarDataURL(_ rawURL: String, timeoutMs: Int) async -> String? {
        guard let url = URL(string: rawURL), url.scheme?.lowercased() == "https" else {
            return nil
        }
        let key = url.absoluteString as NSString
        if let cached = avatarCache.object(forKey: key) {
            return cached as String
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = TimeInterval(max(1, timeoutMs)) / 1_000
        do {
            let data: Data
            let response: URLResponse
            if let requestExecutor {
                let result = try await requestExecutor(request)
                data = result.0
                response = result.1
            } else {
                let result = try await session.data(for: request)
                data = result.0
                response = result.1
            }
            guard let http = response as? HTTPURLResponse,
                  (200..<300).contains(http.statusCode)
            else { return nil }
            let contentType = (http.value(forHTTPHeaderField: "Content-Type") ?? "")
                .split(separator: ";", maxSplits: 1)
                .first
                .map(String.init)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            guard contentType.hasPrefix("image/") else { return nil }
            if let length = http.value(forHTTPHeaderField: "Content-Length"),
               let declared = Int(length),
               declared > 1_048_576 {
                return nil
            }
            guard !data.isEmpty, data.count <= 1_048_576 else { return nil }
            let value = "data:\(contentType);base64,\(data.base64EncodedString())"
            avatarCache.setObject(value as NSString, forKey: key)
            return value
        } catch {
            return nil
        }
    }

    func updateCursorAccountName(_ name: String, timeoutMs: Int = 10_000) async throws {
        let parts = name.split(whereSeparator: { $0.isWhitespace }).map(String.init)
        _ = try await rpc(
            "UpdateUserName",
            body: [
                "firstName": parts.first ?? "",
                "lastName": parts.dropFirst().joined(separator: " "),
            ],
            timeoutMs: timeoutMs
        )
    }

    static func cursorPrivacyMode(from raw: Any?) -> SandPrivacyMode? {
        if let number = raw as? NSNumber {
            return SandPrivacyMode(rawValue: number.intValue)
        }
        guard let value = raw as? String else { return nil }
        switch value {
        case "PRIVACY_MODE_UNSPECIFIED", "UNSPECIFIED":
            return .unspecified
        case "PRIVACY_MODE_NO_STORAGE", "NO_STORAGE":
            return .noStorage
        case "PRIVACY_MODE_NO_TRAINING", "NO_TRAINING":
            return .noTraining
        case "PRIVACY_MODE_USAGE_DATA_TRAINING_ALLOWED", "USAGE_DATA_TRAINING_ALLOWED":
            return .usageDataTrainingAllowed
        case "PRIVACY_MODE_USAGE_CODEBASE_TRAINING_ALLOWED", "USAGE_CODEBASE_TRAINING_ALLOWED":
            return .usageCodebaseTrainingAllowed
        default:
            guard let rawValue = Int(value) else { return nil }
            return SandPrivacyMode(rawValue: rawValue)
        }
    }

    func getCursorPrivacyMode(timeoutMs: Int = 10_000) async -> SandPrivacyMode? {
        guard let response = try? await rpc("GetUserPrivacyMode", body: [:], timeoutMs: timeoutMs) else {
            return nil
        }
        return Self.cursorPrivacyMode(from: response["privacyMode"])
    }

    func getCursorPrivacyModeEnabled(timeoutMs: Int = 10_000) async -> Bool {
        guard let mode = await getCursorPrivacyMode(timeoutMs: timeoutMs) else {
            return true
        }
        return mode != .usageDataTrainingAllowed
            && mode != .usageCodebaseTrainingAllowed
    }

    func getCursorWeeklyUsage(timeoutMs: Int = 10_000) async -> [String: Any]? {
        async let status = optionalRPC("GetSandUsageStatus", timeoutMs: timeoutMs)
        async let period = optionalRPC("GetCurrentPeriodUsage", timeoutMs: timeoutMs)
        guard let sandStatus = await status,
              sandStatus["usesPooledEnterpriseAllowance"] as? Bool != true,
              let percent = finiteDouble(sandStatus["usagePercent"])
        else { return nil }

        let included = sandStatus["hasNonZeroIncludedLimit"] as? Bool == true
        var output: [String: Any] = [
            "percentUsed": max(percent, 0),
            "nextResetMs": jsonValueOrNull(timestampMilliseconds(sandStatus["nextResetTimestampUtc"])),
            "hasNonZeroIncludedLimit": included,
            "onDemand": NSNull(),
        ]
        if included,
           let current = await period,
           let spend = current["spendLimitUsage"] as? [String: Any],
           let used = finiteDouble(spend["individualUsed"]),
           let limit = normalizedLimitCents(spend["individualLimit"]) {
            output["onDemand"] = [
                "usedCents": used,
                "limitCents": limit,
            ]
        }
        return output
    }

    func getCursorUsageSummary(timeoutMs: Int = 15_000) async throws -> [String: Any] {
        async let statusTask = rpc("GetSandUsageStatus", body: [:], timeoutMs: timeoutMs)
        async let periodTask = currentPeriodUsageForSummary(timeoutMs: timeoutMs)
        async let teamsTask = rpc("GetTeams", body: [:], timeoutMs: timeoutMs)
        async let trialTask = rpc("GetSandTrialClaimStatus", body: [:], timeoutMs: timeoutMs)
        let (status, period, teamsResponse, trial) = try await (
            statusTask,
            periodTask,
            teamsTask,
            trialTask
        )

        let nowMs = Int64(Date().timeIntervalSince1970 * 1_000)
        let trialExpiry = timestampMilliseconds(status["sandTrialExpiresAt"])
        let liveTrial = trialExpiry.map { $0 > nowMs } ?? false
        let claimGranted = enumIsGranted(trial["status"])
        let included = status["hasNonZeroIncludedLimit"] as? Bool == true
        let teams = teamsResponse["teams"] as? [[String: Any]] ?? []

        var summary: [String: Any] = [
            "isEnterprise": teams.contains { $0["isEnterprise"] as? Bool == true },
            "sandUsagePercent": jsonValueOrNull(nonNegativeFinite(status["usagePercent"])),
            "sandUsageResetTimestampMs": jsonValueOrNull(timestampMilliseconds(status["nextResetTimestampUtc"])),
            "hasAvailableUsage": status["hasAvailableUsage"] as? Bool == true,
            "isSandTrial": liveTrial,
            "hasEndedSandTrial": claimGranted && !liveTrial,
            "hasNonZeroIncludedLimit": included,
            "canCancelSandTrial": liveTrial && status["sandTrialCancelable"] as? Bool == true,
            "onDemand": NSNull(),
            "upgradeCta": jsonValueOrNull(upgradeCTA(status["upgradeRecommendation"])),
        ]
        if included, let period, let spend = period["spendLimitUsage"] as? [String: Any] {
            let used = finiteDouble(spend["individualUsed"]) ?? 0
            let reset = int64Value(period["billingCycleEnd"]).flatMap { $0 > 0 ? $0 : nil }
            summary["onDemand"] = [
                "usedCents": used,
                "limitCents": jsonValueOrNull(normalizedLimitCents(spend["individualLimit"])),
                "resetTimestampMs": jsonValueOrNull(reset),
            ]
        }
        return summary
    }

    func cancelCursorSandTrial(timeoutMs: Int = 15_000) async -> [String: Any] {
        do {
            _ = try await rpc("CancelSandTrial", body: [:], timeoutMs: timeoutMs)
            return ["ok": true, "message": NSNull()]
        } catch {
            let message = (error as? LocalizedError)?.errorDescription?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return ["ok": false, "message": jsonValueOrNull(message?.isEmpty == false ? message : nil)]
        }
    }

    func invokeCursorDashboardAction(
        action: String,
        args: [String: String],
        timeoutMs: Int = 15_000
    ) async throws -> [String: Any] {
        let response = try await rpc(
            "ClientAction",
            body: ["action": action, "args": args],
            timeoutMs: timeoutMs
        )
        let ok = response["success"] as? Bool == true
        let rawMessage = (ok ? response["infoMessage"] : response["errorMessage"]) as? String
        let message = rawMessage?.trimmingCharacters(in: .whitespacesAndNewlines)
        return [
            "ok": ok,
            "message": jsonValueOrNull(message?.isEmpty == false ? message : nil),
        ]
    }

    func getCursorWeeklyUsageTransfer(timeoutMs: Int = 10_000) async -> IOSCursorDashboardJSON? {
        await getCursorWeeklyUsage(timeoutMs: timeoutMs).map(IOSCursorDashboardJSON.init(value:))
    }

    func getCursorUsageSummaryTransfer(timeoutMs: Int = 15_000) async throws -> IOSCursorDashboardJSON {
        .init(value: try await getCursorUsageSummary(timeoutMs: timeoutMs))
    }

    func cancelCursorSandTrialTransfer(timeoutMs: Int = 15_000) async -> IOSCursorDashboardJSON {
        .init(value: await cancelCursorSandTrial(timeoutMs: timeoutMs))
    }

    func invokeCursorDashboardActionTransfer(
        action: String,
        args: [String: String],
        timeoutMs: Int = 15_000
    ) async throws -> IOSCursorDashboardJSON {
        .init(value: try await invokeCursorDashboardAction(
            action: action,
            args: args,
            timeoutMs: timeoutMs
        ))
    }

    private func optionalRPC(_ method: String, timeoutMs: Int) async -> [String: Any]? {
        try? await rpc(method, body: [:], timeoutMs: timeoutMs)
    }

    private func currentPeriodUsageForSummary(timeoutMs: Int) async throws -> [String: Any]? {
        do {
            return try await rpc("GetCurrentPeriodUsage", body: [:], timeoutMs: timeoutMs)
        } catch {
            let message = error.localizedDescription.lowercased()
            if message.contains("invalid_argument") || message.contains("invalid argument") {
                return nil
            }
            throw error
        }
    }

    private func finiteDouble(_ value: Any?) -> Double? {
        guard let number = value as? NSNumber else { return nil }
        let result = number.doubleValue
        return result.isFinite ? result : nil
    }

    private func nonNegativeFinite(_ value: Any?) -> Double? {
        guard let value = finiteDouble(value), value >= 0 else { return nil }
        return value
    }

    private func normalizedLimitCents(_ value: Any?) -> Double? {
        guard let value = finiteDouble(value),
              value > 0,
              value < 2_147_483_647
        else { return nil }
        return value
    }

    private func int64Value(_ value: Any?) -> Int64? {
        if let value = value as? String { return Int64(value) }
        if let value = value as? NSNumber { return value.int64Value }
        return nil
    }

    private func timestampMilliseconds(_ value: Any?) -> Int64? {
        if let raw = value as? String {
            let fractional = ISO8601DateFormatter()
            fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            let fallback = ISO8601DateFormatter()
            fallback.formatOptions = [.withInternetDateTime]
            if let date = fractional.date(from: raw) ?? fallback.date(from: raw) {
                return Int64(date.timeIntervalSince1970 * 1_000)
            }
        }
        if let object = value as? [String: Any],
           let seconds = int64Value(object["seconds"]) {
            let nanos = int64Value(object["nanos"]) ?? 0
            return seconds * 1_000 + nanos / 1_000_000
        }
        return nil
    }

    private func enumIsGranted(_ value: Any?) -> Bool {
        if let number = value as? NSNumber { return number.intValue == 3 }
        guard let value = value as? String else { return false }
        return value == "SAND_TRIAL_CLAIM_STATUS_GRANTED" || value == "GRANTED"
    }

    private func jsonValueOrNull<T>(_ value: T?) -> Any {
        if let value { return value }
        return NSNull()
    }

    private func upgradeCTA(_ value: Any?) -> [String: Any]? {
        guard let recommendation = value as? [String: Any],
              let button = recommendation["cta"] as? [String: Any],
              let label = button["label"] as? String,
              !label.isEmpty
        else { return nil }
        let disabled = recommendation["disabled"] as? Bool == true
        if let urlAction = button["url"] as? [String: Any],
           let raw = urlAction["url"] as? String,
           let url = URL(string: raw),
           url.scheme == "https" || url.scheme == "http" {
            return [
                "label": label,
                "disabled": disabled,
                "action": ["kind": "open-url", "url": raw],
            ]
        }
        if let dashboardAction = button["dashboardAction"] as? [String: Any],
           dashboardAction["action"] as? String == "requestLimitIncrease" {
            let args = (dashboardAction["args"] as? [String: Any] ?? [:]).compactMapValues { $0 as? String }
            let success = (dashboardAction["successMessage"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return [
                "label": label,
                "disabled": disabled,
                "action": [
                    "kind": "dashboard-action",
                    "action": "requestLimitIncrease",
                    "args": args,
                    "successMessage": jsonValueOrNull(success?.isEmpty == false ? success : nil),
                ],
            ]
        }
        return nil
    }

    func getPrReviewPreferences(timeoutMs: Int = 10_000) async throws -> SandPrReviewPreferences {
        async let userResponse = protoRPC(
            "GetBackgroundComposerUserSettings",
            service: "BackgroundComposerService",
            body: IOSCursorDashboardProto.prReviewUserSettingsRequest(),
            timeoutMs: timeoutMs
        )
        async let teamResponse = protoRPC(
            "GetTeamAdminSettingsOrEmptyIfNotInTeam",
            body: IOSCursorDashboardProto.teamAdminSettingsRequest(),
            timeoutMs: timeoutMs
        )
        let (userData, teamData) = try await (userResponse, teamResponse)
        return .init(
            user: try IOSCursorDashboardProto.decodePrReviewUserDestination(userData),
            team: try IOSCursorDashboardProto.decodePrReviewTeamDestination(teamData)
        )
    }

    func getLocalToolPermissionCeiling(
        timeoutMs: Int = 10_000
    ) async throws -> SandLocalToolPermission? {
        let response = try await protoRPC(
            "GetTeamAdminSettingsOrEmptyIfNotInTeam",
            body: IOSCursorDashboardProto.teamAdminSettingsRequest(),
            timeoutMs: timeoutMs
        )
        return try IOSCursorDashboardProto.decodeLocalToolPermissionCeiling(response)
    }

    func publishSkillPlugin(
        teamId: Int32,
        name: String,
        displayName: String,
        description: String,
        pluginTarGz: Data,
        timeoutMs: Int = 60_000
    ) async throws -> IOSCursorPublishedSkill {
        let body = try IOSCursorDashboardProto.publishPluginRequest(
            teamId: teamId,
            name: name,
            displayName: displayName,
            description: description,
            pluginTarGz: pluginTarGz
        )
        let response = try await protoRPC("PublishPlugin", body: body, timeoutMs: timeoutMs)
        return try IOSCursorDashboardProto.decodePublishedSkill(response)
    }

    func unpublishSkillPlugin(
        pluginId: String,
        teamId: Int32,
        timeoutMs: Int = 60_000
    ) async throws {
        guard let parsedPluginId = Int64(pluginId) else {
            throw IOSCursorDashboardError(message: "Published plugin id is not an int64.")
        }
        let body = try IOSCursorDashboardProto.unpublishPluginRequest(
            pluginId: parsedPluginId,
            teamId: teamId
        )
        _ = try await protoRPC("UnpublishPlugin", body: body, timeoutMs: timeoutMs)
    }

    func getCurrentUserId(timeoutMs: Int = 10_000) async throws -> UInt64 {
        let response = try await rpc("GetMe", body: [:], timeoutMs: timeoutMs)
        guard let userId = uint64(response["userId"]), userId != 0 else {
            throw IOSCursorDashboardError(message: "Dashboard GetMe response was missing the signed-in user id.")
        }
        return userId
    }

    private func uint64(_ value: Any?) -> UInt64? {
        if let value = value as? String { return UInt64(value) }
        if let value = value as? NSNumber { return value.uint64Value }
        return nil
    }

    private func stringArray(_ value: Any?) -> [String] {
        (value as? [Any] ?? []).compactMap { $0 as? String }
    }

    private func jsonValue(_ value: McpJSONValue) -> Any {
        switch value {
        case .null: NSNull()
        case .bool(let value): value
        case .number(let value): value
        case .string(let value): value
        case .array(let values): values.map(jsonValue)
        case .object(let object): object.mapValues(jsonValue)
        }
    }

    func getAvailableMcpServers(timeoutMs: Int) async throws -> [AvailableMcpServer] {
        let response = try await rpc("GetAvailableMcpServers", body: [:], timeoutMs: timeoutMs)
        return (response["servers"] as? [[String: Any]] ?? []).compactMap { row in
            guard let id = uint64(row["id"]), id != 0,
                  let name = row["name"] as? String,
                  let identifier = row["serverIdentifier"] as? String,
                  let type = row["type"] as? String
            else { return nil }
            let accounts = (row["accounts"] as? [[String: Any]] ?? []).compactMap { account -> AvailableMcpAccount? in
                guard let key = account["accountKey"] as? String else { return nil }
                return .init(
                    accountKey: key,
                    serverIdentifier: account["serverIdentifier"] as? String ?? "",
                    userHasAccessToken: account["userHasAccessToken"] as? Bool ?? false
                )
            }
            return .init(
                id: id,
                name: name,
                serverIdentifier: identifier,
                type: type,
                url: row["url"] as? String,
                command: row["command"] as? String,
                args: stringArray(row["args"]),
                enabled: row["enabled"] as? Bool ?? false,
                isTeamServer: row["isTeamServer"] as? Bool ?? false,
                owningTeamId: uint64(row["owningTeamId"]),
                disabledByTeamAdminPolicy: row["disabledByTeamAdminPolicy"] as? Bool ?? false,
                pluginId: uint64(row["pluginId"]),
                isRequired: row["isRequired"] as? Bool ?? false,
                managedByTeamPluginPolicy: row["managedByTeamPluginPolicy"] as? Bool ?? false,
                accounts: accounts.isEmpty ? nil : accounts
            )
        }
    }

    func getMcpConfig(
        teamScope: Bool,
        redactSecrets: Bool,
        teamId: UInt64?,
        timeoutMs: Int?
    ) async throws -> AccountMcpConfigResponse {
        var body: [String: Any] = [
            "teamScope": teamScope,
            "redactSecrets": redactSecrets,
        ]
        if let teamId { body["teamId"] = String(teamId) }
        let response = try await rpc(
            "GetMcpConfig",
            body: body,
            timeoutMs: timeoutMs ?? ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
        let metadata = (response["serverMetadataByName"] as? [String: Any] ?? [:]).reduce(
            into: [String: AccountMcpServerMetadata]()
        ) { result, pair in
            guard let object = pair.value as? [String: Any] else { return }
            result[pair.key] = .init(serverId: uint64(object["serverId"]))
        }
        return .init(
            configJson: response["configJson"] as? String ?? #"{"mcpServers":{}}"#,
            serverMetadataByName: metadata
        )
    }

    private func marketplaceWireMarketplace(_ row: [String: Any]) -> MarketplaceWireMarketplace? {
        guard let id = uint64(row["id"]), id != 0,
              let name = row["name"] as? String, !name.isEmpty
        else { return nil }
        return .init(
            id: id,
            name: name,
            displayName: row["displayName"] as? String ?? "",
            teamId: uint64(row["teamId"]),
            userId: uint64(row["userId"])
        )
    }

    private func marketplaceWirePlugin(_ row: [String: Any]) -> MarketplaceWirePlugin? {
        guard let id = uint64(row["id"]), id != 0,
              let name = row["name"] as? String, !name.isEmpty
        else { return nil }

        let connectors = (row["mcpServers"] as? [[String: Any]] ?? []).compactMap { item -> MarketplaceWireConnector? in
            guard let name = item["name"] as? String, !name.isEmpty else { return nil }
            return .init(
                name: name,
                description: item["description"] as? String ?? "",
                sourceUrl: item["sourceUrl"] as? String
            )
        }
        let skills = (row["skills"] as? [[String: Any]] ?? []).compactMap { item -> MarketplaceWireSkill? in
            guard let name = item["name"] as? String, !name.isEmpty else { return nil }
            return .init(
                name: name,
                description: item["description"] as? String ?? "",
                sourceUrl: item["sourceUrl"] as? String
            )
        }
        let marketplace = (row["marketplace"] as? [String: Any]).flatMap(marketplaceWireMarketplace)
        let publisher: MarketplaceWirePublisher?
        if let item = row["publisher"] as? [String: Any],
           let publisherName = item["name"] as? String, !publisherName.isEmpty {
            publisher = .init(
                name: publisherName,
                displayName: item["displayName"] as? String ?? "",
                logoUrl: item["logoUrl"] as? String,
                websiteUrl: item["websiteUrl"] as? String,
                isUserOwned: item["isUserOwned"] as? Bool ?? false
            )
        } else {
            publisher = nil
        }
        let variableFields = pluginVariablesSchemaToFields(
            row["variables"] as? [String: Any] ?? [:]
        )
        return .init(
            id: id,
            name: name,
            displayName: row["displayName"] as? String ?? "",
            description: row["description"] as? String ?? "",
            logoUrl: row["logoUrl"] as? String,
            repositoryUrl: row["repositoryUrl"] as? String,
            curatedCategoryKeys: stringArray(row["curatedCategoryKeys"]),
            mcpServers: connectors,
            skills: skills,
            variableFields: variableFields,
            marketplace: marketplace,
            publisher: publisher
        )
    }

    func listMarketplacePlugins(
        marketplaceId: UInt64?,
        excludeCloudAgentPlugins: Bool,
        timeoutMs: Int
    ) async throws -> MarketplacePluginListing {
        var body: [String: Any] = [
            "excludeCloudAgentPlugins": excludeCloudAgentPlugins,
        ]
        if let marketplaceId {
            body["marketplaceId"] = String(marketplaceId)
        }
        let response = try await rpc(
            "ListMarketplacePlugins",
            body: body,
            timeoutMs: timeoutMs
        )
        let plugins = (response["plugins"] as? [[String: Any]] ?? [])
            .compactMap(marketplaceWirePlugin)
        return .init(plugins: plugins)
    }

    func listMarketplaces(timeoutMs: Int) async throws -> [MarketplaceWireMarketplace] {
        let response = try await rpc("ListMarketplaces", body: [:], timeoutMs: timeoutMs)
        return (response["marketplaces"] as? [[String: Any]] ?? [])
            .compactMap(marketplaceWireMarketplace)
    }

    func getPluginMcpConfig(pluginId: UInt64, timeoutMs: Int) async throws -> String? {
        let response = try await rpc(
            "GetPluginMcpConfig",
            body: ["pluginId": String(pluginId)],
            timeoutMs: timeoutMs
        )
        return response["configJson"] as? String
    }

    func getEffectiveUserPlugins(excludeConfiguredVariables: Bool) async throws -> [EffectivePluginWire] {
        let response = try await rpc(
            "GetEffectiveUserPlugins",
            body: ["excludeConfiguredVariables": excludeConfiguredVariables],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
        return (response["plugins"] as? [[String: Any]] ?? []).map { row in
            let pluginObject = row["plugin"] as? [String: Any]
            let plugin: EffectivePluginWire.Plugin?
            if let pluginObject,
               let id = uint64(pluginObject["id"]), id != 0,
               let name = pluginObject["name"] as? String {
                let publisher = pluginObject["publisher"] as? [String: Any]
                let marketplace = pluginObject["marketplace"] as? [String: Any]
                plugin = .init(
                    id: id,
                    name: name,
                    displayName: pluginObject["displayName"] as? String ?? "",
                    gitRef: pluginObject["gitRef"] as? String,
                    publisherUserId: uint64(publisher?["ownerUserId"]),
                    marketplaceTeamId: uint64(marketplace?["teamId"])
                )
            } else {
                plugin = nil
            }
            return .init(
                plugin: plugin,
                installMode: (row["installMode"] as? NSNumber)?.intValue ?? 0,
                isTeamRequired: row["isTeamRequired"] as? Bool ?? false,
                isEnabled: row["isEnabled"] as? Bool ?? false,
                pinnedGitRef: row["pinnedGitRef"] as? String,
                hasTeamConfiguredVariables: row["hasTeamConfiguredVariables"] as? Bool ?? false
            )
        }
    }

    func setMcpConfig(configJson: String, serverIdsByName: [String: UInt64]) async throws {
        _ = try await rpc(
            "SetMcpConfig",
            body: [
                "teamScope": false,
                "configJson": configJson,
                "serverIdsByName": serverIdsByName.mapValues { String($0) },
            ],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
    }

    func installUserPlugin(pluginId: UInt64, variables: [String: String]?) async throws {
        var body: [String: Any] = ["pluginId": String(pluginId)]
        if let variables, !variables.isEmpty { body["variables"] = variables }
        _ = try await rpc("InstallUserPlugin", body: body, timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS)
    }

    func uninstallUserPlugin(pluginId: UInt64) async throws {
        _ = try await rpc(
            "UninstallUserPlugin",
            body: ["pluginId": String(pluginId)],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
    }

    func updateUserPluginInstall(pluginId: UInt64, variables: [String: String]) async throws {
        _ = try await rpc(
            "UpdateUserPluginInstall",
            body: ["pluginId": String(pluginId), "variables": variables],
            timeoutMs: ACCOUNT_MCP_RPC_TIMEOUT_MS
        )
    }

    func listSandMcpTools(
        serverIdentifiers: [String],
        timeoutMs: Int
    ) async throws -> [BackendMcpToolServerWire] {
        let response = try await rpc(
            "ListSandMcpTools",
            body: ["serverIdentifiers": serverIdentifiers],
            timeoutMs: timeoutMs
        )
        return (response["servers"] as? [[String: Any]] ?? []).compactMap { row in
            guard let identifier = row["serverIdentifier"] as? String else { return nil }
            let status: String
            if let string = row["status"] as? String {
                status = string
            } else if let number = row["status"] as? NSNumber {
                status = number.stringValue
            } else {
                status = ""
            }
            let tools = (row["tools"] as? [[String: Any]] ?? []).compactMap { tool -> BackendToolWire? in
                guard let name = tool["name"] as? String,
                      let provider = tool["providerIdentifier"] as? String,
                      let toolName = tool["toolName"] as? String
                else { return nil }
                return .init(
                    name: name,
                    providerIdentifier: provider,
                    toolName: toolName,
                    description: tool["description"] as? String ?? "",
                    inputSchema: tool["inputSchema"].map(McpJSONValue.from)
                )
            }
            return .init(
                serverIdentifier: identifier,
                status: status,
                tools: tools,
                accountLabel: row["accountLabel"] as? String,
                rowServerIdentifier: row["rowServerIdentifier"] as? String
            )
        }
    }

    func executeSandMcpTool(
        serverIdentifier: String,
        toolName: String,
        args: McpJSONValue,
        toolCallId: String,
        agentId: String,
        timeoutMs: Int
    ) async throws -> McpExecResult? {
        let response = try await rpc(
            "ExecuteSandMcpTool",
            body: [
                "serverIdentifier": serverIdentifier,
                "toolName": toolName,
                "args": jsonValue(args),
                "toolCallId": toolCallId,
                "agentId": agentId,
            ],
            timeoutMs: timeoutMs
        )
        guard let result = response["result"] as? [String: Any],
              let pair = result.first else { return nil }
        return .init(caseName: pair.key, value: McpJSONValue.from(pair.value))
    }

    func checkHttpMcpStatus(
        serverIds: [String],
        oauthRedirectUri: String,
        forceReauth: Bool,
        accountKey: String,
        timeoutMs: Int
    ) async throws -> [BackendMcpAuthStatusWire] {
        let response = try await rpc(
            "CheckHttpMcpStatus",
            body: [
                "serverIds": serverIds,
                "oauthRedirectUri": oauthRedirectUri,
                "forceReauth": forceReauth,
                "accountKey": accountKey,
            ],
            timeoutMs: timeoutMs
        )
        return (response["statuses"] as? [[String: Any]] ?? []).compactMap { row in
            guard let id = row["id"] as? String else { return nil }
            return .init(
                id: id,
                isAvailable: row["isAvailable"] as? Bool ?? false,
                requiresAuth: row["requiresAuth"] as? Bool ?? false,
                hasValidToken: row["hasValidToken"] as? Bool ?? false,
                authUrl: row["authUrl"] as? String ?? "",
                error: row["error"] as? String ?? ""
            )
        }
    }

    func completeMcpOAuth(
        stateId: String,
        authorizationCode: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "CompleteMcpOAuth",
            body: ["stateId": stateId, "authorizationCode": authorizationCode],
            timeoutMs: timeoutMs
        )
    }

    func validateMcpOAuthTokens(
        targets: [BackendMcpTokenTarget],
        timeoutMs: Int
    ) async throws -> [BackendMcpTokenValidation] {
        let response = try await rpc(
            "ValidateMcpOAuthTokens",
            body: [
                "targets": targets.map {
                    ["serverUrl": $0.serverUrl, "accountKey": $0.accountKey]
                },
            ],
            timeoutMs: timeoutMs
        )
        return (response["results"] as? [[String: Any]] ?? []).compactMap { row in
            guard let serverURL = row["serverUrl"] as? String else { return nil }
            return .init(
                serverUrl: serverURL,
                accountKey: row["accountKey"] as? String,
                hasValidToken: row["hasValidToken"] as? Bool ?? false
            )
        }
    }

    func deleteMcpOAuthToken(
        serverUrl: String,
        accountKey: String,
        source: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "DeleteMcpOAuthToken",
            body: ["serverUrl": serverUrl, "accountKey": accountKey, "source": source],
            timeoutMs: timeoutMs
        )
    }

    func renameMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        newAccountKey: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "RenameMcpOAuthAccount",
            body: [
                "serverId": serverId,
                "accountKey": accountKey,
                "newAccountKey": newAccountKey,
            ],
            timeoutMs: timeoutMs
        )
    }

    func deleteMcpOAuthAccount(
        serverId: String,
        accountKey: String,
        timeoutMs: Int
    ) async throws {
        _ = try await rpc(
            "DeleteMcpOAuthAccount",
            body: ["serverId": serverId, "accountKey": accountKey],
            timeoutMs: timeoutMs
        )
    }
}

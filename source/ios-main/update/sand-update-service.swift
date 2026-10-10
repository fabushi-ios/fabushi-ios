import Foundation

/// App Store/TestFlight counterpart of Grok's desktop self-update service.
///
/// iOS applications cannot replace their own bundle. This service preserves the
/// renderer-visible update state while making the store-owned mechanism
/// explicit and refusing desktop-only apply operations.
@MainActor
final class IOSAppStoreUpdateService {
    enum ServiceError: LocalizedError, Equatable {
        case releaseMetadataUnavailable

        var errorDescription: String? {
            "The signed iOS bundle has no valid release metadata."
        }
    }

    private let metadataProvider: @MainActor () -> IOSReleaseMetadata?

    init(metadataProvider: @escaping @MainActor () -> IOSReleaseMetadata? = {
        IOSReleaseMetadataReader.read()
    }) {
        self.metadataProvider = metadataProvider
    }

    func statusPayload() throws -> CoordinatorPayload {
        guard let metadata = metadataProvider() else {
            throw ServiceError.releaseMetadataUnavailable
        }
        return .object([
            "type": .string("managed-by-app-store"),
            "version": .string(metadata.version),
            "buildNumber": .string(metadata.buildNumber),
            "bundleIdentifier": .string(metadata.bundleIdentifier),
            "mechanism": .string(metadata.updateMechanism),
            "selfUpdateSupported": .bool(false),
        ])
    }

    func unsupportedActionPayload(_ action: String) -> CoordinatorPayload {
        .object([
            "accepted": .bool(false),
            "action": .string(action),
            "reason": .string("managed-by-app-store"),
            "selfUpdateSupported": .bool(false),
        ])
    }
}


enum IOSAppVersionPolicyStrategy: String, Decodable, Equatable, Sendable {
    case none
    case optional
    case force
}

struct IOSAppVersionPolicy: Decodable, Equatable, Sendable {
    let enabled: Bool
    let platform: String
    let channel: String
    let latestVersion: String
    let latestBuildNumber: Int
    let minSupportedBuildNumber: Int
    let forceUpdate: Bool
    let allowSkip: Bool
    let rolloutPercentage: Int
    let promptIntervalHours: Int
    let title: String
    let message: String
    let releaseNotes: [String]
    let downloadUrl: String
    let updateAvailable: Bool
    let strategy: IOSAppVersionPolicyStrategy

    var isRequired: Bool {
        forceUpdate || strategy == .force
    }

    var downloadURL: URL? {
        guard let url = URL(string: downloadUrl),
              let scheme = url.scheme?.lowercased(),
              scheme == "https"
        else { return nil }
        return url
    }
}

enum IOSAppVersionPolicyClient {
    enum ClientError: LocalizedError, Equatable {
        case invalidMetadata
        case invalidEndpoint
        case invalidResponse
        case unexpectedStatus(Int)
        case invalidPolicy

        var errorDescription: String? {
            switch self {
            case .invalidMetadata:
                return "The signed iOS bundle has invalid release metadata."
            case .invalidEndpoint:
                return "The Fabushi version-policy endpoint is invalid."
            case .invalidResponse:
                return "The Fabushi version-policy response was invalid."
            case .unexpectedStatus(let status):
                return "The Fabushi version-policy service returned HTTP \(status)."
            case .invalidPolicy:
                return "The Fabushi version policy failed validation."
            }
        }
    }

    static let productionBaseURL = URL(string: "https://api.ombhrum.com")!

    static func requestURL(
        metadata: IOSReleaseMetadata,
        baseURL: URL = productionBaseURL,
        channel: String = "stable"
    ) -> URL? {
        guard Int(metadata.buildNumber) != nil else { return nil }
        var components = URLComponents(
            url: baseURL.appendingPathComponent("api/app/version-policy"),
            resolvingAgainstBaseURL: false
        )
        components?.queryItems = [
            .init(name: "platform", value: "ios"),
            .init(name: "channel", value: channel),
            .init(name: "version", value: metadata.version),
            .init(name: "buildNumber", value: metadata.buildNumber),
        ]
        return components?.url
    }

    static func decodePolicy(
        _ data: Data,
        statusCode: Int = 200
    ) throws -> IOSAppVersionPolicy {
        guard (200..<300).contains(statusCode) else {
            throw ClientError.unexpectedStatus(statusCode)
        }
        let policy: IOSAppVersionPolicy
        do {
            policy = try JSONDecoder().decode(IOSAppVersionPolicy.self, from: data)
        } catch {
            throw ClientError.invalidResponse
        }
        guard policy.platform.lowercased() == "ios",
              !policy.channel.isEmpty,
              !policy.latestVersion.isEmpty,
              policy.latestBuildNumber >= 0,
              policy.minSupportedBuildNumber >= 0,
              (0...100).contains(policy.rolloutPercentage),
              policy.promptIntervalHours >= 1,
              !policy.title.isEmpty,
              !policy.message.isEmpty,
              !policy.downloadUrl.isEmpty,
              policy.downloadURL != nil,
              policy.forceUpdate == (policy.strategy == .force || policy.forceUpdate),
              policy.updateAvailable == (policy.strategy != .none)
        else {
            throw ClientError.invalidPolicy
        }
        if policy.strategy == .force && !policy.forceUpdate {
            throw ClientError.invalidPolicy
        }
        if policy.strategy == .none && policy.forceUpdate {
            throw ClientError.invalidPolicy
        }
        return policy
    }

    static func fetch(
        metadata: IOSReleaseMetadata,
        baseURL: URL = productionBaseURL,
        channel: String = "stable",
        session: URLSession = .shared
    ) async throws -> IOSAppVersionPolicy {
        guard let url = requestURL(
            metadata: metadata,
            baseURL: baseURL,
            channel: channel
        ) else {
            throw ClientError.invalidMetadata
        }
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.timeoutInterval = 15
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw ClientError.invalidResponse
        }
        return try decodePolicy(data, statusCode: http.statusCode)
    }
}

enum IOSAppVersionPolicyLoadState: Equatable, Sendable {
    case idle
    case loading
    case ready(IOSAppVersionPolicy)
    case failed(message: String, retained: IOSAppVersionPolicy?)

    var policy: IOSAppVersionPolicy? {
        switch self {
        case .ready(let policy):
            return policy
        case .failed(_, let retained):
            return retained
        case .idle, .loading:
            return nil
        }
    }
}

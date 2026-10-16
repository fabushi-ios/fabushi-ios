import Foundation
import UIKit

actor IOSNativeLocalCapabilityBackend: IOSLocalCapabilityBackend {
    enum BackendError: LocalizedError, Equatable {
        case invalidPayload
        case unsupportedCapability(String)

        var errorDescription: String? {
            switch self {
            case .invalidPayload: "ios_local_capability_invalid_payload"
            case .unsupportedCapability(let capability):
                "ios_local_capability_not_implemented: \(capability)"
            }
        }
    }

    private let backgroundTransfers: any IOSBackgroundTransferScheduling

    init(
        backgroundTransfers: any IOSBackgroundTransferScheduling =
            IOSBackgroundTransferService.shared
    ) {
        self.backgroundTransfers = backgroundTransfers
    }

    func execute(
        capability: LocalCapabilityRunner.Capability,
        params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        switch capability {
        case .backgroundTransfer:
            return try await enqueueBackgroundTransfer(params)
        case .clipboardRead:
            let value = await MainActor.run {
                UIPasteboard.general.string
            }
            return value.map(CoordinatorPayload.string) ?? .null
        case .clipboardWrite:
            guard case .object(let object) = params,
                  case .string(let value)? = object["text"],
                  value.utf8.count <= 1_000_000
            else {
                throw BackendError.invalidPayload
            }
            await MainActor.run {
                UIPasteboard.general.string = value
            }
            return .object(["written": .bool(true)])
        case .openExternalURL:
            guard case .object(let object) = params,
                  case .string(let rawURL)? = object["url"],
                  let components = URLComponents(string: rawURL),
                  ["https", "http"].contains(components.scheme?.lowercased() ?? ""),
                  components.user == nil,
                  components.password == nil,
                  let url = components.url
            else {
                throw BackendError.invalidPayload
            }
            await MainActor.run {
                UIApplication.shared.open(url, options: [:], completionHandler: nil)
            }
            return .object(["accepted": .bool(true)])
        case .shareItem:
            throw BackendError.unsupportedCapability(capability.rawValue)
        }
    }

    private func enqueueBackgroundTransfer(
        _ params: CoordinatorPayload
    ) async throws -> CoordinatorPayload {
        guard case .object(let object) = params,
              case .string(let rawURL)? = object["url"]
        else {
            throw BackendError.invalidPayload
        }
        let url = try IOSBackgroundTransferService.validatedRemoteURL(rawURL)
        let fileName: String?
        if case .string(let rawFileName)? = object["fileName"] {
            fileName = try IOSBackgroundTransferService.validatedFileName(rawFileName)
        } else {
            fileName = nil
        }

        let taskIdentifier = try await backgroundTransfers.enqueueDownload(
            url: url,
            fileName: fileName
        )
        return .object([
            "accepted": .bool(true),
            "taskIdentifier": .number(Double(taskIdentifier)),
            "sessionIdentifier": .string(IOSBackgroundTransferService.sessionIdentifier),
        ])
    }
}

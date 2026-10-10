import Foundation

struct PackageAbortReasonInfo: Equatable {
    let abortReasonType: String
    let abortReasonName: String?
    let abortReasonMessage: String?

    init(
        abortReasonType: String,
        abortReasonName: String? = nil,
        abortReasonMessage: String? = nil
    ) {
        self.abortReasonType = abortReasonType
        self.abortReasonName = abortReasonName
        self.abortReasonMessage = abortReasonMessage
    }
}

func packageAbortReasonInfo(_ reason: Any?) -> PackageAbortReasonInfo {
    guard let reason else {
        return .init(abortReasonType: "undefined")
    }
    if let message = reason as? String {
        return .init(
            abortReasonType: "string",
            abortReasonMessage: message
        )
    }
    if let error = reason as? any Error {
        let nsError = error as NSError
        return .init(
            abortReasonType: "error",
            abortReasonName: String(describing: type(of: error)),
            abortReasonMessage: nsError.localizedDescription
        )
    }
    if reason is Bool {
        return .init(abortReasonType: "boolean")
    }
    if reason is NSNumber {
        return .init(abortReasonType: "number")
    }
    return .init(abortReasonType: "object")
}

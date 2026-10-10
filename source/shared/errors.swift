import Foundation

protocol SandStringCodedError: Error {
    var sandErrorCode: String? { get }
}

func errorMessage(_ error: Any) -> String {
    if let localized = error as? LocalizedError, let description = localized.errorDescription {
        return description
    }
    if let error = error as? Error {
        return (error as NSError).localizedDescription
    }
    return String(describing: error)
}

func errorLogTag(_ error: Any) -> String {
    guard let error = error as? Error else {
        return String(describing: type(of: error))
    }
    let name = String(describing: type(of: error))
    if let code = (error as? any SandStringCodedError)?.sandErrorCode, !code.isEmpty {
        return "\(name) (\(code))"
    }
    return name
}

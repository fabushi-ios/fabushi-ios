import Foundation

let ENV_NAME_PATTERN = #"^[A-Za-z_][A-Za-z0-9_]*$"#

func isValidEnvironmentName(_ value: String) -> Bool {
    value.range(of: ENV_NAME_PATTERN, options: .regularExpression) != nil
}

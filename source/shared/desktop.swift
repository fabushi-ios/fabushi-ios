import Foundation

enum FabushiThemePreference: String, CaseIterable, Sendable { case system, light, dark }

enum SandUiDirection: String, Codable, Equatable, Sendable {
    case auto
    case ltr
    case rtl
}

struct SandUiPreferences: Codable, Equatable, Sendable {
    let locale: String
    let direction: SandUiDirection
    let reducedMotion: Bool
    let highContrast: Bool
    let textScale: Double
}

let DEFAULT_SAND_UI_PREFERENCES = SandUiPreferences(
    locale: "system",
    direction: .auto,
    reducedMotion: false,
    highContrast: false,
    textScale: 1
)

private let SAND_RTL_LANGUAGE_TAGS: Set<String> = [
    "ar", "ckb", "dv", "fa", "he", "ku", "ps", "sd", "ug", "ur", "yi",
]

private func normalizeSandLocale(_ value: String?) -> String {
    guard let value else { return "system" }
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, trimmed.lowercased() != "system", trimmed.count <= 64 else {
        return "system"
    }
    let parts = trimmed.split(separator: "-", omittingEmptySubsequences: false).map(String.init)
    guard let first = parts.first,
          (2...3).contains(first.count),
          first.unicodeScalars.allSatisfy({
              $0.value < 128 && CharacterSet.letters.contains($0)
          }),
          parts.allSatisfy({ part in
              (1...8).contains(part.count)
                  && part.unicodeScalars.allSatisfy {
                      $0.value < 128 && CharacterSet.alphanumerics.contains($0)
                  }
          })
    else {
        return "system"
    }
    return ([first.lowercased()] + Array(parts.dropFirst())).joined(separator: "-")
}

func normalizeSandUiPreferences(
    locale: String?,
    direction: String?,
    reducedMotion: Bool?,
    highContrast: Bool?,
    textScale: Double?
) -> SandUiPreferences {
    let scale = textScale?.isFinite == true ? textScale! : 1
    let clampedScale = (min(2, max(0.8, scale)) * 100).rounded() / 100
    return .init(
        locale: normalizeSandLocale(locale),
        direction: SandUiDirection(rawValue: direction ?? "") ?? .auto,
        reducedMotion: reducedMotion == true,
        highContrast: highContrast == true,
        textScale: clampedScale
    )
}

func normalizeSandUiPreferences(_ value: SandUiPreferences) -> SandUiPreferences {
    normalizeSandUiPreferences(
        locale: value.locale,
        direction: value.direction.rawValue,
        reducedMotion: value.reducedMotion,
        highContrast: value.highContrast,
        textScale: value.textScale
    )
}

func resolveSandUiDirection(
    _ preferences: SandUiPreferences,
    systemLocale: String = "en-US"
) -> SandUiDirection {
    guard preferences.direction == .auto else { return preferences.direction }
    let locale = preferences.locale == "system"
        ? normalizeSandLocale(systemLocale)
        : preferences.locale
    let language = (locale == "system" ? "en" : locale.split(separator: "-").first.map(String.init) ?? "en")
        .lowercased()
    return SAND_RTL_LANGUAGE_TAGS.contains(language) ? .rtl : .ltr
}

struct SandCallMediaPreferences: Codable, Equatable, Sendable {
    let microphoneId: String?
    let cameraId: String?
}

let DEFAULT_SAND_CALL_MEDIA_PREFERENCES = SandCallMediaPreferences(
    microphoneId: nil,
    cameraId: nil
)

private func normalizeSandMediaDeviceId(_ value: String?) -> String? {
    guard let value else { return nil }
    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty, trimmed.count <= 512 else { return nil }
    let forbidden = CharacterSet.controlCharacters
    guard trimmed.unicodeScalars.allSatisfy({ !forbidden.contains($0) }) else { return nil }
    return trimmed
}

func normalizeSandCallMediaPreferences(
    microphoneId: String?,
    cameraId: String?
) -> SandCallMediaPreferences {
    .init(
        microphoneId: normalizeSandMediaDeviceId(microphoneId),
        cameraId: normalizeSandMediaDeviceId(cameraId)
    )
}

func normalizeSandCallMediaPreferences(
    _ value: SandCallMediaPreferences
) -> SandCallMediaPreferences {
    normalizeSandCallMediaPreferences(
        microphoneId: value.microphoneId,
        cameraId: value.cameraId
    )
}

enum FabushiDesktopPolicy {
    static let defaultTheme: FabushiThemePreference = .system
    static let pluginDeepLinkPath = "/v1/plugin/add"

    static func isThemePreference(_ value: String) -> Bool { FabushiThemePreference(rawValue: value) != nil }

    static func isDeepLinkPluginID(_ value: String) -> Bool {
        guard !value.isEmpty, value.count <= 19 else { return false }
        return value.unicodeScalars.allSatisfy { CharacterSet.decimalDigits.contains($0) }
    }

    static func buildPluginDeepLink(pluginID: String) -> String? {
        guard isDeepLinkPluginID(pluginID) else { return nil }
        var components = URLComponents()
        components.scheme = "fabushi"
        components.host = "app"
        components.path = pluginDeepLinkPath
        components.queryItems = [URLQueryItem(name: "id", value: pluginID)]
        return components.string
    }
}

import Foundation

let SAND_WEBAUTHN_SIGNER_PLATFORMS = ["ios", "ipados"]

func sandWebauthnSignerShips(_ platform: String) -> Bool {
    SAND_WEBAUTHN_SIGNER_PLATFORMS.contains(platform.lowercased())
}

func sandWebauthnProxyMirroredEnablement(
    _ enabled: Bool,
    platform: String
) -> Bool {
    enabled && sandWebauthnSignerShips(platform)
}

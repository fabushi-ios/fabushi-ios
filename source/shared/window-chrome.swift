import Foundation

struct ChromePoint: Equatable, Sendable {
    let x: Int
    let y: Int
}

let MAC_TRAFFIC_LIGHT_POSITION = ChromePoint(x: 16, y: 15)
let WINDOWS_TITLEBAR_BLOCK_PX = 52
let WINDOWS_TITLE_BAR_OVERLAY_HEIGHT_PX = WINDOWS_TITLEBAR_BLOCK_PX - 1
let WINDOWS_COMPUTER_TOP_BAR_PX = 44
let WINDOWS_COMPUTER_TITLE_BAR_OVERLAY_HEIGHT_PX = WINDOWS_COMPUTER_TOP_BAR_PX - 1
let IOS_USES_NATIVE_SCENE_CHROME = true

private func parseHexRGB(_ backgroundHex: String) -> (Int, Int, Int)? {
    guard backgroundHex.count == 7, backgroundHex.first == "#" else { return nil }
    let hex = String(backgroundHex.dropFirst())
    guard hex.allSatisfy({ $0.isHexDigit }),
          let red = Int(hex.prefix(2), radix: 16),
          let green = Int(hex.dropFirst(2).prefix(2), radix: 16),
          let blue = Int(hex.dropFirst(4).prefix(2), radix: 16) else {
        return nil
    }
    return (red, green, blue)
}

private func toHexByte(_ value: Int) -> String {
    String(format: "%02X", value)
}

func blendRgbaOverHex(
    _ rgb: (Int, Int, Int),
    alpha: Double,
    backgroundHex: String
) -> String {
    let background = parseHexRGB(backgroundHex) ?? (24, 24, 24)
    let red = Int((Double(rgb.0) * alpha + Double(background.0) * (1 - alpha)).rounded())
    let green = Int((Double(rgb.1) * alpha + Double(background.1) * (1 - alpha)).rounded())
    let blue = Int((Double(rgb.2) * alpha + Double(background.2) * (1 - alpha)).rounded())
    return "#\(toHexByte(red))\(toHexByte(green))\(toHexByte(blue))"
}

func windowsTitleBarOverlayHeight(_ isOverlayTone: Bool) -> Int {
    isOverlayTone
        ? WINDOWS_COMPUTER_TITLE_BAR_OVERLAY_HEIGHT_PX
        : WINDOWS_TITLE_BAR_OVERLAY_HEIGHT_PX
}

func windowsTitleBarOverlayBackground(
    _ themeBackground: String,
    isOverlayTone: Bool
) -> String {
    guard isOverlayTone else { return themeBackground }
    let cover = blendRgbaOverHex((20, 20, 20), alpha: 0.95, backgroundHex: themeBackground)
    return blendRgbaOverHex((0, 0, 0), alpha: 0.55, backgroundHex: cover)
}

func titleBarOverlaySymbolColor(_ backgroundHex: String) -> String {
    let rgb = parseHexRGB(backgroundHex) ?? (24, 24, 24)
    let luminance = Double(rgb.0 * 299 + rgb.1 * 587 + rgb.2 * 114) / 1_000
    return luminance < 128 ? "#FFFFFF" : "#000000"
}

import Foundation
import SwiftUI

internal enum MobileSettingsLocale: String, CaseIterable, Identifiable {
    case system
    case en
    case zhHans = "zh-Hans"
    case zhHant = "zh-Hant"
    case ja
    case ko
    case ar
    case he

    var id: String { rawValue }

    var optionLabel: String {
        switch self {
        case .system: return "System language"
        case .en: return "English"
        case .zhHans: return "简体中文"
        case .zhHant: return "繁體中文"
        case .ja: return "日本語"
        case .ko: return "한국어"
        case .ar: return "العربية"
        case .he: return "עברית"
        }
    }
}

internal enum MobileSettingsDirection: String, CaseIterable, Identifiable {
    case auto
    case ltr
    case rtl

    var id: String { rawValue }
}

internal struct MobileSettingsAccessibilityCopy: Equatable {
    let sectionTitle: String
    let language: String
    let systemLanguage: String
    let languageDescription: String
    let readingDirection: String
    let readingDirectionDescription: String
    let textSize: String
    let textSizeDescription: String
    let reduceMotion: String
    let reduceMotionDescription: String
    let highContrast: String
    let highContrastDescription: String
    let directionAutomatic: String
    let directionLtr: String
    let directionRtl: String
    let accessibilityCountOne: String
    let accessibilityCountOther: String

    func accessibilityCount(_ count: Int) -> String {
        let safeCount = max(0, count)
        return (safeCount == 1 ? accessibilityCountOne : accessibilityCountOther)
            .replacingOccurrences(of: "{count}", with: String(safeCount))
    }
}

/// iOS-owned presentation preferences corresponding to DesktopUiPreferences.
///
/// The values are intentionally local presentation state. They never mutate
/// account policy, Host state, Human/Agent stores, or platform accessibility
/// settings. The Scene root is the single runtime projection owner.
internal struct MobileUiPreferences: Equatable {
    static let localeDefaultsKey = "fabushi.ui.locale"
    static let directionDefaultsKey = "fabushi.ui.direction"
    static let textScaleDefaultsKey = "fabushi.ui.text-scale"
    static let reducedMotionDefaultsKey = "fabushi.ui.reduced-motion"
    static let highContrastDefaultsKey = "fabushi.ui.high-contrast"

    static let supportedTextScales: [Double] = [0.9, 1, 1.1, 1.25, 1.5]

    let locale: MobileSettingsLocale
    let direction: MobileSettingsDirection
    let textScale: Double
    let reducedMotion: Bool
    let highContrast: Bool

    init(
        localeRaw: String = MobileSettingsLocale.system.rawValue,
        directionRaw: String = MobileSettingsDirection.auto.rawValue,
        textScale: Double = 1,
        reducedMotion: Bool = false,
        highContrast: Bool = false
    ) {
        locale = MobileSettingsLocale(rawValue: localeRaw) ?? .system
        direction = MobileSettingsDirection(rawValue: directionRaw) ?? .auto
        self.textScale = Self.normalizedTextScale(textScale)
        self.reducedMotion = reducedMotion
        self.highContrast = highContrast
    }

    static func normalizedTextScale(_ value: Double) -> Double {
        guard value.isFinite else { return 1 }
        return supportedTextScales.min(by: {
            abs($0 - value) < abs($1 - value)
        }) ?? 1
    }

    var activeAccessibilityPreferenceCount: Int {
        Int(reducedMotion) + Int(highContrast) + Int(textScale != 1)
    }

    func resolvedLocaleIdentifier(
        systemIdentifier: String = Locale.autoupdatingCurrent.identifier
    ) -> String {
        locale == .system ? systemIdentifier : locale.rawValue
    }

    func resolvedLocale(
        systemIdentifier: String = Locale.autoupdatingCurrent.identifier
    ) -> Locale {
        Locale(identifier: resolvedLocaleIdentifier(systemIdentifier: systemIdentifier))
    }

    func resolvedLayoutDirection(system: LayoutDirection) -> LayoutDirection {
        switch direction {
        case .ltr:
            return .leftToRight
        case .rtl:
            return .rightToLeft
        case .auto:
            guard locale != .system else { return system }
            return Self.isRightToLeftLanguage(
                resolvedLocaleIdentifier(systemIdentifier: "en-US")
            ) ? .rightToLeft : .leftToRight
        }
    }

    func adjustedDynamicTypeSize(system: DynamicTypeSize) -> DynamicTypeSize {
        let sizes: [DynamicTypeSize] = [
            .xSmall, .small, .medium, .large, .xLarge, .xxLarge, .xxxLarge,
            .accessibility1, .accessibility2, .accessibility3,
            .accessibility4, .accessibility5,
        ]
        guard let index = sizes.firstIndex(of: system) else { return system }

        let offset: Int
        switch textScale {
        case ..<0.95: offset = -1
        case ..<1.05: offset = 0
        case ..<1.18: offset = 1
        case ..<1.38: offset = 2
        default: offset = 3
        }
        return sizes[min(max(index + offset, 0), sizes.count - 1)]
    }

    func accessibilityCopy(
        systemIdentifier: String = Locale.autoupdatingCurrent.identifier
    ) -> MobileSettingsAccessibilityCopy {
        switch Self.copyLocaleKey(
            resolvedLocaleIdentifier(systemIdentifier: systemIdentifier)
        ) {
        case .zhHans:
            return .init(
                sectionTitle: "语言与辅助功能",
                language: "语言",
                systemLanguage: "系统语言",
                languageDescription: "切换设置界面的可见文案与区域格式。",
                readingDirection: "阅读方向",
                readingDirectionDescription: "自动模式跟随所选语言或系统语言。",
                textSize: "文字大小",
                textSizeDescription: "调整 Fabushi 界面文字大小，同时保留系统 Dynamic Type。",
                reduceMotion: "减少动态效果",
                reduceMotionDescription: "关闭非必要动画，同时保留系统“减少动态效果”保护。",
                highContrast: "高对比度控件",
                highContrastDescription: "增强界面视觉对比度，不改变内容或账号策略。",
                directionAutomatic: "自动",
                directionLtr: "从左到右",
                directionRtl: "从右到左",
                accessibilityCountOne: "已启用 {count} 项辅助功能偏好",
                accessibilityCountOther: "已启用 {count} 项辅助功能偏好"
            )
        case .zhHant:
            return .init(
                sectionTitle: "語言與輔助功能",
                language: "語言",
                systemLanguage: "系統語言",
                languageDescription: "切換設定介面的可見文案與地區格式。",
                readingDirection: "閱讀方向",
                readingDirectionDescription: "自動模式跟隨所選語言或系統語言。",
                textSize: "文字大小",
                textSizeDescription: "調整 Fabushi 介面文字大小，同時保留系統 Dynamic Type。",
                reduceMotion: "減少動態效果",
                reduceMotionDescription: "關閉非必要動畫，同時保留系統「減少動態效果」保護。",
                highContrast: "高對比度控制項",
                highContrastDescription: "增強介面視覺對比度，不改變內容或帳戶策略。",
                directionAutomatic: "自動",
                directionLtr: "從左到右",
                directionRtl: "從右到左",
                accessibilityCountOne: "已啟用 {count} 項輔助功能偏好",
                accessibilityCountOther: "已啟用 {count} 項輔助功能偏好"
            )
        case .ja:
            return .init(
                sectionTitle: "言語とアクセシビリティ",
                language: "言語",
                systemLanguage: "システム言語",
                languageDescription: "設定の表示文言とロケール依存の書式を切り替えます。",
                readingDirection: "読み方向",
                readingDirectionDescription: "自動では選択言語またはシステム言語に従います。",
                textSize: "文字サイズ",
                textSizeDescription: "システム Dynamic Type を保ちながら Fabushi の文字サイズを調整します。",
                reduceMotion: "モーションを減らす",
                reduceMotionDescription: "不要なアニメーションを無効にし、システム設定も尊重します。",
                highContrast: "高コントラスト",
                highContrastDescription: "内容やアカウントポリシーを変えずに表示コントラストを強めます。",
                directionAutomatic: "自動",
                directionLtr: "左から右",
                directionRtl: "右から左",
                accessibilityCountOne: "{count} 件のアクセシビリティ設定が有効です",
                accessibilityCountOther: "{count} 件のアクセシビリティ設定が有効です"
            )
        case .ko:
            return .init(
                sectionTitle: "언어 및 접근성",
                language: "언어",
                systemLanguage: "시스템 언어",
                languageDescription: "설정의 표시 문구와 로캘 형식을 변경합니다.",
                readingDirection: "읽기 방향",
                readingDirectionDescription: "자동은 선택한 언어 또는 시스템 언어를 따릅니다.",
                textSize: "텍스트 크기",
                textSizeDescription: "시스템 Dynamic Type을 유지하면서 Fabushi 텍스트 크기를 조절합니다.",
                reduceMotion: "동작 줄이기",
                reduceMotionDescription: "불필요한 애니메이션을 끄고 시스템 설정도 존중합니다.",
                highContrast: "고대비 컨트롤",
                highContrastDescription: "콘텐츠나 계정 정책을 바꾸지 않고 화면 대비를 강화합니다.",
                directionAutomatic: "자동",
                directionLtr: "왼쪽에서 오른쪽",
                directionRtl: "오른쪽에서 왼쪽",
                accessibilityCountOne: "접근성 환경설정 {count}개가 활성화됨",
                accessibilityCountOther: "접근성 환경설정 {count}개가 활성화됨"
            )
        case .ar:
            return .init(
                sectionTitle: "اللغة وإمكانية الوصول",
                language: "اللغة",
                systemLanguage: "لغة النظام",
                languageDescription: "يغيّر نص الإعدادات الظاهر والتنسيق الحساس للغة.",
                readingDirection: "اتجاه القراءة",
                readingDirectionDescription: "يتبع الوضع التلقائي اللغة المحددة أو لغة النظام.",
                textSize: "حجم النص",
                textSizeDescription: "يضبط حجم نص Fabushi مع الحفاظ على Dynamic Type في النظام.",
                reduceMotion: "تقليل الحركة",
                reduceMotionDescription: "يعطّل الحركة غير الضرورية ويحترم إعداد النظام أيضًا.",
                highContrast: "تباين عالٍ",
                highContrastDescription: "يقوّي تباين الواجهة من دون تغيير المحتوى أو سياسة الحساب.",
                directionAutomatic: "تلقائي",
                directionLtr: "من اليسار إلى اليمين",
                directionRtl: "من اليمين إلى اليسار",
                accessibilityCountOne: "تفضيل وصول واحد ({count}) مفعّل",
                accessibilityCountOther: "{count} تفضيلات وصول مفعّلة"
            )
        case .he:
            return .init(
                sectionTitle: "שפה ונגישות",
                language: "שפה",
                systemLanguage: "שפת המערכת",
                languageDescription: "משנה את הטקסט הגלוי בהגדרות ואת העיצוב תלוי-האזור.",
                readingDirection: "כיוון קריאה",
                readingDirectionDescription: "אוטומטי עוקב אחר השפה שנבחרה או שפת המערכת.",
                textSize: "גודל טקסט",
                textSizeDescription: "מתאים את גודל הטקסט של Fabushi תוך שמירה על Dynamic Type של המערכת.",
                reduceMotion: "הפחתת תנועה",
                reduceMotionDescription: "מבטל אנימציה לא חיונית ומכבד גם את הגדרת המערכת.",
                highContrast: "ניגודיות גבוהה",
                highContrastDescription: "מחזק את ניגודיות הממשק בלי לשנות תוכן או מדיניות חשבון.",
                directionAutomatic: "אוטומטי",
                directionLtr: "משמאל לימין",
                directionRtl: "מימין לשמאל",
                accessibilityCountOne: "העדפת נגישות {count} פעילה",
                accessibilityCountOther: "{count} העדפות נגישות פעילות"
            )
        case .en, .system:
            return .init(
                sectionTitle: "Language & Accessibility",
                language: "Language",
                systemLanguage: "System language",
                languageDescription: "Changes visible Settings copy and locale-sensitive formatting.",
                readingDirection: "Reading direction",
                readingDirectionDescription: "Automatic follows the selected or system language.",
                textSize: "Text size",
                textSizeDescription: "Scales Fabushi text while preserving the system Dynamic Type baseline.",
                reduceMotion: "Reduce motion",
                reduceMotionDescription: "Disables non-essential animation while preserving the system accessibility setting.",
                highContrast: "High contrast controls",
                highContrastDescription: "Strengthens interface contrast without changing content or account policy.",
                directionAutomatic: "Automatic",
                directionLtr: "Left to right",
                directionRtl: "Right to left",
                accessibilityCountOne: "{count} accessibility preference active",
                accessibilityCountOther: "{count} accessibility preferences active"
            )
        }
    }

    static func copyLocaleKey(_ identifier: String) -> MobileSettingsLocale {
        let normalized = identifier.replacingOccurrences(of: "_", with: "-").lowercased()
        if normalized.hasPrefix("zh-hant")
            || normalized.hasPrefix("zh-tw")
            || normalized.hasPrefix("zh-hk")
            || normalized.hasPrefix("zh-mo")
        {
            return .zhHant
        }
        if normalized.hasPrefix("zh") { return .zhHans }
        if normalized.hasPrefix("ja") { return .ja }
        if normalized.hasPrefix("ko") { return .ko }
        if normalized.hasPrefix("ar") { return .ar }
        if normalized.hasPrefix("he") || normalized.hasPrefix("iw") { return .he }
        return .en
    }

    static func isRightToLeftLanguage(_ identifier: String) -> Bool {
        let normalized = identifier.replacingOccurrences(of: "_", with: "-").lowercased()
        guard let language = normalized.split(separator: "-").first else { return false }
        return [
            "ar", "ckb", "dv", "fa", "he", "ku", "ps", "sd", "ug", "ur", "yi",
        ].contains(String(language))
    }
}

import Foundation
import Observation
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

internal struct MobileSettingsFeatureCopy: Equatable {
    let mediaDevices: String
    let microphone: String
    let camera: String
    let defaultDevice: String
    let refreshDevices: String
    let grantMediaPermissions: String
    let mediaPermissionDescription: String
    let privacy: String
    let privacyMode: String
    let privacyModeDescription: String
    let stateEnabled: String
    let stateDisabled: String
    let autoReviewTitle: String
    let autoReviewDescription: String
    let autoReviewDraftLabel: String
    let ruleBehaviorLabel: String
    let allowAutomatically: String
    let askFirst: String
    let addRule: String
    let saveRule: String
    let cancel: String
    let edit: String
    let delete: String
    let rulesScope: String
}

/// iOS-owned presentation preferences corresponding to DesktopUiPreferences.
///
/// The values are intentionally local presentation state. They never mutate
/// account policy, Host state, Human/Agent stores, or platform accessibility
/// settings. The Scene root is the single runtime projection owner.
internal struct MobileUiPreferences: Equatable {
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

        let requestedOffset: Int
        switch textScale {
        case ..<0.95: requestedOffset = -1
        case ..<1.05: requestedOffset = 0
        case ..<1.18: requestedOffset = 1
        case ..<1.38: requestedOffset = 2
        default: requestedOffset = 3
        }
        let firstAccessibilityIndex = sizes.firstIndex(of: .accessibility1) ?? sizes.count
        let offset = index >= firstAccessibilityIndex
            ? max(0, requestedOffset)
            : requestedOffset
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

    func featureCopy(
        systemIdentifier: String = Locale.autoupdatingCurrent.identifier
    ) -> MobileSettingsFeatureCopy {
        switch Self.copyLocaleKey(
            resolvedLocaleIdentifier(systemIdentifier: systemIdentifier)
        ) {
        case .zhHans:
            return .init(
                mediaDevices: "媒体与设备", microphone: "麦克风", camera: "摄像头",
                defaultDevice: "系统默认", refreshDevices: "刷新设备",
                grantMediaPermissions: "允许麦克风和摄像头",
                mediaPermissionDescription: "设备偏好仅保存在本机，并用于 Human 通话。",
                privacy: "隐私", privacyMode: "隐私模式",
                privacyModeDescription: "由 Fabushi 账户策略强制执行，无法通过本机设置降低保护。",
                stateEnabled: "已启用", stateDisabled: "已停用",
                autoReviewTitle: "自动审查",
                autoReviewDescription: "Fabushi 会在执行前检查每项操作，并在需要时先询问你。你可以设置哪些操作可以自动执行。",
                autoReviewDraftLabel: "自动审查规则草稿", ruleBehaviorLabel: "规则行为",
                allowAutomatically: "自动允许", askFirst: "先询问", addRule: "添加规则",
                saveRule: "保存规则", cancel: "取消", edit: "编辑", delete: "删除",
                rulesScope: "这些规则仅适用于你。内置安全检查始终生效。"
            )
        case .zhHant:
            return .init(
                mediaDevices: "媒體與裝置", microphone: "麥克風", camera: "相機",
                defaultDevice: "系統預設", refreshDevices: "重新整理裝置",
                grantMediaPermissions: "允許麥克風與相機",
                mediaPermissionDescription: "裝置偏好只儲存在本機，並用於 Human 通話。",
                privacy: "隱私", privacyMode: "隱私模式",
                privacyModeDescription: "由 Fabushi 帳戶政策強制執行，無法透過本機設定降低保護。",
                stateEnabled: "已啟用", stateDisabled: "已停用",
                autoReviewTitle: "自動審查",
                autoReviewDescription: "Fabushi 會在執行前檢查每項操作，並在需要時先詢問你。你可以設定哪些操作可自動執行。",
                autoReviewDraftLabel: "自動審查規則草稿", ruleBehaviorLabel: "規則行為",
                allowAutomatically: "自動允許", askFirst: "先詢問", addRule: "新增規則",
                saveRule: "儲存規則", cancel: "取消", edit: "編輯", delete: "刪除",
                rulesScope: "這些規則僅適用於你。內建安全檢查一律生效。"
            )
        case .ja:
            return .init(
                mediaDevices: "メディアとデバイス", microphone: "マイク", camera: "カメラ",
                defaultDevice: "システム既定", refreshDevices: "デバイスを更新",
                grantMediaPermissions: "マイクとカメラを許可",
                mediaPermissionDescription: "デバイス設定はローカルに保存され、Human 通話で使用されます。",
                privacy: "プライバシー", privacyMode: "プライバシーモード",
                privacyModeDescription: "Fabushi アカウントポリシーで強制され、ローカル設定から保護を弱めることはできません。",
                stateEnabled: "有効", stateDisabled: "無効",
                autoReviewTitle: "自動レビュー",
                autoReviewDescription: "Fabushi は各操作を実行前に確認し、必要な場合は確認を求めます。自動実行できる操作をルールで設定できます。",
                autoReviewDraftLabel: "自動レビューのルール案", ruleBehaviorLabel: "ルールの動作",
                allowAutomatically: "自動的に許可", askFirst: "先に確認", addRule: "ルールを追加",
                saveRule: "ルールを保存", cancel: "キャンセル", edit: "編集", delete: "削除",
                rulesScope: "これらのルールはあなたにのみ適用されます。組み込みの安全チェックは常に有効です。"
            )
        case .ko:
            return .init(
                mediaDevices: "미디어 및 기기", microphone: "마이크", camera: "카메라",
                defaultDevice: "시스템 기본값", refreshDevices: "기기 새로고침",
                grantMediaPermissions: "마이크 및 카메라 허용",
                mediaPermissionDescription: "기기 환경설정은 로컬에 저장되고 Human 통화에 사용됩니다.",
                privacy: "개인정보 보호", privacyMode: "개인정보 보호 모드",
                privacyModeDescription: "Fabushi 계정 정책에서 강제하며 로컬 설정으로 보호 수준을 낮출 수 없습니다.",
                stateEnabled: "사용", stateDisabled: "사용 안 함",
                autoReviewTitle: "자동 검토",
                autoReviewDescription: "Fabushi는 작업을 실행하기 전에 확인하고 필요한 경우 먼저 묻습니다. 자동 실행할 작업을 규칙으로 설정하세요.",
                autoReviewDraftLabel: "자동 검토 규칙 초안", ruleBehaviorLabel: "규칙 동작",
                allowAutomatically: "자동 허용", askFirst: "먼저 묻기", addRule: "규칙 추가",
                saveRule: "규칙 저장", cancel: "취소", edit: "편집", delete: "삭제",
                rulesScope: "이 규칙은 사용자에게만 적용됩니다. 기본 안전 검사는 항상 적용됩니다."
            )
        case .ar:
            return .init(
                mediaDevices: "الوسائط والأجهزة", microphone: "الميكروفون", camera: "الكاميرا",
                defaultDevice: "الإعداد الافتراضي للنظام", refreshDevices: "تحديث الأجهزة",
                grantMediaPermissions: "السماح بالميكروفون والكاميرا",
                mediaPermissionDescription: "تُحفظ تفضيلات الأجهزة محليًا وتُستخدم في مكالمات Human.",
                privacy: "الخصوصية", privacyMode: "وضع الخصوصية",
                privacyModeDescription: "تفرضه سياسة حساب Fabushi ولا يمكن خفض مستوى الحماية من إعداد محلي.",
                stateEnabled: "مفعّل", stateDisabled: "معطّل",
                autoReviewTitle: "المراجعة التلقائية",
                autoReviewDescription: "تتحقق Fabushi من كل إجراء قبل تشغيله وتسألك عند الحاجة. أضف قواعد لتحديد ما يمكن تنفيذه تلقائيًا.",
                autoReviewDraftLabel: "مسودة قاعدة المراجعة التلقائية", ruleBehaviorLabel: "سلوك القاعدة",
                allowAutomatically: "السماح تلقائيًا", askFirst: "اسأل أولًا", addRule: "إضافة قاعدة",
                saveRule: "حفظ القاعدة", cancel: "إلغاء", edit: "تعديل", delete: "حذف",
                rulesScope: "تنطبق هذه القواعد عليك فقط. تظل فحوصات الأمان المضمنة مفعلة دائمًا."
            )
        case .he:
            return .init(
                mediaDevices: "מדיה והתקנים", microphone: "מיקרופון", camera: "מצלמה",
                defaultDevice: "ברירת מחדל של המערכת", refreshDevices: "רענון התקנים",
                grantMediaPermissions: "מתן גישה למיקרופון ולמצלמה",
                mediaPermissionDescription: "העדפות ההתקנים נשמרות מקומית ומשמשות בשיחות Human.",
                privacy: "פרטיות", privacyMode: "מצב פרטיות",
                privacyModeDescription: "נאכף על ידי מדיניות חשבון Fabushi ולא ניתן להחליש אותו בהגדרה מקומית.",
                stateEnabled: "מופעל", stateDisabled: "כבוי",
                autoReviewTitle: "בדיקה אוטומטית",
                autoReviewDescription: "Fabushi בודקת כל פעולה לפני ההרצה ושואלת אותך כשצריך. אפשר להוסיף כללים כדי לקבוע מה ניתן לבצע אוטומטית.",
                autoReviewDraftLabel: "טיוטת כלל לבדיקה אוטומטית", ruleBehaviorLabel: "התנהגות הכלל",
                allowAutomatically: "לאפשר אוטומטית", askFirst: "לשאול קודם", addRule: "הוספת כלל",
                saveRule: "שמירת הכלל", cancel: "ביטול", edit: "עריכה", delete: "מחיקה",
                rulesScope: "הכללים האלה חלים רק עליך. בדיקות הבטיחות המובנות תמיד פעילות."
            )
        case .en, .system:
            return .init(
                mediaDevices: "Media & Devices", microphone: "Microphone", camera: "Camera",
                defaultDevice: "System default", refreshDevices: "Refresh devices",
                grantMediaPermissions: "Allow microphone & camera",
                mediaPermissionDescription: "Device preferences are stored locally and used by Human calls.",
                privacy: "Privacy", privacyMode: "Privacy mode",
                privacyModeDescription: "Enforced by the Fabushi account policy and cannot be downgraded by a local setting.",
                stateEnabled: "Enabled", stateDisabled: "Disabled",
                autoReviewTitle: "Auto-review",
                autoReviewDescription: "Fabushi checks each action before it runs and asks you first when needed. Add rules to customize what it can do automatically.",
                autoReviewDraftLabel: "Auto-review rule draft", ruleBehaviorLabel: "Rule behavior",
                allowAutomatically: "Allow automatically", askFirst: "Ask first", addRule: "Add Rule",
                saveRule: "Save Rule", cancel: "Cancel", edit: "Edit", delete: "Delete",
                rulesScope: "These rules apply only to you. Built-in safety checks always apply."
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


/// Observable renderer projection of the canonical SandSettingsStore UI values.
///
/// Persistence is owned exclusively by SandSettingsStore. This object only
/// mirrors normalized values for SwiftUI and writes changes through Coordinator.
@MainActor
@Observable
internal final class MobileUiPreferencesStore {
    private let coordinator: MahayanaCoordinator
    private(set) var preferences: MobileUiPreferences

    init(coordinator: MahayanaCoordinator) {
        self.coordinator = coordinator
        let value = coordinator.uiPreferencesProjection()
        preferences = MobileUiPreferences(
            localeRaw: value.locale,
            directionRaw: value.direction,
            textScale: value.textScale,
            reducedMotion: value.reducedMotion,
            highContrast: value.highContrast
        )
    }

    func refresh() {
        apply(coordinator.uiPreferencesProjection())
    }

    func setLocale(_ locale: MobileSettingsLocale) {
        persist(locale: locale)
    }

    func setDirection(_ direction: MobileSettingsDirection) {
        persist(direction: direction)
    }

    func setTextScale(_ textScale: Double) {
        persist(textScale: textScale)
    }

    func setReducedMotion(_ reducedMotion: Bool) {
        persist(reducedMotion: reducedMotion)
    }

    func setHighContrast(_ highContrast: Bool) {
        persist(highContrast: highContrast)
    }

    private func persist(
        locale: MobileSettingsLocale? = nil,
        direction: MobileSettingsDirection? = nil,
        textScale: Double? = nil,
        reducedMotion: Bool? = nil,
        highContrast: Bool? = nil
    ) {
        let current = preferences
        let value = coordinator.updateUiPreferences(
            locale: (locale ?? current.locale).rawValue,
            direction: (direction ?? current.direction).rawValue,
            reducedMotion: reducedMotion ?? current.reducedMotion,
            highContrast: highContrast ?? current.highContrast,
            textScale: textScale ?? current.textScale
        )
        apply(value)
    }

    private func apply(_ value: MahayanaCoordinator.UiPreferencesProjection) {
        preferences = MobileUiPreferences(
            localeRaw: value.locale,
            directionRaw: value.direction,
            textScale: value.textScale,
            reducedMotion: value.reducedMotion,
            highContrast: value.highContrast
        )
    }
}


private struct MobileUiPreferencesStoreEnvironmentKey: EnvironmentKey {
    static let defaultValue: MobileUiPreferencesStore? = nil
}

extension EnvironmentValues {
    internal var mobileUiPreferencesStore: MobileUiPreferencesStore? {
        get { self[MobileUiPreferencesStoreEnvironmentKey.self] }
        set { self[MobileUiPreferencesStoreEnvironmentKey.self] = newValue }
    }
}

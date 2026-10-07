import SwiftUI

enum FabushiProductModule: String, CaseIterable, Identifiable {
    case globalDharma
    case flashcards
    case sutra
    case meditation
    case faliu
    case ai

    var id: String { rawValue }

    var title: String {
        switch self {
        case .globalDharma: "全球法布施"
        case .flashcards: "背诵闪卡"
        case .sutra: "经文听诵"
        case .meditation: "禅室修行"
        case .faliu: "法流学习"
        case .ai: "大乘 AI"
        }
    }

    var summary: String {
        switch self {
        case .globalDharma: "选择公开可传播素材并进入全球发送流程。"
        case .flashcards: "用挖空与双向复习把经文内容变成可记忆知识点。"
        case .sutra: "在经藏里继续阅读、听诵并记录学习进度。"
        case .meditation: "开始一段可计时、可计数、可回向的每日功课。"
        case .faliu: "浏览短内容、收藏主题并进入全文学习。"
        case .ai: "让大乘 AI 帮你找资源、整理经文与规划功课。"
        }
    }

    var symbol: String {
        switch self {
        case .globalDharma: "globe.asia.australia.fill"
        case .flashcards: "rectangle.on.rectangle.angled"
        case .sutra: "book.closed.fill"
        case .meditation: "figure.mind.and.body"
        case .faliu: "water.waves"
        case .ai: "sparkles"
        }
    }
}

struct FabushiSutraItem: Identifiable, Equatable {
    let id: String
    let title: String
    let category: String
    let minutes: Int
    let summary: String
    let initialProgress: Int
}

struct FabushiFlashcard: Identifiable, Equatable {
    let id: String
    let prompt: String
    let answer: String
}

enum FabushiFlashcardRating: String, CaseIterable, Identifiable {
    case again = "Again"
    case hard = "Hard"
    case good = "Good"
    case easy = "Easy"

    var id: String { rawValue }
}

func flashcardNextIntervalDays(
    rating: FabushiFlashcardRating,
    currentIntervalDays: Int
) -> Int {
    let current = max(1, currentIntervalDays)
    switch rating {
    case .again: 1
    case .hard: max(2, Int((Double(current) * 1.2).rounded(.up)))
    case .good: max(3, Int((Double(current) * 2.5).rounded(.up)))
    case .easy: max(5, Int((Double(current) * 4.0).rounded(.up)))
    }
}

struct FabushiHeroPrompt: Identifiable, Equatable {
    let id: String
    let label: String
    let prompt: String
    let symbol: String
    let tool: FabushiProductModule?
}

let fabushiHeroPrompts: [FabushiHeroPrompt] = [
    .init(id: "who", label: "你是谁", prompt: "你是谁？请用一句话介绍大乘能帮我做什么。", symbol: "sparkle", tool: nil),
    .init(id: "global-dharma", label: "全球法布施", prompt: "帮我整理一段适合全球法布施的善法文字。", symbol: "globe.asia.australia.fill", tool: .globalDharma),
    .init(id: "flashcards", label: "背诵闪卡", prompt: "把这段经文拆成适合背诵的闪卡。", symbol: "rectangle.on.rectangle.angled", tool: .flashcards),
    .init(id: "simple", label: "原来是这样", prompt: "请用庄重、简洁、容易记住的方式解释这段佛法内容。", symbol: "lightbulb.fill", tool: nil),
    .init(id: "today", label: "我今天修什么？", prompt: "根据今天的状态安排一个 20 分钟修行计划。", symbol: "safari.fill", tool: nil),
]

let fabushiGlobalDharmaRegions = [
    "中国", "新加坡", "日本", "印度", "澳大利亚", "德国",
    "法国", "英国", "美国", "加拿大", "巴西", "南非",
]

struct FabushiGeneratedFlashcard: Identifiable, Equatable {
    let id: String
    let front: String
    let back: String
    let kind: String
    let reviews: Int
    let due: String
}

func splitFabushiSentences(_ text: String) -> [String] {
    text
        .components(separatedBy: CharacterSet(charactersIn: "。！？!?；;\n"))
        .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
        .filter { $0.count > 5 }
        .prefix(6)
        .map(String.init)
}

func makeFabushiFlashcards(
    from text: String,
    createID: () -> String = { UUID().uuidString }
) -> [FabushiGeneratedFlashcard] {
    splitFabushiSentences(text).flatMap { sentence in
        let removable = CharacterSet(charactersIn: "，、：, \t\r\n")
        let plainCharacters = sentence.unicodeScalars
            .filter { !removable.contains($0) }
            .map(Character.init)
        let start = max(0, plainCharacters.count / 3 - 1)
        let end = min(plainCharacters.count, start + 4)
        let term = start < end ? String(plainCharacters[start..<end]) : ""
        let cloze: String
        if !term.isEmpty, sentence.contains(term) {
            cloze = sentence.replacingOccurrences(of: term, with: "〔……〕")
        } else {
            let head = String(sentence.prefix(8))
            let tail = String(sentence.dropFirst(min(sentence.count, 14)))
            cloze = "\(head)〔……〕\(tail)"
        }
        let excerpt = String(sentence.prefix(18))
        return [
            FabushiGeneratedFlashcard(
                id: createID(),
                front: cloze,
                back: sentence,
                kind: "挖空",
                reviews: 0,
                due: "现在"
            ),
            FabushiGeneratedFlashcard(
                id: createID(),
                front: "请背诵并解释：\(excerpt)…",
                back: sentence,
                kind: "双向",
                reviews: 0,
                due: "现在"
            ),
        ]
    }
}

func flashcardDueLabel(for rating: FabushiFlashcardRating) -> String {
    switch rating {
    case .again: "10 分钟后"
    case .hard: "明天"
    case .good: "3 天后"
    case .easy: "7 天后"
    }
}

func buildFabushiGlobalDharmaChecklist(_ text: String) -> [(region: String, text: String)] {
    let summary = text.trimmingCharacters(in: .whitespacesAndNewlines)
    let finalText = summary.isEmpty
        ? "愿以此功德，普及于一切，我等与众生，皆共成佛道。"
        : summary
    return fabushiGlobalDharmaRegions.map { ($0, finalText) }
}

struct FabushiProductHub: View {
    let onOpenGlobalDharma: () -> Void
    let onOpenAI: (String?) -> Void

    @State private var selectedModule: FabushiProductModule?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(FabushiBrand.name)
                    .font(.system(size: 16))
                    .foregroundStyle(Color.black.opacity(0.42))
                Spacer()
                Text(FabushiBrand.tagline)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 18)

            LazyVGrid(
                columns: [
                    GridItem(.flexible(), spacing: 10),
                    GridItem(.flexible(), spacing: 10),
                ],
                spacing: 10
            ) {
                ForEach(FabushiProductModule.allCases) { module in
                    Button {
                        open(module)
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: module.symbol)
                                .font(.system(size: 19, weight: .semibold))
                                .frame(width: 30, height: 30)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(module.title)
                                    .font(.system(size: 15, weight: .semibold))
                                    .foregroundStyle(.primary)
                                Text(module.summary)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(2)
                            }
                            Spacer(minLength: 0)
                        }
                        .padding(12)
                        .frame(maxWidth: .infinity, minHeight: 82, alignment: .leading)
                        .background(Color.white.opacity(0.82), in: RoundedRectangle(cornerRadius: 15))
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("fabushi-product-\(module.rawValue)")
                }
            }
            .padding(.horizontal, 18)

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(fabushiHeroPrompts) { item in
                        Button {
                            openHero(item)
                        } label: {
                            Label(item.label, systemImage: item.symbol)
                                .font(.caption.weight(.semibold))
                                .padding(.horizontal, 11)
                                .padding(.vertical, 8)
                                .background(Color.white.opacity(0.78), in: Capsule())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("fabushi-hero-\(item.id)")
                    }
                }
                .padding(.horizontal, 18)
            }
        }
        .sheet(item: $selectedModule) { module in
            NavigationStack {
                moduleSurface(module)
                    .navigationTitle(module.title)
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("关闭") { selectedModule = nil }
                        }
                    }
            }
        }
    }

    private func open(_ module: FabushiProductModule) {
        switch module {
        case .globalDharma:
            onOpenGlobalDharma()
        case .ai:
            onOpenAI(nil)
        default:
            selectedModule = module
        }
    }

    private func openHero(_ item: FabushiHeroPrompt) {
        switch item.tool {
        case .globalDharma:
            onOpenGlobalDharma()
        case .flashcards:
            selectedModule = .flashcards
        default:
            onOpenAI(item.prompt)
        }
    }

    @ViewBuilder
    private func moduleSurface(_ module: FabushiProductModule) -> some View {
        switch module {
        case .flashcards:
            FabushiFlashcardReviewView()
        case .sutra:
            FabushiSutraLibraryView()
        case .meditation:
            FabushiMeditationView()
        case .faliu:
            FabushiFaliuView()
        case .globalDharma, .ai:
            EmptyView()
        }
    }
}

struct FabushiFlashcardReviewView: View {
    private let sampleCards = [
        FabushiFlashcard(id: "heart-sutra-form", prompt: "色不异空，下一句？", answer: "空不异色。色即是空，空即是色。"),
        FabushiFlashcard(id: "diamond-no-abiding", prompt: "应无所住，下一句？", answer: "而生其心。"),
        FabushiFlashcard(id: "dedication", prompt: "每日功课结束后，最小可持续动作是什么？", answer: "如实记录，并作回向。"),
    ]

    @State private var sourceText = ""
    @State private var generatedCards: [FabushiGeneratedFlashcard] = []
    @State private var index = 0
    @State private var revealed = false
    @State private var intervalDays = 1
    @State private var dueLabel = "现在"
    @AppStorage("fabushi.flashcards.reviewCount") private var reviewCount = 0

    var body: some View {
        ScrollView {
            VStack(spacing: 18) {
                VStack(alignment: .leading, spacing: 8) {
                    Text("从经文制卡").font(.headline)
                    TextField("粘贴一段经文或学习内容", text: $sourceText, axis: .vertical)
                        .lineLimit(2...5)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("fabushi-flashcard-source")
                    Button("生成闪卡") {
                        generatedCards = makeFabushiFlashcards(from: sourceText)
                        index = 0
                        revealed = false
                        dueLabel = "现在"
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(splitFabushiSentences(sourceText).isEmpty)
                    .accessibilityIdentifier("fabushi-flashcard-generate")
                }

                Text("今日复习 \(reviewCount)")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                VStack(alignment: .leading, spacing: 16) {
                    Text(currentPrompt)
                        .font(.title3.weight(.semibold))
                    if revealed {
                        Divider()
                        Text(currentAnswer)
                            .font(.body)
                    } else {
                        Button("显示答案") { revealed = true }
                            .buttonStyle(.borderedProminent)
                            .accessibilityIdentifier("fabushi-flashcard-reveal")
                    }
                }
                .padding(20)
                .frame(maxWidth: .infinity, minHeight: 220, alignment: .topLeading)
                .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 18))

                if revealed {
                    HStack {
                        ForEach(FabushiFlashcardRating.allCases) { rating in
                            Button(rating.rawValue) { review(rating) }
                                .buttonStyle(.bordered)
                                .accessibilityIdentifier("fabushi-flashcard-rating-\(rating.rawValue.lowercased())")
                        }
                    }
                }
                Text("当前间隔：\(intervalDays) 天 · 下次：\(dueLabel)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding()
        }
        .accessibilityIdentifier("fabushi-flashcards-surface")
    }

    private var currentPrompt: String {
        if generatedCards.isEmpty {
            return sampleCards[index % sampleCards.count].prompt
        }
        return generatedCards[index % generatedCards.count].front
    }

    private var currentAnswer: String {
        if generatedCards.isEmpty {
            return sampleCards[index % sampleCards.count].answer
        }
        return generatedCards[index % generatedCards.count].back
    }

    private var activeCardCount: Int {
        generatedCards.isEmpty ? sampleCards.count : generatedCards.count
    }

    private func review(_ rating: FabushiFlashcardRating) {
        intervalDays = flashcardNextIntervalDays(
            rating: rating,
            currentIntervalDays: intervalDays
        )
        dueLabel = flashcardDueLabel(for: rating)
        reviewCount += 1
        index = (index + 1) % max(1, activeCardCount)
        revealed = false
    }
}

struct FabushiSutraLibraryView: View {
    private let sutras = [
        FabushiSutraItem(
            id: "heart-sutra",
            title: "心经",
            category: "般若",
            minutes: 8,
            summary: "适合每日短时听诵，把注意力收回空性与慈悲。",
            initialProgress: 86
        ),
        FabushiSutraItem(
            id: "diamond-sutra",
            title: "金刚经",
            category: "般若",
            minutes: 42,
            summary: "适合阶段性精读，结合重点偈句与回向记录。",
            initialProgress: 64
        ),
        FabushiSutraItem(
            id: "ksitigarbha",
            title: "地藏经",
            category: "大乘经典",
            minutes: 108,
            summary: "适合分品听诵，配合家庭、祖先和众生回向。",
            initialProgress: 32
        ),
        FabushiSutraItem(
            id: "shurangama-mantra",
            title: "楞严咒",
            category: "咒语",
            minutes: 24,
            summary: "适合固定功课，逐段熟悉发音与节奏。",
            initialProgress: 51
        ),
    ]

    @State private var progress: [String: Int] = [:]

    var body: some View {
        List(sutras) { sutra in
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(sutra.title).font(.headline)
                    Spacer()
                    Text(sutra.category).font(.caption).foregroundStyle(.secondary)
                }
                Text(sutra.summary).font(.subheadline).foregroundStyle(.secondary)
                ProgressView(value: Double(currentProgress(for: sutra)), total: 100)
                HStack {
                    Text("\(currentProgress(for: sutra))% · 约 \(sutra.minutes) 分钟")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("继续") {
                        progress[sutra.id] = min(100, currentProgress(for: sutra) + 5)
                    }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("fabushi-sutra-continue-\(sutra.id)")
                }
            }
            .padding(.vertical, 6)
        }
        .accessibilityIdentifier("fabushi-sutra-surface")
    }

    private func currentProgress(for sutra: FabushiSutraItem) -> Int {
        progress[sutra.id] ?? sutra.initialProgress
    }
}

struct FabushiMeditationView: View {
    private let presets: [(title: String, minutes: Int, dedication: String)] = [
        ("心经", 18, "回向给今日同行者与一切众生"),
        ("金刚经", 42, "愿以读诵功德增长智慧与慈悲"),
        ("地藏经", 54, "回向父母眷属、祖先与有缘众生"),
        ("楞严咒", 24, "愿身心清明，护持正念"),
    ]

    @State private var presetIndex = 0
    @State private var startedAt: Date?
    @State private var accumulatedSeconds = 0
    @State private var recitationCount = 0
    @AppStorage("fabushi.meditation.completedSessions") private var completedSessions = 0

    var body: some View {
        VStack(spacing: 18) {
            Picker("功课", selection: $presetIndex) {
                ForEach(presets.indices, id: \.self) { index in
                    Text(presets[index].title).tag(index)
                }
            }
            .pickerStyle(.segmented)

            TimelineView(.periodic(from: .now, by: 1)) { context in
                let seconds = elapsedSeconds(at: context.date)
                Text(durationText(seconds))
                    .font(.system(size: 46, weight: .medium, design: .rounded))
                    .monospacedDigit()
            }

            Text("目标 \(presets[presetIndex].minutes) 分钟")
                .foregroundStyle(.secondary)

            HStack {
                Button(startedAt == nil ? "开始" : "暂停") { toggleTimer() }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("fabushi-meditation-toggle")
                Button("念诵 +1") { recitationCount += 1 }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("fabushi-meditation-count")
            }

            Text("念诵 \(recitationCount) 次")
                .font(.title3.weight(.semibold))
            Text(presets[presetIndex].dedication)
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)

            Button("完成并回向") {
                if let startedAt {
                    accumulatedSeconds += max(0, Int(Date().timeIntervalSince(startedAt)))
                }
                startedAt = nil
                completedSessions += 1
                accumulatedSeconds = 0
                recitationCount = 0
            }
            .buttonStyle(.bordered)
            .accessibilityIdentifier("fabushi-meditation-complete")

            Text("已完成 \(completedSessions) 次功课")
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding()
        .accessibilityIdentifier("fabushi-meditation-surface")
    }

    private func elapsedSeconds(at date: Date) -> Int {
        accumulatedSeconds + (startedAt.map { max(0, Int(date.timeIntervalSince($0))) } ?? 0)
    }

    private func toggleTimer() {
        if let startedAt {
            accumulatedSeconds += max(0, Int(Date().timeIntervalSince(startedAt)))
            self.startedAt = nil
        } else {
            startedAt = Date()
        }
    }

    private func durationText(_ seconds: Int) -> String {
        String(format: "%02d:%02d", seconds / 60, seconds % 60)
    }
}

struct FabushiFaliuView: View {
    private let items = [
        ("如何把一段经文整理成可分享资料", "法布施", "4 分钟"),
        ("每日功课不稳定时，先保留一个最小动作", "修行", "3 分钟"),
        ("共修关系里最重要的是清楚、温和与可持续", "共修", "5 分钟"),
    ]

    @State private var favorites: Set<Int> = []

    var body: some View {
        List(Array(items.enumerated()), id: \.offset) { index, item in
            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .top) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text(item.0).font(.headline)
                        Text("\(item.1) · \(item.2)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button {
                        if favorites.contains(index) {
                            favorites.remove(index)
                        } else {
                            favorites.insert(index)
                        }
                    } label: {
                        Image(systemName: favorites.contains(index) ? "bookmark.fill" : "bookmark")
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("fabushi-faliu-favorite-\(index)")
                }
                Text("打开全文后可继续阅读、收藏，并从当前主题进入大乘 AI 问经。")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            .padding(.vertical, 6)
        }
        .accessibilityIdentifier("fabushi-faliu-surface")
    }
}

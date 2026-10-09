import Foundation

enum MobileSignedInOnboardingStep: Int, CaseIterable, Sendable {
    case meet = 0
    case computerDemo
    case jobs
    case tools
    case create
    case handOff
    case completed

    var next: MobileSignedInOnboardingStep {
        switch self {
        case .meet: .computerDemo
        case .computerDemo: .jobs
        case .jobs: .tools
        case .tools: .create
        case .create: .handOff
        case .handOff, .completed: .completed
        }
    }

    var previous: MobileSignedInOnboardingStep? {
        switch self {
        case .meet: nil
        case .computerDemo: .meet
        case .jobs: .computerDemo
        case .tools: .jobs
        case .create: .tools
        case .handOff: .create
        case .completed: .handOff
        }
    }
}

enum MobileOnboardingCharacterCatalog {
    static let colorIds = ["brown", "red", "orange", "yellow", "green", "cyan", "blue", "violet", "magenta", "gray"]
    static let shapeIds = ["blob", "pebble", "squircle", "tablet", "wedge", "hex", "cloud", "teardrop"]

    static var colors: [MobileAvatarColor] {
        colorIds.compactMap { id in AvatarImagePolicy.colors.first(where: { $0.id == id }) }
    }
}

struct MobileSignedInOnboardingDraft: Equatable, Sendable {
    var name = ""
    var description = ""
    var color = "blue"
    var shape = "blob"
    var pickedTemplateId: String?

    var normalized: Self {
        var value = self
        if !MobileOnboardingCharacterCatalog.colorIds.contains(value.color) {
            value.color = "blue"
        }
        if !MobileOnboardingCharacterCatalog.shapeIds.contains(value.shape) {
            value.shape = "blob"
        }
        return value
    }

    var canSubmit: Bool {
        !normalized.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}

struct MobileOnboardingTool: Identifiable, Equatable, Sendable {
    let id: String
    let label: String

    static let all: [Self] = [
        .init(id: "workspace", label: "Workspace"), .init(id: "slack", label: "Slack"),
        .init(id: "notion", label: "Notion"), .init(id: "salesforce", label: "Salesforce"),
        .init(id: "ms365", label: "Microsoft 365"), .init(id: "linkedin", label: "LinkedIn"),
        .init(id: "zoom", label: "Zoom"), .init(id: "github", label: "GitHub"),
        .init(id: "jira", label: "Jira"), .init(id: "figma", label: "Figma"),
        .init(id: "hubspot", label: "HubSpot"), .init(id: "canva", label: "Canva"),
        .init(id: "trello", label: "Trello"), .init(id: "monday", label: "monday.com"),
        .init(id: "clickup", label: "ClickUp"), .init(id: "intercom", label: "Intercom"),
        .init(id: "zendesk", label: "Zendesk"), .init(id: "box", label: "Box"),
        .init(id: "dropbox", label: "Dropbox"), .init(id: "docusign", label: "DocuSign"),
        .init(id: "calendly", label: "Calendly"), .init(id: "loom", label: "Loom"),
        .init(id: "outreach", label: "Outreach"), .init(id: "salesloft", label: "Salesloft"),
        .init(id: "apollo", label: "Apollo"), .init(id: "clay", label: "Clay"),
        .init(id: "zoominfo", label: "ZoomInfo"), .init(id: "nooks", label: "Nooks"),
        .init(id: "stripe", label: "Stripe"), .init(id: "shopify", label: "Shopify"),
        .init(id: "quickbooks", label: "QuickBooks"), .init(id: "netsuite", label: "NetSuite"),
        .init(id: "ramp", label: "Ramp"), .init(id: "workday", label: "Workday"),
        .init(id: "rippling", label: "Rippling"), .init(id: "ashby", label: "Ashby"),
        .init(id: "greenhouse", label: "Greenhouse"), .init(id: "vercel", label: "Vercel"),
        .init(id: "tableau", label: "Tableau"), .init(id: "hex", label: "Hex"),
        .init(id: "amplitude", label: "Amplitude"), .init(id: "mixpanel", label: "Mixpanel"),
        .init(id: "snowflake", label: "Snowflake"), .init(id: "databricks", label: "Databricks"),
        .init(id: "mailchimp", label: "Mailchimp"),
    ]

    static func filtered(_ query: String) -> [Self] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return all }
        return all.filter { $0.label.lowercased().contains(needle) }
    }
}

struct MobileOnboardingSuggestionChoice: Identifiable, Equatable, Sendable {
    let suggestion: MobileOnboardingSuggestion
    let renderedDescription: String
    var id: String { suggestion.id }
}

struct MobileOnboardingSuggestion: Identifiable, Equatable, Sendable {
    enum Eligibility: Equatable, Sendable {
        case universal
        case selectedTools([String])
    }

    let id: String
    let name: String
    let description: String
    let eligibility: Eligibility

    static let catalog: [Self] = [
        .init(id:"night-shift",name:"Night Shift",description:"Works overnight and preps your morning digest",eligibility:.universal),
        .init(id:"inbox-triage",name:"Inbox Triage",description:"Sorts your email and drafts replies in your voice",eligibility:.universal),
        .init(id:"chief-of-staff",name:"Chief of Staff",description:"Manages your other Bots and pulls you in for decisions",eligibility:.universal),
        .init(id:"negotiator",name:"Negotiator",description:"Researches fair pricing and haggles in your voice",eligibility:.universal),
        .init(id:"prototyper",name:"Prototyper",description:"Turns your ideas into working prototypes",eligibility:.universal),
        .init(id:"researcher",name:"Researcher",description:"Digs into any question across your tools and the web",eligibility:.universal),
        .init(id:"shopper",name:"Shopper",description:"Gathers quotes and options into a clear comparison",eligibility:.universal),
        .init(id:"apartment-scout",name:"Apartment Scout",description:"Shortlists listings the moment they drop and books tours",eligibility:.universal),
        .init(id:"lookout",name:"Lookout",description:"Watches any site and alerts you to changes",eligibility:.universal),
        .init(id:"competitor-watcher",name:"Competitor Watcher",description:"Tracks competitor pricing and launches, and briefs you weekly",eligibility:.universal),
        .init(id:"crm-scribe",name:"CRM Scribe",description:"Turns your calls into {tool} updates and follow-ups",eligibility:.selectedTools(["Salesforce","HubSpot","Outreach","Salesloft","Apollo"])),
        .init(id:"pipeline-scout",name:"Pipeline Scout",description:"Researches target accounts in {tool} and builds your attack plan",eligibility:.selectedTools(["Salesforce","HubSpot","Apollo","Clay","ZoomInfo","LinkedIn","Outreach","Salesloft"])),
        .init(id:"first-responder",name:"First Responder",description:"Answers new leads in minutes and books the meeting in {tool}",eligibility:.selectedTools(["Calendly","HubSpot","Salesforce","Intercom","Outreach","Salesloft","Apollo"])),
        .init(id:"win-loss-analyst",name:"Win-Loss Analyst",description:"Reads every lost deal in {tool} and reports why you’re really losing",eligibility:.selectedTools(["Salesforce","HubSpot","Outreach","Salesloft","Apollo"])),
        .init(id:"icebreaker",name:"Icebreaker",description:"Watches {tool} for launches and hires worth a warm intro",eligibility:.selectedTools(["LinkedIn","Clay","ZoomInfo","Apollo","Salesforce","HubSpot"])),
        .init(id:"call-coach",name:"Call Coach",description:"Rewatches your {tool} calls and gives specific coaching",eligibility:.selectedTools(["Zoom","Nooks","Loom","Outreach","Salesloft"])),
        .init(id:"deck-designer",name:"Deck Designer",description:"Turns your notes into an on-brand {tool} deck",eligibility:.selectedTools(["Canva","Figma","Workspace","Microsoft 365"])),
        .init(id:"channel-digest",name:"Channel Digest",description:"Summarizes your {tool} channels and flags what needs you",eligibility:.selectedTools(["Slack","Microsoft 365"])),
        .init(id:"ticket-triager",name:"Ticket Triager",description:"Triages everything new in {tool} and drafts the first reply",eligibility:.selectedTools(["Zendesk","Intercom","Jira","Trello","monday.com","ClickUp"])),
        .init(id:"feedback-miner",name:"Feedback Miner",description:"Clusters your {tool} feedback into clear themes",eligibility:.selectedTools(["Zendesk","Intercom","Shopify","Notion"])),
        .init(id:"review-responder",name:"Review Responder",description:"Drafts on-brand replies to reviews and messages in {tool}",eligibility:.selectedTools(["Shopify","Zendesk","Intercom"])),
        .init(id:"marketing-analyst",name:"Marketing Analyst",description:"Reports on {tool} campaign performance and where to spend next",eligibility:.selectedTools(["HubSpot","Mailchimp","Amplitude","Mixpanel","Shopify"])),
        .init(id:"shopkeeper",name:"Shopkeeper",description:"Watches orders and payouts in {tool} and flags anything odd",eligibility:.selectedTools(["Shopify","Stripe"])),
        .init(id:"invoice-chaser",name:"Invoice Chaser",description:"Tracks unpaid {tool} invoices and drafts the reminders",eligibility:.selectedTools(["QuickBooks","NetSuite","Stripe","Ramp"])),
        .init(id:"expense-auditor",name:"Expense Auditor",description:"Files receipts in {tool} daily and categorizes every charge",eligibility:.selectedTools(["QuickBooks","NetSuite","Ramp","Rippling","Workday"])),
        .init(id:"subscription-sleuth",name:"Subscription Sleuth",description:"Finds subscriptions you no longer use across your {tool} spend",eligibility:.selectedTools(["QuickBooks","NetSuite","Ramp","Stripe"])),
        .init(id:"paralegal",name:"Paralegal",description:"Reviews contracts in {tool} and drafts redlines for approval",eligibility:.selectedTools(["DocuSign","Box","Dropbox"])),
        .init(id:"application-screener",name:"Application Screener",description:"Screens new {tool} applications and surfaces the top candidates",eligibility:.selectedTools(["Ashby","Greenhouse","Workday","Rippling","LinkedIn"])),
        .init(id:"sourcing-scout",name:"Sourcing Scout",description:"Delivers qualified profiles matched to the roles open in {tool}",eligibility:.selectedTools(["Ashby","Greenhouse","Workday","Rippling","LinkedIn"])),
        .init(id:"qa-engineer",name:"QA Engineer",description:"Clicks through every new {tool} deploy and reports what breaks",eligibility:.selectedTools(["Vercel","GitHub"])),
        .init(id:"dashboard-watcher",name:"Dashboard Watcher",description:"Watches your {tool} metrics and alerts you on anomalies",eligibility:.selectedTools(["Tableau","Hex","Amplitude","Mixpanel","Snowflake","Databricks","Stripe","Shopify"])),
        .init(id:"data-scientist",name:"Data Scientist",description:"Answers data questions with real {tool} queries and charts",eligibility:.selectedTools(["Tableau","Hex","Amplitude","Mixpanel","Snowflake","Databricks"])),
    ]

    struct Identity: Equatable, Sendable {
        let color: String
        let shape: String
    }

    static func identities(for suggestions: [MobileOnboardingSuggestionChoice]) -> [Identity] {
        var usedColors = Set<String>()
        var usedShapes = Set<String>()
        return suggestions.map { choice in
            let seededColor = colorFor(choice.suggestion.name)
            let seededShape = shapeFor(choice.suggestion.name)
            let color = firstUnused(seededColor, values: MobileOnboardingCharacterCatalog.colorIds, used: usedColors)
            let shape = firstUnused(seededShape, values: MobileOnboardingCharacterCatalog.shapeIds, used: usedShapes)
            usedColors.insert(color)
            usedShapes.insert(shape)
            return .init(color: color, shape: shape)
        }
    }

    private static func fnv1a(_ value: String) -> UInt32 {
        var hash: UInt32 = 2_166_136_261
        for byte in value.utf8 {
            hash ^= UInt32(byte)
            hash = hash &* 16_777_619
        }
        return hash
    }

    private static func nextRandom(_ state: inout UInt32) -> Double {
        state = state &+ 1_831_565_813
        var next = (state ^ (state >> 15)) &* (1 | state)
        next = next &+ (((next ^ (next >> 7)) &* (61 | next)) ^ next)
        next = next ^ (next >> 14)
        return Double(next) / 4_294_967_296.0
    }

    private static func colorFor(_ name: String) -> String {
        let seed = fnv1a(name) ^ (1 &* 2_654_435_769)
        var state = seed ^ (1 &* 2_654_435_769)
        let index = Int(nextRandom(&state) * Double(MobileOnboardingCharacterCatalog.colorIds.count))
        return MobileOnboardingCharacterCatalog.colorIds[min(max(index, 0), MobileOnboardingCharacterCatalog.colorIds.count - 1)]
    }

    private static func shapeFor(_ name: String) -> String {
        var hash = fnv1a(name)
        hash = (hash ^ (hash >> 16)) &* 73_244_475
        hash = (hash ^ (hash >> 13)) &* 3_266_489_909
        hash = hash ^ (hash >> 16)
        return MobileOnboardingCharacterCatalog.shapeIds[Int(hash % UInt32(MobileOnboardingCharacterCatalog.shapeIds.count))]
    }

    private static func firstUnused(_ candidate: String, values: [String], used: Set<String>) -> String {
        guard used.contains(candidate) else { return candidate }
        let start = values.firstIndex(of: candidate) ?? 0
        for offset in 1..<values.count {
            let value = values[(start + offset) % values.count]
            if !used.contains(value) { return value }
        }
        return candidate
    }

    static func selected(for tools: [String], limit: Int = 10) -> [MobileOnboardingSuggestionChoice] {
        var output: [MobileOnboardingSuggestionChoice] = []
        var used = Set<String>()

        func append(_ suggestion: Self, tool: String?) {
            guard output.count < limit, used.insert(suggestion.id).inserted else { return }
            output.append(.init(
                suggestion: suggestion,
                renderedDescription: suggestion.description.replacingOccurrences(of: "{tool}", with: tool ?? "")
            ))
        }

        for tool in tools {
            guard output.count < limit else { break }
            let candidate = catalog.first { suggestion in
                guard !used.contains(suggestion.id),
                      case let .selectedTools(recommended) = suggestion.eligibility
                else { return false }
                return recommended.contains(tool)
            }
            if let candidate { append(candidate, tool: tool) }
        }
        for suggestion in catalog {
            guard output.count < limit else { break }
            guard !used.contains(suggestion.id),
                  case let .selectedTools(recommended) = suggestion.eligibility,
                  let tool = recommended.first(where: tools.contains)
            else { continue }
            append(suggestion, tool: tool)
        }
        for suggestion in catalog {
            guard output.count < limit else { break }
            guard !used.contains(suggestion.id), suggestion.eligibility == .universal else { continue }
            append(suggestion, tool: nil)
        }
        return output
    }
}

enum MobileSignedInOnboardingContract {
    enum Route: Equatable {
        case signIn
        case onboarding
        case shell
    }

    static let handOffDwellNanoseconds: UInt64 = 1_500_000_000
    static let jobs = ["Invoice Chaser", "Weekly Standup", "Sales Forecast"]
    static let meetText = "Hand off any task to your team of agents"

    static func resolveRoute(
        isSignedIn: Bool,
        hasSeenOnboarding: Bool,
        agentCount: Int?
    ) -> Route {
        guard isSignedIn else { return .signIn }
        if hasSeenOnboarding || (agentCount ?? 0) > 0 { return .shell }
        return .onboarding
    }

    static func descriptionWithDailyTools(_ description: String, tools: [String]) -> String {
        guard !tools.isEmpty else { return description }
        return "\(description) The user works with \(tools.joined(separator: ", ")) every day — start with those tools when suggesting connectors or taking on work."
    }

    static func createErrorMessage(_ error: Error) -> String {
        if MobileHiddenChatsMutationController.isTransportFailure(error) {
            return "Can't reach your computer right now. Check your connection and try again."
        }
        return error.localizedDescription
    }
}

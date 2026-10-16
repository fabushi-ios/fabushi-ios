import Foundation

let MAX_CRON_SEARCH_MINUTES = 366 * 24 * 60
private let AUTOMATION_MINUTE_MS: Int64 = 60_000
private let AUTOMATION_UNIT_MS: [String: Int64] = ["s":1_000,"m":60_000,"h":3_600_000,"d":86_400_000]
private let CRON_ALIASES = [
    "@hourly":"0 * * * *","@daily":"0 0 * * *","@midnight":"0 0 * * *",
    "@weekly":"0 0 * * 0","@monthly":"0 0 1 * *","@yearly":"0 0 1 1 *","@annually":"0 0 1 1 *",
]

struct CronMatcher: Equatable, Sendable {
    let minute: Set<Int>
    let hour: Set<Int>
    let dayOfMonth: Set<Int>
    let month: Set<Int>
    let dayOfWeek: Set<Int>
    let isDayOfMonthRestricted: Bool
    let isDayOfWeekRestricted: Bool
    var timeZone: String? = nil
}

struct AutomationWallClock: Equatable, Sendable {
    let year: Int
    let minute: Int
    let hour: Int
    let month: Int
    let dayOfMonth: Int
    let dayOfWeek: Int
}

func normalizeSchedule(_ raw: String) -> String {
    raw.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
}

func splitScheduleTimeZone(_ schedule: String) -> (schedule: String, timeZone: String?) {
    let normalized = normalizeSchedule(schedule)
    guard let firstSpace = normalized.firstIndex(of: " ") else { return (normalized, nil) }
    let prefix = String(normalized[..<firstSpace])
    guard prefix.hasPrefix("CRON_TZ=") || prefix.hasPrefix("TZ=") else { return (normalized, nil) }
    guard let equal = prefix.firstIndex(of: "=") else { return (normalized, nil) }
    let zone = String(prefix[prefix.index(after: equal)...])
    return (String(normalized[normalized.index(after: firstSpace)...]), zone.isEmpty ? nil : zone)
}

func expandCronAlias(_ schedule: String) -> String {
    CRON_ALIASES[schedule.lowercased()] ?? schedule
}

func parseCronField(_ field: String, min: Int, max: Int) -> Set<Int>? {
    var values = Set<Int>()
    for partSub in field.split(separator: ",", omittingEmptySubsequences: false) {
        let split = partSub.split(separator: "/", omittingEmptySubsequences: false).map(String.init)
        guard split.count <= 2 else { return nil }
        let rangePart = split[0]
        let step = split.count == 2 ? Int(split[1]) : 1
        guard let step, step > 0 else { return nil }
        let start: Int
        let end: Int
        if rangePart == "*" || rangePart.isEmpty {
            start = min; end = max
        } else if rangePart.contains("-") {
            let pieces = rangePart.split(separator: "-", omittingEmptySubsequences: false)
            guard pieces.count == 2, let a = Int(pieces[0]), let b = Int(pieces[1]) else { return nil }
            start = a; end = b
        } else {
            guard let value = Int(rangePart) else { return nil }
            start = value
            end = split.count == 2 ? max : value
        }
        guard start >= min, end <= max, start <= end else { return nil }
        var value = start
        while value <= end {
            values.insert(value)
            value += step
        }
    }
    return values.isEmpty ? nil : values
}

func parseCron(_ expression: String) -> CronMatcher? {
    let fields = expression.split(separator: " ", omittingEmptySubsequences: false).map(String.init)
    guard fields.count == 5,
          let minute = parseCronField(fields[0], min: 0, max: 59),
          let hour = parseCronField(fields[1], min: 0, max: 23),
          let dom = parseCronField(fields[2], min: 1, max: 31),
          let month = parseCronField(fields[3], min: 1, max: 12),
          let rawDow = parseCronField(fields[4], min: 0, max: 7) else { return nil }
    return .init(
        minute: minute,
        hour: hour,
        dayOfMonth: dom,
        month: month,
        dayOfWeek: Set(rawDow.map { $0 == 7 ? 0 : $0 }),
        isDayOfMonthRestricted: fields[2] != "*",
        isDayOfWeekRestricted: fields[4] != "*"
    )
}

func cronDayMatches(_ matcher: CronMatcher, wall: AutomationWallClock) -> Bool {
    guard matcher.month.contains(wall.month) else { return false }
    let dom = matcher.dayOfMonth.contains(wall.dayOfMonth)
    let dow = matcher.dayOfWeek.contains(wall.dayOfWeek)
    if matcher.isDayOfMonthRestricted && matcher.isDayOfWeekRestricted { return dom || dow }
    return (matcher.isDayOfMonthRestricted ? dom : true) && (matcher.isDayOfWeekRestricted ? dow : true)
}

func cronMatchesWallClock(_ matcher: CronMatcher, wall: AutomationWallClock) -> Bool {
    matcher.minute.contains(wall.minute) && matcher.hour.contains(wall.hour) && cronDayMatches(matcher, wall: wall)
}

func automationTimeZone(_ identifier: String?) -> TimeZone? {
    guard let identifier else { return nil }
    return TimeZone(identifier: identifier)
}

func wallClockOfInstant(_ ms: Int64, timeZone: String? = nil) -> AutomationWallClock {
    var calendar = Calendar(identifier: .gregorian)
    if let zone = automationTimeZone(timeZone) { calendar.timeZone = zone }
    let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
    let parts = calendar.dateComponents([.year,.minute,.hour,.month,.day,.weekday], from: date)
    return .init(
        year: parts.year ?? 0,
        minute: parts.minute ?? 0,
        hour: parts.hour ?? 0,
        month: parts.month ?? 0,
        dayOfMonth: parts.day ?? 0,
        dayOfWeek: max(0, (parts.weekday ?? 1) - 1)
    )
}

func nextCronRun(
    _ matcher: CronMatcher,
    afterMs: Int64,
    wallClockOf: (Int64) -> AutomationWallClock
) -> Int64? {
    var cursor = (afterMs / AUTOMATION_MINUTE_MS) * AUTOMATION_MINUTE_MS + AUTOMATION_MINUTE_MS
    let deadline = cursor + Int64(MAX_CRON_SEARCH_MINUTES) * AUTOMATION_MINUTE_MS
    while cursor < deadline {
        if cronMatchesWallClock(matcher, wall: wallClockOf(cursor)) { return cursor }
        cursor += AUTOMATION_MINUTE_MS
    }
    return nil
}

func parseEveryIntervalMs(_ schedule: String) -> Int64? {
    let pattern = #"(?i)^@every\s+(\d+)\s*(s|m|h|d)$"#
    let trimmed = schedule.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let regex = try? NSRegularExpression(pattern: pattern),
          let match = regex.firstMatch(in: trimmed, range: NSRange(trimmed.startIndex..<trimmed.endIndex, in: trimmed)) else { return nil }
    let raw = trimmed as NSString
    guard let amount = Int64(raw.substring(with: match.range(at: 1))), amount > 0 else { return nil }
    let unit = raw.substring(with: match.range(at: 2)).lowercased()
    guard let unitMs = AUTOMATION_UNIT_MS[unit] else { return nil }
    return amount * unitMs
}

func compileCronMatcher(_ schedule: String) -> CronMatcher? {
    let split = splitScheduleTimeZone(schedule)
    guard var matcher = parseCron(expandCronAlias(split.schedule)) else { return nil }
    if let zone = split.timeZone {
        guard automationTimeZone(zone) != nil else { return nil }
        matcher.timeZone = zone
    }
    return matcher
}

func formatTimestamp(_ ms: Int64?, timeZone: String? = nil) -> String {
    guard let ms else { return "never" }
    let formatter = DateFormatter()
    formatter.dateStyle = .short
    formatter.timeStyle = .short
    if let zone = automationTimeZone(timeZone) { formatter.timeZone = zone }
    return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
}

func computeNextRunAt(_ schedule: String, afterMs: Int64, timeZone: String? = nil) -> Int64? {
    let normalized = normalizeSchedule(schedule)
    if let interval = parseEveryIntervalMs(normalized) { return afterMs + interval }
    guard let matcher = compileCronMatcher(normalized) else { return nil }
    let zone = matcher.timeZone ?? timeZone
    if zone != nil && automationTimeZone(zone) == nil { return nil }
    return nextCronRun(matcher, afterMs: afterMs) { wallClockOfInstant($0, timeZone: zone) }
}

func automationAnchor(createdAt: Int64, lastRunAt: Int64?) -> Int64 {
    lastRunAt ?? createdAt
}

private let AUTOMATION_DAYS = ["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"]
private let AUTOMATION_DAYS_SHORT = ["Sun","Mon","Tue","Wed","Thu","Fri","Sat"]
private let AUTOMATION_MONTHS = ["January","February","March","April","May","June","July","August","September","October","November","December"]

private func automationJoinAnd(_ parts: [String]) -> String {
    if parts.count <= 1 { return parts.first ?? "" }
    if parts.count == 2 { return "\(parts[0]) and \(parts[1])" }
    return "\(parts.dropLast().joined(separator: ", ")), and \(parts.last!)"
}

private func automationOrdinal(_ day: Int) -> String {
    var suffix = "th"
    if day % 100 < 11 || day % 100 > 13 {
        suffix = day % 10 == 1 ? "st" : day % 10 == 2 ? "nd" : day % 10 == 3 ? "rd" : "th"
    }
    return "\(day)\(suffix)"
}

private func automationStride(_ sorted: [Int]) -> Int? {
    guard sorted.count >= 2 else { return nil }
    let step = sorted[1] - sorted[0]
    guard step > 0 else { return nil }
    for index in 2..<sorted.count where sorted[index] - sorted[index - 1] != step { return nil }
    return step
}

private func describeAutomationDays(_ matcher: CronMatcher) -> (lead: String, on: String?)? {
    let monthFull = matcher.month.count == 12
    let dom = matcher.isDayOfMonthRestricted && matcher.dayOfMonth.count < 31
    let dow = matcher.isDayOfWeekRestricted && matcher.dayOfWeek.count < 7
    if dom && dow { return nil }
    if dow {
        guard monthFull else { return nil }
        if matcher.dayOfWeek == Set([1,2,3,4,5]) { return ("Weekdays", " on weekdays") }
        if matcher.dayOfWeek == Set([0,6]) { return ("Weekends", " on weekends") }
        let days = matcher.dayOfWeek.sorted()
        if days.count > 3 {
            guard automationStride(days) == 1 else { return nil }
            let range = "\(AUTOMATION_DAYS_SHORT[days.first!])–\(AUTOMATION_DAYS_SHORT[days.last!])"
            return (range, ", \(range)")
        }
        let joined = automationJoinAnd(days.map { AUTOMATION_DAYS[$0] })
        return ("Every \(joined)", " on \(joined)")
    }
    if dom {
        let days = matcher.dayOfMonth.sorted()
        if monthFull {
            guard days.count <= 3 else { return nil }
            let ordinals = automationJoinAnd(days.map(automationOrdinal))
            return ("On the \(ordinals) of every month", " on the \(ordinals) of every month")
        }
        if matcher.month.count == 1, days.count == 1, let month = matcher.month.first {
            let date = "\(AUTOMATION_MONTHS[month - 1]) \(days[0])"
            return ("Every \(date)", " on \(date)")
        }
        return nil
    }
    return monthFull ? ("Every day", nil) : nil
}

private func automationClock(_ hour: Int, _ minute: Int) -> String {
    let period = hour < 12 ? "AM" : "PM"
    let display = hour % 12 == 0 ? 12 : hour % 12
    return String(format: "%d:%02d %@", display, minute, period)
}

private enum AutomationTimeDescription {
    case times([String])
    case interval(base: String, window: String?)
}

private func describeAutomationTime(_ matcher: CronMatcher) -> AutomationTimeDescription? {
    let minutes=matcher.minute.sorted(), hours=matcher.hour.sorted()
    guard let firstM=minutes.first, let lastM=minutes.last, let firstH=hours.first, let lastH=hours.last else { return nil }
    let fullHours=hours.count==24
    if minutes.count==1 {
        let suffix=firstM==0 ? "" : String(format:" at :%02d",firstM)
        if fullHours { return .interval(base:"Every hour\(suffix)",window:nil) }
        if hours.count==1 { return .times([automationClock(firstH,firstM)]) }
        if let step=automationStride(hours) {
            let base=step==1 ? "Every hour" : "Every \(step) hours"
            if firstH==0 && lastH+step>23 { return .interval(base:"\(base)\(suffix)",window:nil) }
            if step==1 || hours.count>3 { return .interval(base:base,window:"\(automationClock(firstH,firstM)) – \(automationClock(lastH,firstM))") }
        }
        return hours.count<=3 ? .times(hours.map { automationClock($0,firstM) }) : nil
    }
    let step=minutes.first==0 ? automationStride(minutes) : nil
    let interval=(step != nil && lastM+step!>59) ? step : nil
    let base:String
    if let interval {
        base=interval==1 ? "Every minute" : "Every \(interval) minutes"
    } else {
        guard minutes.count<=3 else { return nil }
        if !fullHours && hours.count==1 { return .times(minutes.map { automationClock(firstH,$0) }) }
        base="Every hour at \(automationJoinAnd(minutes.map { String(format:":%02d",$0) }))"
    }
    if fullHours { return .interval(base:base,window:nil) }
    guard hours.count==1 || automationStride(hours)==1 else { return nil }
    return .interval(base:base,window:"\(automationClock(firstH,firstM)) – \(automationClock(lastH,lastM))")
}

func describeSchedule(_ schedule: String) -> String {
    let normalized=normalizeSchedule(schedule)
    if let _=parseEveryIntervalMs(normalized) {
        let parts=normalized.split(whereSeparator:{$0.isWhitespace})
        guard parts.count>=2 else{return normalized}
        let token=String(parts[1])
        let digits=token.prefix{ $0.isNumber }
        let unit=token.dropFirst(digits.count).lowercased()
        let names=["s":"second","m":"minute","h":"hour","d":"day"]
        guard let name=names[unit] else{return normalized}
        return digits=="1" ? "Every \(name)" : "Every \(digits) \(name)s"
    }
    guard let matcher=compileCronMatcher(normalized),
          let days=describeAutomationDays(matcher),
          let time=describeAutomationTime(matcher) else{return normalized}
    let prose:String
    switch time {
    case .times(let times): prose="\(days.lead) at \(automationJoinAnd(times))"
    case .interval(let base,let window): prose="\(base)\(days.on ?? "")\(window.map { ", \($0)" } ?? "")"
    }
    return matcher.timeZone.map { "\(prose) (\($0))" } ?? prose
}

func describeTrigger(_ trigger: AutomationTrigger) -> String {
    triggerList(trigger).enumerated().map { index, member in
        let value: String
        if case .cron(let cron)=member { value=describeSchedule(cron.schedule) }
        else { value=describeListener(member) }
        guard index>0,let first=value.first else{return value}
        return first.lowercased() + String(value.dropFirst())
    }.joined(separator:" or ")
}

//! Fixed-source context-compaction planning at the Runner boundary.
//!
//! This module only decides whether a canonical transcript projection needs
//! compaction and where a tool-safe retained tail begins. It owns no session
//! writes, summarization transport, or provider serialization.

use std::collections::BTreeSet;

use serde_json::{Value, json};

const CHARS_PER_TOKEN_ESTIMATE: u64 = 4;
const IMAGE_BLOCK_TOKENS: u64 = 2_000;
pub const MAX_COMPACTION_SUMMARY_CHARS: usize = 16_000;
pub const MAX_LATEST_USER_REQUEST_CHARS: usize = 800;
pub const MAX_SUMMARY_INPUT_CHARS: usize = 160_000;
const MAX_SAMPLED_ENTRY_CHARS: usize = 6_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionPlan {
    /// The canonical row observed by the producer. The transcript owner uses
    /// this as a second CAS fence because ordinary appends do not advance the
    /// session lifecycle revision.
    pub observed_transcript_tail_id: String,
    pub first_kept_entry_id: String,
    pub entries_to_summarize: Vec<Value>,
    pub turn_prefix_entries: Vec<Value>,
    pub previous_summary: Option<String>,
    pub previous_details: Option<Value>,
    pub tokens_before: u64,
    pub summary_token_budget: u64,
    pub latest_user_request: Option<String>,
    pub is_split_turn: bool,
}

/// Mirrors the fixed source threshold contract. Invalid windows never trigger
/// a destructive history operation.
pub fn should_compact(
    context_tokens: u64,
    context_window: u64,
    settings: CompactionSettings,
) -> bool {
    settings.enabled
        && context_window > 0
        && settings.reserve_tokens > 0
        && context_tokens > context_window.saturating_sub(settings.reserve_tokens)
}

/// Estimate one canonical transcript row using the fixed source's conservative
/// character heuristic. Unknown metadata rows contribute no context tokens;
/// malformed replay rows are rejected later by the typed projection owner.
pub fn estimate_entry_tokens(entry: &Value) -> u64 {
    let object = match entry.as_object() {
        Some(object) => object,
        None => return 0,
    };
    let mut chars = 0u64;
    match object.get("kind").and_then(Value::as_str) {
        Some("message") => {
            chars = weighted_value_chars(object.get("content"));
            if object.get("role").and_then(Value::as_str) == Some("assistant") {
                if let Some(calls) = object.get("toolCalls").and_then(Value::as_array) {
                    chars = chars
                        .saturating_add(weighted_value_chars(Some(&Value::Array(calls.clone()))));
                }
            }
        }
        Some("send-message") => {
            chars =
                weighted_value_chars(object.get("message").and_then(|value| value.get("content")));
        }
        Some("user-attachment") => return IMAGE_BLOCK_TOKENS,
        Some("tool-call") => {
            chars = weighted_value_chars(object.get("name"));
            chars = chars.saturating_add(weighted_value_chars(
                object.get("arguments").or_else(|| object.get("args")),
            ));
        }
        Some("notice") => chars = weighted_value_chars(object.get("text")),
        Some("event") => chars = weighted_value_chars(object.get("event")),
        Some("compaction") => chars = weighted_value_chars(object.get("summary")),
        _ => {}
    }
    if chars == 0 {
        0
    } else {
        chars.saturating_add(CHARS_PER_TOKEN_ESTIMATE - 1) / CHARS_PER_TOKEN_ESTIMATE
    }
}

/// Prepare a bounded retained tail. The plan is pure and does not imply that
/// summary generation or durable commit succeeded.
pub fn prepare_compaction_plan(
    entries: &[Value],
    context_tokens: u64,
    context_window: u64,
    settings: CompactionSettings,
    preserve_from_entry_id: Option<&str>,
) -> Option<CompactionPlan> {
    if !should_compact(context_tokens, context_window, settings) || entries.len() < 2 {
        return None;
    }
    let last = entries.last()?;
    if last.get("kind").and_then(Value::as_str) == Some("compaction") {
        return None;
    }

    let mut cut = None;
    let mut last_allowed = entries.len() - 1;
    for (index, entry) in entries.iter().enumerate() {
        if preserve_from_entry_id
            .is_some_and(|id| entry.get("id").and_then(Value::as_str) == Some(id))
        {
            last_allowed = index;
        }
        if is_cut_point(entry) {
            cut = Some(index);
        }
    }
    let mut cut = cut?;
    cut = resolve_tool_pair_boundary(entries, cut);
    last_allowed = resolve_tool_pair_boundary(entries, last_allowed);

    let mut retained_tokens = 0u64;
    for index in (0..entries.len()).rev() {
        retained_tokens = retained_tokens.saturating_add(estimate_entry_tokens(&entries[index]));
        if retained_tokens >= settings.keep_recent_tokens {
            break;
        }
        if is_cut_point(&entries[index]) {
            cut = resolve_tool_pair_boundary(entries, index);
        }
    }
    cut = cut.min(last_allowed);
    if cut == 0 || cut >= entries.len() {
        return None;
    }
    let first_kept_entry_id = entries[cut]
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?
        .to_string();
    let observed_transcript_tail_id = last
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?
        .to_string();
    let tokens_before = entries[..cut]
        .iter()
        .map(estimate_entry_tokens)
        .fold(0u64, u64::saturating_add);
    let previous_summary = entries[..cut]
        .iter()
        .rev()
        .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("compaction"))
        .and_then(|entry| entry.get("summary").and_then(Value::as_str))
        .map(ToOwned::to_owned);
    let is_split_turn = entry_role(&entries[cut]) == Some("assistant");
    let turn_prefix_start = if is_split_turn {
        entries[..cut]
            .iter()
            .rposition(|entry| entry_role(entry) == Some("user"))
            .unwrap_or(cut)
    } else {
        cut
    };
    let turn_prefix_entries = entries[turn_prefix_start..cut]
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) != Some("compaction"))
        .cloned()
        .collect();
    let entries_to_summarize = entries[..turn_prefix_start]
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) != Some("compaction"))
        .cloned()
        .collect();
    let latest_user_request = entries
        .iter()
        .rev()
        .find(|entry| entry_role(entry) == Some("user"))
        .and_then(|entry| entry_text(entry))
        .filter(|text| !text.trim().is_empty())
        .map(bound_latest_user_request);

    Some(CompactionPlan {
        observed_transcript_tail_id,
        first_kept_entry_id,
        entries_to_summarize,
        turn_prefix_entries,
        previous_summary,
        previous_details: entries[..cut]
            .iter()
            .rev()
            .find(|entry| entry.get("kind").and_then(Value::as_str) == Some("compaction"))
            .and_then(|entry| entry.get("details").cloned()),
        tokens_before,
        summary_token_budget: (settings.reserve_tokens.saturating_mul(4) / 5).max(1),
        latest_user_request,
        is_split_turn,
    })
}

/// Build the bounded user prompt consumed by the existing summarization
/// inference owner. The prompt is deliberately a projection of the plan;
/// this module does not decide transport, persistence, or provider format.
pub fn build_compaction_summary_prompt(plan: &CompactionPlan) -> Result<String, String> {
    let transcript = serialize_entries_with_budget(&plan.entries_to_summarize)?;
    let previous = plan.previous_summary.as_deref().unwrap_or("(none)");
    let latest_request = plan.latest_user_request.as_deref().unwrap_or("(none)");
    let mode = if plan.previous_summary.is_some() {
        "The transcript rows are NEW conversation messages. Update the previous summary: preserve existing facts, add new progress and decisions, move completed work into Done, keep blockers separate, and remove only stale or duplicate detail."
    } else {
        "The transcript rows are a conversation to summarize. Create a structured context checkpoint for another agent to continue the work."
    };
    Ok(format!(
        "{mode}\n\nPreserve user intent, constraints, unresolved work, tool outcomes, exact paths, identifiers, and errors. Do not invent facts. The summary must fit within {budget} tokens. Use EXACTLY this format:\n\n## Goal\n- [What the user is trying to accomplish]\n\n## Constraints & Preferences\n- [Constraints or (none)]\n\n## Progress\n### Done\n- [Completed work]\n### In Progress\n- [Current work]\n\n### Blocked\n- [Blockers or (none)]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered next action]\n\n## Critical Context\n- [Facts needed to continue or (none)]\n\nPrevious compaction summary:\n<previous-summary>\n{previous}\n</previous-summary>\n\nLatest unresolved user request (keep actionable):\n<latest-request>\n{latest_request}\n</latest-request>\n\nTranscript rows to summarize (bounded JSON/sample, canonical order):\n<conversation>\n{transcript}\n</conversation>",
        mode = mode,
        budget = plan.summary_token_budget,
    ))
}

/// Prompt for the prefix of a turn whose recent suffix remains verbatim.
pub fn build_turn_prefix_summary_prompt(plan: &CompactionPlan) -> Result<String, String> {
    let transcript = serialize_entries_with_budget(&plan.turn_prefix_entries)?;
    Ok(format!(
        "This is the PREFIX of a turn that was too large to keep. The SUFFIX (recent work) is retained verbatim. Summarize only the prefix so the next agent can understand the retained suffix. Use exactly these sections:\n\n## Original Request\n[What did the user ask for?]\n\n## Early Progress\n- [Key decisions and work done in the prefix]\n\n## Context for Suffix\n- [Information needed to understand the kept suffix]\n\nBe concise and preserve exact paths, identifiers, and errors.\n\nPrefix transcript rows (JSON):\n{transcript}"
    ))
}

fn serialize_entries_with_budget(entries: &[Value]) -> Result<String, String> {
    let rendered = entries
        .iter()
        .map(|entry| {
            serde_json::to_string(entry)
                .map_err(|error| format!("could not encode compaction transcript row: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let full_chars = rendered
        .iter()
        .map(|text| weighted_chars(text) + 1)
        .sum::<usize>();
    if full_chars <= MAX_SUMMARY_INPUT_CHARS {
        return Ok(rendered.join("\n"));
    }

    // Match the fixed source's bounded tail/user/head/middle selection. The
    // passes only order selection; try_take_summary_entry enforces one global
    // budget and accounts for omission markers when a selected row splits a
    // gap.
    const OMISSION_MARKER_CHARS: usize = 40;
    const SUMMARY_INPUT_MIDDLE_SLICES: usize = 8;
    const MAX_USER_ENTRY_CHARS: usize = 2_000;
    const MIN_USER_ENTRY_CHARS: usize = 400;

    let mut selected = std::collections::BTreeMap::<usize, String>::new();
    let mut used_chars = OMISSION_MARKER_CHARS;
    let tail_limit = MAX_SUMMARY_INPUT_CHARS / 2;
    let mut tail_start = entries.len();
    for index in (0..entries.len()).rev() {
        let remaining = tail_limit.saturating_sub(used_chars);
        let verbatim = weighted_chars(&rendered[index]) + 1 <= remaining;
        if (!verbatim && tail_start != entries.len())
            || try_take_summary_entry(
                index,
                remaining,
                remaining,
                &rendered,
                &mut selected,
                &mut used_chars,
                MAX_SUMMARY_INPUT_CHARS,
                OMISSION_MARKER_CHARS,
            )
            .is_none()
        {
            break;
        }
        tail_start = index;
    }

    let older_users = entries
        .iter()
        .enumerate()
        .filter(|(index, entry)| *index < tail_start && entry_role(entry) == Some("user"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if !older_users.is_empty() {
        let user_budget = MAX_SUMMARY_INPUT_CHARS / 4;
        let user_limit = used_chars.saturating_add(user_budget);
        let user_entry_chars = MAX_USER_ENTRY_CHARS.min(
            MIN_USER_ENTRY_CHARS.max(
                user_budget
                    .checked_div(older_users.len())
                    .unwrap_or_default()
                    .saturating_sub(OMISSION_MARKER_CHARS),
            ),
        );
        let user_costs = older_users
            .iter()
            .map(|index| {
                (weighted_chars(&rendered[*index]) + 1)
                    .min(user_entry_chars + 1)
                    .saturating_add(OMISSION_MARKER_CHARS)
            })
            .sum::<usize>();
        let stride = (user_costs as f64 / user_budget.max(1) as f64).max(1.0);
        let mut position = older_users.len() as f64 - 1.0;
        while position >= 0.0 {
            let Some(index) = older_users.get(position.round() as usize).copied() else {
                break;
            };
            if try_take_summary_entry(
                index,
                user_limit.saturating_sub(used_chars),
                user_entry_chars,
                &rendered,
                &mut selected,
                &mut used_chars,
                MAX_SUMMARY_INPUT_CHARS,
                OMISSION_MARKER_CHARS,
            )
            .is_none()
            {
                break;
            }
            position -= stride;
        }
    }

    let head_limit = used_chars.saturating_add(MAX_SUMMARY_INPUT_CHARS / 10);
    let mut head_end = 0usize;
    while head_end < tail_start {
        if try_take_summary_entry(
            head_end,
            head_limit.saturating_sub(used_chars),
            MAX_SAMPLED_ENTRY_CHARS,
            &rendered,
            &mut selected,
            &mut used_chars,
            MAX_SUMMARY_INPUT_CHARS,
            OMISSION_MARKER_CHARS,
        )
        .is_none()
        {
            break;
        }
        head_end += 1;
    }

    let middle_count = tail_start.saturating_sub(head_end);
    if middle_count > 0 {
        let slice_limit =
            (MAX_SUMMARY_INPUT_CHARS.saturating_sub(used_chars)) / SUMMARY_INPUT_MIDDLE_SLICES;
        let mut next_free = head_end;
        for slice in 0..SUMMARY_INPUT_MIDDLE_SLICES {
            let mut index = next_free
                .max(head_end + slice.saturating_mul(middle_count) / SUMMARY_INPUT_MIDDLE_SLICES);
            let slice_end = used_chars.saturating_add(slice_limit);
            while index < tail_start {
                if try_take_summary_entry(
                    index,
                    slice_end.saturating_sub(used_chars),
                    MAX_SAMPLED_ENTRY_CHARS,
                    &rendered,
                    &mut selected,
                    &mut used_chars,
                    MAX_SUMMARY_INPUT_CHARS,
                    OMISSION_MARKER_CHARS,
                )
                .is_none()
                {
                    break;
                }
                index += 1;
            }
            next_free = index;
        }
    }

    let mut output = String::new();
    let mut gap = 0usize;
    for index in 0..=entries.len() {
        let selected_text = selected.get(&index);
        if selected_text.is_none() && index < entries.len() {
            gap += 1;
            continue;
        }
        if gap > 0 {
            append_summary_text(
                &mut output,
                &format!(
                    "[... {gap} {} omitted ...]\n",
                    if gap == 1 { "entry" } else { "entries" }
                ),
            );
            gap = 0;
        }
        if let Some(text) = selected_text {
            append_summary_text(&mut output, text);
            append_summary_text(&mut output, "\n");
        }
    }
    Ok(output.trim_end().to_string())
}

fn try_take_summary_entry(
    index: usize,
    limit: usize,
    max_entry_chars: usize,
    rendered: &[String],
    selected: &mut std::collections::BTreeMap<usize, String>,
    used_chars: &mut usize,
    max_chars: usize,
    omission_marker_chars: usize,
) -> Option<()> {
    if selected.contains_key(&index) {
        return Some(());
    }
    let left_open = index > 0 && !selected.contains_key(&(index - 1));
    let right_open = index + 1 < rendered.len() && !selected.contains_key(&(index + 1));
    let marker_delta = if left_open && right_open {
        omission_marker_chars as isize
    } else if !left_open && !right_open {
        -(omission_marker_chars as isize)
    } else {
        0
    };
    let marker_cost = marker_delta.max(0) as usize;
    let entry_limit = limit
        .saturating_sub(marker_cost)
        .min(max_entry_chars.saturating_add(1));
    let mut text = rendered[index].clone();
    let mut chars = weighted_chars(&text) + 1;
    if chars > entry_limit {
        text = elide_summary_entry(&text, entry_limit.saturating_sub(1))?;
        chars = weighted_chars(&text) + 1;
    }
    let total_cost = chars.saturating_add(marker_cost);
    if used_chars.saturating_add(total_cost) > max_chars {
        return None;
    }
    *used_chars = used_chars.saturating_add(total_cost);
    selected.insert(index, text);
    Some(())
}

fn elide_summary_entry(text: &str, max_chars: usize) -> Option<String> {
    const MARKER: &str = "\n\n[... characters omitted ...]\n\n";
    const MIN_TRIMMED_ENTRY_CHARS: usize = 200;
    const ELISION_MARKER_RESERVE_CHARS: usize = 64;
    if weighted_chars(text) <= max_chars {
        return Some(text.to_string());
    }
    let available = max_chars.saturating_sub(ELISION_MARKER_RESERVE_CHARS);
    if available < MIN_TRIMMED_ENTRY_CHARS {
        return None;
    }
    let head_budget = available * 7 / 10;
    let head = take_prefix_with_weight(text, head_budget);
    let tail = take_suffix_with_weight(text, available.saturating_sub(weighted_chars(&head)));
    Some(format!("{head}{MARKER}{tail}"))
}

fn append_summary_text(output: &mut String, text: &str) {
    let remaining = MAX_SUMMARY_INPUT_CHARS.saturating_sub(weighted_chars(output));
    if remaining > 0 {
        output.push_str(&take_prefix_with_weight(text, remaining));
    }
}

/// Fit model output to the provider-independent persisted-summary bound.
/// The marker is part of the durable summary so a later turn knows that the
/// tail was intentionally elided rather than silently missing.
pub fn fit_compaction_summary(summary: &str, summary_token_budget: u64) -> Result<String, String> {
    let normalized = normalize_summary_text(summary)?;
    let token_bound = usize::try_from(summary_token_budget)
        .unwrap_or(usize::MAX)
        .saturating_mul(CHARS_PER_TOKEN_ESTIMATE as usize);
    let max_chars = MAX_COMPACTION_SUMMARY_CHARS.min(token_bound.max(1));
    if weighted_chars(&normalized) <= max_chars {
        return Ok(normalized);
    }
    const MARKER: &str = "\n\n[Compaction summary truncated to fit budget]";
    if max_chars <= weighted_chars(MARKER) {
        return Ok(take_prefix_with_weight(&normalized, max_chars));
    }
    let prefix_chars = max_chars.saturating_sub(weighted_chars(MARKER));
    Ok(format!(
        "{}{}",
        take_prefix_with_weight(&normalized, prefix_chars),
        MARKER
    ))
}

/// Render the durable file-operation details into the summary body as bounded
/// model-visible metadata. Structured details remain persisted separately.
pub fn format_compaction_details_suffix(details: Option<&Value>) -> String {
    let Some(details) = details.and_then(Value::as_object) else {
        return String::new();
    };
    let read = bounded_detail_list(details.get("readFiles"));
    let modified = bounded_detail_list(details.get("modifiedFiles"));
    let mut sections = Vec::new();
    if !read.is_empty() {
        sections.push(format!("<read-files>\n{}\n</read-files>", read.join("\n")));
    }
    if !modified.is_empty() {
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified.join("\n")
        ));
    }
    if let Some(request) = details
        .get("latestUnresolvedUserRequest")
        .and_then(Value::as_str)
        .filter(|request| !request.trim().is_empty())
    {
        sections.push(format!(
            "## Latest unresolved user request\n{}",
            serde_json::to_string(request).unwrap_or_else(|_| "\"(unavailable)\"".into())
        ));
    }
    if details
        .get("qualityDegraded")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        sections.push("[Summary quality degraded: older context was retained without a successful model summary.]".into());
    }
    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", sections.join("\n\n"))
    }
}

/// Fit a summary while preserving the model-visible details suffix at the
/// end, matching the fixed source's required-prefix fitting behavior.
pub fn fit_compaction_summary_with_suffix(
    summary: &str,
    summary_token_budget: u64,
    suffix: &str,
) -> Result<String, String> {
    let normalized = normalize_summary_text(summary)?;
    let token_bound = usize::try_from(summary_token_budget)
        .unwrap_or(usize::MAX)
        .saturating_mul(CHARS_PER_TOKEN_ESTIMATE as usize);
    let max_chars = MAX_COMPACTION_SUMMARY_CHARS.min(token_bound.max(1));
    let suffix_chars = weighted_chars(suffix);
    if suffix_chars >= max_chars {
        return Err("compaction details suffix leaves no room for summary text".into());
    }
    let available = max_chars - suffix_chars;
    if weighted_chars(&normalized) <= available {
        return Ok(format!("{normalized}{suffix}"));
    }
    const MARKER: &str = "\n\n[Compaction summary truncated to fit budget]";
    if available <= weighted_chars(MARKER) {
        return Err("compaction summary cannot fit beside required details".into());
    }
    let prefix_chars = available - weighted_chars(MARKER);
    Ok(format!(
        "{}{}{}",
        take_prefix_with_weight(&normalized, prefix_chars),
        MARKER,
        suffix
    ))
}

fn bounded_detail_list(value: Option<&Value>) -> Vec<String> {
    const MAX_LIST_CHARS: usize = 900;
    let Some(values) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut output = Vec::new();
    let mut used = 0usize;
    for path in values.iter().filter_map(Value::as_str) {
        let cost = path.chars().count() + 1;
        if used + cost > MAX_LIST_CHARS {
            let remaining = values.len().saturating_sub(output.len());
            output.push(format!("...and {remaining} more"));
            break;
        }
        output.push(path.to_string());
        used += cost;
    }
    output
}

/// Fixed-source no-summary fallback. It deliberately names the dropped span
/// and carries forward the previous summary/request instead of inventing facts.
pub fn compaction_without_summary(plan: &CompactionPlan) -> String {
    let mut fallback = format!(
        "[{} earlier message(s) were removed without a summary because summarization failed. The messages after this summary are retained verbatim; ask the user if older details matter.]",
        plan.entries_to_summarize.len() + plan.turn_prefix_entries.len()
    );
    if let Some(request) = plan
        .latest_user_request
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        fallback.push_str("\n\n## Original request of the current turn\n");
        fallback.push_str(
            &serde_json::to_string(request).unwrap_or_else(|_| "\"(unavailable)\"".into()),
        );
    }
    if let Some(previous) = plan
        .previous_summary
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        fallback.push_str("\n\n## Previous summary\n");
        fallback.push_str(previous);
    }
    fallback
}

/// Reconstruct the fixed source's bounded file-operation details from the
/// canonical transcript rows. The returned value is the only details shape
/// accepted by the compaction entry owner; unknown tool rows are ignored just
/// as the fixed source ignores tools outside its read/write/edit contract.
pub fn build_compaction_details(plan: &CompactionPlan) -> Option<Value> {
    build_compaction_details_with_quality(plan, false)
}

/// Build details while recording whether the summary was produced by the
/// no-summary/quality-degraded safeguard path.
pub fn build_compaction_details_with_quality(
    plan: &CompactionPlan,
    quality_degraded: bool,
) -> Option<Value> {
    let mut read = BTreeSet::new();
    let mut written = BTreeSet::new();
    let mut edited = BTreeSet::new();
    let mut quality_degraded = quality_degraded;
    let mut previous_latest_request = None::<String>;

    if let Some(previous) = plan.previous_details.as_ref().and_then(Value::as_object) {
        add_string_array(&mut read, previous.get("readFiles"));
        add_string_array(&mut edited, previous.get("modifiedFiles"));
        quality_degraded |= previous
            .get("qualityDegraded")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        previous_latest_request = previous
            .get("latestUnresolvedUserRequest")
            .and_then(Value::as_str)
            .filter(|request| !request.trim().is_empty())
            .map(ToOwned::to_owned);
    }

    for entry in plan
        .entries_to_summarize
        .iter()
        .chain(plan.turn_prefix_entries.iter())
    {
        let Some(object) = entry.as_object() else {
            continue;
        };
        match object.get("role").and_then(Value::as_str) {
            Some("assistant") => {
                if let Some(calls) = object.get("toolCalls").and_then(Value::as_array) {
                    for call in calls {
                        let Some(call) = call.as_object() else {
                            continue;
                        };
                        let Some(name) = call.get("name").and_then(Value::as_str) else {
                            continue;
                        };
                        let path = call
                            .get("arguments")
                            .or_else(|| call.get("args"))
                            .and_then(Value::as_object)
                            .and_then(|arguments| {
                                ["path", "file_path", "filePath"]
                                    .into_iter()
                                    .find_map(|key| arguments.get(key).and_then(Value::as_str))
                            });
                        let Some(path) = path.filter(|path| !path.trim().is_empty()) else {
                            continue;
                        };
                        match normalize_file_tool_name(name).as_str() {
                            "read" => {
                                read.insert(path.to_string());
                            }
                            "write" => {
                                written.insert(path.to_string());
                            }
                            "edit" => {
                                edited.insert(path.to_string());
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("tool") => {
                if normalize_file_tool_name(
                    object
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ) == "apply_patch"
                {
                    for value in [object.get("result"), object.get("content")]
                        .into_iter()
                        .flatten()
                    {
                        collect_apply_patch_summary(value, &mut written, &mut edited);
                    }
                }
            }
            _ => {}
        }
    }

    let modified = written.into_iter().chain(edited).collect::<BTreeSet<_>>();
    let read_files = read.difference(&modified).cloned().collect::<Vec<_>>();
    let modified_files = modified.into_iter().collect::<Vec<_>>();
    let latest_request = plan
        .latest_user_request
        .as_deref()
        .filter(|request| !request.trim().is_empty())
        .or(previous_latest_request.as_deref());
    if read_files.is_empty()
        && modified_files.is_empty()
        && latest_request.is_none()
        && !quality_degraded
    {
        return None;
    }

    let mut details = json!({
        "readFiles": read_files,
        "modifiedFiles": modified_files,
    });
    let object = details.as_object_mut().expect("details object");
    if let Some(request) = latest_request {
        object.insert(
            "latestUnresolvedUserRequest".into(),
            Value::String(request.to_string()),
        );
    }
    if quality_degraded {
        object.insert("qualityDegraded".into(), Value::Bool(true));
    }
    Some(details)
}

fn add_string_array(target: &mut BTreeSet<String>, value: Option<&Value>) {
    let Some(values) = value.and_then(Value::as_array) else {
        return;
    };
    target.extend(
        values
            .iter()
            .filter_map(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(ToOwned::to_owned),
    );
}

fn collect_apply_patch_summary(
    value: &Value,
    written: &mut BTreeSet<String>,
    edited: &mut BTreeSet<String>,
) {
    let Some(object) = value.as_object() else {
        if let Some(values) = value.as_array() {
            for value in values {
                collect_apply_patch_summary(value, written, edited);
            }
        }
        return;
    };
    let Some(summary) = object
        .get("details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("summary"))
        .and_then(Value::as_object)
    else {
        return;
    };
    add_string_array(written, summary.get("added"));
    add_string_array(edited, summary.get("modified"));
}

fn normalize_file_tool_name(value: &str) -> String {
    let name = value.to_ascii_lowercase();
    let start = if name.starts_with("mcp__") { 5 } else { 0 };
    name[start..]
        .find("__")
        .map(|offset| name[start + offset + 2..].to_string())
        .unwrap_or(name)
}

/// Validate summarizer output before the session owner is allowed to mutate
/// durable transcript state. Empty, oversized, or control-only output is a
/// no-summary failure and leaves the old transcript authoritative.
pub fn validate_compaction_summary(summary: &str) -> Result<String, String> {
    let normalized = normalize_summary_text(summary)?;
    if normalized.chars().count() > MAX_COMPACTION_SUMMARY_CHARS {
        return Err(format!(
            "compaction summary exceeds {} characters",
            MAX_COMPACTION_SUMMARY_CHARS
        ));
    }
    Ok(normalized)
}

fn normalize_summary_text(summary: &str) -> Result<String, String> {
    let normalized = summary.trim();
    if normalized.is_empty() {
        return Err("compaction summarizer returned an empty summary".into());
    }
    if normalized
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err("compaction summary contains unsupported control characters".into());
    }
    Ok(normalized.to_string())
}

fn weighted_chars(text: &str) -> usize {
    text.encode_utf16().count()
        + text
            .chars()
            .map(|character| match character as u32 {
                0x00b7 | 0x3000..=0x319f | 0x4e00..=0x9fa5 | 0xac00..=0xd7af | 0xff01..=0xff60 => 3,
                0x1100..=0x11ff
                | 0x2e80..=0x2fff
                | 0x31a0..=0x4dff
                | 0x9fa6..=0x9fff
                | 0xa000..=0xa4ff
                | 0xa700..=0xa707
                | 0xa960..=0xa97f
                | 0xd7b0..=0xd7ff
                | 0xf900..=0xfaff => 11,
                0x02c7
                | 0x02c9..=0x02cb
                | 0x02d9
                | 0x02ea..=0x02eb
                | 0x0305
                | 0x0323
                | 0xfe10..=0xfe4f
                | 0xff61..=0xffdc
                | 0xffe0..=0xffe6 => 7,
                0x1d360..=0x1d371 => 10,
                0x16fe0..=0x16fff
                | 0x1aff0..=0x1afff
                | 0x1b000..=0x1b16f
                | 0x1f200..=0x1f2ff
                | 0x20000..=0x2fa1f
                | 0x30000..=0x3347f => 14,
                _ => 0,
            })
            .sum::<usize>()
}

fn weighted_value_chars(value: Option<&Value>) -> u64 {
    value
        .map(|value| weighted_chars(&value.to_string()) as u64)
        .unwrap_or_default()
}

fn take_prefix_with_weight(text: &str, max_weight: usize) -> String {
    if weighted_chars(text) <= max_weight {
        return text.to_string();
    }
    let chars = text.chars().collect::<Vec<_>>();
    let mut low = 0usize;
    let mut high = chars.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        let candidate = chars[..middle].iter().collect::<String>();
        if weighted_chars(&candidate) <= max_weight {
            low = middle;
        } else {
            high = middle.saturating_sub(1);
        }
    }
    chars[..low].iter().collect()
}

fn take_suffix_with_weight(text: &str, max_weight: usize) -> String {
    if weighted_chars(text) <= max_weight {
        return text.to_string();
    }
    let chars = text.chars().collect::<Vec<_>>();
    let mut low = 0usize;
    let mut high = chars.len();
    while low < high {
        let middle = (low + high).div_ceil(2);
        let start = chars.len().saturating_sub(middle);
        let candidate = chars[start..].iter().collect::<String>();
        if weighted_chars(&candidate) <= max_weight {
            low = middle;
        } else {
            high = middle.saturating_sub(1);
        }
    }
    chars[chars.len().saturating_sub(low)..].iter().collect()
}

fn entry_role(entry: &Value) -> Option<&str> {
    entry.get("role").and_then(Value::as_str).or_else(|| {
        entry
            .get("message")
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str)
    })
}

fn entry_text(entry: &Value) -> Option<String> {
    entry
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| entry.get("text").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn bound_latest_user_request(text: String) -> String {
    if text.chars().count() <= MAX_LATEST_USER_REQUEST_CHARS {
        return text;
    }
    const MARKER: &str = "\n[... latest user request truncated ...]\n";
    let budget = MAX_LATEST_USER_REQUEST_CHARS.saturating_sub(MARKER.chars().count());
    let head = budget / 2;
    let tail = budget.saturating_sub(head);
    let chars = text.chars().collect::<Vec<_>>();
    format!(
        "{}{}{}",
        chars.iter().take(head).copied().collect::<String>(),
        MARKER,
        chars
            .iter()
            .rev()
            .take(tail)
            .rev()
            .copied()
            .collect::<String>()
    )
}

fn is_cut_point(entry: &Value) -> bool {
    matches!(entry_role(entry), Some("user" | "assistant"))
}

fn resolve_tool_pair_boundary(entries: &[Value], index: usize) -> usize {
    let mut pair_start = index;
    for (candidate_index, entry) in entries.iter().enumerate() {
        let Some(calls) = entry.get("toolCalls").and_then(Value::as_array) else {
            continue;
        };
        let ids = calls
            .iter()
            .filter_map(|call| call.get("id").and_then(Value::as_str))
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let Some(pair_end) = entries[candidate_index + 1..]
            .iter()
            .enumerate()
            .filter_map(|(offset, result)| {
                result
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.iter().any(|candidate| *candidate == id))
                    .then_some(candidate_index + 1 + offset)
            })
            .max()
        else {
            continue;
        };
        if candidate_index <= index && index <= pair_end {
            pair_start = pair_start.min(candidate_index);
        }
    }
    pair_start
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invalid_window_does_not_trigger_compaction() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 10,
            keep_recent_tokens: 1,
        };
        assert!(!should_compact(100, 0, settings));
    }

    #[test]
    fn zero_reserve_does_not_trigger_compaction() {
        let settings = CompactionSettings {
            enabled: true,
            reserve_tokens: 0,
            keep_recent_tokens: 1,
        };
        assert!(!should_compact(101, 100, settings));
    }

    #[test]
    fn positive_reserve_always_has_a_nonzero_summary_budget() {
        let entries = vec![
            json!({"id":"old", "kind":"message", "role":"user", "content":"old"}),
            json!({"id":"new", "kind":"message", "role":"user", "content":"new"}),
        ];
        let plan = prepare_compaction_plan(
            &entries,
            100,
            100,
            CompactionSettings {
                enabled: true,
                reserve_tokens: 1,
                keep_recent_tokens: 1,
            },
            None,
        )
        .expect("positive reserve should produce a plan");
        assert_eq!(plan.summary_token_budget, 1);
    }

    #[test]
    fn cjk_weighting_matches_fixed_source_buckets() {
        assert_eq!(weighted_chars("a"), 1);
        assert_eq!(weighted_chars("中"), 4);
        assert_eq!(weighted_chars("豈"), 12);
        assert_eq!(weighted_chars("😀"), 2);
        assert_eq!(weighted_chars("𠀀"), 16);
    }

    #[test]
    fn weighted_prefix_and_suffix_stay_within_budget() {
        let text = "中文和 English 混合文本";
        let prefix = take_prefix_with_weight(text, 12);
        let suffix = take_suffix_with_weight(text, 12);
        assert!(weighted_chars(&prefix) <= 12);
        assert!(weighted_chars(&suffix) <= 12);
        assert!(text.starts_with(&prefix));
        assert!(text.ends_with(&suffix));
    }

    #[test]
    fn plan_keeps_tool_call_boundary_and_latest_user() {
        let entries = vec![
            json!({"id":"u1", "kind":"message", "role":"user", "content":"old"}),
            json!({"id":"a1", "kind":"message", "role":"assistant", "content":"call", "toolCalls":[{"id":"c1","name":"read","arguments":{}}]}),
            json!({"id":"t1", "kind":"message", "role":"tool", "toolCallId":"c1", "toolName":"read", "result":{"ok":true}}),
            json!({"id":"u2", "kind":"message", "role":"user", "content":"latest"}),
        ];
        let plan = prepare_compaction_plan(
            &entries,
            100,
            100,
            CompactionSettings {
                enabled: true,
                reserve_tokens: 1,
                keep_recent_tokens: 1,
            },
            None,
        )
        .expect("plan");
        assert_eq!(plan.first_kept_entry_id, "u2");
        assert_eq!(plan.latest_user_request.as_deref(), Some("latest"));
        assert_eq!(plan.entries_to_summarize.len(), 3);
    }

    #[test]
    fn split_turn_separates_prefix_from_older_history() {
        let entries = vec![
            json!({"id":"old", "kind":"message", "role":"user", "content":"old"}),
            json!({"id":"old-a", "kind":"message", "role":"assistant", "content":"done"}),
            json!({"id":"current", "kind":"message", "role":"user", "content":"current request"}),
            json!({"id":"current-a", "kind":"message", "role":"assistant", "content":"partial"}),
            json!({"id":"kept", "kind":"message", "role":"assistant", "content":"follow up"}),
        ];
        let plan = prepare_compaction_plan(
            &entries,
            100,
            100,
            CompactionSettings {
                enabled: true,
                reserve_tokens: 1,
                keep_recent_tokens: 1,
            },
            None,
        )
        .expect("plan");
        assert!(plan.is_split_turn);
        assert_eq!(plan.turn_prefix_entries.len(), 2);
        assert_eq!(plan.turn_prefix_entries[0]["id"], "current");
        assert_eq!(plan.entries_to_summarize.len(), 2);
    }

    #[test]
    fn prior_compaction_is_summary_context_not_new_history() {
        let entries = vec![
            json!({"id":"c0", "kind":"compaction", "summary":"prior", "tokensBefore":10}),
            json!({"id":"u1", "kind":"message", "role":"user", "content":"new"}),
            json!({"id":"a1", "kind":"message", "role":"assistant", "content":"answer"}),
        ];
        let plan = prepare_compaction_plan(
            &entries,
            100,
            100,
            CompactionSettings {
                enabled: true,
                reserve_tokens: 1,
                keep_recent_tokens: 1,
            },
            None,
        )
        .expect("plan");
        assert_eq!(plan.previous_summary.as_deref(), Some("prior"));
        assert!(
            plan.entries_to_summarize
                .iter()
                .all(|entry| { entry.get("kind").and_then(Value::as_str) != Some("compaction") })
        );
    }

    #[test]
    fn file_operation_details_merge_previous_and_current_rows() {
        let plan = CompactionPlan {
            observed_transcript_tail_id: "tail".into(),
            first_kept_entry_id: "keep".into(),
            entries_to_summarize: vec![
                json!({
                    "kind":"message",
                    "role":"assistant",
                    "toolCalls":[
                        {"name":"read", "arguments":{"path":"src/lib.rs"}},
                        {"name":"write", "arguments":{"path":"src/new.rs"}}
                    ]
                }),
                json!({
                    "kind":"message",
                    "role":"tool",
                    "toolName":"apply_patch",
                    "result":{"details":{"summary":{"modified":["src/lib.rs"],"added":["src/a.rs"]}}}
                }),
            ],
            turn_prefix_entries: Vec::new(),
            previous_summary: Some("prior".into()),
            previous_details: Some(
                json!({"readFiles":["src/old.rs"],"modifiedFiles":["src/kept.rs"]}),
            ),
            tokens_before: 10,
            summary_token_budget: 10,
            latest_user_request: Some("continue".into()),
            is_split_turn: false,
        };
        assert_eq!(
            build_compaction_details(&plan),
            Some(json!({
                "readFiles":["src/old.rs"],
                "modifiedFiles":["src/a.rs","src/kept.rs","src/lib.rs","src/new.rs"],
                "latestUnresolvedUserRequest":"continue"
            }))
        );
    }

    #[test]
    fn summary_failure_fallback_preserves_previous_context_and_request() {
        let plan = CompactionPlan {
            observed_transcript_tail_id: "tail".into(),
            first_kept_entry_id: "keep".into(),
            entries_to_summarize: vec![json!({"kind":"message","role":"user","content":"old"})],
            turn_prefix_entries: Vec::new(),
            previous_summary: Some("prior decisions".into()),
            previous_details: None,
            tokens_before: 10,
            summary_token_budget: 10,
            latest_user_request: Some("finish this".into()),
            is_split_turn: true,
        };
        let fallback = compaction_without_summary(&plan);
        assert!(fallback.contains("without a summary"));
        assert!(fallback.contains("finish this"));
        assert!(fallback.contains("prior decisions"));
    }

    #[test]
    fn degraded_fallback_is_recorded_in_compaction_details() {
        let plan = CompactionPlan {
            observed_transcript_tail_id: "tail".into(),
            first_kept_entry_id: "keep".into(),
            entries_to_summarize: vec![json!({"kind":"message","role":"user","content":"old"})],
            turn_prefix_entries: Vec::new(),
            previous_summary: None,
            previous_details: None,
            tokens_before: 10,
            summary_token_budget: 10,
            latest_user_request: None,
            is_split_turn: false,
        };
        assert_eq!(
            build_compaction_details_with_quality(&plan, true),
            Some(json!({"readFiles":[],"modifiedFiles":[],"qualityDegraded":true}))
        );
    }

    #[test]
    fn degraded_fallback_remains_true_when_previous_details_were_clean() {
        let plan = CompactionPlan {
            observed_transcript_tail_id: "tail".into(),
            first_kept_entry_id: "keep".into(),
            entries_to_summarize: vec![json!({"kind":"message","role":"user","content":"old"})],
            turn_prefix_entries: Vec::new(),
            previous_summary: Some("prior".into()),
            previous_details: Some(json!({"readFiles":[],"modifiedFiles":[]})),
            tokens_before: 10,
            summary_token_budget: 10,
            latest_user_request: None,
            is_split_turn: false,
        };
        assert_eq!(
            build_compaction_details_with_quality(&plan, true),
            Some(json!({"readFiles":[],"modifiedFiles":[],"qualityDegraded":true}))
        );
    }

    #[test]
    fn details_suffix_remains_visible_when_summary_is_fitted() {
        let details = json!({
            "readFiles":["src/read.rs"],
            "modifiedFiles":["src/write.rs"],
            "latestUnresolvedUserRequest":"continue",
            "qualityDegraded":true
        });
        let suffix = format_compaction_details_suffix(Some(&details));
        let fitted = fit_compaction_summary_with_suffix(&"x".repeat(20_000), 200, &suffix)
            .expect("fitted summary");
        assert!(fitted.ends_with(&suffix));
        assert!(fitted.contains("<read-files>"));
        assert!(fitted.contains("quality degraded"));
    }

    #[test]
    fn summary_prompt_bounds_large_transcript_and_keeps_recent_user_context() {
        let entries = (0..40)
            .map(|index| {
                json!({
                    "kind":"message",
                    "role": if index % 2 == 0 { "user" } else { "assistant" },
                    "content": format!("row-{index}-{}", "x".repeat(6_000))
                })
            })
            .collect::<Vec<_>>();
        let plan = CompactionPlan {
            observed_transcript_tail_id: "tail".into(),
            first_kept_entry_id: "keep".into(),
            entries_to_summarize: entries,
            turn_prefix_entries: Vec::new(),
            previous_summary: None,
            previous_details: None,
            tokens_before: 1,
            summary_token_budget: 100,
            latest_user_request: Some("latest request".into()),
            is_split_turn: false,
        };
        let prompt = build_compaction_summary_prompt(&plan).expect("prompt");
        assert!(prompt.contains("latest request"));
        assert!(prompt.contains("omitted"));
        assert!(prompt.contains("row-0"));
        assert!(prompt.contains("row-39"));
        assert!(prompt.len() < MAX_SUMMARY_INPUT_CHARS + 2_000);
    }
}

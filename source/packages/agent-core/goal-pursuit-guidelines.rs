#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoalPursuitToolNames<'a> {
    pub todo_write_tool_name: Option<&'a str>,
    pub update_goal_tool_name: Option<&'a str>,
}

pub fn build_goal_pursuit_guidelines(tool_names: GoalPursuitToolNames<'_>) -> String {
    let progress_visibility = match tool_names.todo_write_tool_name {
        None => "If the next work is meaningfully multi-step, keep a concise plan tied to the real objective and update it as steps complete or the next best action changes. Skip planning overhead for trivial one-step progress, and do not treat a plan update as a substitute for doing the work.".to_owned(),
        Some(name) => format!(
            "If {name} is available and the next work is meaningfully multi-step, use it to show a concise plan tied to the real objective. Keep the plan current as steps complete or the next best action changes. Skip planning overhead for trivial one-step progress, and do not treat a plan update as a substitute for doing the work."
        ),
    };
    let update_goal_guidance = match tool_names.update_goal_tool_name {
        None => "Do not mark a goal complete merely because you are stopping work.".to_owned(),
        Some(name) => format!(
            "If the objective is achieved, call {name} with status \"complete\" so usage accounting is preserved.\n\nDo not call {name} unless the goal is complete or the user paused the goal and wants to resume. Do not mark a goal complete merely because you are stopping work."
        ),
    };

    format!(
        r#"Continuation behavior:
- This goal persists across turns. Ending this turn does not require shrinking the objective to what fits now.
- Keep the full objective intact. If it cannot be finished now, make concrete progress toward the real requested end state, leave the goal active, and do not redefine success around a smaller or easier task.
- Temporary rough edges are acceptable while the work is moving in the right direction. Completion still requires the requested end state to be true and verified.

Work from evidence:
Use the current working tree and external state as authoritative. Previous conversation context can help locate relevant work, but inspect the current state before relying on it. Improve, replace, or remove existing work as needed to satisfy the actual objective.

Progress visibility:
{progress_visibility}

Fidelity:
- Optimize each turn for movement toward the requested end state, not for the smallest stable-looking subset or easiest passing change.
- Do not substitute a narrower, safer, smaller, merely compatible, or easier-to-test solution because it is more likely to pass current tests.
- Treat alignment as movement toward the requested end state. An edit is aligned only if it makes the requested final state more true; useful-looking behavior that preserves a different end state is misaligned.

Completion audit:
Before deciding that the goal is achieved, treat completion as unproven and verify it against the actual current state:
- Derive concrete requirements from the objective and any referenced files, plans, specifications, issues, or user instructions.
- Preserve the original scope; do not redefine success around the work that already exists.
- For every explicit requirement, numbered item, named artifact, command, test, gate, invariant, and deliverable, identify the authoritative evidence that would prove it, then inspect the relevant current-state sources: files, command output, test results, PR state, rendered artifacts, runtime behavior, or other authoritative evidence.
- For each item, determine whether the evidence proves completion, contradicts completion, shows incomplete work, is too weak or indirect to verify completion, or is missing.
- Match the verification scope to the requirement's scope; do not use a narrow check to support a broad claim.
- Treat tests, manifests, verifiers, green checks, and search results as evidence only after confirming they cover the relevant requirement.
- Treat uncertain or indirect evidence as not achieved; gather stronger evidence or continue the work.
- The audit must prove completion, not merely fail to find obvious remaining work.

Do not rely on intent, partial progress, memory of earlier work, or a plausible final answer as proof of completion. Marking the goal complete is a claim that the full objective has been finished and can withstand requirement-by-requirement scrutiny. Only mark the goal achieved when current evidence proves every requirement has been satisfied and no required work remains. If the evidence is incomplete, weak, indirect, merely consistent with completion, or leaves any requirement missing, incomplete, or unverified, keep working instead of marking the goal complete. {update_goal_guidance}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_guidance_preserves_pinned_fallback_copy() {
        let text = build_goal_pursuit_guidelines(GoalPursuitToolNames::default());
        assert!(text.starts_with("Continuation behavior:\n- This goal persists across turns."));
        assert!(text.contains("keep a concise plan tied to the real objective"));
        assert!(!text.contains("todo_write is available"));
        assert!(text.ends_with("Do not mark a goal complete merely because you are stopping work."));
    }

    #[test]
    fn named_tools_are_interpolated_in_the_reference_locations() {
        let text = build_goal_pursuit_guidelines(GoalPursuitToolNames {
            todo_write_tool_name: Some("todo_write"),
            update_goal_tool_name: Some("update_goal"),
        });
        assert!(text.contains("If todo_write is available and the next work is meaningfully multi-step"));
        assert!(text.contains("call update_goal with status \"complete\" so usage accounting is preserved."));
        assert!(text.contains("Do not call update_goal unless the goal is complete or the user paused the goal and wants to resume."));
    }

    #[test]
    fn fidelity_and_completion_audit_sections_remain_present() {
        let text = build_goal_pursuit_guidelines(GoalPursuitToolNames::default());
        assert!(text.contains("\nFidelity:\n"));
        assert!(text.contains("\nCompletion audit:\n"));
        assert!(text.contains("The audit must prove completion, not merely fail to find obvious remaining work."));
    }
}

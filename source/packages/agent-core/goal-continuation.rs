pub const GOAL_STATUS_ACTIVE: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalState {
    pub active_duration_ms: Option<i128>,
    pub last_accrued_at_ms: Option<i128>,
    pub status: Option<i32>,
    pub goal_id: String,
    pub conversation_id: Option<String>,
    pub agent_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalClock {
    pub active_duration_ms: i128,
    pub last_accrued_at_ms: Option<i128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalIdentity<'a> {
    pub conversation_id: &'a str,
    pub agent_session_id: Option<&'a str>,
}

pub fn goal_worked_ms(goal_state: Option<&GoalState>) -> i128 {
    goal_state
        .and_then(|state| state.active_duration_ms)
        .unwrap_or(0)
}

pub fn goal_elapsed_ms_from(
    accrued_ms: i128,
    accrual_anchor_ms: Option<i128>,
    now_ms: i128,
) -> i128 {
    match accrual_anchor_ms {
        None => accrued_ms,
        Some(anchor_ms) => accrued_ms.saturating_add(now_ms.saturating_sub(anchor_ms).max(0)),
    }
}

pub fn goal_elapsed_ms(goal_state: Option<&GoalState>, now_ms: i128) -> i128 {
    goal_elapsed_ms_from(
        goal_worked_ms(goal_state),
        goal_state.and_then(|state| state.last_accrued_at_ms),
        now_ms,
    )
}

pub fn goal_clock_on_activation(goal_state: Option<&GoalState>, now_ms: i128) -> GoalClock {
    GoalClock {
        active_duration_ms: goal_worked_ms(goal_state),
        last_accrued_at_ms: Some(now_ms),
    }
}

pub fn goal_clock_on_deactivation(goal_state: Option<&GoalState>, now_ms: i128) -> GoalClock {
    GoalClock {
        active_duration_ms: goal_elapsed_ms(goal_state, now_ms),
        last_accrued_at_ms: None,
    }
}

pub fn is_goal_owner_valid(goal_state: &GoalState, agent_session_id: Option<&str>) -> bool {
    match goal_state.agent_session_id.as_deref() {
        None => true,
        Some(goal_owner) if !goal_owner.is_empty() => {
            agent_session_id.is_some_and(|candidate| !candidate.is_empty() && candidate == goal_owner)
        }
        Some(_) => false,
    }
}

pub fn is_goal_identity_valid(goal_state: &GoalState, identity: GoalIdentity<'_>) -> bool {
    match goal_state.agent_session_id.as_deref() {
        None => goal_state.conversation_id.as_deref() == Some(identity.conversation_id),
        Some(_) => is_goal_owner_valid(goal_state, identity.agent_session_id),
    }
}

pub fn is_goal_id_valid(goal_id: &str) -> bool {
    let bytes = goal_id.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().copied().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

pub fn is_goal_state_shape_valid(
    goal_state: Option<&GoalState>,
    identity: GoalIdentity<'_>,
) -> bool {
    goal_state.is_some_and(|state| {
        is_goal_identity_valid(state, identity)
            && state.status == Some(GOAL_STATUS_ACTIVE)
            && is_goal_id_valid(&state.goal_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_goal() -> GoalState {
        GoalState {
            active_duration_ms: Some(125),
            last_accrued_at_ms: Some(1_000),
            status: Some(GOAL_STATUS_ACTIVE),
            goal_id: "01234567-89ab-cdef-0123-456789abcdef".to_owned(),
            conversation_id: Some("conversation-a".to_owned()),
            agent_session_id: None,
        }
    }

    #[test]
    fn accrues_only_non_negative_elapsed_time() {
        let goal = active_goal();
        assert_eq!(goal_worked_ms(Some(&goal)), 125);
        assert_eq!(goal_elapsed_ms(Some(&goal), 1_075), 200);
        assert_eq!(goal_elapsed_ms(Some(&goal), 900), 125);
        assert_eq!(goal_elapsed_ms_from(50, None, 9_999), 50);
    }

    #[test]
    fn activation_and_deactivation_preserve_pinned_clock_semantics() {
        let goal = active_goal();
        assert_eq!(
            goal_clock_on_activation(Some(&goal), 2_000),
            GoalClock { active_duration_ms: 125, last_accrued_at_ms: Some(2_000) }
        );
        assert_eq!(
            goal_clock_on_deactivation(Some(&goal), 1_100),
            GoalClock { active_duration_ms: 225, last_accrued_at_ms: None }
        );
    }

    #[test]
    fn validates_conversation_and_session_ownership() {
        let conversation_goal = active_goal();
        assert!(is_goal_identity_valid(
            &conversation_goal,
            GoalIdentity { conversation_id: "conversation-a", agent_session_id: None }
        ));
        assert!(!is_goal_identity_valid(
            &conversation_goal,
            GoalIdentity { conversation_id: "conversation-b", agent_session_id: None }
        ));

        let mut session_goal = active_goal();
        session_goal.agent_session_id = Some("session-1".to_owned());
        assert!(is_goal_owner_valid(&session_goal, Some("session-1")));
        assert!(is_goal_identity_valid(
            &session_goal,
            GoalIdentity { conversation_id: "different-conversation", agent_session_id: Some("session-1") }
        ));
        assert!(!is_goal_owner_valid(&session_goal, Some("session-2")));
        session_goal.agent_session_id = Some(String::new());
        assert!(!is_goal_owner_valid(&session_goal, Some("session-1")));
    }

    #[test]
    fn validates_active_state_and_case_insensitive_uuid_shape() {
        let mut goal = active_goal();
        goal.goal_id = "ABCDEF01-2345-6789-ABCD-EF0123456789".to_owned();
        let identity = GoalIdentity { conversation_id: "conversation-a", agent_session_id: None };
        assert!(is_goal_state_shape_valid(Some(&goal), identity));

        goal.goal_id = "not-a-goal-id".to_owned();
        assert!(!is_goal_state_shape_valid(Some(&goal), identity));

        goal.goal_id = "01234567-89ab-cdef-0123-456789abcdef".to_owned();
        goal.status = Some(0);
        assert!(!is_goal_state_shape_valid(Some(&goal), identity));
    }
}

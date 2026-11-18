pub mod capability_token;
pub mod floor_control;
pub mod loop_budget;
pub mod workspace;

pub use capability_token::ScopedCapabilityToken;
pub use floor_control::{FloorControlQueue, FloorPriority};
pub use loop_budget::{LoopBudget, LoopBudgetExceededError, DEFAULT_MAX_LOOP_HOPS};
pub use workspace::{SharedGroupWorkspace, SharedWorkspaceEntry};

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn test_floor_control_and_loop_budget_five_hops() {
        let mut q = FloorControlQueue::new(500);
        assert!(q.request_floor_at("a1", FloorPriority::GeneralContribution, 10));
        assert!(!q.request_floor_at("a2", FloorPriority::DirectMention, 20));
        assert!(q.request_floor_at("human", FloorPriority::UserInterruption, 30));
        assert_eq!(q.current_speaker(), Some("human"));
        q.release_floor("human");
        assert_eq!(q.current_speaker(), Some("a2"));

        let mut budget = LoopBudget::default();
        for i in 1..=5 {
            assert_eq!(budget.record_hop(&format!("agent_{i}")).unwrap(), i);
        }
        assert!(budget.is_exhausted());
        assert!(budget.record_hop("agent_6").is_err());
    }

    #[test]
    fn test_scoped_capability_token_and_shared_workspace() {
        let sk = SigningKey::from_bytes(&[11u8; 32]);
        let vk = sk.verifying_key();
        let now = chrono::Utc::now().timestamp();
        let tok = ScopedCapabilityToken::issue(&sk, "agent_x", 77, vec!["chat:write".into()], 60);
        assert!(tok.verify_for_scope(&vk, now + 5, "agent_x", 77, "chat:write"));
        assert!(!tok.verify_for_scope(&vk, now + 5, "agent_x", 78, "chat:write"));

        let mut ws = SharedGroupWorkspace::new(77);
        let r1 = ws.set("plan", serde_json::json!({"step": 1}), "agent_x");
        assert_eq!(r1, 1);
        assert!(ws.set_with_expected_revision(0, "plan", serde_json::json!({}), "a").is_err());
        assert_eq!(
            ws.set_with_expected_revision(1, "plan", serde_json::json!({"step": 2}), "agent_x")
                .unwrap(),
            2
        );
    }

    #[test]
    fn test_floor_control_stale_lease_upgrade_and_anti_starvation() {
        // 1. Eagerly expire stale lease inside request_floor_at
        let mut q_stale = FloorControlQueue::new(500);
        assert!(q_stale.request_floor_at("holder", FloorPriority::GeneralContribution, 100));
        assert!(q_stale.request_floor_at("challenger", FloorPriority::DirectMention, 5000));
        assert_eq!(q_stale.current_speaker(), Some("challenger"));

        // 2. Priority upgrade for already-queued agent
        let mut q_prio = FloorControlQueue::new(500);
        assert!(q_prio.request_floor_at("speaker", FloorPriority::GeneralContribution, 10));
        assert!(!q_prio.request_floor_at("a_upgrade", FloorPriority::GeneralContribution, 20));
        assert!(!q_prio.request_floor_at("a_upgrade", FloorPriority::DirectMention, 25));
        assert!(!q_prio.request_floor_at("a_dm", FloorPriority::DirectMention, 30));
        q_prio.release_floor("speaker");
        assert_eq!(q_prio.current_speaker(), Some("a_upgrade"));

        // 3. Preempted UserInterruption retains UserInterruption priority over DirectMention
        let mut q_burst = FloorControlQueue::new(500);
        assert!(q_burst.request_floor_at("human_1", FloorPriority::UserInterruption, 10));
        assert!(!q_burst.request_floor_at("a2", FloorPriority::DirectMention, 20));
        assert!(q_burst.request_floor_at("human_2", FloorPriority::UserInterruption, 30));
        q_burst.release_floor("human_2");
        assert_eq!(q_burst.current_speaker(), Some("human_1"));

        // 4. Anti-starvation under sustained DirectMention bursts
        let mut q_starve = FloorControlQueue::new(500);
        assert!(q_starve.request_floor_at("init", FloorPriority::DirectMention, 1));
        assert!(!q_starve.request_floor_at("low_agent", FloorPriority::GeneralContribution, 2));
        let mut low_served = false;
        for i in 0..50 {
            let dm = format!("dm_{i}");
            q_starve.request_floor_at(&dm, FloorPriority::DirectMention, 10 + i as u64);
            if let Some(cur) = q_starve.current_speaker().map(str::to_string) {
                if cur == "low_agent" {
                    low_served = true;
                    break;
                }
                q_starve.release_floor(&cur);
            }
            if q_starve.current_speaker() == Some("low_agent") {
                low_served = true;
                break;
            }
        }
        assert!(low_served, "low-priority agent must not starve under burst traffic");
    }
}

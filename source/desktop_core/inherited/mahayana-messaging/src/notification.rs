use crate::actor::ActorId;
use crate::conversation::ConversationId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationImportance {
    Silent,
    Normal,
    High,
    Urgent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuietHours {
    pub enabled: bool,
    pub start_minute_local: u16,
    pub end_minute_local: u16,
    pub allow_mentions: bool,
    pub allow_calls: bool,
}

impl QuietHours {
    pub fn active_at(&self, minute_local: u16) -> bool {
        if !self.enabled {
            return false;
        }
        let minute = minute_local % (24 * 60);
        let start = self.start_minute_local % (24 * 60);
        let end = self.end_minute_local % (24 * 60);
        if start == end {
            return true;
        }
        if start < end {
            minute >= start && minute < end
        } else {
            minute >= start || minute < end
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationNotificationRule {
    pub conversation_id: ConversationId,
    pub muted_until_ms: Option<i64>,
    pub sound: Option<String>,
    pub show_preview: bool,
    pub notify_mentions: bool,
    pub importance: NotificationImportance,
    #[serde(default)]
    pub topic_muted_until_ms: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationCandidate {
    pub id: String,
    pub conversation_id: ConversationId,
    pub sender_id: ActorId,
    pub title: String,
    pub body: String,
    pub created_at_ms: i64,
    pub mentioned_actor_ids: BTreeSet<ActorId>,
    pub is_call: bool,
    pub silent_message: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationDecision {
    pub deliver: bool,
    pub show_preview: bool,
    pub sound: Option<String>,
    pub importance: NotificationImportance,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPolicy {
    pub quiet_hours: Option<QuietHours>,
    pub conversation_rules: BTreeMap<ConversationId, ConversationNotificationRule>,
}

impl NotificationPolicy {
    pub fn decide(
        &self,
        candidate: &NotificationCandidate,
        current_actor_id: &ActorId,
        now_ms: i64,
        minute_local: u16,
    ) -> NotificationDecision {
        let rule = self.conversation_rules.get(&candidate.conversation_id);
        let importance = rule
            .map(|value| value.importance)
            .unwrap_or(NotificationImportance::Normal);
        let show_preview = rule.map(|value| value.show_preview).unwrap_or(true);
        let sound = rule.and_then(|value| value.sound.clone());
        if candidate.silent_message {
            return NotificationDecision {
                deliver: true,
                show_preview,
                sound: None,
                importance: NotificationImportance::Silent,
                reason: "silent-message".into(),
            };
        }
        if rule
            .and_then(|value| value.muted_until_ms)
            .is_some_and(|until| until > now_ms)
        {
            let mentioned = candidate.mentioned_actor_ids.contains(current_actor_id);
            if !(mentioned && rule.is_some_and(|value| value.notify_mentions)) {
                return NotificationDecision {
                    deliver: false,
                    show_preview: false,
                    sound: None,
                    importance: NotificationImportance::Silent,
                    reason: "conversation-muted".into(),
                };
            }
        }
        if let Some(ref tid) = candidate.topic_id {
            if rule
                .and_then(|r| r.topic_muted_until_ms.get(tid))
                .is_some_and(|&until| until > now_ms)
            {
                let mentioned = candidate.mentioned_actor_ids.contains(current_actor_id);
                if !(mentioned && rule.is_some_and(|r| r.notify_mentions)) {
                    return NotificationDecision {
                        deliver: false,
                        show_preview: false,
                        sound: None,
                        importance: NotificationImportance::Silent,
                        reason: "topic-muted".into(),
                    };
                }
            }
        }
        if let Some(quiet) = &self.quiet_hours {
            if quiet.active_at(minute_local) {
                let mention_allowed = quiet.allow_mentions
                    && candidate.mentioned_actor_ids.contains(current_actor_id);
                let call_allowed = quiet.allow_calls && candidate.is_call;
                if !mention_allowed && !call_allowed {
                    return NotificationDecision {
                        deliver: false,
                        show_preview: false,
                        sound: None,
                        importance: NotificationImportance::Silent,
                        reason: "quiet-hours".into(),
                    };
                }
            }
        }
        NotificationDecision {
            deliver: true,
            show_preview,
            sound,
            importance,
            reason: "deliver".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate(
        conversation_id: &str,
        topic_id: Option<&str>,
        mentioned: bool,
        current_actor: &ActorId,
    ) -> NotificationCandidate {
        let mut mentioned_actor_ids = BTreeSet::new();
        if mentioned {
            mentioned_actor_ids.insert(current_actor.clone());
        }
        NotificationCandidate {
            id: "notif-1".into(),
            conversation_id: ConversationId::new(conversation_id),
            sender_id: ActorId::new("sender-1"),
            title: "Test Message".into(),
            body: "Hello world".into(),
            created_at_ms: 1_000,
            mentioned_actor_ids,
            is_call: false,
            silent_message: false,
            topic_id: topic_id.map(|s| s.to_string()),
        }
    }

    fn make_rule(
        conversation_id: &str,
        muted_until_ms: Option<i64>,
        notify_mentions: bool,
        topic_mutes: Vec<(&str, i64)>,
    ) -> ConversationNotificationRule {
        let mut topic_muted_until_ms = BTreeMap::new();
        for (topic, until) in topic_mutes {
            topic_muted_until_ms.insert(topic.to_string(), until);
        }
        ConversationNotificationRule {
            conversation_id: ConversationId::new(conversation_id),
            muted_until_ms,
            sound: Some("bell.mp3".into()),
            show_preview: true,
            notify_mentions,
            importance: NotificationImportance::Normal,
            topic_muted_until_ms,
        }
    }

    #[test]
    fn test_topic_muted_blocks_unmentioned_notification() {
        let current_actor = ActorId::new("user-me");
        let rule = make_rule("group-1", None, true, vec![("topic-general", 10_000)]);
        let mut rules = BTreeMap::new();
        rules.insert(rule.conversation_id.clone(), rule);
        let policy = NotificationPolicy {
            quiet_hours: None,
            conversation_rules: rules,
        };

        let candidate = make_candidate("group-1", Some("topic-general"), false, &current_actor);
        let decision = policy.decide(&candidate, &current_actor, 1_000, 12 * 60);

        assert!(!decision.deliver);
        assert!(!decision.show_preview);
        assert_eq!(decision.sound, None);
        assert_eq!(decision.importance, NotificationImportance::Silent);
        assert_eq!(decision.reason, "topic-muted");
    }

    #[test]
    fn test_topic_muted_mention_penetration() {
        let current_actor = ActorId::new("user-me");
        let rule = make_rule("group-1", None, true, vec![("topic-general", 10_000)]);
        let mut rules = BTreeMap::new();
        rules.insert(rule.conversation_id.clone(), rule);
        let policy = NotificationPolicy {
            quiet_hours: None,
            conversation_rules: rules,
        };

        let candidate = make_candidate("group-1", Some("topic-general"), true, &current_actor);
        let decision = policy.decide(&candidate, &current_actor, 1_000, 12 * 60);

        assert!(decision.deliver);
        assert!(decision.show_preview);
        assert_eq!(decision.sound.as_deref(), Some("bell.mp3"));
        assert_eq!(decision.importance, NotificationImportance::Normal);
        assert_eq!(decision.reason, "deliver");
    }

    #[test]
    fn test_topic_muted_mention_penetration_disabled_when_notify_mentions_false() {
        let current_actor = ActorId::new("user-me");
        let rule = make_rule("group-1", None, false, vec![("topic-general", 10_000)]);
        let mut rules = BTreeMap::new();
        rules.insert(rule.conversation_id.clone(), rule);
        let policy = NotificationPolicy {
            quiet_hours: None,
            conversation_rules: rules,
        };

        let candidate = make_candidate("group-1", Some("topic-general"), true, &current_actor);
        let decision = policy.decide(&candidate, &current_actor, 1_000, 12 * 60);

        assert!(!decision.deliver);
        assert_eq!(decision.reason, "topic-muted");
    }

    #[test]
    fn test_topic_muted_expired_allows_notification() {
        let current_actor = ActorId::new("user-me");
        let rule = make_rule("group-1", None, true, vec![("topic-general", 5_000)]);
        let mut rules = BTreeMap::new();
        rules.insert(rule.conversation_id.clone(), rule);
        let policy = NotificationPolicy {
            quiet_hours: None,
            conversation_rules: rules,
        };

        let candidate = make_candidate("group-1", Some("topic-general"), false, &current_actor);
        // At exact expiry or past expiry (now_ms >= until_ms)
        let decision_at_expiry = policy.decide(&candidate, &current_actor, 5_000, 12 * 60);
        assert!(decision_at_expiry.deliver);
        assert_eq!(decision_at_expiry.reason, "deliver");

        let decision_past_expiry = policy.decide(&candidate, &current_actor, 6_000, 12 * 60);
        assert!(decision_past_expiry.deliver);
        assert_eq!(decision_past_expiry.reason, "deliver");
    }

    #[test]
    fn test_unmuted_topic_delivers() {
        let current_actor = ActorId::new("user-me");
        let rule = make_rule("group-1", None, true, vec![("topic-other", 10_000)]);
        let mut rules = BTreeMap::new();
        rules.insert(rule.conversation_id.clone(), rule);
        let policy = NotificationPolicy {
            quiet_hours: None,
            conversation_rules: rules,
        };

        let candidate = make_candidate("group-1", Some("topic-general"), false, &current_actor);
        let decision = policy.decide(&candidate, &current_actor, 1_000, 12 * 60);

        assert!(decision.deliver);
        assert_eq!(decision.reason, "deliver");
    }

    #[test]
    fn test_conversation_muted_takes_precedence_over_topic_muted() {
        let current_actor = ActorId::new("user-me");
        let rule = make_rule(
            "group-1",
            Some(20_000),
            true,
            vec![("topic-general", 10_000)],
        );
        let mut rules = BTreeMap::new();
        rules.insert(rule.conversation_id.clone(), rule);
        let policy = NotificationPolicy {
            quiet_hours: None,
            conversation_rules: rules,
        };

        let candidate = make_candidate("group-1", Some("topic-general"), false, &current_actor);
        let decision = policy.decide(&candidate, &current_actor, 1_000, 12 * 60);

        assert!(!decision.deliver);
        assert_eq!(decision.reason, "conversation-muted");
    }

    #[test]
    fn test_silent_message_overrides_conversation_and_topic_mutes() {
        let current_actor = ActorId::new("user-me");
        let rule = make_rule(
            "group-1",
            Some(20_000),
            true,
            vec![("topic-general", 10_000)],
        );
        let mut rules = BTreeMap::new();
        rules.insert(rule.conversation_id.clone(), rule);
        let policy = NotificationPolicy {
            quiet_hours: None,
            conversation_rules: rules,
        };

        let mut candidate =
            make_candidate("group-1", Some("topic-general"), false, &current_actor);
        candidate.silent_message = true;
        let decision = policy.decide(&candidate, &current_actor, 1_000, 12 * 60);

        assert!(decision.deliver);
        assert_eq!(decision.importance, NotificationImportance::Silent);
        assert_eq!(decision.sound, None);
        assert_eq!(decision.reason, "silent-message");
    }

    #[test]
    fn test_backward_compatibility_serialization() {
        let json_rule = r#"{
            "conversationId": "chat:1",
            "mutedUntilMs": null,
            "sound": null,
            "showPreview": true,
            "notifyMentions": true,
            "importance": "normal"
        }"#;
        let rule: ConversationNotificationRule = serde_json::from_str(json_rule).unwrap();
        assert!(rule.topic_muted_until_ms.is_empty());

        let json_cand = r#"{
            "id": "cand-1",
            "conversationId": "chat:1",
            "senderId": "sender:1",
            "title": "Title",
            "body": "Body",
            "createdAtMs": 100,
            "mentionedActorIds": [],
            "isCall": false,
            "silentMessage": false
        }"#;
        let cand: NotificationCandidate = serde_json::from_str(json_cand).unwrap();
        assert_eq!(cand.topic_id, None);
    }
}

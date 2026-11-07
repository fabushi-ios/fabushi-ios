use crate::actor::ActorId;
use crate::conversation::ConversationId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallKind {
    Voice,
    Video,
    GroupVoice,
    GroupVideo,
    LiveStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallState {
    Ringing,
    Connecting,
    Active,
    Held,
    Ended,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallParticipant {
    pub actor_id: ActorId,
    pub joined_at_ms: Option<i64>,
    pub left_at_ms: Option<i64>,
    pub muted: bool,
    pub video_enabled: bool,
    pub screen_sharing: bool,
    pub speaking: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRoute {
    pub region: String,
    pub signaling_url: String,
    pub media_relay_urls: Vec<String>,
    pub ice_servers: Vec<IceServer>,
    pub end_to_end_encrypted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallSession {
    pub id: CallId,
    pub conversation_id: ConversationId,
    pub kind: CallKind,
    pub state: CallState,
    pub initiator_id: ActorId,
    pub participants: BTreeMap<ActorId, CallParticipant>,
    pub route: Option<CallRoute>,
    pub created_at_ms: i64,
    pub connected_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum CallSignal {
    Invite {
        call_id: CallId,
        conversation_id: ConversationId,
        kind: CallKind,
        from: ActorId,
        participants: Vec<ActorId>,
    },
    Ringing {
        call_id: CallId,
        actor_id: ActorId,
    },
    Accept {
        call_id: CallId,
        actor_id: ActorId,
    },
    Decline {
        call_id: CallId,
        actor_id: ActorId,
        reason: String,
    },
    SdpOffer {
        call_id: CallId,
        from: ActorId,
        to: ActorId,
        sdp: String,
    },
    SdpAnswer {
        call_id: CallId,
        from: ActorId,
        to: ActorId,
        sdp: String,
    },
    IceCandidate {
        call_id: CallId,
        from: ActorId,
        to: Option<ActorId>,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    MediaState {
        call_id: CallId,
        actor_id: ActorId,
        muted: bool,
        video_enabled: bool,
        screen_sharing: bool,
    },
    Speaking {
        call_id: CallId,
        actor_id: ActorId,
        active: bool,
    },
    Hangup {
        call_id: CallId,
        actor_id: ActorId,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypingState {
    pub conversation_id: ConversationId,
    pub actor_id: ActorId,
    pub action: String,
    pub expires_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
}

impl TypingState {
    pub fn new(
        conversation_id: ConversationId,
        actor_id: ActorId,
        action: impl Into<String>,
        expires_at_ms: i64,
    ) -> Self {
        Self {
            conversation_id,
            actor_id,
            action: action.into(),
            expires_at_ms,
            topic_id: None,
        }
    }

    pub fn with_topic(mut self, topic_id: impl Into<String>) -> Self {
        self.topic_id = Some(topic_id.into());
        self
    }
}

pub const DEFAULT_TYPING_TTL_MS: i64 = 6_000;
pub const MAX_TYPING_TTL_MS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
pub enum TypingPolicyDenial {
    #[error("actor is not a participant in conversation")]
    NotParticipant,
    #[error("channel typing is not allowed")]
    ChannelTypingNotAllowed,
    #[error("actor is restricted from sending messages")]
    SendMessagesRestricted,
    #[error("target topic is closed")]
    TopicClosed,
    #[error("invalid typing action")]
    InvalidAction,
    #[error("invalid typing ttl")]
    InvalidTtl,
}

pub fn evaluate_typing_publication(
    conversation_id: ConversationId,
    conversation_kind: crate::conversation::ConversationKind,
    actor_id: ActorId,
    sender_is_participant: bool,
    member: Option<&crate::community::CommunityMember>,
    topic_id: Option<String>,
    topic_closed: bool,
    action: &str,
    now_ms: i64,
    requested_ttl_ms: Option<i64>,
) -> Result<TypingState, TypingPolicyDenial> {
    let trimmed_action = action.trim();
    if trimmed_action.is_empty() || trimmed_action.len() > 64 || action.len() > 64 {
        return Err(TypingPolicyDenial::InvalidAction);
    }

    let ttl = match requested_ttl_ms {
        Some(ttl) if ttl <= 0 => return Err(TypingPolicyDenial::InvalidTtl),
        Some(ttl) => ttl.clamp(1, MAX_TYPING_TTL_MS),
        None => DEFAULT_TYPING_TTL_MS,
    };

    match conversation_kind {
        crate::conversation::ConversationKind::Direct
        | crate::conversation::ConversationKind::SavedMessages
        | crate::conversation::ConversationKind::Secret => {
            if !sender_is_participant {
                return Err(TypingPolicyDenial::NotParticipant);
            }
        }
        crate::conversation::ConversationKind::Group
        | crate::conversation::ConversationKind::Channel => {
            let member = member.ok_or(TypingPolicyDenial::NotParticipant)?;
            match member.status {
                crate::community::MemberStatus::Left
                | crate::community::MemberStatus::Banned => {
                    return Err(TypingPolicyDenial::NotParticipant);
                }
                _ => {}
            }

            if conversation_kind == crate::conversation::ConversationKind::Channel {
                let can_post_channel = member.status == crate::community::MemberStatus::Owner
                    || (member.status == crate::community::MemberStatus::Administrator
                        && member.admin_rights.post_messages);
                if !can_post_channel {
                    return Err(TypingPolicyDenial::ChannelTypingNotAllowed);
                }
            }

            if member.status == crate::community::MemberStatus::Restricted {
                let restriction_active = match member.restrictions.until_ms {
                    None => true,
                    Some(until_ms) => now_ms < until_ms,
                };
                if restriction_active && member.restrictions.send_messages {
                    return Err(TypingPolicyDenial::SendMessagesRestricted);
                }
            }
        }
    }

    if topic_closed {
        let can_manage_topics = member.is_some_and(|m| {
            m.status == crate::community::MemberStatus::Owner
                || (m.status == crate::community::MemberStatus::Administrator
                    && m.admin_rights.manage_topics)
        });
        if !can_manage_topics {
            return Err(TypingPolicyDenial::TopicClosed);
        }
    }

    let expires_at_ms = now_ms.saturating_add(ttl);
    Ok(TypingState {
        conversation_id,
        actor_id,
        action: trimmed_action.to_string(),
        expires_at_ms,
        topic_id,
    })
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RealtimeError {
    #[error("call {0:?} already exists")]
    DuplicateCall(CallId),
    #[error("call {0:?} does not exist")]
    CallNotFound(CallId),
    #[error("actor {actor_id:?} is not a participant in call {call_id:?}")]
    ParticipantNotFound { call_id: CallId, actor_id: ActorId },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeState {
    pub calls: BTreeMap<CallId, CallSession>,
    pub typing: BTreeMap<(ConversationId, ActorId), TypingState>,
}

impl RealtimeState {
    pub fn create_call(&mut self, call: CallSession) -> Result<(), RealtimeError> {
        if self.calls.contains_key(&call.id) {
            return Err(RealtimeError::DuplicateCall(call.id));
        }
        self.calls.insert(call.id.clone(), call);
        Ok(())
    }

    pub fn set_participant_media(
        &mut self,
        call_id: &CallId,
        actor_id: &ActorId,
        muted: bool,
        video_enabled: bool,
        screen_sharing: bool,
    ) -> Result<(), RealtimeError> {
        let call = self
            .calls
            .get_mut(call_id)
            .ok_or_else(|| RealtimeError::CallNotFound(call_id.clone()))?;
        let participant = call.participants.get_mut(actor_id).ok_or_else(|| {
            RealtimeError::ParticipantNotFound {
                call_id: call_id.clone(),
                actor_id: actor_id.clone(),
            }
        })?;
        participant.muted = muted;
        participant.video_enabled = video_enabled;
        participant.screen_sharing = screen_sharing;
        Ok(())
    }

    pub fn set_typing(&mut self, state: TypingState) {
        self.typing.insert(
            (state.conversation_id.clone(), state.actor_id.clone()),
            state,
        );
    }

    pub fn expire_typing(&mut self, now_ms: i64) {
        self.typing.retain(|_, state| state.expires_at_ms > now_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::ActorId;
    use crate::community::{AdminRights, CommunityMember, MemberRestrictions, MemberStatus};
    use crate::conversation::{ConversationId, ConversationKind};

    fn make_member(status: MemberStatus) -> CommunityMember {
        CommunityMember {
            actor_id: ActorId::new("actor-1"),
            status,
            admin_title: None,
            admin_rights: AdminRights::default(),
            restrictions: MemberRestrictions::default(),
            joined_at_ms: 1_000,
            invited_by: None,
        }
    }

    #[test]
    fn test_action_validation_rejects_empty_and_oversized() {
        let conv = ConversationId::new("conv-1");
        let actor = ActorId::new("actor-1");

        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Direct,
                actor.clone(),
                true,
                None,
                None,
                false,
                "",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::InvalidAction)
        );

        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Direct,
                actor.clone(),
                true,
                None,
                None,
                false,
                "   \t  \n ",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::InvalidAction)
        );

        let long_action = "a".repeat(65);
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Direct,
                actor.clone(),
                true,
                None,
                None,
                false,
                &long_action,
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::InvalidAction)
        );

        // 64 chars is allowed
        let exact_64 = "a".repeat(64);
        let res = evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Direct,
            actor.clone(),
            true,
            None,
            None,
            false,
            &exact_64,
            1_000,
            None,
        );
        assert!(res.is_ok());
        assert_eq!(res.unwrap().action, exact_64);

        // Trimming whitespace
        let res_trimmed = evaluate_typing_publication(
            conv,
            ConversationKind::Direct,
            actor,
            true,
            None,
            None,
            false,
            "   typing   ",
            1_000,
            None,
        );
        assert!(res_trimmed.is_ok());
        assert_eq!(res_trimmed.unwrap().action, "typing");
    }

    #[test]
    fn test_ttl_validation_and_clamping() {
        let conv = ConversationId::new("conv-1");
        let actor = ActorId::new("actor-1");

        // Zero or negative TTL rejected
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Direct,
                actor.clone(),
                true,
                None,
                None,
                false,
                "typing",
                1_000,
                Some(0),
            ),
            Err(TypingPolicyDenial::InvalidTtl)
        );

        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Direct,
                actor.clone(),
                true,
                None,
                None,
                false,
                "typing",
                1_000,
                Some(-100),
            ),
            Err(TypingPolicyDenial::InvalidTtl)
        );

        // None defaults to DEFAULT_TYPING_TTL_MS (6_000)
        let state_default = evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Direct,
            actor.clone(),
            true,
            None,
            None,
            false,
            "typing",
            1_000,
            None,
        )
        .unwrap();
        assert_eq!(state_default.expires_at_ms, 1_000 + DEFAULT_TYPING_TTL_MS);

        // Values clamped to MAX_TYPING_TTL_MS (10_000)
        let state_clamped = evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Direct,
            actor.clone(),
            true,
            None,
            None,
            false,
            "typing",
            1_000,
            Some(20_000),
        )
        .unwrap();
        assert_eq!(state_clamped.expires_at_ms, 1_000 + MAX_TYPING_TTL_MS);

        // In-range TTL accepted
        let state_custom = evaluate_typing_publication(
            conv,
            ConversationKind::Direct,
            actor,
            true,
            None,
            None,
            false,
            "typing",
            1_000,
            Some(3_500),
        )
        .unwrap();
        assert_eq!(state_custom.expires_at_ms, 1_000 + 3_500);
    }

    #[test]
    fn test_direct_and_saved_messages_participant_boundaries() {
        let conv = ConversationId::new("conv-1");
        let actor = ActorId::new("actor-1");

        // Direct: not participant
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Direct,
                actor.clone(),
                false,
                None,
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::NotParticipant)
        );

        // Direct: participant
        assert!(evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Direct,
            actor.clone(),
            true,
            None,
            None,
            false,
            "typing",
            1_000,
            None,
        )
        .is_ok());

        // SavedMessages: not participant
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::SavedMessages,
                actor.clone(),
                false,
                None,
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::NotParticipant)
        );

        // SavedMessages: participant
        assert!(evaluate_typing_publication(
            conv,
            ConversationKind::SavedMessages,
            actor,
            true,
            None,
            None,
            false,
            "typing",
            1_000,
            None,
        )
        .is_ok());
    }

    #[test]
    fn test_group_member_boundaries() {
        let conv = ConversationId::new("group-1");
        let actor = ActorId::new("actor-1");

        // No member
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Group,
                actor.clone(),
                true,
                None,
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::NotParticipant)
        );

        // Left member
        let left_member = make_member(MemberStatus::Left);
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Group,
                actor.clone(),
                true,
                Some(&left_member),
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::NotParticipant)
        );

        // Banned member
        let banned_member = make_member(MemberStatus::Banned);
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Group,
                actor.clone(),
                true,
                Some(&banned_member),
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::NotParticipant)
        );

        // Normal active member
        let regular_member = make_member(MemberStatus::Member);
        assert!(evaluate_typing_publication(
            conv,
            ConversationKind::Group,
            actor,
            true,
            Some(&regular_member),
            None,
            false,
            "typing",
            1_000,
            None,
        )
        .is_ok());
    }

    #[test]
    fn test_channel_typing_permissions() {
        let conv = ConversationId::new("chan-1");
        let actor = ActorId::new("actor-1");

        // Regular member cannot type in channel
        let regular_member = make_member(MemberStatus::Member);
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Channel,
                actor.clone(),
                true,
                Some(&regular_member),
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::ChannelTypingNotAllowed)
        );

        // Admin without post_messages cannot type in channel
        let mut admin_no_post = make_member(MemberStatus::Administrator);
        admin_no_post.admin_rights.post_messages = false;
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Channel,
                actor.clone(),
                true,
                Some(&admin_no_post),
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::ChannelTypingNotAllowed)
        );

        // Admin with post_messages can type
        let mut admin_can_post = make_member(MemberStatus::Administrator);
        admin_can_post.admin_rights.post_messages = true;
        assert!(evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Channel,
            actor.clone(),
            true,
            Some(&admin_can_post),
            None,
            false,
            "typing",
            1_000,
            None,
        )
        .is_ok());

        // Owner can always type in channel
        let owner = make_member(MemberStatus::Owner);
        assert!(evaluate_typing_publication(
            conv,
            ConversationKind::Channel,
            actor,
            true,
            Some(&owner),
            None,
            false,
            "typing",
            1_000,
            None,
        )
        .is_ok());
    }

    #[test]
    fn test_restricted_member_send_messages_and_expiry() {
        let conv = ConversationId::new("group-1");
        let actor = ActorId::new("actor-1");

        // Restricted indefinitely
        let mut restricted_perm = make_member(MemberStatus::Restricted);
        restricted_perm.restrictions.send_messages = true;
        restricted_perm.restrictions.until_ms = None;
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Group,
                actor.clone(),
                true,
                Some(&restricted_perm),
                None,
                false,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::SendMessagesRestricted)
        );

        // Restricted temporarily, not yet expired
        let mut restricted_temp = make_member(MemberStatus::Restricted);
        restricted_temp.restrictions.send_messages = true;
        restricted_temp.restrictions.until_ms = Some(5_000);
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Group,
                actor.clone(),
                true,
                Some(&restricted_temp),
                None,
                false,
                "typing",
                4_999,
                None,
            ),
            Err(TypingPolicyDenial::SendMessagesRestricted)
        );

        // Restricted temporarily, now expired -> allowed!
        assert!(evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Group,
            actor.clone(),
            true,
            Some(&restricted_temp),
            None,
            false,
            "typing",
            5_000,
            None,
        )
        .is_ok());

        // Restricted for media only, send_messages not restricted -> allowed!
        let mut restricted_media_only = make_member(MemberStatus::Restricted);
        restricted_media_only.restrictions.send_messages = false;
        restricted_media_only.restrictions.send_media = true;
        restricted_media_only.restrictions.until_ms = Some(10_000);
        assert!(evaluate_typing_publication(
            conv,
            ConversationKind::Group,
            actor,
            true,
            Some(&restricted_media_only),
            None,
            false,
            "typing",
            1_000,
            None,
        )
        .is_ok());
    }

    #[test]
    fn test_topic_closed_policy() {
        let conv = ConversationId::new("group-1");
        let actor = ActorId::new("actor-1");

        // Topic open -> regular member can type
        let regular_member = make_member(MemberStatus::Member);
        assert!(evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Group,
            actor.clone(),
            true,
            Some(&regular_member),
            Some("topic-1".into()),
            false,
            "typing",
            1_000,
            None,
        )
        .is_ok());

        // Topic closed -> regular member blocked
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Group,
                actor.clone(),
                true,
                Some(&regular_member),
                Some("topic-1".into()),
                true,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::TopicClosed)
        );

        // Topic closed -> admin without manage_topics blocked
        let mut admin_no_topics = make_member(MemberStatus::Administrator);
        admin_no_topics.admin_rights.manage_topics = false;
        assert_eq!(
            evaluate_typing_publication(
                conv.clone(),
                ConversationKind::Group,
                actor.clone(),
                true,
                Some(&admin_no_topics),
                Some("topic-1".into()),
                true,
                "typing",
                1_000,
                None,
            ),
            Err(TypingPolicyDenial::TopicClosed)
        );

        // Topic closed -> admin with manage_topics allowed
        let mut admin_topics = make_member(MemberStatus::Administrator);
        admin_topics.admin_rights.manage_topics = true;
        let res_admin = evaluate_typing_publication(
            conv.clone(),
            ConversationKind::Group,
            actor.clone(),
            true,
            Some(&admin_topics),
            Some("topic-1".into()),
            true,
            "typing",
            1_000,
            None,
        );
        assert!(res_admin.is_ok());
        assert_eq!(res_admin.unwrap().topic_id.as_deref(), Some("topic-1"));

        // Topic closed -> owner allowed
        let owner = make_member(MemberStatus::Owner);
        let res_owner = evaluate_typing_publication(
            conv,
            ConversationKind::Group,
            actor,
            true,
            Some(&owner),
            Some("topic-1".into()),
            true,
            "typing",
            1_000,
            None,
        );
        assert!(res_owner.is_ok());
        assert_eq!(res_owner.unwrap().topic_id.as_deref(), Some("topic-1"));
    }

    #[test]
    fn test_typing_state_serialization_with_and_without_topic_id() {
        let state_without = TypingState::new(
            ConversationId::new("conv-1"),
            ActorId::new("actor-1"),
            "typing",
            1_500,
        );
        let json_without = serde_json::to_string(&state_without).unwrap();
        assert!(!json_without.contains("topicId"));

        let deserialized_without: TypingState = serde_json::from_str(&json_without).unwrap();
        assert_eq!(deserialized_without.topic_id, None);

        let state_with = state_without.with_topic("topic-99");
        let json_with = serde_json::to_string(&state_with).unwrap();
        assert!(json_with.contains("\"topicId\":\"topic-99\""));

        let deserialized_with: TypingState = serde_json::from_str(&json_with).unwrap();
        assert_eq!(deserialized_with.topic_id.as_deref(), Some("topic-99"));
    }

    #[test]
    fn test_crate_root_exports_realtime_pure_policy_types_and_functions() {
        use crate::{
            evaluate_typing_publication, TypingPolicyDenial, DEFAULT_TYPING_TTL_MS,
            MAX_TYPING_TTL_MS,
        };
        assert_eq!(DEFAULT_TYPING_TTL_MS, 6_000);
        assert_eq!(MAX_TYPING_TTL_MS, 10_000);
        assert_eq!(
            TypingPolicyDenial::NotParticipant,
            TypingPolicyDenial::NotParticipant
        );
        let _fn_ptr = evaluate_typing_publication;
    }
}

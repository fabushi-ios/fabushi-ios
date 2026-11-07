use crate::actor::ActorId;
use crate::conversation::{ConversationId, ConversationKind};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemberStatus {
    Owner,
    Administrator,
    Member,
    Restricted,
    Left,
    Banned,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminRights {
    pub change_info: bool,
    pub post_messages: bool,
    pub edit_messages: bool,
    pub delete_messages: bool,
    pub ban_members: bool,
    pub invite_members: bool,
    pub pin_messages: bool,
    pub manage_topics: bool,
    pub manage_calls: bool,
    pub add_admins: bool,
    pub remain_anonymous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberRestrictions {
    pub send_messages: bool,
    pub send_media: bool,
    pub send_polls: bool,
    #[serde(default)]
    pub send_other: bool,
    #[serde(default)]
    pub send_inline: bool,
    #[serde(default)]
    pub send_games: bool,
    pub embed_links: bool,
    pub add_members: bool,
    pub pin_messages: bool,
    pub change_info: bool,
    pub until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityMember {
    pub actor_id: ActorId,
    pub status: MemberStatus,
    pub admin_title: Option<String>,
    pub admin_rights: AdminRights,
    pub restrictions: MemberRestrictions,
    pub joined_at_ms: i64,
    pub invited_by: Option<ActorId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteLink {
    pub id: String,
    pub conversation_id: ConversationId,
    pub creator_id: ActorId,
    pub token: String,
    pub name: Option<String>,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub member_limit: Option<u32>,
    pub join_request: bool,
    pub revoked: bool,
    pub joined_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequest {
    pub conversation_id: ConversationId,
    pub actor_id: ActorId,
    pub invite_link_id: Option<String>,
    pub bio: Option<String>,
    pub requested_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForumTopicState {
    pub id: String,
    pub conversation_id: ConversationId,
    pub title: String,
    pub icon: Option<String>,
    pub creator_id: ActorId,
    pub created_at_ms: i64,
    pub pinned: bool,
    pub closed: bool,
    pub hidden: bool,
    pub unread_count: u32,
    pub last_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSubscription {
    pub actor_id: ActorId,
    pub subscribed_at_ms: i64,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommunityAuditAction {
    CommunityUpdated,
    SubscriptionAdded,
    SubscriptionRemoved,
    MemberChanged,
    InviteCreated,
    InviteRevoked,
    JoinApproved,
    JoinRejected,
    TopicUpserted,
    TopicDeleted,
    SlowModeChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityAuditEntry {
    pub id: String,
    pub actor_id: ActorId,
    pub action: CommunityAuditAction,
    pub target_actor_id: Option<ActorId>,
    pub target_id: Option<String>,
    pub reason: Option<String>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityState {
    pub conversation_id: ConversationId,
    pub public_username: Option<String>,
    pub linked_discussion_id: Option<ConversationId>,
    pub signatures_enabled: bool,
    pub join_to_send: bool,
    pub join_request_required: bool,
    pub slow_mode_seconds: Option<u32>,
    pub members: BTreeMap<ActorId, CommunityMember>,
    pub invite_links: BTreeMap<String, InviteLink>,
    pub pending_join_requests: BTreeMap<ActorId, JoinRequest>,
    pub topics: BTreeMap<String, ForumTopicState>,
    pub banned_words: BTreeSet<String>,
    #[serde(default)]
    pub subscribers: BTreeMap<ActorId, ChannelSubscription>,
    #[serde(default)]
    pub admin_log: Vec<CommunityAuditEntry>,
}

impl CommunityState {
    pub fn new(conversation_id: ConversationId) -> Self {
        Self {
            conversation_id,
            public_username: None,
            linked_discussion_id: None,
            signatures_enabled: false,
            join_to_send: false,
            join_request_required: false,
            slow_mode_seconds: None,
            members: BTreeMap::new(),
            invite_links: BTreeMap::new(),
            pending_join_requests: BTreeMap::new(),
            topics: BTreeMap::new(),
            banned_words: BTreeSet::new(),
            subscribers: BTreeMap::new(),
            admin_log: Vec::new(),
        }
    }

    pub fn is_subscriber(&self, actor_id: &ActorId) -> bool {
        self.subscribers.contains_key(actor_id)
    }

    pub fn subscribe_channel(
        &mut self,
        actor_id: ActorId,
        subscribed_at_ms: i64,
    ) -> Result<(), CommunityError> {
        if self.members.get(&actor_id).is_some_and(|member| {
            matches!(member.status, MemberStatus::Banned | MemberStatus::Left)
        }) {
            return Err(CommunityError::PermissionDenied);
        }
        self.subscribers
            .entry(actor_id.clone())
            .or_insert(ChannelSubscription {
                actor_id,
                subscribed_at_ms,
                muted: false,
            });
        Ok(())
    }

    pub fn unsubscribe_channel(&mut self, actor_id: &ActorId) -> Result<(), CommunityError> {
        if is_owner(self, actor_id) {
            return Err(CommunityError::OwnerCannotUnsubscribe);
        }
        self.subscribers.remove(actor_id);
        Ok(())
    }

    pub fn append_audit(&mut self, entry: CommunityAuditEntry) {
        self.admin_log.push(entry);
        const MAX_AUDIT_ENTRIES: usize = 1_000;
        if self.admin_log.len() > MAX_AUDIT_ENTRIES {
            let excess = self.admin_log.len() - MAX_AUDIT_ENTRIES;
            self.admin_log.drain(0..excess);
        }
    }

    pub fn member_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> (Vec<CommunityMember>, Option<String>) {
        let mut members = self.members.values().cloned().collect::<Vec<_>>();
        for subscription in self.subscribers.values() {
            if !self.members.contains_key(&subscription.actor_id) {
                members.push(CommunityMember {
                    actor_id: subscription.actor_id.clone(),
                    status: MemberStatus::Member,
                    admin_title: None,
                    admin_rights: AdminRights::default(),
                    restrictions: MemberRestrictions::default(),
                    joined_at_ms: subscription.subscribed_at_ms,
                    invited_by: None,
                });
            }
        }
        members.sort_by(|left, right| left.actor_id.cmp(&right.actor_id));
        page_by_actor_id(members, cursor, limit)
    }

    pub fn audit_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> (Vec<CommunityAuditEntry>, Option<String>) {
        let mut entries = self.admin_log.clone();
        entries.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        let start = cursor
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0)
            .min(entries.len());
        let page_limit = limit.clamp(1, 100);
        let end = start.saturating_add(page_limit).min(entries.len());
        let next = (end < entries.len()).then(|| end.to_string());
        (
            entries.into_iter().skip(start).take(end - start).collect(),
            next,
        )
    }

    pub fn can_moderate(&self, actor_id: &ActorId) -> bool {
        self.members.get(actor_id).is_some_and(|member| {
            matches!(member.status, MemberStatus::Owner)
                || (matches!(member.status, MemberStatus::Administrator)
                    && member.admin_rights.delete_messages)
        })
    }

    pub fn can_manage_invites(&self, actor_id: &ActorId) -> bool {
        self.members.get(actor_id).is_some_and(|member| {
            matches!(member.status, MemberStatus::Owner)
                || (matches!(member.status, MemberStatus::Administrator)
                    && member.admin_rights.invite_members)
        })
    }

    pub fn upsert_member(&mut self, member: CommunityMember) {
        self.pending_join_requests.remove(&member.actor_id);
        self.members.insert(member.actor_id.clone(), member);
    }

    pub fn request_join(&mut self, request: JoinRequest) {
        self.pending_join_requests
            .insert(request.actor_id.clone(), request);
    }

    pub fn approve_join(
        &mut self,
        actor_id: &ActorId,
        approved_by: &ActorId,
        now_ms: i64,
    ) -> Result<(), CommunityError> {
        if !self.can_manage_invites(approved_by) {
            return Err(CommunityError::PermissionDenied);
        }
        let request = self
            .pending_join_requests
            .remove(actor_id)
            .ok_or_else(|| CommunityError::JoinRequestNotFound(actor_id.clone()))?;
        self.upsert_member(CommunityMember {
            actor_id: request.actor_id,
            status: MemberStatus::Member,
            admin_title: None,
            admin_rights: AdminRights::default(),
            restrictions: MemberRestrictions::default(),
            joined_at_ms: now_ms,
            invited_by: Some(approved_by.clone()),
        });
        Ok(())
    }

    pub fn revoke_invite(&mut self, invite_id: &str) -> Result<(), CommunityError> {
        let invite = self
            .invite_links
            .get_mut(invite_id)
            .ok_or_else(|| CommunityError::InviteNotFound(invite_id.to_string()))?;
        invite.revoked = true;
        Ok(())
    }
}

fn is_owner(community: &CommunityState, actor_id: &ActorId) -> bool {
    community
        .members
        .get(actor_id)
        .is_some_and(|member| matches!(member.status, MemberStatus::Owner))
}

fn page_by_actor_id(
    members: Vec<CommunityMember>,
    cursor: Option<&str>,
    limit: usize,
) -> (Vec<CommunityMember>, Option<String>) {
    let start = cursor
        .and_then(|value| {
            members
                .iter()
                .position(|member| member.actor_id.0.as_str() > value)
        })
        .unwrap_or_else(|| if cursor.is_some() { members.len() } else { 0 });
    let page_limit = limit.clamp(1, 100);
    let end = start.saturating_add(page_limit).min(members.len());
    let next = (end < members.len()).then(|| members[end - 1].actor_id.0.clone());
    (
        members.into_iter().skip(start).take(end - start).collect(),
        next,
    )
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CommunityError {
    #[error("community permission denied")]
    PermissionDenied,
    #[error("join request for actor {0:?} was not found")]
    JoinRequestNotFound(ActorId),
    #[error("invite link {0} was not found")]
    InviteNotFound(String),
    #[error("community owner cannot unsubscribe from their own channel")]
    OwnerCannotUnsubscribe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommunitySendDenial {
    NotMember,
    MemberLeft,
    MemberBanned,
    ChannelPostNotAllowed,
    SendMessagesRestricted { until_ms: Option<i64> },
    SendMediaRestricted { until_ms: Option<i64> },
    SendPollsRestricted { until_ms: Option<i64> },
    SendOtherRestricted { until_ms: Option<i64> },
    SendInlineRestricted { until_ms: Option<i64> },
    SendGamesRestricted { until_ms: Option<i64> },
    TopicNotFound,
    TopicClosed,
    SlowModeActive { retry_after_ms: u64 },
}

impl std::fmt::Display for CommunitySendDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMember => write!(f, "sender is not a member of the community"),
            Self::MemberLeft => write!(f, "sender has left the community"),
            Self::MemberBanned => write!(f, "sender is banned from the community"),
            Self::ChannelPostNotAllowed => {
                write!(f, "sender is not authorized to post in this channel")
            }
            Self::SendMessagesRestricted { until_ms } => {
                write!(f, "sending messages is restricted (until: {until_ms:?})")
            }
            Self::SendMediaRestricted { until_ms } => {
                write!(f, "sending media is restricted (until: {until_ms:?})")
            }
            Self::SendPollsRestricted { until_ms } => {
                write!(f, "sending polls is restricted (until: {until_ms:?})")
            }
            Self::SendOtherRestricted { until_ms } => {
                write!(f, "sending other content is restricted (until: {until_ms:?})")
            }
            Self::SendInlineRestricted { until_ms } => {
                write!(f, "sending inline content is restricted (until: {until_ms:?})")
            }
            Self::SendGamesRestricted { until_ms } => {
                write!(f, "sending games is restricted (until: {until_ms:?})")
            }
            Self::TopicNotFound => write!(f, "target topic was not found"),
            Self::TopicClosed => write!(f, "target topic is closed"),
            Self::SlowModeActive { retry_after_ms } => {
                write!(f, "slow mode active, retry after {retry_after_ms}ms")
            }
        }
    }
}

impl std::error::Error for CommunitySendDenial {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunitySendContentFlags {
    pub has_media: bool,
    pub has_poll: bool,
    pub has_other: bool,
    pub has_inline: bool,
    pub has_game: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunitySendPolicyInput<'a> {
    pub conversation_kind: ConversationKind,
    pub member: Option<&'a CommunityMember>,
    pub slow_mode_seconds: Option<u32>,
    pub last_sender_message_at_ms: Option<i64>,
    pub now_ms: i64,
    pub target_topic: Option<&'a ForumTopicState>,
    pub topic_requested: bool,
    pub content: CommunitySendContentFlags,
}

pub fn evaluate_community_send_policy(
    input: &CommunitySendPolicyInput<'_>,
) -> Result<(), CommunitySendDenial> {
    let member = match input.member {
        None => return Err(CommunitySendDenial::NotMember),
        Some(m) => m,
    };

    match member.status {
        MemberStatus::Left => return Err(CommunitySendDenial::MemberLeft),
        MemberStatus::Banned => return Err(CommunitySendDenial::MemberBanned),
        _ => {}
    }

    if input.conversation_kind == ConversationKind::Channel {
        let is_channel_poster = member.status == MemberStatus::Owner
            || (member.status == MemberStatus::Administrator && member.admin_rights.post_messages);
        if !is_channel_poster {
            return Err(CommunitySendDenial::ChannelPostNotAllowed);
        }
    }

    if member.status == MemberStatus::Restricted {
        let restriction_active = match member.restrictions.until_ms {
            None => true,
            Some(until_ms) => input.now_ms < until_ms,
        };

        if restriction_active {
            if member.restrictions.send_messages {
                return Err(CommunitySendDenial::SendMessagesRestricted {
                    until_ms: member.restrictions.until_ms,
                });
            }
            if input.content.has_media && member.restrictions.send_media {
                return Err(CommunitySendDenial::SendMediaRestricted {
                    until_ms: member.restrictions.until_ms,
                });
            }
            if input.content.has_poll && member.restrictions.send_polls {
                return Err(CommunitySendDenial::SendPollsRestricted {
                    until_ms: member.restrictions.until_ms,
                });
            }
            if input.content.has_other && member.restrictions.send_other {
                return Err(CommunitySendDenial::SendOtherRestricted {
                    until_ms: member.restrictions.until_ms,
                });
            }
            if input.content.has_inline && member.restrictions.send_inline {
                return Err(CommunitySendDenial::SendInlineRestricted {
                    until_ms: member.restrictions.until_ms,
                });
            }
            if input.content.has_game && member.restrictions.send_games {
                return Err(CommunitySendDenial::SendGamesRestricted {
                    until_ms: member.restrictions.until_ms,
                });
            }
        }
    }

    if input.topic_requested && input.target_topic.is_none() {
        return Err(CommunitySendDenial::TopicNotFound);
    }

    if let Some(topic) = input.target_topic {
        if topic.closed {
            let can_post_in_closed_topic = member.status == MemberStatus::Owner
                || (member.status == MemberStatus::Administrator
                    && member.admin_rights.manage_topics);
            if !can_post_in_closed_topic {
                return Err(CommunitySendDenial::TopicClosed);
            }
        }
    }

    let is_slow_mode_exempt = member.status == MemberStatus::Owner
        || member.status == MemberStatus::Administrator;
    if !is_slow_mode_exempt {
        if let Some(sec) = input.slow_mode_seconds {
            if sec > 0 {
                if let Some(last_ms) = input.last_sender_message_at_ms {
                    let cooldown_ms = (sec as i64).saturating_mul(1000);
                    let next_allowed_ms = last_ms.saturating_add(cooldown_ms);
                    if input.now_ms < next_allowed_ms {
                        let retry_after_ms = (next_allowed_ms - input.now_ms) as u64;
                        return Err(CommunitySendDenial::SlowModeActive { retry_after_ms });
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_member(status: MemberStatus) -> CommunityMember {
        CommunityMember {
            actor_id: ActorId("actor-1".to_string()),
            status,
            admin_title: None,
            admin_rights: AdminRights::default(),
            restrictions: MemberRestrictions::default(),
            joined_at_ms: 1_000,
            invited_by: None,
        }
    }

    fn sample_topic(closed: bool) -> ForumTopicState {
        ForumTopicState {
            id: "topic-100".to_string(),
            conversation_id: ConversationId("conv-1".to_string()),
            title: "General".to_string(),
            icon: None,
            creator_id: ActorId("actor-1".to_string()),
            created_at_ms: 1_000,
            pinned: false,
            closed,
            hidden: false,
            unread_count: 0,
            last_message_id: None,
        }
    }

    #[test]
    fn evaluate_send_policy_rejects_non_member_and_left_or_banned() {
        let input_no_member = CommunitySendPolicyInput {
            conversation_kind: ConversationKind::Group,
            member: None,
            slow_mode_seconds: None,
            last_sender_message_at_ms: None,
            now_ms: 5_000,
            target_topic: None,
            topic_requested: false,
            content: CommunitySendContentFlags::default(),
        };
        assert_eq!(
            evaluate_community_send_policy(&input_no_member),
            Err(CommunitySendDenial::NotMember)
        );

        let left_member = sample_member(MemberStatus::Left);
        let input_left = CommunitySendPolicyInput {
            member: Some(&left_member),
            ..input_no_member.clone()
        };
        assert_eq!(
            evaluate_community_send_policy(&input_left),
            Err(CommunitySendDenial::MemberLeft)
        );

        let banned_member = sample_member(MemberStatus::Banned);
        let input_banned = CommunitySendPolicyInput {
            member: Some(&banned_member),
            ..input_no_member
        };
        assert_eq!(
            evaluate_community_send_policy(&input_banned),
            Err(CommunitySendDenial::MemberBanned)
        );
    }

    #[test]
    fn evaluate_send_policy_enforces_channel_posting_boundaries() {
        let member = sample_member(MemberStatus::Member);
        let input = CommunitySendPolicyInput {
            conversation_kind: ConversationKind::Channel,
            member: Some(&member),
            slow_mode_seconds: None,
            last_sender_message_at_ms: None,
            now_ms: 5_000,
            target_topic: None,
            topic_requested: false,
            content: CommunitySendContentFlags::default(),
        };
        assert_eq!(
            evaluate_community_send_policy(&input),
            Err(CommunitySendDenial::ChannelPostNotAllowed)
        );

        let mut admin = sample_member(MemberStatus::Administrator);
        admin.admin_rights.post_messages = false;
        let input_admin_no_post = CommunitySendPolicyInput {
            member: Some(&admin),
            ..input.clone()
        };
        assert_eq!(
            evaluate_community_send_policy(&input_admin_no_post),
            Err(CommunitySendDenial::ChannelPostNotAllowed)
        );

        admin.admin_rights.post_messages = true;
        let input_admin_post = CommunitySendPolicyInput {
            member: Some(&admin),
            ..input.clone()
        };
        assert_eq!(evaluate_community_send_policy(&input_admin_post), Ok(()));

        let owner = sample_member(MemberStatus::Owner);
        let input_owner = CommunitySendPolicyInput {
            member: Some(&owner),
            ..input
        };
        assert_eq!(evaluate_community_send_policy(&input_owner), Ok(()));
    }

    #[test]
    fn evaluate_send_policy_enforces_restricted_sub_permissions_and_expiry() {
        let mut restricted_temp = sample_member(MemberStatus::Restricted);
        restricted_temp.restrictions.send_messages = true;
        restricted_temp.restrictions.until_ms = Some(10_000);

        let active_input = CommunitySendPolicyInput {
            conversation_kind: ConversationKind::Group,
            member: Some(&restricted_temp),
            slow_mode_seconds: None,
            last_sender_message_at_ms: None,
            now_ms: 9_999,
            target_topic: None,
            topic_requested: false,
            content: CommunitySendContentFlags::default(),
        };
        assert_eq!(
            evaluate_community_send_policy(&active_input),
            Err(CommunitySendDenial::SendMessagesRestricted {
                until_ms: Some(10_000)
            })
        );

        let expired_at_input = CommunitySendPolicyInput {
            now_ms: 10_000,
            ..active_input
        };
        assert_eq!(evaluate_community_send_policy(&expired_at_input), Ok(()));

        let expired_after_input = CommunitySendPolicyInput {
            now_ms: 10_001,
            ..expired_at_input
        };
        assert_eq!(evaluate_community_send_policy(&expired_after_input), Ok(()));

        let mut restricted_perm = sample_member(MemberStatus::Restricted);
        restricted_perm.restrictions.send_messages = true;
        restricted_perm.restrictions.until_ms = None;
        let permanent_input = CommunitySendPolicyInput {
            conversation_kind: ConversationKind::Group,
            member: Some(&restricted_perm),
            slow_mode_seconds: None,
            last_sender_message_at_ms: None,
            now_ms: 999_999,
            target_topic: None,
            topic_requested: false,
            content: CommunitySendContentFlags::default(),
        };
        assert_eq!(
            evaluate_community_send_policy(&permanent_input),
            Err(CommunitySendDenial::SendMessagesRestricted { until_ms: None })
        );

        let mut restricted_sub = sample_member(MemberStatus::Restricted);
        restricted_sub.restrictions.send_messages = false;
        restricted_sub.restrictions.send_media = true;
        restricted_sub.restrictions.send_polls = true;
        restricted_sub.restrictions.send_other = true;
        restricted_sub.restrictions.send_inline = true;
        restricted_sub.restrictions.send_games = true;

        let plain_input = CommunitySendPolicyInput {
            conversation_kind: ConversationKind::Group,
            member: Some(&restricted_sub),
            slow_mode_seconds: None,
            last_sender_message_at_ms: None,
            now_ms: 1_000,
            target_topic: None,
            topic_requested: false,
            content: CommunitySendContentFlags::default(),
        };
        assert_eq!(evaluate_community_send_policy(&plain_input), Ok(()));

        let media_input = CommunitySendPolicyInput {
            content: CommunitySendContentFlags {
                has_media: true,
                ..Default::default()
            },
            ..plain_input
        };
        assert_eq!(
            evaluate_community_send_policy(&media_input),
            Err(CommunitySendDenial::SendMediaRestricted { until_ms: None })
        );

        let poll_input = CommunitySendPolicyInput {
            content: CommunitySendContentFlags {
                has_poll: true,
                ..Default::default()
            },
            ..plain_input
        };
        assert_eq!(
            evaluate_community_send_policy(&poll_input),
            Err(CommunitySendDenial::SendPollsRestricted { until_ms: None })
        );

        let other_input = CommunitySendPolicyInput {
            content: CommunitySendContentFlags {
                has_other: true,
                ..Default::default()
            },
            ..plain_input
        };
        assert_eq!(
            evaluate_community_send_policy(&other_input),
            Err(CommunitySendDenial::SendOtherRestricted { until_ms: None })
        );

        let inline_input = CommunitySendPolicyInput {
            content: CommunitySendContentFlags {
                has_inline: true,
                ..Default::default()
            },
            ..plain_input
        };
        assert_eq!(
            evaluate_community_send_policy(&inline_input),
            Err(CommunitySendDenial::SendInlineRestricted { until_ms: None })
        );

        let game_input = CommunitySendPolicyInput {
            content: CommunitySendContentFlags {
                has_game: true,
                ..Default::default()
            },
            ..plain_input
        };
        assert_eq!(
            evaluate_community_send_policy(&game_input),
            Err(CommunitySendDenial::SendGamesRestricted { until_ms: None })
        );
    }

    #[test]
    fn evaluate_send_policy_validates_topic_not_found_and_topic_closed() {
        let member = sample_member(MemberStatus::Member);

        let missing_topic_input = CommunitySendPolicyInput {
            conversation_kind: ConversationKind::Group,
            member: Some(&member),
            slow_mode_seconds: None,
            last_sender_message_at_ms: None,
            now_ms: 5_000,
            target_topic: None,
            topic_requested: true,
            content: CommunitySendContentFlags::default(),
        };
        assert_eq!(
            evaluate_community_send_policy(&missing_topic_input),
            Err(CommunitySendDenial::TopicNotFound)
        );

        let open_topic = sample_topic(false);
        let open_topic_input = CommunitySendPolicyInput {
            target_topic: Some(&open_topic),
            topic_requested: true,
            ..missing_topic_input.clone()
        };
        assert_eq!(evaluate_community_send_policy(&open_topic_input), Ok(()));

        let closed_topic = sample_topic(true);
        let closed_topic_input = CommunitySendPolicyInput {
            target_topic: Some(&closed_topic),
            topic_requested: true,
            ..missing_topic_input.clone()
        };
        assert_eq!(
            evaluate_community_send_policy(&closed_topic_input),
            Err(CommunitySendDenial::TopicClosed)
        );

        let mut admin = sample_member(MemberStatus::Administrator);
        admin.admin_rights.manage_topics = false;
        let admin_no_manage_input = CommunitySendPolicyInput {
            member: Some(&admin),
            target_topic: Some(&closed_topic),
            topic_requested: true,
            ..missing_topic_input.clone()
        };
        assert_eq!(
            evaluate_community_send_policy(&admin_no_manage_input),
            Err(CommunitySendDenial::TopicClosed)
        );

        admin.admin_rights.manage_topics = true;
        let admin_manage_input = CommunitySendPolicyInput {
            member: Some(&admin),
            target_topic: Some(&closed_topic),
            topic_requested: true,
            ..missing_topic_input.clone()
        };
        assert_eq!(evaluate_community_send_policy(&admin_manage_input), Ok(()));

        let owner = sample_member(MemberStatus::Owner);
        let owner_input = CommunitySendPolicyInput {
            member: Some(&owner),
            target_topic: Some(&closed_topic),
            topic_requested: true,
            ..missing_topic_input
        };
        assert_eq!(evaluate_community_send_policy(&owner_input), Ok(()));
    }

    #[test]
    fn evaluate_send_policy_enforces_slow_mode_and_exemptions() {
        let member = sample_member(MemberStatus::Member);

        let cooling_input = CommunitySendPolicyInput {
            conversation_kind: ConversationKind::Group,
            member: Some(&member),
            slow_mode_seconds: Some(30),
            last_sender_message_at_ms: Some(10_000),
            now_ms: 25_000,
            target_topic: None,
            topic_requested: false,
            content: CommunitySendContentFlags::default(),
        };
        assert_eq!(
            evaluate_community_send_policy(&cooling_input),
            Err(CommunitySendDenial::SlowModeActive {
                retry_after_ms: 15_000
            })
        );

        let exact_allowed_input = CommunitySendPolicyInput {
            now_ms: 40_000,
            ..cooling_input.clone()
        };
        assert_eq!(
            evaluate_community_send_policy(&exact_allowed_input),
            Ok(())
        );

        let well_after_input = CommunitySendPolicyInput {
            now_ms: 45_000,
            ..cooling_input.clone()
        };
        assert_eq!(evaluate_community_send_policy(&well_after_input), Ok(()));

        let first_message_input = CommunitySendPolicyInput {
            last_sender_message_at_ms: None,
            ..cooling_input.clone()
        };
        assert_eq!(
            evaluate_community_send_policy(&first_message_input),
            Ok(())
        );

        let zero_seconds_input = CommunitySendPolicyInput {
            slow_mode_seconds: Some(0),
            ..cooling_input.clone()
        };
        assert_eq!(
            evaluate_community_send_policy(&zero_seconds_input),
            Ok(())
        );

        let admin = sample_member(MemberStatus::Administrator);
        let admin_input = CommunitySendPolicyInput {
            member: Some(&admin),
            ..cooling_input.clone()
        };
        assert_eq!(evaluate_community_send_policy(&admin_input), Ok(()));

        let owner = sample_member(MemberStatus::Owner);
        let owner_input = CommunitySendPolicyInput {
            member: Some(&owner),
            ..cooling_input.clone()
        };
        assert_eq!(evaluate_community_send_policy(&owner_input), Ok(()));

        let mut restricted = sample_member(MemberStatus::Restricted);
        restricted.restrictions.until_ms = Some(15_000);
        let restricted_cooling = CommunitySendPolicyInput {
            member: Some(&restricted),
            now_ms: 25_000,
            ..cooling_input
        };
        assert_eq!(
            evaluate_community_send_policy(&restricted_cooling),
            Err(CommunitySendDenial::SlowModeActive {
                retry_after_ms: 15_000
            })
        );
    }

    #[test]
    fn community_send_denial_serde_roundtrip() {
        let cases = vec![
            CommunitySendDenial::NotMember,
            CommunitySendDenial::MemberLeft,
            CommunitySendDenial::MemberBanned,
            CommunitySendDenial::ChannelPostNotAllowed,
            CommunitySendDenial::SendMessagesRestricted {
                until_ms: Some(1_234),
            },
            CommunitySendDenial::SendMediaRestricted { until_ms: None },
            CommunitySendDenial::SendPollsRestricted {
                until_ms: Some(5_678),
            },
            CommunitySendDenial::SendOtherRestricted { until_ms: None },
            CommunitySendDenial::SendInlineRestricted { until_ms: None },
            CommunitySendDenial::SendGamesRestricted { until_ms: None },
            CommunitySendDenial::TopicNotFound,
            CommunitySendDenial::TopicClosed,
            CommunitySendDenial::SlowModeActive {
                retry_after_ms: 4_500,
            },
        ];

        for case in cases {
            let json = serde_json::to_string(&case).expect("serialize");
            let roundtrip: CommunitySendDenial =
                serde_json::from_str(&json).expect("deserialize");
            assert_eq!(case, roundtrip);
        }
    }

    #[test]
    fn crate_root_exports_pure_policy_types_and_functions() {
        use crate::{
            authorize_topic_destination, evaluate_community_send_policy, CommunitySendContentFlags,
            CommunitySendDenial, CommunitySendPolicyInput, TopicDestinationEligibilityInput,
        };
        let _ = evaluate_community_send_policy;
        let _ = authorize_topic_destination;
        let _ = CommunitySendDenial::NotMember;
        let _ = CommunitySendContentFlags::default();
        let _ = TopicDestinationEligibilityInput::default();
        let _ = std::mem::size_of::<CommunitySendPolicyInput<'_>>();
    }
}

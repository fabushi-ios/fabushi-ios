use crate::actor::{ActorId, Participant, ParticipantRole};
use crate::community::{
    AdminRights, CommunityAuditAction, CommunityAuditEntry, CommunityError, CommunityMember,
    CommunityState, InviteLink, JoinRequest, MemberStatus,
};
use crate::conversation::{ConversationId, ConversationKind};
use crate::engine::{EngineError, Event, MessagingEngine};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityAdminAction {
    ChangeInfo,
    InviteMembers,
    BanMembers,
    ManageTopics,
    AddAdmins,
}

pub fn is_community_owner(community: &CommunityState, actor_id: &ActorId) -> bool {
    community
        .members
        .get(actor_id)
        .is_some_and(|member| matches!(member.status, MemberStatus::Owner))
}

pub fn require_community_admin(
    community: &CommunityState,
    actor_id: &ActorId,
    action: CommunityAdminAction,
) -> Result<(), EngineError> {
    let allowed = community.members.get(actor_id).is_some_and(|member| {
        if matches!(member.status, MemberStatus::Owner) {
            return true;
        }
        if !matches!(member.status, MemberStatus::Administrator) {
            return false;
        }
        match action {
            CommunityAdminAction::ChangeInfo => member.admin_rights.change_info,
            CommunityAdminAction::InviteMembers => member.admin_rights.invite_members,
            CommunityAdminAction::BanMembers => member.admin_rights.ban_members,
            CommunityAdminAction::ManageTopics => member.admin_rights.manage_topics,
            CommunityAdminAction::AddAdmins => member.admin_rights.add_admins,
        }
    });
    if allowed {
        Ok(())
    } else {
        Err(EngineError::CommunityPermissionDenied)
    }
}

pub fn community_has_access(community: &CommunityState, actor_id: &ActorId) -> bool {
    community
        .members
        .get(actor_id)
        .is_some_and(|member| !matches!(member.status, MemberStatus::Left | MemberStatus::Banned))
        || community.is_subscriber(actor_id)
}

pub fn append_community_audit(
    community: &mut CommunityState,
    actor_id: &ActorId,
    action: CommunityAuditAction,
    target_actor_id: Option<ActorId>,
    target_id: Option<String>,
    reason: Option<String>,
    created_at_ms: i64,
) {
    let id = format!(
        "audit:{}:{}:{}:{}",
        actor_id.0,
        created_at_ms,
        community.admin_log.len(),
        target_id.as_deref().unwrap_or("community")
    );
    community.append_audit(CommunityAuditEntry {
        id,
        actor_id: actor_id.clone(),
        action,
        target_actor_id,
        target_id,
        reason,
        created_at_ms,
    });
}

pub fn participant_for_community_member(member: &CommunityMember) -> Option<Participant> {
    let role = match member.status {
        MemberStatus::Owner => ParticipantRole::Owner,
        MemberStatus::Administrator => ParticipantRole::Admin,
        MemberStatus::Member => ParticipantRole::Member,
        MemberStatus::Restricted => ParticipantRole::Restricted,
        MemberStatus::Left | MemberStatus::Banned => return None,
    };
    Some(Participant {
        actor_id: member.actor_id.clone(),
        role,
        joined_at_ms: member.joined_at_ms,
        muted_until_ms: member.restrictions.until_ms,
    })
}

impl MessagingEngine {
    pub(crate) fn decide_update_community(
        &self,
        actor_id: ActorId,
        mut community: CommunityState,
    ) -> Result<Vec<Event>, EngineError> {
        let conversation = self.require_conversation(&community.conversation_id)?;
        if !matches!(
            conversation.kind,
            ConversationKind::Group | ConversationKind::Channel
        ) {
            return Err(EngineError::CommunityPermissionDenied);
        }
        if let Some(existing) = self.state.communities.get(&community.conversation_id) {
            require_community_admin(existing, &actor_id, CommunityAdminAction::ChangeInfo)?;
        } else if conversation.owner_id.as_ref() != Some(&actor_id)
            && !conversation.participants.iter().any(|participant| {
                participant.actor_id == actor_id
                    && matches!(participant.role, ParticipantRole::Owner)
            })
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        if let Some(existing) = self.state.communities.get(&community.conversation_id) {
            // Membership, moderation, invites, topics and policy are server-owned.
            // Only the community metadata above is client-editable through this command.
            community.members = existing.members.clone();
            community.invite_links = existing.invite_links.clone();
            community.pending_join_requests = existing.pending_join_requests.clone();
            community.topics = existing.topics.clone();
            community.banned_words = existing.banned_words.clone();
            community.slow_mode_seconds = existing.slow_mode_seconds;
            community.subscribers = existing.subscribers.clone();
            community.admin_log = existing.admin_log.clone();
        } else {
            // Only the conversation owner may create the first community state.
            // Seed that owner as the sole owner so a non-owner cannot squat the state.
            community.subscribers.clear();
            community.admin_log.clear();
            let requested_members = std::mem::take(&mut community.members);
            community.invite_links.clear();
            community.pending_join_requests.clear();
            community.topics.clear();
            community.banned_words.clear();
            community.members = conversation
                .participants
                .iter()
                .map(|participant| {
                    let requested = requested_members.get(&participant.actor_id);
                    let (status, admin_rights) = match participant.role {
                        ParticipantRole::Owner => (
                            MemberStatus::Owner,
                            AdminRights {
                                change_info: true,
                                post_messages: true,
                                edit_messages: true,
                                delete_messages: true,
                                ban_members: true,
                                invite_members: true,
                                pin_messages: true,
                                manage_topics: true,
                                manage_calls: true,
                                add_admins: true,
                                remain_anonymous: true,
                            },
                        ),
                        ParticipantRole::Admin => (
                            MemberStatus::Administrator,
                            requested
                                .map(|member| member.admin_rights.clone())
                                .unwrap_or_default(),
                        ),
                        ParticipantRole::Restricted => (
                            MemberStatus::Restricted,
                            requested
                                .map(|member| member.admin_rights.clone())
                                .unwrap_or_default(),
                        ),
                        ParticipantRole::Member => (MemberStatus::Member, AdminRights::default()),
                    };
                    (
                        participant.actor_id.clone(),
                        CommunityMember {
                            actor_id: participant.actor_id.clone(),
                            status,
                            admin_title: requested
                                .and_then(|member| member.admin_title.clone()),
                            admin_rights,
                            restrictions: requested
                                .map(|member| member.restrictions.clone())
                                .unwrap_or_default(),
                            joined_at_ms: participant.joined_at_ms,
                            invited_by: requested
                                .and_then(|member| member.invited_by.clone()),
                        },
                    )
                })
                .collect();
        }
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_set_community_slow_mode(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        seconds: Option<u32>,
        changed_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        let conversation = self.require_conversation(&conversation_id)?;
        if !matches!(
            conversation.kind,
            ConversationKind::Group | ConversationKind::Channel
        ) {
            return Err(EngineError::CommunityPermissionDenied);
        }
        if seconds.is_some_and(|seconds| seconds > 86_400) {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        require_community_admin(&community, &actor_id, CommunityAdminAction::ChangeInfo)?;
        community.slow_mode_seconds = seconds;
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::SlowModeChanged,
            None,
            Some("slowMode".into()),
            None,
            changed_at_ms,
        );
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_moderate_community_member(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        member: CommunityMember,
        reason: Option<String>,
        decided_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        self.require_actor(&member.actor_id)?;
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        require_community_admin(&community, &actor_id, CommunityAdminAction::BanMembers)?;
        if member.actor_id == actor_id
            || community
                .members
                .get(&member.actor_id)
                .is_some_and(|existing| matches!(existing.status, MemberStatus::Owner))
            || matches!(member.status, MemberStatus::Owner)
            || matches!(member.status, MemberStatus::Administrator)
            || community
                .members
                .get(&member.actor_id)
                .is_some_and(|existing| {
                    matches!(existing.status, MemberStatus::Administrator)
                })
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let target_actor_id = member.actor_id.clone();
        let target_id = format!("member:{}", target_actor_id.0);
        community.upsert_member(member.clone());
        if matches!(member.status, MemberStatus::Left | MemberStatus::Banned) {
            community.subscribers.remove(&target_actor_id);
        }
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::MemberChanged,
            Some(target_actor_id),
            Some(target_id),
            reason,
            decided_at_ms,
        );
        let participant_event = self
            .state
            .conversations
            .get(&conversation_id)
            .is_some_and(|conversation| {
                matches!(
                    conversation.kind,
                    ConversationKind::Group | ConversationKind::Channel
                )
            })
            .then(|| {
                community
                    .members
                    .get(&member.actor_id)
                    .and_then(participant_for_community_member)
                    .map(|participant| Event::ConversationParticipantUpserted {
                        conversation_id: conversation_id.clone(),
                        participant,
                    })
                    .unwrap_or_else(|| Event::ConversationParticipantRemoved {
                        conversation_id: conversation_id.clone(),
                        actor_id: member.actor_id.clone(),
                    })
            });
        let mut events = vec![Event::CommunityChanged { community }];
        if let Some(event) = participant_event {
            events.push(event);
        }
        Ok(events)
    }

    pub(crate) fn decide_set_community_member(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        member: CommunityMember,
    ) -> Result<Vec<Event>, EngineError> {
        self.require_actor(&member.actor_id)?;
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        let caller_is_owner = is_community_owner(&community, &actor_id);
        let target_is_owner = community
            .members
            .get(&member.actor_id)
            .is_some_and(|existing| matches!(existing.status, MemberStatus::Owner));
        let target_is_administrator = community
            .members
            .get(&member.actor_id)
            .is_some_and(|existing| matches!(existing.status, MemberStatus::Administrator));
        if (target_is_owner || matches!(member.status, MemberStatus::Owner))
            && !caller_is_owner
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let action = if target_is_owner || target_is_administrator {
            CommunityAdminAction::AddAdmins
        } else {
            match member.status {
                MemberStatus::Owner | MemberStatus::Administrator => {
                    CommunityAdminAction::AddAdmins
                }
                MemberStatus::Restricted | MemberStatus::Left | MemberStatus::Banned => {
                    CommunityAdminAction::BanMembers
                }
                MemberStatus::Member => CommunityAdminAction::InviteMembers,
            }
        };
        require_community_admin(&community, &actor_id, action)?;
        let target_actor_id = member.actor_id.clone();
        let joined_at_ms = member.joined_at_ms;
        community.upsert_member(member);
        if matches!(
            community
                .members
                .get(&target_actor_id)
                .map(|member| member.status),
            Some(MemberStatus::Left | MemberStatus::Banned)
        ) {
            community.subscribers.remove(&target_actor_id);
        }
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::MemberChanged,
            Some(target_actor_id.clone()),
            Some(format!("member:{}", target_actor_id.0)),
            None,
            joined_at_ms,
        );
        let participant_event = self
            .state
            .conversations
            .get(&conversation_id)
            .is_some_and(|conversation| {
                matches!(
                    conversation.kind,
                    ConversationKind::Group | ConversationKind::Channel
                )
            })
            .then(|| {
                community
                    .members
                    .get(&target_actor_id)
                    .and_then(participant_for_community_member)
                    .map(|participant| Event::ConversationParticipantUpserted {
                        conversation_id: conversation_id.clone(),
                        participant,
                    })
                    .unwrap_or_else(|| Event::ConversationParticipantRemoved {
                        conversation_id: conversation_id.clone(),
                        actor_id: target_actor_id.clone(),
                    })
            });
        let mut events = vec![Event::CommunityChanged { community }];
        if let Some(event) = participant_event {
            events.push(event);
        }
        Ok(events)
    }

    pub(crate) fn decide_create_invite_link(
        &self,
        actor_id: ActorId,
        invite: InviteLink,
    ) -> Result<Vec<Event>, EngineError> {
        let mut community = self
            .state
            .communities
            .get(&invite.conversation_id)
            .cloned()
            .ok_or_else(|| {
                EngineError::CommunityNotFound(invite.conversation_id.clone())
            })?;
        require_community_admin(
            &community,
            &actor_id,
            CommunityAdminAction::InviteMembers,
        )?;
        if invite.creator_id != actor_id
            || invite.id.trim().is_empty()
            || invite.token.trim().is_empty()
            || invite.revoked
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let invite_id = invite.id.clone();
        let created_at_ms = invite.created_at_ms;
        community.invite_links.insert(invite_id.clone(), invite);
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::InviteCreated,
            None,
            Some(format!("invite:{}", invite_id)),
            None,
            created_at_ms,
        );
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_revoke_invite_link(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        invite_id: String,
        revoked_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        require_community_admin(
            &community,
            &actor_id,
            CommunityAdminAction::InviteMembers,
        )?;
        community.revoke_invite(&invite_id)?;
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::InviteRevoked,
            None,
            Some(format!("invite:{invite_id}")),
            None,
            revoked_at_ms,
        );
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_request_community_join(
        &self,
        actor_id: ActorId,
        request: JoinRequest,
    ) -> Result<Vec<Event>, EngineError> {
        if request.actor_id != actor_id {
            return Err(EngineError::CommunityPermissionDenied);
        }
        self.require_actor(&actor_id)?;
        let conversation = self.require_conversation(&request.conversation_id)?;
        if !matches!(
            conversation.kind,
            ConversationKind::Group | ConversationKind::Channel
        ) {
            return Err(EngineError::CommunityPermissionDenied);
        }
        let requested_actor_id = request.actor_id.clone();
        let requested_at_ms = request.requested_at_ms;
        let mut community = self
            .state
            .communities
            .get(&request.conversation_id)
            .cloned()
            .ok_or_else(|| {
                EngineError::CommunityNotFound(request.conversation_id.clone())
            })?;
        if community
            .members
            .get(&actor_id)
            .is_some_and(|member| matches!(member.status, MemberStatus::Banned))
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        if community
            .public_username
            .as_deref()
            .is_none_or(|username| username.trim().is_empty())
        {
            return Err(EngineError::CommunityPermissionDenied);
        }
        community.request_join(request);
        append_community_audit(
            &mut community,
            &actor_id,
            CommunityAuditAction::MemberChanged,
            Some(requested_actor_id),
            Some("join-request".into()),
            None,
            requested_at_ms,
        );
        Ok(vec![Event::CommunityChanged { community }])
    }

    pub(crate) fn decide_respond_community_join(
        &self,
        actor_id: ActorId,
        conversation_id: ConversationId,
        requester_id: ActorId,
        approved: bool,
        decided_at_ms: i64,
    ) -> Result<Vec<Event>, EngineError> {
        let mut community = self
            .state
            .communities
            .get(&conversation_id)
            .cloned()
            .ok_or_else(|| EngineError::CommunityNotFound(conversation_id.clone()))?;
        require_community_admin(
            &community,
            &actor_id,
            CommunityAdminAction::InviteMembers,
        )?;
        if approved {
            community.approve_join(&requester_id, &actor_id, decided_at_ms)?;
            append_community_audit(
                &mut community,
                &actor_id,
                CommunityAuditAction::JoinApproved,
                Some(requester_id.clone()),
                None,
                None,
                decided_at_ms,
            );
        } else {
            community
                .pending_join_requests
                .remove(&requester_id)
                .ok_or_else(|| CommunityError::JoinRequestNotFound(requester_id.clone()))?;
            append_community_audit(
                &mut community,
                &actor_id,
                CommunityAuditAction::JoinRejected,
                Some(requester_id.clone()),
                None,
                None,
                decided_at_ms,
            );
        }
        let participant_event = if approved
            && self
                .state
                .conversations
                .get(&conversation_id)
                .is_some_and(|conversation| {
                    matches!(
                        conversation.kind,
                        ConversationKind::Group | ConversationKind::Channel
                    )
                }) {
            community
                .members
                .get(&requester_id)
                .and_then(participant_for_community_member)
                .map(|participant| Event::ConversationParticipantUpserted {
                    conversation_id: conversation_id.clone(),
                    participant,
                })
        } else {
            None
        };
        let mut events = vec![Event::CommunityChanged { community }];
        if let Some(event) = participant_event {
            events.push(event);
        }
        Ok(events)
    }
}

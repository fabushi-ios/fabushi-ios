use crate::actor::ActorId;
use crate::message::{compose_share_comment, FormattedText, MediaRef};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StoryId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StoryPrivacyKind {
    Everyone,
    Contacts,
    CloseFriends,
    Selected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryPrivacy {
    pub kind: StoryPrivacyKind,
    pub included_actor_ids: BTreeSet<ActorId>,
    pub excluded_actor_ids: BTreeSet<ActorId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryView {
    pub actor_id: ActorId,
    pub viewed_at_ms: i64,
    pub reaction: Option<String>,
    pub forwarded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Story {
    pub id: StoryId,
    pub owner_id: ActorId,
    pub media: MediaRef,
    pub caption: FormattedText,
    pub privacy: StoryPrivacy,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub edited_at_ms: Option<i64>,
    pub pinned_to_profile: bool,
    pub protected_content: bool,
    pub allow_replies: bool,
    pub views: BTreeMap<ActorId, StoryView>,
}

impl Story {
    pub fn is_visible_to(
        &self,
        actor_id: &ActorId,
        is_contact: bool,
        is_close_friend: bool,
    ) -> bool {
        if &self.owner_id == actor_id {
            return true;
        }
        if self.privacy.excluded_actor_ids.contains(actor_id) {
            return false;
        }
        if self.privacy.included_actor_ids.contains(actor_id) {
            return true;
        }
        match self.privacy.kind {
            StoryPrivacyKind::Everyone => true,
            StoryPrivacyKind::Contacts => is_contact,
            StoryPrivacyKind::CloseFriends => is_close_friend,
            StoryPrivacyKind::Selected => false,
        }
    }

    pub fn record_view(&mut self, actor_id: ActorId, viewed_at_ms: i64) -> Result<(), StoryError> {
        if viewed_at_ms > self.expires_at_ms && !self.pinned_to_profile {
            return Err(StoryError::Expired(self.id.clone()));
        }
        self.views
            .entry(actor_id.clone())
            .and_modify(|view| view.viewed_at_ms = view.viewed_at_ms.min(viewed_at_ms))
            .or_insert(StoryView {
                actor_id,
                viewed_at_ms,
                reaction: None,
                forwarded: false,
            });
        Ok(())
    }

    pub fn react(
        &mut self,
        actor_id: &ActorId,
        reaction: Option<String>,
    ) -> Result<(), StoryError> {
        let view = self
            .views
            .get_mut(actor_id)
            .ok_or_else(|| StoryError::ViewerNotFound(actor_id.clone()))?;
        view.reaction = reaction;
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StoryError {
    #[error("story {0:?} has expired")]
    Expired(StoryId),
    #[error("story viewer {0:?} was not found")]
    ViewerNotFound(ActorId),
}

/// Resolves the canonical active/visible `StoryId` to open for `owner_id` (`LookupShownId`).
///
/// Enforces stale-suggestion fencing:
/// - `suggested_id` is accepted only when it belongs to `owner_id`, is currently active
///   (`pinned_to_profile || expires_at_ms > now_ms`), and is visible to `viewer_id`.
/// - Otherwise (including foreign-owner suggestions, expired/deleted stories, or `None`),
///   falls back to the first unread story (`!story.views.contains_key(viewer_id)`) in
///   deterministic `(created_at_ms, id)` order, or the first active visible story if all
///   have already been viewed.
pub fn resolve_shown_story_id<'a, I>(
    stories: I,
    owner_id: &ActorId,
    viewer_id: &ActorId,
    is_contact: bool,
    is_close_friend: bool,
    now_ms: i64,
    suggested_id: Option<&StoryId>,
) -> Option<StoryId>
where
    I: IntoIterator<Item = &'a Story>,
{
    let mut visible = stories
        .into_iter()
        .filter(|story| {
            &story.owner_id == owner_id
                && (story.pinned_to_profile || story.expires_at_ms > now_ms)
                && story.is_visible_to(viewer_id, is_contact, is_close_friend)
        })
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| {
        left.created_at_ms
            .cmp(&right.created_at_ms)
            .then_with(|| left.id.cmp(&right.id))
    });
    if let Some(suggested) = suggested_id {
        if visible.iter().any(|story| &story.id == suggested) {
            return Some(suggested.clone());
        }
    }
    visible
        .iter()
        .find(|story| !story.views.contains_key(viewer_id))
        .or_else(|| visible.first())
        .map(|story| story.id.clone())
}

/// Generation- and owner-bound callback token that fences async story/media resolution
/// so delayed completions from an older sibling slot or foreign owner cannot mutate a dead
/// or rebound surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryResolutionToken {
    pub owner_id: ActorId,
    pub story_id: StoryId,
    pub generation: u64,
}

impl StoryResolutionToken {
    pub fn matches_callback(
        &self,
        owner_id: &ActorId,
        story_id: &StoryId,
        generation: u64,
    ) -> bool {
        &self.owner_id == owner_id && &self.story_id == story_id && self.generation == generation
    }
}

/// Canonical media/thumbnail fallback state machine for story sibling previews.
///
/// Keeps thumbnail fallback available immediately, transitions to `Ready` only when the
/// expected media resource resolves, and settles streaming/download failures to
/// `TerminalFallback` without spinning an infinite retry loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum StorySiblingMediaState {
    Initial {
        thumbnail_id: Option<String>,
        full_media_id: String,
    },
    Loading {
        thumbnail_id: Option<String>,
        full_media_id: String,
    },
    Ready {
        media_id: String,
    },
    TerminalFallback {
        thumbnail_id: Option<String>,
    },
}

impl StorySiblingMediaState {
    pub fn from_media(media: &MediaRef) -> Self {
        Self::Initial {
            thumbnail_id: media
                .thumbnail_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(ToOwned::to_owned),
            full_media_id: media.id.trim().to_owned(),
        }
    }

    pub fn begin_load(&mut self) -> bool {
        match self {
            Self::Initial {
                thumbnail_id,
                full_media_id,
            } => {
                *self = Self::Loading {
                    thumbnail_id: thumbnail_id.clone(),
                    full_media_id: full_media_id.clone(),
                };
                true
            }
            Self::Loading { .. } | Self::Ready { .. } | Self::TerminalFallback { .. } => false,
        }
    }

    pub fn complete_load(&mut self, resolved_media_id: &str) -> bool {
        let resolved = resolved_media_id.trim();
        match self {
            Self::Initial { full_media_id, .. } | Self::Loading { full_media_id, .. }
                if !resolved.is_empty() && full_media_id == resolved =>
            {
                *self = Self::Ready {
                    media_id: resolved.to_owned(),
                };
                true
            }
            _ => false,
        }
    }

    pub fn fail_stream_or_load(&mut self) {
        match self {
            Self::Initial { thumbnail_id, .. } | Self::Loading { thumbnail_id, .. } => {
                *self = Self::TerminalFallback {
                    thumbnail_id: thumbnail_id.clone(),
                };
            }
            Self::Ready { .. } | Self::TerminalFallback { .. } => {}
        }
    }

    pub fn active_display_resource_id(&self) -> Option<&str> {
        match self {
            Self::Ready { media_id } => Some(media_id.as_str()),
            Self::Initial { thumbnail_id, .. }
            | Self::Loading { thumbnail_id, .. }
            | Self::TerminalFallback { thumbnail_id } => thumbnail_id.as_deref(),
        }
    }
}

/// Derived projection cache key for participant header (avatar + display name) in story
/// sibling previews. Invalidates whenever participant identity, display name, avatar,
/// available width, or device pixel ratio changes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorySiblingHeaderCacheKey {
    pub owner_id: ActorId,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub available_width_px: u32,
    pub device_pixel_ratio_milli: u32,
}

/// Formats a video share-at-time offset in seconds as `M:SS` or `H:MM:SS`
/// (`FormatShareAtTime` in `media_stories_share.h`).
pub fn format_share_at_time(timestamp_seconds: u32) -> String {
    let hours = timestamp_seconds / 3_600;
    let minutes = (timestamp_seconds % 3_600) / 60;
    let seconds = timestamp_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Appends or replaces a canonical `?t=<seconds>` / `&t=<seconds>` share-at-time query
/// parameter on a story/video share URL when `timestamp_seconds` is positive.
pub fn append_share_at_time_query(url: &str, timestamp_seconds: Option<u32>) -> String {
    let base = url.trim();
    let Some(seconds) = timestamp_seconds.filter(|seconds| *seconds > 0) else {
        return base.to_owned();
    };
    let (without_fragment, fragment) = match base.split_once('#') {
        Some((prefix, frag)) => (prefix, Some(frag)),
        None => (base, None),
    };
    let separator = if without_fragment.contains('?') {
        '&'
    } else {
        '?'
    };
    match fragment {
        Some(frag) => format!("{without_fragment}{separator}t={seconds}#{frag}"),
        None => format!("{without_fragment}{separator}t={seconds}"),
    }
}

/// Canonical story-share context (`PrepareShareBox` / `PrepareShareAtTimeBox` in
/// `media_stories_share.{h,cpp}`) carrying story identity, optional video timestamp,
/// source-disappearance revalidation, outgoing message counting, and UTF-16-safe
/// link + formatted comment composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryShareContext {
    pub owner_id: ActorId,
    pub story_id: StoryId,
    pub share_url: String,
    pub video_timestamp_seconds: Option<u32>,
}

impl StoryShareContext {
    pub fn formatted_timestamp(&self) -> Option<String> {
        self.video_timestamp_seconds
            .filter(|seconds| *seconds > 0)
            .map(format_share_at_time)
    }

    pub fn resolved_share_url(&self) -> String {
        append_share_at_time_query(&self.share_url, self.video_timestamp_seconds)
    }

    /// Computes the per-destination outgoing message count (`computeStarsCount` in
    /// `media_stories_share.cpp`): when sharing a story object (as opposed to a plain link)
    /// accompanied by a non-empty user comment, two messages are sent per destination
    /// (the story media/object plus the comment message); otherwise one message is sent.
    pub fn outgoing_message_count(&self, share_as_link_only: bool, comment: Option<&FormattedText>) -> u32 {
        let has_comment = comment.is_some_and(|text| !text.text.trim().is_empty());
        if !share_as_link_only && has_comment {
            2
        } else {
            1
        }
    }

    /// Composes a link-share payload by prepending `<resolved_share_url>\n` to the optional
    /// formatted comment while shifting all existing UTF-16 entity offsets atomically.
    pub fn compose_link_with_comment(
        &self,
        comment: Option<&FormattedText>,
    ) -> Option<FormattedText> {
        let url = self.resolved_share_url();
        match comment {
            Some(existing) => compose_share_comment(&url, existing.clone()).ok(),
            None => Some(FormattedText::plain(url)),
        }
    }

    /// Revalidates before exposure and again at submit time that the source story still exists,
    /// belongs to `owner_id`, has not expired, is not protected content, and is visible to `viewer_id`.
    pub fn revalidate_source<'a, I>(
        &self,
        stories: I,
        viewer_id: &ActorId,
        is_contact: bool,
        is_close_friend: bool,
        now_ms: i64,
    ) -> Option<&'a Story>
    where
        I: IntoIterator<Item = &'a Story>,
    {
        stories.into_iter().find(|story| {
            story.id == self.story_id
                && story.owner_id == self.owner_id
                && !story.protected_content
                && (story.pinned_to_profile || story.expires_at_ms > now_ms)
                && story.is_visible_to(viewer_id, is_contact, is_close_friend)
        })
    }
}

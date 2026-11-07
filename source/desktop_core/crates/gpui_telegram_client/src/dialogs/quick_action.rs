//! Upstream:
//! - Telegram/SourceFiles/dialogs/dialogs_quick_action.h:L1-L58
//! - Telegram/SourceFiles/dialogs/dialogs_quick_action.cpp:L1-L320
//! - Telegram/SourceFiles/dialogs/dialogs_three_state_icon.h:L1-L22
//! - Telegram/SourceFiles/dialogs/ui/dialogs_quick_action.h:L1-L24
//! - Telegram/SourceFiles/dialogs/ui/dialogs_quick_action_context.h:L1-L49

use serde::{Deserialize, Serialize};

/// Quick dialog swipe action configuration.
/// Upstream: `dialogs/ui/dialogs_quick_action.h:L14-21`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QuickDialogAction {
    Mute,
    Pin,
    Read,
    Archive,
    Delete,
    #[default]
    Disabled,
}

/// Dynamic quick action display label depending on item state.
/// Upstream: `dialogs/ui/dialogs_quick_action_context.h:L25-37`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum QuickDialogActionLabel {
    Mute,
    Unmute,
    Pin,
    Unpin,
    Read,
    Unread,
    Archive,
    Unarchive,
    Delete,
    Ungroup,
    #[default]
    Disabled,
}

/// Lottie animation file asset keys for quick swipe actions.
/// Upstream: `dialogs_quick_action.cpp:L165-190`
pub fn resolve_quick_dialog_lottie_icon_name(action: QuickDialogActionLabel) -> &'static str {
    match action {
        QuickDialogActionLabel::Mute => "swipe_mute",
        QuickDialogActionLabel::Unmute => "swipe_unmute",
        QuickDialogActionLabel::Pin => "swipe_pin",
        QuickDialogActionLabel::Unpin => "swipe_unpin",
        QuickDialogActionLabel::Read => "swipe_read",
        QuickDialogActionLabel::Unread => "swipe_unread",
        QuickDialogActionLabel::Archive => "swipe_archive",
        QuickDialogActionLabel::Unarchive => "swipe_unarchive",
        QuickDialogActionLabel::Delete => "swipe_delete",
        QuickDialogActionLabel::Ungroup => "swipe_ungroup",
        QuickDialogActionLabel::Disabled => "swipe_disabled",
    }
}

/// Resolve dynamic quick dialog action label given thread state.
/// Upstream: `dialogs_quick_action.cpp:L192-231`
pub fn resolve_quick_dialog_label(
    action: QuickDialogAction,
    is_self: bool,
    is_muted: bool,
    is_pinned: bool,
    has_unread: bool,
    is_archived: bool,
    can_archive: bool,
    is_community_ungroup: bool,
) -> QuickDialogActionLabel {
    if is_community_ungroup {
        return QuickDialogActionLabel::Ungroup;
    }
    match action {
        QuickDialogAction::Mute => {
            if is_self {
                QuickDialogActionLabel::Disabled
            } else if is_muted {
                QuickDialogActionLabel::Unmute
            } else {
                QuickDialogActionLabel::Mute
            }
        }
        QuickDialogAction::Pin => {
            if is_pinned {
                QuickDialogActionLabel::Unpin
            } else {
                QuickDialogActionLabel::Pin
            }
        }
        QuickDialogAction::Read => {
            if has_unread {
                QuickDialogActionLabel::Read
            } else {
                QuickDialogActionLabel::Unread
            }
        }
        QuickDialogAction::Archive => {
            if !can_archive {
                QuickDialogActionLabel::Disabled
            } else if is_archived {
                QuickDialogActionLabel::Unarchive
            } else {
                QuickDialogActionLabel::Archive
            }
        }
        QuickDialogAction::Delete => QuickDialogActionLabel::Delete,
        QuickDialogAction::Disabled => QuickDialogActionLabel::Disabled,
    }
}

/// Background color hex for quick swipe action pill.
/// Upstream: `dialogs_quick_action.cpp:L260-280`
pub fn resolve_quick_action_bg_hex(action: QuickDialogActionLabel) -> &'static str {
    match action {
        QuickDialogActionLabel::Delete | QuickDialogActionLabel::Ungroup => "#E53935", // st::attentionButtonFg
        QuickDialogActionLabel::Disabled => "#8E8E93",                                 // st::windowSubTextFgOver
        _ => "#2481CC",                                                               // st::windowBgActive
    }
}

/// Three state icon helper.
/// Upstream: `dialogs_three_state_icon.h:L14-19`
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThreeStateIcon<T> {
    pub normal: T,
    pub over: T,
    pub active: T,
}

impl<T> ThreeStateIcon<T> {
    pub fn select<'a>(&'a self, active: bool, over: bool) -> &'a T {
        if active {
            &self.active
        } else if over {
            &self.over
        } else {
            &self.normal
        }
    }
}

/// Dynamic swipe font sizing from normal down to minimum.
/// Upstream: `dialogs_quick_action.cpp:L50-52` (`kNormalFontSize = 13`, `kMinFontSize = 5`)
pub const K_NORMAL_FONT_SIZE: i32 = 13;
pub const K_MIN_FONT_SIZE: i32 = 5;

pub fn fit_swipe_action_font_size(text_len: usize, available_width: i32) -> i32 {
    let approx_char_width = 8;
    for sz in (K_MIN_FONT_SIZE..=K_NORMAL_FONT_SIZE).rev() {
        let estimated_width = (text_len as i32 * approx_char_width * sz) / K_NORMAL_FONT_SIZE;
        if estimated_width <= available_width || sz == K_MIN_FONT_SIZE {
            return sz;
        }
    }
    K_MIN_FONT_SIZE
}

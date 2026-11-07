use crate::actor::ActorId;
use crate::conversation::ConversationId;
use crate::miniapp_service_call::{
    MiniAppServiceCallId, MiniAppServiceCallInput, MiniAppServiceCallMode, MiniAppServiceCallState,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MiniAppPermission {
    Identity,
    Theme,
    ClipboardRead,
    ClipboardWrite,
    Camera,
    Microphone,
    Location,
    Contacts,
    Files,
    Notifications,
    Haptics,
    Payments,
    OpenExternal,
    SendMessage,
    ReadConversation,
    ServiceCall,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniAppManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon_url: Option<String>,
    pub start_url: String,
    pub allowed_origins: Vec<String>,
    pub requested_permissions: Vec<MiniAppPermission>,
    pub bot_actor_id: Option<ActorId>,
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniAppGrant {
    pub mini_app_id: String,
    pub actor_id: ActorId,
    pub permissions: Vec<MiniAppPermission>,
    pub granted_at_ms: i64,
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MiniAppSession {
    pub id: String,
    pub mini_app_id: String,
    pub actor_id: ActorId,
    pub conversation_id: Option<ConversationId>,
    pub start_parameter: Option<String>,
    pub granted_permissions: Vec<MiniAppPermission>,
    pub opened_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum MiniAppRequest {
    Ready,
    Expand,
    Close,
    SetHeaderColor {
        value: String,
    },
    SetBackgroundColor {
        value: String,
    },
    RequestTheme,
    RequestViewport,
    RequestIdentity,
    RequestLocation,
    RequestContact,
    RequestWriteAccess,
    ReadClipboard,
    WriteClipboard {
        text: String,
    },
    OpenExternal {
        url: String,
    },
    OpenInvoice {
        invoice_id: String,
    },
    SendData {
        data: String,
    },
    SendMessage {
        text: String,
    },
    Haptic {
        kind: String,
    },
    StartServiceCall {
        call_id: MiniAppServiceCallId,
        mode: MiniAppServiceCallMode,
    },
    SubmitServiceCallInput {
        call_id: MiniAppServiceCallId,
        input: MiniAppServiceCallInput,
    },
    EndServiceCall {
        call_id: MiniAppServiceCallId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum MiniAppResponse {
    Ok,
    Error {
        code: String,
        message: String,
    },
    Theme {
        color_scheme: String,
        values: Vec<(String, String)>,
    },
    Viewport {
        width: f64,
        height: f64,
        stable_height: f64,
        expanded: bool,
    },
    Identity {
        actor_id: ActorId,
        display_name: String,
        username: Option<String>,
    },
    Location {
        latitude: f64,
        longitude: f64,
    },
    Contact {
        actor_id: Option<ActorId>,
        display_name: String,
    },
    Clipboard {
        text: Option<String>,
    },
    InvoiceStatus {
        invoice_id: String,
        status: String,
    },
    ServiceCallStatus {
        call_id: MiniAppServiceCallId,
        state: MiniAppServiceCallState,
    },
}

impl MiniAppRequest {
    pub fn required_permissions(&self) -> Vec<MiniAppPermission> {
        match self {
            Self::RequestIdentity => vec![MiniAppPermission::Identity],
            Self::RequestLocation => vec![MiniAppPermission::Location],
            Self::RequestContact => vec![MiniAppPermission::Contacts],
            Self::RequestWriteAccess => vec![MiniAppPermission::SendMessage],
            Self::ReadClipboard => vec![MiniAppPermission::ClipboardRead],
            Self::WriteClipboard { .. } => vec![MiniAppPermission::ClipboardWrite],
            Self::OpenExternal { .. } => vec![MiniAppPermission::OpenExternal],
            Self::OpenInvoice { .. } => vec![MiniAppPermission::Payments],
            Self::SendMessage { .. } | Self::SendData { .. } => {
                vec![MiniAppPermission::SendMessage]
            }
            Self::Haptic { .. } => vec![MiniAppPermission::Haptics],
            Self::StartServiceCall { mode, .. } => {
                let mut permissions = vec![MiniAppPermission::ServiceCall];
                if matches!(
                    mode,
                    MiniAppServiceCallMode::Voice | MiniAppServiceCallMode::Hybrid
                ) {
                    permissions.push(MiniAppPermission::Microphone);
                }
                permissions
            }
            Self::SubmitServiceCallInput { input, .. } => {
                let mut permissions = vec![MiniAppPermission::ServiceCall];
                if matches!(input, MiniAppServiceCallInput::SpeechTranscript { .. }) {
                    permissions.push(MiniAppPermission::Microphone);
                }
                permissions
            }
            Self::EndServiceCall { .. } => vec![MiniAppPermission::ServiceCall],
            Self::Ready
            | Self::Expand
            | Self::Close
            | Self::SetHeaderColor { .. }
            | Self::SetBackgroundColor { .. }
            | Self::RequestTheme
            | Self::RequestViewport => Vec::new(),
        }
    }

    pub fn required_permission(&self) -> Option<MiniAppPermission> {
        self.required_permissions().into_iter().next()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum MiniAppAction {
    SendMessage {
        conversation_id: ConversationId,
        text: String,
    },
    SendData {
        conversation_id: ConversationId,
        data: String,
    },
    DirectResponse {
        response: MiniAppResponse,
    },
    HostActionRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum MiniAppDenial {
    SessionExpired {
        expired_at_ms: i64,
        now_ms: i64,
    },
    GrantExpired {
        expired_at_ms: i64,
        now_ms: i64,
    },
    GrantMismatch,
    PermissionDenied {
        permission: MiniAppPermission,
    },
    MissingConversationBinding,
}

pub fn authorize_miniapp_request(
    session: &MiniAppSession,
    grant: &MiniAppGrant,
    request: &MiniAppRequest,
    now_ms: i64,
) -> Result<MiniAppAction, MiniAppDenial> {
    if session.expires_at_ms > 0 && now_ms > session.expires_at_ms {
        return Err(MiniAppDenial::SessionExpired {
            expired_at_ms: session.expires_at_ms,
            now_ms,
        });
    }

    if session.mini_app_id != grant.mini_app_id || session.actor_id != grant.actor_id {
        return Err(MiniAppDenial::GrantMismatch);
    }

    if let Some(grant_expires_at) = grant.expires_at_ms {
        if grant_expires_at > 0 && now_ms > grant_expires_at {
            return Err(MiniAppDenial::GrantExpired {
                expired_at_ms: grant_expires_at,
                now_ms,
            });
        }
    }

    for permission in request.required_permissions() {
        if !session.granted_permissions.contains(&permission)
            || !grant.permissions.contains(&permission)
        {
            return Err(MiniAppDenial::PermissionDenied { permission });
        }
    }

    match request {
        MiniAppRequest::SendMessage { text } => {
            let conversation_id = session
                .conversation_id
                .clone()
                .ok_or(MiniAppDenial::MissingConversationBinding)?;
            Ok(MiniAppAction::SendMessage {
                conversation_id,
                text: text.clone(),
            })
        }
        MiniAppRequest::SendData { data } => {
            let conversation_id = session
                .conversation_id
                .clone()
                .ok_or(MiniAppDenial::MissingConversationBinding)?;
            Ok(MiniAppAction::SendData {
                conversation_id,
                data: data.clone(),
            })
        }
        MiniAppRequest::Ready
        | MiniAppRequest::Expand
        | MiniAppRequest::Close
        | MiniAppRequest::SetHeaderColor { .. }
        | MiniAppRequest::SetBackgroundColor { .. } => Ok(MiniAppAction::DirectResponse {
            response: MiniAppResponse::Ok,
        }),
        _ => Ok(MiniAppAction::HostActionRequired),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_service_call_requires_service_call_and_microphone_permissions() {
        let request = MiniAppRequest::StartServiceCall {
            call_id: MiniAppServiceCallId::new("svc-call:1"),
            mode: MiniAppServiceCallMode::Voice,
        };
        assert_eq!(
            request.required_permissions(),
            vec![
                MiniAppPermission::ServiceCall,
                MiniAppPermission::Microphone
            ]
        );
    }

    #[test]
    fn text_service_call_does_not_require_microphone() {
        let request = MiniAppRequest::SubmitServiceCallInput {
            call_id: MiniAppServiceCallId::new("svc-call:1"),
            input: MiniAppServiceCallInput::ChatText {
                text: "查询余额".into(),
            },
        };
        assert_eq!(
            request.required_permissions(),
            vec![MiniAppPermission::ServiceCall]
        );
    }

    #[test]
    fn authorized_send_message_produces_send_message_action() {
        let actor = ActorId::new("human:42");
        let conv_id = ConversationId::new("conv:100");
        let session = MiniAppSession {
            id: "sess:1".into(),
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor.clone(),
            conversation_id: Some(conv_id.clone()),
            start_parameter: Some("ref_code".into()),
            granted_permissions: vec![MiniAppPermission::SendMessage],
            opened_at_ms: 1000,
            expires_at_ms: 5000,
        };
        let grant = MiniAppGrant {
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor,
            permissions: vec![MiniAppPermission::SendMessage],
            granted_at_ms: 1000,
            expires_at_ms: Some(10000),
        };
        let request = MiniAppRequest::SendMessage {
            text: "Hello from MiniApp".into(),
        };

        let action = authorize_miniapp_request(&session, &grant, &request, 2000).unwrap();
        assert_eq!(
            action,
            MiniAppAction::SendMessage {
                conversation_id: conv_id,
                text: "Hello from MiniApp".into(),
            }
        );
    }

    #[test]
    fn authorized_send_data_produces_send_data_action() {
        let actor = ActorId::new("human:42");
        let conv_id = ConversationId::new("conv:100");
        let session = MiniAppSession {
            id: "sess:1".into(),
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor.clone(),
            conversation_id: Some(conv_id.clone()),
            start_parameter: None,
            granted_permissions: vec![MiniAppPermission::SendMessage],
            opened_at_ms: 1000,
            expires_at_ms: 5000,
        };
        let grant = MiniAppGrant {
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor,
            permissions: vec![MiniAppPermission::SendMessage],
            granted_at_ms: 1000,
            expires_at_ms: None,
        };
        let request = MiniAppRequest::SendData {
            data: "{\"orderId\": 123}".into(),
        };

        let action = authorize_miniapp_request(&session, &grant, &request, 2000).unwrap();
        assert_eq!(
            action,
            MiniAppAction::SendData {
                conversation_id: conv_id,
                data: "{\"orderId\": 123}".into(),
            }
        );
    }

    #[test]
    fn send_message_without_conversation_binding_is_denied() {
        let actor = ActorId::new("human:42");
        let session = MiniAppSession {
            id: "sess:1".into(),
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor.clone(),
            conversation_id: None,
            start_parameter: None,
            granted_permissions: vec![MiniAppPermission::SendMessage],
            opened_at_ms: 1000,
            expires_at_ms: 5000,
        };
        let grant = MiniAppGrant {
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor,
            permissions: vec![MiniAppPermission::SendMessage],
            granted_at_ms: 1000,
            expires_at_ms: None,
        };
        let request = MiniAppRequest::SendMessage {
            text: "Hello".into(),
        };

        let denial = authorize_miniapp_request(&session, &grant, &request, 2000).unwrap_err();
        assert_eq!(denial, MiniAppDenial::MissingConversationBinding);
    }

    #[test]
    fn send_message_without_permission_is_denied() {
        let actor = ActorId::new("human:42");
        let conv_id = ConversationId::new("conv:100");
        let session = MiniAppSession {
            id: "sess:1".into(),
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor.clone(),
            conversation_id: Some(conv_id),
            start_parameter: None,
            granted_permissions: vec![MiniAppPermission::Theme],
            opened_at_ms: 1000,
            expires_at_ms: 5000,
        };
        let grant = MiniAppGrant {
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor,
            permissions: vec![MiniAppPermission::Theme],
            granted_at_ms: 1000,
            expires_at_ms: None,
        };
        let request = MiniAppRequest::SendMessage {
            text: "Hello".into(),
        };

        let denial = authorize_miniapp_request(&session, &grant, &request, 2000).unwrap_err();
        assert_eq!(
            denial,
            MiniAppDenial::PermissionDenied {
                permission: MiniAppPermission::SendMessage
            }
        );
    }

    #[test]
    fn expired_session_and_grant_are_denied() {
        let actor = ActorId::new("human:42");
        let conv_id = ConversationId::new("conv:100");
        let session = MiniAppSession {
            id: "sess:1".into(),
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor.clone(),
            conversation_id: Some(conv_id.clone()),
            start_parameter: None,
            granted_permissions: vec![MiniAppPermission::SendMessage],
            opened_at_ms: 1000,
            expires_at_ms: 3000,
        };
        let grant = MiniAppGrant {
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor.clone(),
            permissions: vec![MiniAppPermission::SendMessage],
            granted_at_ms: 1000,
            expires_at_ms: Some(2500),
        };
        let request = MiniAppRequest::SendMessage {
            text: "Hello".into(),
        };

        // Session expired (now_ms = 4000 > session expires_at_ms = 3000)
        let denial = authorize_miniapp_request(&session, &grant, &request, 4000).unwrap_err();
        assert_eq!(
            denial,
            MiniAppDenial::SessionExpired {
                expired_at_ms: 3000,
                now_ms: 4000
            }
        );

        // Grant expired (now_ms = 2600 > grant expires_at_ms = 2500, but session still active)
        let denial = authorize_miniapp_request(&session, &grant, &request, 2600).unwrap_err();
        assert_eq!(
            denial,
            MiniAppDenial::GrantExpired {
                expired_at_ms: 2500,
                now_ms: 2600
            }
        );

        // Grant mismatch
        let mismatch_grant = MiniAppGrant {
            mini_app_id: "org.other.app".into(),
            actor_id: actor,
            permissions: vec![MiniAppPermission::SendMessage],
            granted_at_ms: 1000,
            expires_at_ms: None,
        };
        let denial = authorize_miniapp_request(&session, &mismatch_grant, &request, 2000).unwrap_err();
        assert_eq!(denial, MiniAppDenial::GrantMismatch);
    }

    #[test]
    fn direct_response_and_host_action_cases() {
        let actor = ActorId::new("human:42");
        let session = MiniAppSession {
            id: "sess:1".into(),
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor.clone(),
            conversation_id: None,
            start_parameter: None,
            granted_permissions: vec![MiniAppPermission::Payments],
            opened_at_ms: 1000,
            expires_at_ms: 5000,
        };
        let grant = MiniAppGrant {
            mini_app_id: "org.fabushi.shop".into(),
            actor_id: actor,
            permissions: vec![MiniAppPermission::Payments],
            granted_at_ms: 1000,
            expires_at_ms: None,
        };

        let ready_action = authorize_miniapp_request(&session, &grant, &MiniAppRequest::Ready, 2000).unwrap();
        assert_eq!(ready_action, MiniAppAction::DirectResponse { response: MiniAppResponse::Ok });

        let invoice_action = authorize_miniapp_request(&session, &grant, &MiniAppRequest::OpenInvoice { invoice_id: "inv:1".into() }, 2000).unwrap();
        assert_eq!(invoice_action, MiniAppAction::HostActionRequired);
    }

}

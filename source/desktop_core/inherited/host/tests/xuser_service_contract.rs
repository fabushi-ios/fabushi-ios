use mahayana_host_runtime::extensions::cross_user_sharing::extension::*;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_entry_publisher::*;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_relay::*;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_sharing_service::*;
use mahayana_host_runtime::extensions::cross_user_sharing::xuser_state_reconcile::*;
use mahayana_host_runtime::extensions::experiments::{
    HostExperimentsExtension, HostExperimentsOptions,
};
use mahayana_host_runtime::extensions::notify_bus::extension::{
    NotifyBusExtensionOptions, start_notify_bus_extension_with_options,
};
use mahayana_host_runtime::extensions::notify_bus::notify_bus_client::NotifyBusTiming;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

struct FakeTransport {
    requests: Mutex<Vec<String>>,
    value: serde_json::Value,
    status: u16,
}
impl XuserRelayTransport for FakeTransport {
    fn request(
        &self,
        _: &str,
        url: &str,
        _: &str,
        _: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value, SandXuserRelayHttpError> {
        self.requests.lock().unwrap().push(url.into());
        if self.status >= 400 {
            Err(SandXuserRelayHttpError {
                status: self.status,
                path: url.into(),
            })
        } else {
            Ok(self.value.clone())
        }
    }
}
fn service_with_transport(
    value: serde_json::Value,
) -> (Arc<SandXuserSharingService>, Arc<FakeTransport>) {
    let t = Arc::new(FakeTransport {
        requests: Mutex::new(vec![]),
        value,
        status: 200,
    });
    let relay = Arc::new(SandXuserRelayClient::new(
        "https://example.invalid".into(),
        Arc::new(|| Ok("token".into())),
        Arc::clone(&t) as Arc<dyn XuserRelayTransport>,
    ));
    (Arc::new(SandXuserSharingService::new(relay)), t)
}

fn service(value: serde_json::Value) -> Arc<SandXuserSharingService> {
    service_with_transport(value).0
}
#[test]
fn relay_errors_keep_frozen_user_messages() {
    assert_eq!(
        describe_relay_error(&SandXuserRelayHttpError {
            status: 403,
            path: "x".into()
        }),
        "Sharing isn't enabled for your account."
    );
    assert!(
        describe_relay_error(&SandXuserRelayHttpError {
            status: 429,
            path: "x".into()
        })
        .contains("too often")
    );
}
#[test]
fn relay_departure_raw_surface_preserves_http_status() {
    let transport = Arc::new(FakeTransport {
        requests: Mutex::new(vec![]),
        value: serde_json::json!({}),
        status: 404,
    });
    let relay = SandXuserRelayClient::new(
        "https://example.invalid".into(),
        Arc::new(|| Ok("token".into())),
        Arc::clone(&transport) as Arc<dyn XuserRelayTransport>,
    );
    let leave = relay
        .leave_room_with_target_raw("room-gone", None)
        .expect_err("404 must remain typed");
    assert_eq!(leave.status, 404);
    let remove = relay
        .remove_deleted_agent_raw("agent-gone")
        .expect_err("404 must remain typed");
    assert_eq!(remove.status, 404);
}

#[test]
fn remote_ids_and_publish_payload_are_fail_closed() {
    assert_eq!(
        parse_remote_agent_id("sand-remote:owner/agent"),
        Some(("owner".into(), "agent".into()))
    );
    assert!(parse_remote_agent_id("local").is_none());
    assert!(
        build_publish_payload(
            &serde_json::json!({"id":"1","kind":"message","content":""}),
            "me"
        )
        .is_none()
    );
    assert_eq!(
        build_publish_payload(
            &serde_json::json!({"id":"1","kind":"message","content":"hi"}),
            "me"
        )
        .unwrap()["text"],
        "hi"
    );
}
#[test]
fn reconcile_reports_added_and_revoked_rooms() {
    let old = vec![XuserRoom {
        room_id: "old".into(),
        name: "Old".into(),
        host_auth_id: "me".into(),
        members: vec![],
        avatar_data_url: None,
    }];
    let new = vec![XuserRoom {
        room_id: "new".into(),
        name: "New".into(),
        host_auth_id: "me".into(),
        members: vec![],
        avatar_data_url: None,
    }];
    let r = reconcile_rooms(&old, new, Some("me"));
    assert_eq!(r.revoked_room_ids, vec!["old"]);
    assert_eq!(r.added_room_ids, vec!["new"]);
}
#[test]
fn extension_applies_dev_environment_gate_before_feature_gate() {
    let svc = service(serde_json::json!({"rooms":[],"pendingJoinRequests":[]}));
    let ext = CrossUserSharingExtension::new(
        Arc::clone(&svc),
        "https://api.ombhrum.com",
        &BTreeMap::new(),
    );
    ext.set_feature_gate(true);
    assert!(!ext.is_enabled());
    assert!(ext.reason().is_some());
    let mut env = BTreeMap::new();
    env.insert("SAND_DEV_XUSER_SHARING".into(), "1".into());
    env.insert("SAND_XUSER_SHARING_ALLOW_PROD".into(), "1".into());
    let ext = CrossUserSharingExtension::new(svc, "https://api.ombhrum.com", &env);
    ext.set_feature_gate(true);
    assert!(ext.is_enabled());
}
#[test]
fn enabled_service_reconciles_backend_state() {
    let svc = service(
        serde_json::json!({"pendingJoinRequests":[{"id":"j"}],"rooms":[{"roomId":"r","name":"Room","hostAuthId":"me","members":[]}]}),
    );
    svc.set_enabled(true);
    svc.set_self_auth_id(Some("me".into()));
    let s = svc.reconcile_share_state().unwrap();
    assert_eq!(s.rooms.len(), 1);
    assert_eq!(s.pending_join_requests.len(), 1);
}

#[test]
fn extension_tracks_multiplayer_gate_and_owns_service_lifecycle() {
    let svc = service(serde_json::json!({
        "rooms": [],
        "pendingJoinRequests": []
    }));
    let mut env = BTreeMap::new();
    env.insert("SAND_DEV_XUSER_SHARING".into(), "1".into());

    let extension = Arc::new(CrossUserSharingExtension::new(
        Arc::clone(&svc),
        "http://127.0.0.1:4200",
        &env,
    ));
    let experiments = Arc::new(HostExperimentsExtension::new(
        HostExperimentsOptions {
            is_dev_build: true,
            env_gate_overrides: Some("sand_multiplayer=true".into()),
        },
    ));
    let notify_bus = start_notify_bus_extension_with_options(
        Arc::clone(&experiments)
            as Arc<dyn mahayana_host_runtime::extensions::notify_bus::extension::NotifyBusExperimentsApi>,
        NotifyBusExtensionOptions {
            get_backend_url: Arc::new(|| Ok("https://example.invalid".into())),
            get_access_token: Arc::new(|_| Ok("token".into())),
            now_ms: Arc::new(|| 1_000),
            timing: NotifyBusTiming::default(),
            log: Arc::new(|_| {}),
        },
    )
    .unwrap();

    extension
        .start_background_work(Arc::clone(&experiments), notify_bus.clone())
        .unwrap();
    assert!(extension.is_enabled());
    assert!(svc.is_started());

    experiments.replace_feature_flag_overrides(BTreeMap::from([
        ("sand_multiplayer".into(), false),
    ]));
    assert!(!extension.is_enabled());
    assert!(!svc.is_started());

    experiments.replace_feature_flag_overrides(BTreeMap::from([
        ("sand_multiplayer".into(), true),
    ]));
    assert!(extension.is_enabled());
    assert!(svc.is_started());

    extension.prepare_for_upgrade();
    assert!(!svc.is_started());
    extension.stop();
    notify_bus.stop();
}


#[test]
fn sharing_service_exposes_frozen_room_invite_and_typing_relay_surface() {
    let (svc, transport) = service_with_transport(serde_json::json!({
        "room": {"roomId":"room-a"},
        "shareUrl":"https://share.invalid/a",
        "expiresAtMs":1234
    }));
    svc.set_enabled(true);
    let invite = svc.create_room_invite("room-a").unwrap();
    assert_eq!(invite["status"], "ok");
    assert_eq!(invite["roomId"], "room-a");
    assert!(transport.requests.lock().unwrap().iter().any(|url| url.ends_with("/sand/share-rooms/invite-links")));

    let (typing, typing_transport) = service_with_transport(serde_json::json!({}));
    typing.set_enabled(true);
    typing.set_room_typing("room-a", true).unwrap();
    assert!(typing_transport.requests.lock().unwrap().iter().any(|url| url.ends_with("/sand/xuser/send")));
}

#[test]
fn sharing_service_join_request_settlement_updates_local_room_state() {
    let svc = service(serde_json::json!({
        "status":"approved",
        "room": {"roomId":"room-b","name":"Room B","hostAuthId":"me","members":[]}
    }));
    svc.set_enabled(true);
    svc.set_self_auth_id(Some("me".into()));
    // Seed a pending request through the canonical reconcile path using a dedicated service.
    // The settlement assertion below focuses on the service-owned local room install.
    let state = svc.respond_to_join_request(&serde_json::json!({"requestId":"req-b"})).unwrap();
    assert_eq!(state.rooms.len(), 1);
    assert_eq!(state.rooms[0].room_id, "room-b");
}

#[test]
fn sharing_service_create_add_remove_leave_and_deleted_agent_use_relay_owner() {
    let room = serde_json::json!({"roomId":"room-c","name":"Room C","hostAuthId":"me","members":[]});
    let (create, create_transport) = service_with_transport(serde_json::json!({
        "status":"created",
        "room": room.clone()
    }));
    create.set_enabled(true);
    create.set_self_auth_id(Some("me".into()));
    let created = create.create_shared_room(&serde_json::json!({"agents":[]})).unwrap();
    assert_eq!(created["status"], "ok");
    assert_eq!(created["roomId"], "room-c");
    assert_eq!(create.get_state().rooms.len(), 1);
    assert!(create_transport.requests.lock().unwrap().iter().any(|url| url.ends_with("/sand/share-rooms")));

    let (add, add_transport) = service_with_transport(serde_json::json!({"room": room.clone()}));
    add.set_enabled(true);
    add.add_own_agent(&serde_json::json!({"roomId":"room-c","agentId":"agent-a"})).unwrap();
    assert_eq!(add.get_state().rooms.len(), 1);
    assert!(add_transport.requests.lock().unwrap().iter().any(|url| url.ends_with("/sand/share-rooms/agents/add")));

    let (remove, remove_transport) = service_with_transport(serde_json::json!({"room": room.clone()}));
    remove.set_enabled(true);
    remove.remove_own_agent("room-c", "agent-a").unwrap();
    assert_eq!(remove.get_state().rooms.len(), 1);
    assert!(remove_transport.requests.lock().unwrap().iter().any(|url| url.ends_with("/sand/share-rooms/agents/remove")));

    let (deleted, deleted_transport) = service_with_transport(serde_json::json!({}));
    deleted.set_enabled(true);
    deleted.note_agent_deleted("agent-a").unwrap();
    assert!(deleted_transport.requests.lock().unwrap().iter().any(|url| url.ends_with("/sand/share-rooms/agents/remove-deleted")));
}



#[derive(Default)]
struct FakeSharingManager {
    installed: Mutex<Vec<String>>,
    revoked: Mutex<Vec<String>>,
    posts: Mutex<Vec<String>>,
    mirror_entries: Mutex<Vec<String>>,
}

impl XuserSharingManager for FakeSharingManager {
    fn install_room(&self, room: &XuserRoom, _: Option<&str>) -> Result<(), String> {
        self.installed.lock().unwrap().push(room.room_id.clone());
        Ok(())
    }

    fn mark_mirror_room_revoked(&self, room_id: &str) -> Result<(), String> {
        self.revoked.lock().unwrap().push(room_id.to_string());
        Ok(())
    }

    fn post_shared_room_guest_message(&self, event: &serde_json::Value) -> Result<(), String> {
        self.posts.lock().unwrap().push(
            event
                .get("roomId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        Ok(())
    }

    fn append_mirror_room_entry(
        &self,
        event: &serde_json::Value,
        _: &str,
    ) -> Result<bool, String> {
        self.mirror_entries.lock().unwrap().push(
            event
                .get("roomId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        Ok(true)
    }
}

#[test]
fn sharing_service_routes_live_room_events_through_transcript_manager_seam() {
    let svc = service(serde_json::json!({
        "pendingJoinRequests": [],
        "rooms": []
    }));
    svc.set_enabled(true);
    svc.set_self_auth_id(Some("self-auth".into()));
    let manager = Arc::new(FakeSharingManager::default());
    svc.bind_manager(manager.clone() as Arc<dyn XuserSharingManager>);

    assert!(svc
        .handle_event(&serde_json::json!({
            "kind":"room-upsert",
            "room":{
                "roomId":"room-live",
                "name":"Live",
                "hostAuthId":"self-auth",
                "members":[{"authId":"peer","name":"Peer"}]
            }
        }))
        .unwrap());
    assert_eq!(*manager.installed.lock().unwrap(), vec!["room-live"]);

    assert!(svc
        .handle_event(&serde_json::json!({
            "kind":"room-post",
            "roomId":"room-live",
            "text":"hello"
        }))
        .unwrap());
    assert_eq!(*manager.posts.lock().unwrap(), vec!["room-live"]);

    assert!(svc
        .handle_event(&serde_json::json!({
            "kind":"room-entry",
            "roomId":"room-live",
            "entry":{"entryId":"entry-1","kind":"human-message","text":"hi"}
        }))
        .unwrap());
    assert_eq!(*manager.mirror_entries.lock().unwrap(), vec!["room-live"]);

    assert!(svc
        .handle_event(&serde_json::json!({
            "kind":"room-typing",
            "user":{
                "roomId":"room-live",
                "authId":"peer",
                "name":"Peer",
                "expiresAtMs": 4102444800000.0
            },
            "isTyping":true
        }))
        .unwrap());
    assert_eq!(svc.get_state().typing_users.len(), 1);

    assert!(svc
        .handle_event(&serde_json::json!({
            "kind":"room-ended",
            "roomId":"room-live"
        }))
        .unwrap());
    assert_eq!(*manager.revoked.lock().unwrap(), vec!["room-live"]);
}

#[test]
fn sharing_service_reconcile_materializes_registry_and_revokes_missing_rooms() {
    let svc = service(serde_json::json!({
        "pendingJoinRequests": [],
        "rooms":[{
            "roomId":"new-room",
            "name":"New",
            "hostAuthId":"self-auth",
            "members":[]
        }]
    }));
    svc.set_enabled(true);
    svc.set_self_auth_id(Some("self-auth".into()));
    let manager = Arc::new(FakeSharingManager::default());
    svc.bind_manager(manager.clone() as Arc<dyn XuserSharingManager>);

    svc.handle_event(&serde_json::json!({
        "kind":"room-upsert",
        "room":{
            "roomId":"old-room",
            "name":"Old",
            "hostAuthId":"self-auth",
            "members":[]
        }
    }))
    .unwrap();
    manager.installed.lock().unwrap().clear();

    svc.reconcile_share_state().unwrap();
    assert_eq!(*manager.revoked.lock().unwrap(), vec!["old-room"]);
    assert_eq!(*manager.installed.lock().unwrap(), vec!["new-room"]);
}

struct FakePublisherHost {
    enabled: bool,
    self_auth_id: Option<String>,
    now_ms: u64,
    attachments: Mutex<BTreeMap<String, ResolvedXuserAttachment>>,
    sent: Mutex<Vec<serde_json::Value>>,
    restamped: Mutex<Vec<(String, String, f64)>>,
}

impl XuserEntryPublisherHost for FakePublisherHost {
    fn relay_send(&self, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.sent.lock().unwrap().push(payload.clone());
        Ok(serde_json::json!({ "timestampMs": 4321 }))
    }

    fn restamp_room_entry(
        &self,
        room_id: &str,
        entry_id: &str,
        timestamp_ms: f64,
    ) -> Result<(), String> {
        self.restamped.lock().unwrap().push((
            room_id.to_string(),
            entry_id.to_string(),
            timestamp_ms,
        ));
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        self.now_ms
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn self_auth_id(&self) -> Option<String> {
        self.self_auth_id.clone()
    }

    fn resolve_attachment(
        &self,
        url: &str,
    ) -> Result<Option<ResolvedXuserAttachment>, String> {
        Ok(self.attachments.lock().unwrap().get(url).cloned())
    }
}

fn publisher_fixture() -> (Arc<FakePublisherHost>, Arc<SandXuserEntryPublisher>) {
    let host = Arc::new(FakePublisherHost {
        enabled: true,
        self_auth_id: Some("self-auth".into()),
        now_ms: 1234,
        attachments: Mutex::new(BTreeMap::from([
            (
                "attachment://ok".into(),
                ResolvedXuserAttachment {
                    data: vec![1, 2, 3],
                    mime_type: "image/png".into(),
                },
            ),
            (
                "attachment://not-image".into(),
                ResolvedXuserAttachment {
                    data: b"plain".to_vec(),
                    mime_type: "text/plain".into(),
                },
            ),
            (
                "attachment://too-large".into(),
                ResolvedXuserAttachment {
                    data: vec![0; SAND_SHARED_ROOM_IMAGE_BYTES_MAX + 1],
                    mime_type: "image/png".into(),
                },
            ),
        ])),
        sent: Mutex::new(Vec::new()),
        restamped: Mutex::new(Vec::new()),
    });
    let publisher_host: Arc<dyn XuserEntryPublisherHost> = host.clone();
    let publisher = Arc::new(SandXuserEntryPublisher::new(publisher_host));
    (host, publisher)
}

#[test]
fn entry_publisher_preserves_frozen_projection_attachment_and_restamp_contract() {
    let (host, publisher) = publisher_fixture();

    publisher
        .publish_entry(
            "room-a",
            &serde_json::json!({
                "id": "human-1",
                "kind": "message",
                "role": "user",
                "content": "hello",
                "clientNonce": "nonce-1",
                "images": [
                    {"url":"attachment://ok","alt":"ok"},
                    {"url":"attachment://not-image"},
                    {"url":"attachment://too-large"}
                ]
            }),
        )
        .unwrap();

    let sent = host.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["kind"], "room-entry");
    assert_eq!(sent[0]["roomId"], "room-a");
    assert_eq!(sent[0]["entry"]["kind"], "human-message");
    assert_eq!(sent[0]["entry"]["authorAuthId"], "self-auth");
    assert_eq!(sent[0]["entry"]["authorName"], "Host");
    assert_eq!(sent[0]["entry"]["clientNonce"], "nonce-1");
    assert_eq!(sent[0]["entry"]["timestampMs"].as_f64(), Some(1234.0));
    let images = sent[0]["entry"]["images"].as_array().unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["base64"], "AQID");
    assert_eq!(images[0]["mediaType"], "image/png");
    assert_eq!(images[0]["alt"], "ok");
    drop(sent);

    assert_eq!(
        *host.restamped.lock().unwrap(),
        vec![("room-a".into(), "human-1".into(), 4321.0)]
    );

    publisher
        .publish_entry(
            "room-a",
            &serde_json::json!({
                "id": "remote-agent",
                "kind": "send-message",
                "message": {"type":"text","content":"must not echo"},
                "author": {"id":"sand-remote:owner/agent","name":"Remote"}
            }),
        )
        .unwrap();
    assert_eq!(host.sent.lock().unwrap().len(), 1);

    publisher
        .publish_entry(
            "room-a",
            &serde_json::json!({
                "id": "local-agent",
                "kind": "send-message",
                "message": {"type":"text","content":"local answer"},
                "author": {"id":"agent-local","name":"Local Agent"},
                "timestampMs": 2222
            }),
        )
        .unwrap();
    let sent = host.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["entry"]["kind"], "agent-message");
    assert_eq!(sent[1]["entry"]["agentOwnerAuthId"], "self-auth");
    assert_eq!(sent[1]["entry"]["agentId"], "agent-local");
    assert_eq!(sent[1]["entry"]["timestampMs"].as_f64(), Some(2222.0));
}

#[test]
fn entry_publisher_allows_image_only_human_messages_for_mirror_rooms() {
    let (host, publisher) = publisher_fixture();
    publisher
        .publish_entry(
            "room-image-only",
            &serde_json::json!({
                "id": "human-image-only",
                "kind": "message",
                "role": "user",
                "content": "",
                "images": [{"url":"attachment://ok","alt":"image only"}]
            }),
        )
        .unwrap();
    let sent = host.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["entry"]["text"], "");
    assert_eq!(sent[0]["entry"]["images"].as_array().unwrap().len(), 1);
}

#[test]
fn manager_delegate_serializes_room_publications_through_bound_publisher() {
    let (host, publisher) = publisher_fixture();
    let svc = service(serde_json::json!({}));
    svc.set_enabled(true);
    svc.bind_entry_publisher(Arc::clone(&publisher));

    let delegate = svc.build_manager_delegate();
    assert!(delegate.is_enabled());
    for (id, content) in [("delegate-1", "first"), ("delegate-2", "second")] {
        delegate
            .publish_room_entry(
                "room-delegate",
                &serde_json::json!({
                    "id": id,
                    "kind": "message",
                    "role": "user",
                    "content": content
                }),
            )
            .unwrap();
    }
    publisher.flush_room("room-delegate").unwrap();
    delegate
        .publish_room_entry_and_wait(
            "room-delegate",
            &serde_json::json!({
                "id": "delegate-3",
                "kind": "message",
                "role": "user",
                "content": "third"
            }),
        )
        .unwrap();

    publisher
        .publish_entry_ordered_and_wait(
            "room-delegate",
            &serde_json::json!({
                "id": "delegate-4",
                "kind": "message",
                "role": "user",
                "content": "fourth"
            }),
        )
        .unwrap();

    let sent = host.sent.lock().unwrap();
    assert_eq!(sent.len(), 4);
    assert_eq!(sent[0]["entry"]["entryId"], "delegate-1");
    assert_eq!(sent[1]["entry"]["entryId"], "delegate-2");
    assert_eq!(sent[2]["entry"]["entryId"], "delegate-3");
    assert_eq!(sent[3]["entry"]["entryId"], "delegate-4");
    drop(sent);

    svc.set_enabled(false);
    assert!(!delegate.is_enabled());
    delegate
        .publish_room_entry(
            "room-delegate",
            &serde_json::json!({
                "id": "disabled",
                "kind": "message",
                "role": "user",
                "content": "must not publish"
            }),
        )
        .unwrap();
    publisher.flush_room("room-delegate").unwrap();
    assert_eq!(host.sent.lock().unwrap().len(), 4);
}

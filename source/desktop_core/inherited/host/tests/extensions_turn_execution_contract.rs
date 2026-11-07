use std::collections::BTreeMap;
use std::future::{ready, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use mahayana_host_runtime::extensions::registry::{
    assemble_host_extension_registry, HostExtensionDeclaration, HostExtensionRegistryError,
    HOST_EXTENSION_ORDER,
};
use mahayana_host_runtime::extensions::transcript::transcript_manager::TranscriptTurnExecutionPort;
use mahayana_host_runtime::extensions::turn_execution::{
    turn_execution_extension, TurnExecutionError, TurnExecutionRegistry, TurnExecutor,
};
use serde_json::{json, Value};

struct FakeExecutor;
impl TurnExecutor for FakeExecutor {
    fn is_inference_ready(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(ready(true))
    }
    fn create_runner(&self, session: Value, hooks: Value) -> Value {
        json!({"session":session,"hooks":hooks,"kind":"runner"})
    }
    fn create_group_member_runner(&self, session: Value, hooks: Value, overrides: Value) -> Value {
        json!({"session":session,"hooks":hooks,"overrides":overrides,"kind":"group"})
    }
}

#[test]
fn extension_ids_and_order_match_frozen_grok_registry() {
    assert_eq!(HOST_EXTENSION_ORDER.len(), 36);
    assert_eq!(HOST_EXTENSION_ORDER[0], HostExtensionId::Notifications);
    assert_eq!(HOST_EXTENSION_ORDER[17], HostExtensionId::TurnExecution);
    assert_eq!(HOST_EXTENSION_ORDER[35], HostExtensionId::Wallpaper);
    assert_eq!(HostExtensionId::parse("mcp"), Some(HostExtensionId::Mcp));
    assert_eq!(HostExtensionId::WebauthnProxy.as_str(), "webauthn-proxy");
}

#[test]
fn registry_rejects_missing_slots_and_preserves_order() {
    let empty = assemble_host_extension_registry::<()>(BTreeMap::new()).unwrap_err();
    assert_eq!(empty, HostExtensionRegistryError::Missing("notifications"));

    let mut by_id = BTreeMap::new();
    for id in HOST_EXTENSION_ORDER {
        by_id.insert(*id, HostExtensionDeclaration { id: *id, value: id.as_str() });
    }
    let ordered = assemble_host_extension_registry(by_id).unwrap();
    assert_eq!(ordered.len(), HOST_EXTENSION_ORDER.len());
    assert_eq!(ordered[0].id, HostExtensionId::Notifications);
    assert_eq!(ordered.last().unwrap().id, HostExtensionId::Wallpaper);
}

#[test]
fn turn_execution_registry_enforces_single_executor_and_unbound_guard() {
    let (id, mut registry) = turn_execution_extension();
    assert_eq!(id, HostExtensionId::TurnExecution);
    assert!(!registry.can_execute());
    assert!(matches!(
        registry.create_runner(json!({}), json!({})),
        Err(TurnExecutionError::Unbound)
    ));
    registry.bind_executor(Box::new(FakeExecutor)).unwrap();
    assert!(registry.can_execute());
    assert_eq!(
        registry.create_runner(json!({"id":1}), json!({"h":2})).unwrap()["kind"],
        "runner"
    );
    assert!(matches!(
        registry.bind_executor(Box::new(FakeExecutor)),
        Err(TurnExecutionError::DoubleBind)
    ));
}


#[test]
fn transcript_turn_execution_port_is_cloneable_send_sync_and_forwards_frozen_capabilities() {
    let (_, registry) = turn_execution_extension();
    let registry = Arc::new(Mutex::new(registry));
    let port = TranscriptTurnExecutionPort::new(Arc::clone(&registry));

    assert!(!port.can_execute());
    assert!(!port.can_execute_group_member());
    assert!(!futures::executor::block_on(port.is_run_ready()));

    registry
        .lock()
        .expect("turn execution registry")
        .bind_executor(Box::new(FakeExecutor))
        .expect("bind executor");

    assert!(port.can_execute());
    assert!(port.can_execute_group_member());

    let cloned = port.clone();
    let (ready, runner, group) = std::thread::spawn(move || {
        let ready = futures::executor::block_on(cloned.is_run_ready());
        let runner = cloned
            .create_runner(json!({"id":"session"}), json!({"hook":"main"}))
            .expect("runner");
        let group = cloned
            .create_group_member_runner(
                json!({"id":"member"}),
                json!({"hook":"group"}),
                json!({"model":"override"}),
            )
            .expect("group runner");
        (ready, runner, group)
    })
    .join()
    .expect("send-safe execution port thread");

    assert!(ready);
    assert_eq!(runner["kind"], "runner");
    assert_eq!(runner["session"]["id"], "session");
    assert_eq!(group["kind"], "group");
    assert_eq!(group["overrides"]["model"], "override");
}

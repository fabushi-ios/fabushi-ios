use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use mahayana_host_runtime::mcp_auth::mcp_auth_wait_registry::{
    McpAuthCompletionIdentity,McpAuthWaitEvent,McpAuthWaitRegistry,normalize_connector_name,
};

#[test]
fn wait_registry_prefers_server_id_then_consumes_name_fallback() {
    assert_eq!(normalize_connector_name(" GitHub / Cloud "),"githubcloud");
    let now=Arc::new(AtomicU64::new(1_000)); let clock=Arc::clone(&now);
    let mut waits=McpAuthWaitRegistry::new(100,Box::new(move||clock.load(Ordering::SeqCst)));
    waits.register(McpAuthWaitEvent{agent_id:"name".into(),connector:"GitHub".into(),server_id:None});
    waits.register(McpAuthWaitEvent{agent_id:"id".into(),connector:"GitHub Work".into(),server_id:Some("srv-1".into())});
    assert_eq!(waits.take(&McpAuthCompletionIdentity{server_id:"srv-1".into(),server_name:"GitHub".into()}),Some("id".into()));
    assert!(waits.is_empty());
}

#[test]
fn wait_registry_prunes_expired_and_supports_id_only_key() {
    let now=Arc::new(AtomicU64::new(5)); let clock=Arc::clone(&now);
    let mut waits=McpAuthWaitRegistry::new(10,Box::new(move||clock.load(Ordering::SeqCst)));
    waits.register(McpAuthWaitEvent{agent_id:"agent".into(),connector:"!!!".into(),server_id:Some("srv".into())});
    assert_eq!(waits.len(),1);
    now.store(15,Ordering::SeqCst);
    waits.prune();
    assert!(waits.is_empty());
}

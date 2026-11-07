use std::collections::BTreeMap;

use mahayana_node_agent_coordinator::gateway::gateway_client::CoordinatorGatewayClient;
use mahayana_node_agent_coordinator::gateway::host_supervisor::GatewayConnection;

#[test]
fn coordinator_status_dispatch_uses_the_registered_gateway_method() {
    let mut client = CoordinatorGatewayClient::default();
    client
        .install_connection(GatewayConnection {
            base_url: "http://127.0.0.1:1234".into(),
            headers: BTreeMap::new(),
        })
        .unwrap();
    client.start(1).unwrap();

    let projection = client
        .dispatch_cloud_worker_status(|connection, method, args| {
            assert_eq!(connection.base_url, "http://127.0.0.1:1234");
            assert_eq!(method, "cloudWorkers.status");
            assert_eq!(args, serde_json::json!({}));
            Ok(serde_json::json!({
                "owner": "cloud-workers",
                "status": "disabled",
                "providerReady": false,
                "productionMutation": false
            }))
        })
        .unwrap();

    assert!(projection.is_disabled());
    assert!(!projection.provider_ready);
}

#[test]
fn coordinator_capabilities_dispatch_preserves_metadata_only_boundary() {
    let mut client = CoordinatorGatewayClient::default();
    client
        .install_connection(GatewayConnection {
            base_url: "http://127.0.0.1:1234".into(),
            headers: BTreeMap::new(),
        })
        .unwrap();
    client.start(1).unwrap();

    let projection = client
        .dispatch_cloud_worker_capabilities(|connection, method, args| {
            assert_eq!(connection.base_url, "http://127.0.0.1:1234");
            assert_eq!(method, "cloudWorkers.capabilities");
            assert_eq!(args, serde_json::json!({}));
            Ok(serde_json::json!({
                "owner": "cloud-workers",
                "metadataOnly": true,
                "providers": [{"providerId": "crabbox", "pluginId": "crabbox"}]
            }))
        })
        .unwrap();

    assert!(projection.metadata_only);
    assert_eq!(projection.providers[0].provider_id, "crabbox");
}

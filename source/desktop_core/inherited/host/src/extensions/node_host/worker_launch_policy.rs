//! Node-worker launch-mode admission without process side effects.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeWorkerLaunchMode {
    LocalProcess,
    Container,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeWorkerLaunchPolicyInput {
    pub container_requested: bool,
    pub runtime_local_inference: bool,
    pub native_inference_snapshot_present: bool,
    pub websocket_endpoint: bool,
    pub gateway_is_loopback: bool,
    pub browser_assignment_present: bool,
    pub container_lifecycle_present: bool,
}

pub fn admit_node_worker_launch(
    input: NodeWorkerLaunchPolicyInput,
) -> Result<NodeWorkerLaunchMode, String> {
    if input.runtime_local_inference && input.container_requested {
        return Err(
            "Worker-local inference requires non-container worker isolation".to_string(),
        );
    }
    if input.runtime_local_inference && !input.native_inference_snapshot_present {
        return Err("Worker-local inference startup custody is unavailable".into());
    }
    if !input.container_requested {
        return Ok(NodeWorkerLaunchMode::LocalProcess);
    }
    if !input.websocket_endpoint {
        return Err("container-isolated workers require a WebSocket Gateway endpoint".into());
    }
    if input.gateway_is_loopback {
        return Err("container-isolated workers cannot use a loopback Gateway endpoint".into());
    }
    if input.browser_assignment_present {
        return Err("container-isolated workers cannot use a host browser assignment".into());
    }
    if !input.container_lifecycle_present {
        return Err("container-isolated workers have no lifecycle owner".into());
    }
    Ok(NodeWorkerLaunchMode::Container)
}

#[cfg(test)]
mod tests {
    use super::{admit_node_worker_launch, NodeWorkerLaunchMode, NodeWorkerLaunchPolicyInput};

    fn local() -> NodeWorkerLaunchPolicyInput {
        NodeWorkerLaunchPolicyInput {
            container_requested: false,
            runtime_local_inference: false,
            native_inference_snapshot_present: false,
            websocket_endpoint: false,
            gateway_is_loopback: false,
            browser_assignment_present: false,
            container_lifecycle_present: false,
        }
    }

    #[test]
    fn local_launch_is_admitted_without_container_requirements() {
        assert_eq!(admit_node_worker_launch(local()), Ok(NodeWorkerLaunchMode::LocalProcess));
    }

    #[test]
    fn container_launch_rejects_loopback_or_missing_lifecycle() {
        let mut input = local();
        input.container_requested = true;
        input.websocket_endpoint = true;
        input.gateway_is_loopback = true;
        assert!(admit_node_worker_launch(input).is_err());
        input.gateway_is_loopback = false;
        assert!(admit_node_worker_launch(input).is_err());
        input.container_lifecycle_present = true;
        assert_eq!(admit_node_worker_launch(input), Ok(NodeWorkerLaunchMode::Container));
    }
}

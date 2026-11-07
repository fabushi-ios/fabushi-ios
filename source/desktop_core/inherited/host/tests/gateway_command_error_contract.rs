use std::io;

use mahayana_host_runtime::gateway_command_error::{
    GatewayCommandError, classify_gateway_command_error,
};
use mahayana_host_runtime::ports::r#box::{
    SandBoxDaemonUnreachableError, SandBoxNoMonitorAvailableError,
};

#[test]
fn gateway_error_classifier_preserves_grok_box_and_network_reasons() {
    let refused = SandBoxDaemonUnreachableError::new("refused", "exec daemon unavailable");
    let classified = classify_gateway_command_error(&refused);
    assert_eq!(classified.reason, "daemon_refused");
    assert_eq!(classified.error_class, "SandBoxDaemonUnreachableError");

    let timeout = SandBoxDaemonUnreachableError::new("timeout", "exec daemon timed out");
    assert_eq!(
        classify_gateway_command_error(&timeout).reason,
        "daemon_timeout"
    );

    let crash = SandBoxDaemonUnreachableError::new("crash", "exec daemon exited");
    assert_eq!(
        classify_gateway_command_error(&crash).reason,
        "daemon_crash"
    );

    let no_monitor = SandBoxNoMonitorAvailableError::default();
    let classified = classify_gateway_command_error(&no_monitor);
    assert_eq!(classified.reason, "no_monitor");
    assert_eq!(classified.error_class, "SandBoxNoMonitorAvailableError");

    let refused_io = io::Error::new(io::ErrorKind::ConnectionRefused, "connect failed");
    let classified = classify_gateway_command_error(&refused_io);
    assert_eq!(classified.reason, "refused");
    assert_eq!(classified.errno.as_deref(), Some("ECONNREFUSED"));

    let timeout_io = io::Error::new(io::ErrorKind::TimedOut, "operation timed out");
    let classified = classify_gateway_command_error(&timeout_io);
    assert_eq!(classified.reason, "timeout");
    assert_eq!(classified.errno.as_deref(), Some("ETIMEDOUT"));

    let reset_io = io::Error::new(io::ErrorKind::ConnectionReset, "connection reset");
    let classified = classify_gateway_command_error(&reset_io);
    assert_eq!(classified.reason, "network");
    assert_eq!(classified.errno.as_deref(), Some("ECONNRESET"));

    let dns = GatewayCommandError::Internal("resolver failed: EAI_AGAIN".into());
    let classified = classify_gateway_command_error(&dns);
    assert_eq!(classified.reason, "dns");
    assert_eq!(classified.errno.as_deref(), Some("EAI_AGAIN"));

    let application = GatewayCommandError::Internal("provider rejected request".into());
    let classified = classify_gateway_command_error(&application);
    assert_eq!(classified.reason, "application");
    assert_eq!(classified.error_class, "GatewayCommandError");
    assert_eq!(classified.errno, None);
}

#[test]
fn gateway_command_error_keeps_http_status_contract() {
    assert_eq!(GatewayCommandError::UnknownMethod("x".into()).status(), 404);
    assert_eq!(GatewayCommandError::BadRequest("x".into()).status(), 400);
    assert_eq!(GatewayCommandError::Unauthorized("x".into()).status(), 401);
    assert_eq!(GatewayCommandError::Forbidden("x".into()).status(), 403);
    assert_eq!(GatewayCommandError::Conflict("x".into()).status(), 409);
    assert_eq!(GatewayCommandError::Internal("x".into()).status(), 500);
}

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, RecvTimeoutError},
};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::webauthn::{
    ApprovedWebAuthnConsent, WebAuthnCeremony, WebAuthnSigner, WebAuthnSignerError,
    WebAuthnSignerResult,
};

pub const SIGNER_EVENT_PREFIX: &str = "[signer-event] ";

#[derive(Debug, Clone, Default)]
pub struct WebAuthnSignCancellation {
    cancelled: Arc<AtomicBool>,
}

impl WebAuthnSignCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAuthnPinRequest {
    pub invalid: bool,
    pub retries: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SignerEvent {
    PresenceRequired,
    SelectDevice,
    PinNotSet,
    PinBlocked,
    UvBlocked,
    UvInvalid,
    PinRequired,
    PinInvalid {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retries: Option<u32>,
    },
}

pub fn describe_signer_event_as_status(event: &SignerEvent) -> Option<&'static str> {
    match event {
        SignerEvent::PresenceRequired => Some("Touch your security key now"),
        SignerEvent::SelectDevice => Some("Touch the security key you want to use"),
        SignerEvent::PinNotSet => Some("This security key has no PIN set, but the site asked for one"),
        SignerEvent::PinBlocked => Some("Your security key is locked"),
        SignerEvent::UvBlocked => Some("Your security key's fingerprint check is locked"),
        SignerEvent::UvInvalid => Some("That didn't match — try again on the key"),
        SignerEvent::PinRequired | SignerEvent::PinInvalid { .. } => None,
    }
}

pub fn parse_signer_event_line(line: &str) -> Option<SignerEvent> {
    let body = line.strip_prefix(SIGNER_EVENT_PREFIX)?;
    serde_json::from_str(body).ok()
}

pub fn signer_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "sand-webauthn-signer.exe"
    } else {
        "sand-webauthn-signer"
    }
}

pub fn resolve_web_authn_signer_path(
    is_packaged: bool,
    resources_path: &Path,
    repo_root: &Path,
    override_path: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return path.is_file().then(|| path.to_path_buf());
    }

    let candidates = if is_packaged {
        vec![
            resources_path
                .join("app.asar.unpacked")
                .join("dist")
                .join("native")
                .join(signer_binary_name()),
        ]
    } else {
        vec![
            repo_root.join("target").join("release").join(signer_binary_name()),
            repo_root.join("target").join("debug").join(signer_binary_name()),
        ]
    };
    candidates.into_iter().find(|path| path.is_file())
}

#[derive(Debug, Clone)]
pub struct SpawnedWebAuthnSigner {
    pub binary_path: PathBuf,
}

impl SpawnedWebAuthnSigner {
    fn failure(
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> WebAuthnSignerResult {
        WebAuthnSignerResult::Failed {
            error: WebAuthnSignerError {
                name: "NotAllowedError".into(),
                code: Some(code.into()),
                message: message.into(),
            },
        }
    }
}


impl SpawnedWebAuthnSigner {
    pub fn sign_interactive<Status, Pin>(
        &self,
        ceremony: &WebAuthnCeremony,
        approved: Option<&ApprovedWebAuthnConsent>,
        cancellation: &WebAuthnSignCancellation,
        mut on_status: Status,
        mut on_pin_request: Pin,
    ) -> WebAuthnSignerResult
    where
        Status: FnMut(&str),
        Pin: FnMut(WebAuthnPinRequest, &str) -> Option<String>,
    {
        #[derive(Debug)]
        enum ProcessEvent {
            Stdout(Vec<u8>),
            Signer(SignerEvent),
        }

        let mut child = match Command::new(&self.binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return Self::failure(
                    "helper_spawn_failed",
                    format!("could not start the security key helper: {error}"),
                );
            }
        };

        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.kill();
            return Self::failure(
                "helper_spawn_failed",
                "security key helper stdin is unavailable",
            );
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Self::failure(
                "helper_spawn_failed",
                "security key helper stdout is unavailable",
            );
        };
        let Some(stderr) = child.stderr.take() else {
            let _ = child.kill();
            return Self::failure(
                "helper_spawn_failed",
                "security key helper stderr is unavailable",
            );
        };

        let mut request = serde_json::to_value(ceremony).unwrap_or_else(|_| json!({}));
        if let Some(window_handle) = approved.and_then(|value| value.window_handle) {
            if let Some(object) = request.as_object_mut() {
                object.insert("windowHandle".into(), Value::from(window_handle));
            }
        }
        if writeln!(stdin, "{request}")
            .and_then(|_| stdin.flush())
            .is_err()
        {
            let _ = child.kill();
            return Self::failure(
                "helper_io_failed",
                "could not write the security key request",
            );
        }

        let (events_tx, events_rx) = mpsc::channel::<ProcessEvent>();
        let stdout_tx = events_tx.clone();
        let stdout_thread = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = BufReader::new(stdout).read_to_end(&mut bytes);
            let _ = stdout_tx.send(ProcessEvent::Stdout(bytes));
        });
        let stderr_tx = events_tx.clone();
        let stderr_thread = thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(event) = parse_signer_event_line(line.trim()) {
                    let _ = stderr_tx.send(ProcessEvent::Signer(event));
                }
            }
        });
        drop(events_tx);

        let mut stdout_bytes: Option<Vec<u8>> = None;
        let process_event = |event: ProcessEvent,
                             stdin: &mut std::process::ChildStdin,
                             stdout_bytes: &mut Option<Vec<u8>>,
                             on_status: &mut Status,
                             on_pin_request: &mut Pin|
         -> Result<(), WebAuthnSignerResult> {
            match event {
                ProcessEvent::Stdout(bytes) => {
                    *stdout_bytes = Some(bytes);
                    Ok(())
                }
                ProcessEvent::Signer(event) => {
                    let request = match event {
                        SignerEvent::PinRequired => Some(WebAuthnPinRequest {
                            invalid: false,
                            retries: None,
                        }),
                        SignerEvent::PinInvalid { retries } => Some(WebAuthnPinRequest {
                            invalid: true,
                            retries,
                        }),
                        other => {
                            if let Some(status) = describe_signer_event_as_status(&other) {
                                on_status(status);
                            }
                            None
                        }
                    };
                    let Some(request) = request else {
                        return Ok(());
                    };
                    let pin = approved
                        .and_then(|value| value.prompt_id.as_deref())
                        .and_then(|prompt_id| on_pin_request(request, prompt_id))
                        .filter(|pin| !pin.is_empty());
                    let reply = match pin {
                        Some(pin) => json!({ "kind": "pin", "pin": pin }),
                        None => json!({ "kind": "cancel" }),
                    };
                    writeln!(stdin, "{reply}")
                        .and_then(|_| stdin.flush())
                        .map_err(|error| {
                            Self::failure(
                                "helper_io_failed",
                                format!("could not answer security key helper: {error}"),
                            )
                        })
                }
            }
        };

        let exit_status = loop {
            if cancellation.is_cancelled() {
                let _ = writeln!(stdin, "{}", json!({ "kind": "cancel" }));
                let _ = stdin.flush();
                let _ = child.kill();
                let _ = child.wait();
                drop(stdin);
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Self::failure(
                    "cancelled_or_timeout",
                    "the security key request was cancelled",
                );
            }

            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(stdin);
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Self::failure(
                        "helper_wait_failed",
                        format!("security key helper wait failed: {error}"),
                    );
                }
            }

            match events_rx.recv_timeout(Duration::from_millis(20)) {
                Ok(event) => {
                    if let Err(result) = process_event(
                        event,
                        &mut stdin,
                        &mut stdout_bytes,
                        &mut on_status,
                        &mut on_pin_request,
                    ) {
                        let _ = child.kill();
                        let _ = child.wait();
                        drop(stdin);
                        let _ = stdout_thread.join();
                        let _ = stderr_thread.join();
                        return result;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {}
            }
        };

        drop(stdin);
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        while let Ok(event) = events_rx.try_recv() {
            if let ProcessEvent::Stdout(bytes) = event {
                stdout_bytes = Some(bytes);
            }
        }

        let output = stdout_bytes.unwrap_or_default();
        if output.is_empty() {
            return Self::failure(
                "helper_no_result",
                format!(
                    "the security key helper exited with code {:?} and no result",
                    exit_status.code()
                ),
            );
        }
        let value = match serde_json::from_slice::<Value>(&output) {
            Ok(value) => value,
            Err(error) => {
                return Self::failure(
                    "helper_no_result",
                    format!("unreadable security key helper result: {error}"),
                );
            }
        };
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            return WebAuthnSignerResult::Success {
                credential_json: value
                    .get("credentialJson")
                    .cloned()
                    .unwrap_or(Value::Null),
            };
        }
        let error = value
            .get("error")
            .cloned()
            .and_then(|value| serde_json::from_value::<WebAuthnSignerError>(value).ok())
            .unwrap_or_else(|| WebAuthnSignerError {
                name: "NotAllowedError".into(),
                code: Some("helper_failed".into()),
                message: "the security key helper reported a failure".into(),
            });
        WebAuthnSignerResult::Failed { error }
    }
}

impl WebAuthnSigner for SpawnedWebAuthnSigner {
    fn sign(
        &mut self,
        ceremony: &WebAuthnCeremony,
        approved: Option<&ApprovedWebAuthnConsent>,
    ) -> WebAuthnSignerResult {
        let mut child = match Command::new(&self.binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return Self::failure(
                    "helper_spawn_failed",
                    format!("could not start the security key helper: {error}"),
                );
            }
        };

        let mut request = serde_json::to_value(ceremony).unwrap_or_else(|_| json!({}));
        if let Some(window_handle) = approved.and_then(|value| value.window_handle) {
            if let Some(object) = request.as_object_mut() {
                object.insert("windowHandle".into(), Value::from(window_handle));
            }
        }

        let Some(stdin) = child.stdin.as_mut() else {
            return Self::failure(
                "helper_spawn_failed",
                "security key helper stdin is unavailable",
            );
        };
        if writeln!(stdin, "{}", request).is_err() {
            return Self::failure(
                "helper_io_failed",
                "could not write the security key request",
            );
        }
        drop(child.stdin.take());

        let output = match child.wait_with_output() {
            Ok(output) => output,
            Err(error) => {
                return Self::failure(
                    "helper_wait_failed",
                    format!("security key helper wait failed: {error}"),
                );
            }
        };
        if output.stdout.is_empty() {
            return Self::failure(
                "helper_no_result",
                format!(
                    "the security key helper exited with code {:?} and no result",
                    output.status.code()
                ),
            );
        }

        let value = match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(value) => value,
            Err(error) => {
                return Self::failure(
                    "helper_no_result",
                    format!("unreadable security key helper result: {error}"),
                );
            }
        };
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            return WebAuthnSignerResult::Success {
                credential_json: value
                    .get("credentialJson")
                    .cloned()
                    .unwrap_or(Value::Null),
            };
        }

        let error = value
            .get("error")
            .cloned()
            .and_then(|value| serde_json::from_value::<WebAuthnSignerError>(value).ok())
            .unwrap_or_else(|| WebAuthnSignerError {
                name: "NotAllowedError".into(),
                code: Some("helper_failed".into()),
                message: "the security key helper reported a failure".into(),
            });
        WebAuthnSignerResult::Failed { error }
    }
}

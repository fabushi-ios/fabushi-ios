#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Duration;

    use mahayana_node_agent_coordinator::webauthn::signer::{
        SpawnedWebAuthnSigner, WebAuthnPinRequest, WebAuthnSignCancellation,
    };
    use mahayana_node_agent_coordinator::webauthn::{
        ApprovedWebAuthnConsent, WebAuthnCeremony, WebAuthnSignerResult,
    };
    use serde_json::{Value, json};
    use uuid::Uuid;

    fn write_script(body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-webauthn-helper-{}.sh",
            Uuid::new_v4()
        ));
        fs::write(&path, body).expect("write helper script");
        let mut permissions = fs::metadata(&path).expect("helper metadata").permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).expect("make helper executable");
        path
    }

    fn shell_path(path: &Path) -> String {
        path.to_string_lossy().replace('"', "\\\"")
    }

    #[test]
    fn interactive_signer_answers_pin_retry_and_preserves_status_events() {
        let reply_path = std::env::temp_dir().join(format!(
            "fabushi-webauthn-replies-{}.jsonl",
            Uuid::new_v4()
        ));
        let script = format!(
            "#!/bin/sh\n\
             IFS= read -r request\n\
             printf '%s\\n' '[signer-event] {{\"kind\":\"presence-required\"}}' >&2\n\
             printf '%s\\n' '[signer-event] {{\"kind\":\"pin-required\"}}' >&2\n\
             IFS= read -r first\n\
             printf '%s\\n' \"$first\" >> \"{}\"\n\
             printf '%s\\n' '[signer-event] {{\"kind\":\"pin-invalid\",\"retries\":2}}' >&2\n\
             IFS= read -r second\n\
             printf '%s\\n' \"$second\" >> \"{}\"\n\
             printf '%s' '{{\"ok\":true,\"credentialJson\":{{\"id\":\"credential-1\"}}}}'\n",
            shell_path(&reply_path),
            shell_path(&reply_path),
        );
        let script_path = write_script(&script);
        let signer = SpawnedWebAuthnSigner {
            binary_path: script_path.clone(),
        };
        let ceremony = WebAuthnCeremony {
            kind: "get".into(),
            origin: "https://example.test".into(),
            payload: json!({"challenge":"abc"}),
        };
        let approved = ApprovedWebAuthnConsent {
            approved: true,
            prompt_id: Some("prompt-1".into()),
            window_handle: None,
        };
        let cancellation = WebAuthnSignCancellation::default();
        let mut statuses = Vec::<String>::new();
        let mut prompts = Vec::<WebAuthnPinRequest>::new();
        let result = signer.sign_interactive(
            &ceremony,
            Some(&approved),
            &cancellation,
            |status| statuses.push(status.to_string()),
            |request, prompt_id| {
                assert_eq!(prompt_id, "prompt-1");
                let pin = if prompts.is_empty() { "1111" } else { "2222" };
                prompts.push(request);
                Some(pin.into())
            },
        );

        assert!(matches!(
            result,
            WebAuthnSignerResult::Success { credential_json }
                if credential_json["id"] == "credential-1"
        ));
        assert_eq!(statuses, vec!["Touch your security key now"]);
        assert_eq!(
            prompts,
            vec![
                WebAuthnPinRequest {
                    invalid: false,
                    retries: None,
                },
                WebAuthnPinRequest {
                    invalid: true,
                    retries: Some(2),
                },
            ]
        );

        let replies = fs::read_to_string(&reply_path).expect("read helper replies");
        let replies = replies
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("reply json"))
            .collect::<Vec<_>>();
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["kind"], "pin");
        assert_eq!(replies[0]["pin"], "1111");
        assert_eq!(replies[1]["kind"], "pin");
        assert_eq!(replies[1]["pin"], "2222");

        let _ = fs::remove_file(script_path);
        let _ = fs::remove_file(reply_path);
    }


    #[test]
    fn interactive_signer_preserves_numeric_native_window_handle() {
        let request_path = std::env::temp_dir().join(format!(
            "fabushi-webauthn-window-handle-{}.json",
            Uuid::new_v4()
        ));
        let script = format!(
            "#!/bin/sh\n\
             IFS= read -r request\n\
             printf '%s' \"$request\" > \"{}\"\n\
             printf '%s' '{{\"ok\":true,\"credentialJson\":{{\"id\":\"credential-window\"}}}}'\n",
            shell_path(&request_path),
        );
        let script_path = write_script(&script);
        let signer = SpawnedWebAuthnSigner {
            binary_path: script_path.clone(),
        };
        let ceremony = WebAuthnCeremony {
            kind: "get".into(),
            origin: "https://example.test".into(),
            payload: json!({"challenge":"window-handle"}),
        };
        let approved = ApprovedWebAuthnConsent {
            approved: true,
            prompt_id: Some("prompt-window".into()),
            window_handle: Some(4_294_967_297),
        };
        let cancellation = WebAuthnSignCancellation::default();
        let result = signer.sign_interactive(
            &ceremony,
            Some(&approved),
            &cancellation,
            |_| {},
            |_, _| None,
        );
        assert!(matches!(
            result,
            WebAuthnSignerResult::Success { credential_json }
                if credential_json["id"] == "credential-window"
        ));

        let request: Value = serde_json::from_str(
            &fs::read_to_string(&request_path).expect("read signer request"),
        )
        .expect("signer request json");
        assert_eq!(request["windowHandle"].as_u64(), Some(4_294_967_297));

        let _ = fs::remove_file(script_path);
        let _ = fs::remove_file(request_path);
    }

    #[test]
    fn interactive_signer_cancel_terminates_an_inflight_helper() {
        let script_path = write_script(
            "#!/bin/sh\n\
             IFS= read -r request\n\
             IFS= read -r cancellation\n\
             printf '%s' '{\"ok\":true,\"credentialJson\":{\"unexpected\":true}}'\n",
        );
        let signer = SpawnedWebAuthnSigner {
            binary_path: script_path.clone(),
        };
        let ceremony = WebAuthnCeremony {
            kind: "get".into(),
            origin: "https://example.test".into(),
            payload: json!({"challenge":"cancel"}),
        };
        let cancellation = WebAuthnSignCancellation::default();
        let cancellation_for_signer = cancellation.clone();
        let worker = thread::spawn(move || {
            signer.sign_interactive(
                &ceremony,
                None,
                &cancellation_for_signer,
                |_| {},
                |_, _| None,
            )
        });

        thread::sleep(Duration::from_millis(80));
        cancellation.cancel();
        let result = worker.join().expect("signer worker joins");
        assert!(matches!(
            result,
            WebAuthnSignerResult::Failed { error }
                if error.code.as_deref() == Some("cancelled_or_timeout")
        ));

        let _ = fs::remove_file(script_path);
    }
}

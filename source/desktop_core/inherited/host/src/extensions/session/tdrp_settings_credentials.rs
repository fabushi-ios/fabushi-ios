//! Telegram / TDRP Settings, Native Credential State, Legacy Stack Retirement, and Settlement Tests.

use super::*;

impl ProductionSessionWorkers {
    pub fn set_has_valid_native_credential(&self, valid: bool) {
        self.has_valid_native_credential.store(valid, Ordering::SeqCst);
    }

    pub fn has_valid_native_credential(&self) -> bool {
        self.has_valid_native_credential.load(Ordering::SeqCst)
    }

    pub fn with_valid_native_credential(self, valid: bool) -> Self {
        self.set_has_valid_native_credential(valid);
        self
    }

    pub fn set_account_data_root(&self, path: impl Into<PathBuf>) {
        *self.account_data_root.lock().expect("account_data_root lock") = Some(path.into());
    }

    pub fn account_data_root(&self) -> PathBuf {
        if let Some(ref path) = *self.account_data_root.lock().expect("account_data_root lock") {
            return path.clone();
        }
        if self.agents_root.join("app-host/feature-host").exists() {
            return self.agents_root.clone();
        }
        if let Some(parent) = self.agents_root.parent() {
            if parent.join("app-host/feature-host").exists() {
                return parent.to_path_buf();
            }
        }
        if self.agents_root.file_name().and_then(|s| s.to_str()) == Some("agents") {
            return self.agents_root.parent().map(Path::to_path_buf).unwrap_or_else(|| self.agents_root.clone());
        }
        self.agents_root.clone()
    }

    pub fn set_telegram_key_clear_callback<F>(&self, callback: F)
    where
        F: Fn() -> Result<(), String> + Send + Sync + 'static,
    {
        *self.telegram_key_clear_callback.lock().expect("telegram_key_clear_callback lock") =
            Some(Arc::new(callback));
    }

    pub fn retire_legacy_telegram_data(
        &self,
        remove_immediately: bool,
    ) -> Result<LegacyTelegramRetirementReport, LegacyTelegramRetirementError> {
        let account_root = self.account_data_root();
        let has_credential = self.has_valid_native_credential();
        let callback = self.telegram_key_clear_callback.lock().expect("callback lock").clone();
        retire_legacy_telegram_data_internal(
            &account_root,
            has_credential,
            remove_immediately,
            callback.as_deref().map(|c| c as &dyn Fn() -> Result<(), String>),
        )
    }

    pub fn session_db_path(&self, agent_id: &str) -> Result<PathBuf, String> {
        get_agent_db_path(&self.agents_root, agent_id).map_err(|error| error.to_string())
    }

    pub fn list_agent_record_ids(&self) -> Result<Vec<String>, String> {
        list_agent_record_ids(&self.agents_root).map_err(|error| error.to_string())
    }

    pub fn count_owned_agents(&self) -> Result<usize, String> {
        count_owned_agents(&self.agents_root).map_err(|error| error.to_string())
    }

    pub fn is_agent_cap_reached(&self) -> Result<bool, String> {
        is_agent_cap_reached(&self.agents_root).map_err(|error| error.to_string())
    }

    pub fn reclaim_pruned_placeholders(
        &self,
        active_agent_id: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let visible_agent_ids = self
            .list_agent_summaries(active_agent_id)?
            .into_iter()
            .map(|summary| summary.id)
            .collect::<BTreeSet<_>>();
        let candidates = list_pruned_placeholder_ids(
            &self.agents_root,
            active_agent_id,
            &visible_agent_ids,
        )
        .map_err(|error| error.to_string())?;
        let mut reclaimed = Vec::new();
        for agent_id in candidates {
            let db_path = self.session_db_path(&agent_id)?;
            let blob_path = conversation_blobs_path(&db_path);
            let _ = self.close_agent_store_owner(&agent_id, false);
            let _ = self.close_agent_db_owner(&agent_id, false);
            futures::executor::block_on(self.pool.close_store(&blob_path));
            match fs::remove_dir_all(self.agents_root.join(&agent_id)) {
                Ok(()) => reclaimed.push(agent_id),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    report_materialization_failure(
                        "placeholder_reclaim_failed",
                        &agent_id,
                        &format!("io::{:?}", error.kind()),
                    );
                }
            }
        }
        Ok(reclaimed)
    }

}

#[cfg(test)]
mod human_send_settlement_credential_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn write_credential(path: &Path, issuer: &str, token: &str) {
        fs::write(path, serde_json::json!({
            "accessToken": token, "deviceId": "device-a", "userId": 1,
            "sessionId": "fixture-session", "issuer": issuer,
        }).to_string()).expect("write private settlement credential fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .expect("private settlement fixture permissions");
        }
    }

    fn remote_message() -> serde_json::Value {
        serde_json::json!({
            "id": 96, "senderUserId": 1, "recipientUserId": 2,
            "text": "post-lock lease", "clientRequestId": "post-lock-lease",
            "createdAt": "2026-10-05T01:00:36Z", "isOutgoing": true,
            "attachments": [], "reactions": [],
        })
    }

    fn successful_response() -> Result<FabushiRemoteHumanMessage, FabushiHumanSendError> {
        Ok(serde_json::from_value(remote_message()).expect("valid persisted message fixture"))
    }

    fn completed_http_fixture(disconnect: bool) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind settlement HTTP fixture");
        listener.set_nonblocking(true).expect("bounded settlement fixture accept");
        let address = listener.local_addr().expect("settlement fixture address");
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "settlement fixture accept timed out");
                        thread::yield_now();
                    }
                    Err(error) => panic!("settlement fixture accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).expect("blocking settlement fixture stream");
            stream.set_read_timeout(Some(Duration::from_secs(15)))
                .expect("bounded settlement request read");
            stream.set_write_timeout(Some(Duration::from_secs(15)))
                .expect("bounded settlement response write");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            let header_end = loop {
                let count = stream.read(&mut buffer).expect("read settlement request headers");
                assert!(count > 0, "settlement request closed before headers");
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() <= 64 * 1024, "settlement request exceeded fixture budget");
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = std::str::from_utf8(&request[..header_end]).expect("HTTP request headers");
            assert!(headers.starts_with("POST /api/social/messages "));
            let content_length = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("request body length"))
            }).expect("bounded request body declaration");
            assert!(content_length <= 64 * 1024, "settlement request body exceeded fixture budget");
            while request.len() < header_end + content_length {
                let count = stream.read(&mut buffer).expect("read settlement request body");
                assert!(count > 0, "settlement request closed before its body");
                request.extend_from_slice(&buffer[..count]);
            }
            let body: serde_json::Value = serde_json::from_slice(
                &request[header_end..header_end + content_length],
            ).expect("settlement request JSON");
            assert_eq!(body["clientRequestId"], "post-lock-lease");
            if disconnect { return; }
            let body = serde_json::json!({"success": true, "message": remote_message()}).to_string();
            write!(stream,
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body,
            ).expect("write completed settlement response");
            stream.flush().expect("flush completed settlement response");
        });
        (format!("http://{address}"), server)
    }

    #[test]
    fn shipping_human_settlement_credential_rotation_after_completed_http_is_rejected() {
        for disconnect in [false, true] {
            let root = std::env::temp_dir().join(format!("fabushi-settlement-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).expect("settlement fixture directory");
            let path = root.join("credential.json");
            let (issuer, server) = completed_http_fixture(disconnect);
            write_credential(&path, &issuer, "original-fixture-token");
            let client = FabushiNativeMessagingClient::new(&issuer, &path)
                .expect("real Native credential and HTTP owner");
            let fingerprint = client.request_credential_fingerprint().expect("dispatch fingerprint");
            let serial = Arc::new(Mutex::new(()));
            let competing_guard = serial.lock().expect("hold competing Session mutation");
            let (http_done, http_wait) = mpsc::channel();
            let (settled, settlement_wait) = mpsc::channel();
            let worker_serial = Arc::clone(&serial);
            let worker = thread::spawn(move || {
                let response = client.send_direct_message_for_settlement(
                    "2", "post-lock lease", "post-lock-lease", None, &[], "1",
                );
                if disconnect {
                    assert!(matches!(&response, Err(FabushiHumanSendError::Transport(_))),
                        "complete disconnect must be an ambiguous transport outcome");
                } else {
                    assert!(response.is_ok(), "complete HTTP body must be accepted before rotation");
                }
                // This signal follows the real Native send method, including
                // body consumption and its final credential stability check.
                http_done.send(()).expect("observe completed Native HTTP result");
                let result = ProductionSessionWorkers::acquire_human_send_settlement_guard(
                    &client, worker_serial.as_ref(), None, &fingerprint, &response,
                ).map(|_guard| ());
                settled.send(result).expect("observe post-lock settlement result");
            });
            http_wait.recv_timeout(Duration::from_secs(15))
                .expect("Native HTTP must complete while competing mutation holds the lock");
            // The lock is still held here. No scheduler delay or socket-write
            // observation is used to infer that the response was consumed.
            write_credential(&path, &issuer, "rotated-after-completed-http");
            drop(competing_guard);
            let result = settlement_wait.recv_timeout(Duration::from_secs(15))
                .expect("post-lock settlement must terminate");
            assert_eq!(result, Err(
                "Fabushi native messaging credential changed before Human settlement".to_string(),
            ));
            worker.join().expect("settlement worker");
            server.join().expect("settlement HTTP fixture");
            fs::remove_dir_all(root).expect("remove settlement fixture");
        }
    }

    #[test]
    fn shipping_human_settlement_credential_unchanged_retains_the_existing_lock() {
        let root = std::env::temp_dir().join(format!("fabushi-settlement-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("settlement fixture directory");
        let path = root.join("credential.json");
        let issuer = "http://127.0.0.1:4200";
        write_credential(&path, issuer, "unchanged-fixture-token");
        let client = FabushiNativeMessagingClient::new(issuer, &path).expect("Native credential owner");
        let fingerprint = client.request_credential_fingerprint().expect("dispatch fingerprint");
        let serial = Mutex::new(());
        for response in [successful_response(), Err(FabushiHumanSendError::Transport("disconnect".into()))] {
            let guard = ProductionSessionWorkers::acquire_human_send_settlement_guard(
                &client, &serial, None, &fingerprint, &response,
            ).expect("unchanged completed result can enter settlement");
            assert!(guard.is_some(), "ordinary send must acquire the mutation guard");
            assert!(matches!(serial.try_lock(), Err(std::sync::TryLockError::WouldBlock)),
                "returned guard must protect the caller's later durable settlement");
            drop(guard);
            let held = serial.lock().expect("sync already owns the mutation lane");
            let borrowed = ProductionSessionWorkers::acquire_human_send_settlement_guard(
                &client, &serial, Some(&held), &fingerprint, &response,
            ).expect("sync must reuse its existing guard without recursive acquisition");
            assert!(borrowed.is_none());
            assert!(matches!(serial.try_lock(), Err(std::sync::TryLockError::WouldBlock)));
            drop(held);
        }
        fs::remove_dir_all(root).expect("remove settlement fixture");
    }

    #[test]
    fn shipping_human_settlement_credential_rejection_preserves_the_original_refusal() {
        let root = std::env::temp_dir().join(format!("fabushi-settlement-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("settlement fixture directory");
        let path = root.join("credential.json");
        let issuer = "http://127.0.0.1:4200";
        let client = FabushiNativeMessagingClient::new(issuer, &path).expect("Native credential owner");
        let serial = Mutex::new(());
        for refusal in [
            "Fabushi native messaging credential changed during Human request",
            "Fabushi Human message send was rejected (HTTP 403): forbidden",
        ] {
            write_credential(&path, issuer, "original-fixture-token");
            let fingerprint = client.request_credential_fingerprint().expect("dispatch fingerprint");
            write_credential(&path, issuer, "later-fixture-token");
            let response = Err(FabushiHumanSendError::Rejected(refusal.into()));
            let error = ProductionSessionWorkers::acquire_human_send_settlement_guard(
                &client, &serial, None, &fingerprint, &response,
            ).expect_err("explicit rejection never enters successful settlement or transport recovery");
            assert_eq!(error, refusal);
            fs::remove_file(&path).expect("invalidate the later credential snapshot");
            let held = serial.lock().expect("sync mutation guard");
            let error = ProductionSessionWorkers::acquire_human_send_settlement_guard(
                &client, &serial, Some(&held), &fingerprint, &response,
            ).expect_err("later credential unavailability cannot replace the original rejection");
            assert_eq!(error, refusal);
        }
        fs::remove_dir_all(root).expect("remove settlement fixture");
    }
}



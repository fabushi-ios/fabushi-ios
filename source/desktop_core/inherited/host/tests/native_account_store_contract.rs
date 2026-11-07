use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use base64::Engine;
use mahayana_host_runtime::extensions::auth::auth_service::{HostAuthEnvironment, HostAuthService, HostAuthServiceOptions, FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV};
use mahayana_host_runtime::extensions::box_store_sync::native_account_store::{NativeAccountStoreClient, NativeFileError, NativeWritePrecondition, NativeWriteOutcome, NativeDeleteOutcome};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Account { root: PathBuf, path: PathBuf, auth: Arc<HostAuthService> }
fn write_account(path: &PathBuf, issuer: &str, user: &str) {
    fs::write(path, json!({"version":1,"issuer":issuer,"accessToken":"native-file-fixture-token","expiresAtMs":600000,
        "sessionId":format!("session-{user}"),"deviceId":"file-device","userId":user}).to_string()).unwrap();
}
impl Account {
    fn new(issuer: &str) -> Self {
        let root = std::env::temp_dir().join(format!("native-file-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap(); let path = root.join("credential.json"); write_account(&path, issuer, "a");
        let auth = Arc::new(HostAuthService::new(HostAuthServiceOptions {
            environment: HostAuthEnvironment::from_pairs(BTreeMap::from([(FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV.into(), path.to_string_lossy().into_owned())])),
            backend_url: Some(issuer.into()), backend: None, machine_id: Arc::new(||Ok("file-device".into())),
            log: Arc::new(|_|{}), now_ms: Arc::new(||10_000),
        }).unwrap());
        Self { root, path, auth }
    }
}
impl Drop for Account { fn drop(&mut self) { self.auth.dispose(); let _ = fs::remove_dir_all(&self.root); } }
fn read_request(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut bytes = Vec::new();
    loop {
        let mut buf = [0;1024]; let count = stream.read(&mut buf).unwrap(); assert!(count > 0); bytes.extend_from_slice(&buf[..count]);
        assert!(bytes.len() < 65536);
        if let Some(i) = bytes.windows(4).position(|v|v==b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..i]).to_ascii_lowercase();
            let length = headers.lines().find_map(|v| v.strip_prefix("content-length:").map(|n|n.trim().parse::<usize>().unwrap())).unwrap_or(0);
            if bytes.len() >= i + 4 + length { return String::from_utf8(bytes).unwrap(); }
        }
    }
}
fn fixture<T>(status: u16, value: Value, call: impl FnOnce(NativeAccountStoreClient) -> T) -> (T, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let base = format!("http://{}", listener.local_addr().unwrap());
    let account = Account::new(&base); let client = NativeAccountStoreClient::for_current_account(&base, account.auth.clone()).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap(); let request = read_request(&mut stream); let body = value.to_string();
        write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap(); request
    });
    let result = call(client); (result, server.join().unwrap())
}
fn object(bytes: &[u8]) -> Value {
    let sha = format!("{:x}", Sha256::digest(bytes));
    json!({"agentId":"agent-a","path":"dir/file","dataBase64":base64::engine::general_purpose::STANDARD.encode(bytes),
        "sha256":sha,"blobId":sha,"etag":format!("sha256:{sha}"),"sizeBytes":bytes.len()})
}
fn etag() -> String { format!("sha256:{}", "a".repeat(64)) }

#[test]
fn native_files_read_empty_and_binary_validate_digest_and_send_only_native_headers() {
    for bytes in [vec![], vec![0,255,1,2,13,10]] {
        let (result, request) = fixture(200, object(&bytes), |client| client.get("agent-a", "dir/file"));
        assert_eq!(result.unwrap().unwrap().bytes, bytes);
        let request = request.to_ascii_lowercase();
        assert!(request.starts_with("get /v1/account/agents/agent-a/store?path=dir%2ffile "));
        assert!(request.contains("authorization: bearer native-file-fixture-token"));
        assert!(request.contains("x-fabushi-device-id: file-device"));
        assert!(!request.contains("cursor") && !request.contains("ghost-mode"));
    }
    let mut corrupt = object(b"hello"); corrupt["dataBase64"] = json!("eA==");
    assert!(matches!(fixture(200, corrupt, |c|c.get("agent-a","dir/file")).0, Err(NativeFileError::Protocol)));
    let absent = json!({"error":"agent_store_object_not_found","agentId":"agent-a","path":"dir/file"});
    assert!(fixture(404, absent, |c|c.get("agent-a","dir/file")).0.unwrap().is_none());
    assert!(fixture(404, json!({"error":"route_not_found"}), |c|c.get("agent-a","dir/file")).0.is_err());
}
#[test]
fn native_files_require_cas_and_preserve_write_and_delete_conflicts() {
    let mut written = object(b""); written["outcome"] = json!("written");
    let (result, request) = fixture(200, written, |c|c.put("agent-a","dir/file",b"",NativeWritePrecondition::ExpectAbsent));
    assert!(matches!(result.unwrap(), NativeWriteOutcome::Written{..}));
    let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body, json!({"path":"dir/file","dataBase64":"","expectAbsent":true}));
    let conflict = json!({"path":"dir/file","outcome":"conflict","conflictPath":".conflicts/100-hash/dir/file","baseEtag":etag()});
    let (result, request) = fixture(409, conflict, |c|c.put("agent-a","dir/file",b"x",NativeWritePrecondition::BaseEtag(etag())));
    assert!(matches!(result.unwrap(), NativeWriteOutcome::Conflict{..}));
    assert!(request.contains("baseEtag"));
    let conflict = json!({"agentId":"agent-a","path":"dir/file","deleted":false,"conflict":true,"etag":etag()});
    assert!(matches!(fixture(200,conflict,|c|c.delete("agent-a","dir/file",&etag())).0.unwrap(),NativeDeleteOutcome::Conflict{..}));
    let files = json!({"agentId":"agent-a","files":[{"path":"dir/file","etag":etag()}]});
    assert_eq!(fixture(200,files,|c|c.list("agent-a","dir")).0.unwrap(),vec!["dir/file"]);
    let invalid = json!({"agentId":"agent-a","files":[{"path":"../secret","etag":etag()}]});
    assert!(fixture(200,invalid,|c|c.list("agent-a","")).0.is_err());
}
#[test]
fn native_file_origin_input_and_redirect_boundaries() {
    let account = Account::new("https://api.ombhrum.com");
    assert!(NativeAccountStoreClient::for_current_account("https://api2.cursor.sh",account.auth.clone()).is_err());
    assert!(NativeAccountStoreClient::for_current_account("https://fabushi-ci-isolated-auth.bhrumom.workers.dev",account.auth.clone()).is_err());
    let client = NativeAccountStoreClient::for_current_account("https://api.ombhrum.com",account.auth.clone()).unwrap();
    assert!(matches!(client.get("../other","dir/file"),Err(NativeFileError::InvalidInput)));
    assert!(matches!(client.get("agent-a","../secret"),Err(NativeFileError::InvalidInput)));
    for path in ["dir/CON", "dir/file.", "dir/file ", "dir/control\nname", "dir//file", "C:/file"] {
        assert!(matches!(client.get("agent-a",path),Err(NativeFileError::InvalidInput)));
    }
    assert!(matches!(client.put("agent-a","dir/file",b"x",NativeWritePrecondition::BaseEtag("invalid".into())),Err(NativeFileError::InvalidInput)));

    let origin = TcpListener::bind("127.0.0.1:0").unwrap(); let foreign = TcpListener::bind("127.0.0.1:0").unwrap(); foreign.set_nonblocking(true).unwrap();
    let base = format!("http://{}",origin.local_addr().unwrap()); let account = Account::new(&base);
    let client = NativeAccountStoreClient::for_current_account(&base,account.auth.clone()).unwrap();
    let location = format!("http://{}/private",foreign.local_addr().unwrap());
    let server = thread::spawn(move||{let(mut stream,_)=origin.accept().unwrap();read_request(&mut stream);write!(stream,"HTTP/1.1 307 Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();});
    assert!(matches!(client.get("agent-a","dir/file"),Err(NativeFileError::Status(307))));
    server.join().unwrap(); assert!(foreign.accept().is_err());
}
#[test]
fn native_file_account_change_discards_inflight_response_and_loss_is_unknown() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let base = format!("http://{}",listener.local_addr().unwrap());
    let account = Account::new(&base); let client = NativeAccountStoreClient::for_current_account(&base,account.auth.clone()).unwrap();
    let credential_path = account.path.clone();
    let server = thread::spawn(move||{
        let(mut stream,_)=listener.accept().unwrap(); read_request(&mut stream); write_account(&credential_path,&base,"b");
        let body=object(b"x").to_string(); write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    });
    assert!(matches!(client.get("agent-a","dir/file"),Err(NativeFileError::Account))); server.join().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap(); let base = format!("http://{}",listener.local_addr().unwrap());
    let account = Account::new(&base); let client = NativeAccountStoreClient::for_current_account(&base,account.auth.clone()).unwrap();
    let server = thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();read_request(&mut stream);});
    assert!(matches!(client.put("agent-a","dir/file",b"x",NativeWritePrecondition::ExpectAbsent),Err(NativeFileError::OutcomeUnknown))); server.join().unwrap();
}

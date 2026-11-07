use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::thread;

use mahayana_host_runtime::extensions::auth::auth_service::{HostAuthEnvironment, HostAuthService, HostAuthServiceOptions, FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV};
use mahayana_host_runtime::extensions::automations::backend_transport::{AutomationsBackendError, AutomationsBackendTransport};
use mahayana_host_runtime::extensions::automations::native_backend_transport::NativeAutomationsBackendTransport;
use serde_json::{Value, json};

struct Account {
    root: PathBuf,
    path: PathBuf,
    auth: Arc<HostAuthService>,
}
impl Account {
    fn new(issuer: &str) -> Self {
        let root=std::env::temp_dir().join(format!("native-automation-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path=root.join("credential.json");
        write_account(&path,issuer,"a");
        let auth=Arc::new(HostAuthService::new(HostAuthServiceOptions {
            environment:HostAuthEnvironment::from_pairs(BTreeMap::from([(FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV.into(),path.to_string_lossy().into_owned())])),
            backend_url:Some(issuer.into()),backend:None,
            machine_id:Arc::new(||Ok("fixture-device".into())),log:Arc::new(|_|{}),now_ms:Arc::new(||10_000),
        }).unwrap());
        Self{root,path,auth}
    }
}
impl Drop for Account {
    fn drop(&mut self) { self.auth.dispose(); let _=fs::remove_dir_all(&self.root); }
}
fn write_account(path:&PathBuf,issuer:&str,user:&str) {
    let temporary=path.with_extension("tmp");
    fs::write(&temporary,json!({"version":1,"issuer":issuer,"accessToken":"native-fixture-token",
        "expiresAtMs":600000,"sessionId":format!("session-{user}"),"deviceId":"fixture-device","userId":user}).to_string()).unwrap();
    fs::rename(temporary,path).unwrap();
}
fn read_request(stream:&mut TcpStream)->String {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut bytes=Vec::new();
    loop {
        let mut buf=[0;1024]; let n=stream.read(&mut buf).unwrap(); assert!(n>0); bytes.extend_from_slice(&buf[..n]);
        assert!(bytes.len()<16_384);
        if let Some(index)=bytes.windows(4).position(|v|v==b"\r\n\r\n") {
            let headers=String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
            let length=headers.lines().find_map(|line|line.strip_prefix("content-length:").map(|v|v.trim().parse::<usize>().unwrap())).unwrap_or(0);
            if bytes.len()>=index+4+length {return String::from_utf8(bytes).unwrap();}
        }
    }
}
fn reply(stream:&mut TcpStream,status:u16,body:&str) {
    write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
}
fn completed()->Value {
    json!({"protocol":"fabushi.automation.v1","event":{"id":"run-1"},"completion":{"runUuid":"run-1","status":"succeeded","errorMessage":null},"receipt":"a".repeat(64),"completedAt":100})
}
#[test]
fn native_completion_uses_first_party_path_and_credentials_without_vendor_headers() {
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();
    let base=format!("http://{}",listener.local_addr().unwrap());
    let account=Account::new(&base);
    let transport=NativeAutomationsBackendTransport::for_current_account(&base,account.auth.clone()).unwrap();
    let server=thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();let request=read_request(&mut stream);reply(&mut stream,200,&completed().to_string());request});
    transport.post_json("/sand/automation-runs/complete",&json!({"runUuid":"run-1","status":"succeeded","errorMessage":null})).unwrap();
    let request=server.join().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("post /v1/automation-runs/complete "));
    assert!(request.contains("authorization: bearer native-fixture-token"));
    assert!(request.contains("x-fabushi-device-id: fixture-device"));
    assert!(!request.contains("x-cursor"));
    assert!(!request.contains("x-ghost-mode"));
}
#[test]
fn native_journal_does_not_accept_empty_unversioned_mismatched_or_error_completion() {
    let mut mismatched=completed(); mismatched["event"]["id"]=json!("another-run");
    for (status,body) in [(204,String::new()),(200,"{}".into()),(200,mismatched.to_string()),(404,"{}".into()),(409,"{}".into()),(307,"{}".into())] {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap(); let base=format!("http://{}",listener.local_addr().unwrap());
        let account=Account::new(&base); let transport=NativeAutomationsBackendTransport::for_current_account(&base,account.auth.clone()).unwrap();
        let server=thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();read_request(&mut stream);reply(&mut stream,status,&body);});
        assert!(transport.post_json("/sand/automation-runs/complete",&json!({"runUuid":"run-1","status":"succeeded"})).is_err());
        server.join().unwrap();
    }
}
#[test]
fn account_switch_discards_inflight_native_poll_response() {
    let listener=TcpListener::bind("127.0.0.1:0").unwrap(); let base=format!("http://{}",listener.local_addr().unwrap());
    let account=Account::new(&base); let transport=NativeAutomationsBackendTransport::for_current_account(&base,account.auth.clone()).unwrap();
    let (seen_tx,seen_rx)=mpsc::channel(); let (release_tx,release_rx)=mpsc::channel();
    let server=thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();read_request(&mut stream);seen_tx.send(()).unwrap();release_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();reply(&mut stream,200,r#"{"protocol":"fabushi.automation.v1","events":[]}"#);});
    let request=thread::spawn(move||transport.post_json("/sand/automation-events/poll",&json!({"ackRunUuids":[]})));
    seen_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    write_account(&account.path,&base,"b"); release_tx.send(()).unwrap();
    assert!(matches!(request.join().unwrap(),Err(AutomationsBackendError::Auth(_))));
    server.join().unwrap();
}
#[test]
fn foreign_origin_is_refused_before_any_native_credential_request() {
    let account=Account::new("https://api.ombhrum.com");
    assert!(NativeAutomationsBackendTransport::for_current_account("https://api2.cursor.sh",account.auth.clone()).is_err());
    assert!(NativeAutomationsBackendTransport::for_current_account("https://fabushi-ci-isolated-auth.bhrumom.workers.dev",account.auth.clone()).is_err());
}

#[test]
fn native_journal_never_follows_redirect_to_another_origin() {
    use std::sync::atomic::{AtomicBool,Ordering};
    let initial=TcpListener::bind("127.0.0.1:0").unwrap();
    let redirected=TcpListener::bind("127.0.0.1:0").unwrap();
    redirected.set_nonblocking(true).unwrap();
    let base=format!("http://{}",initial.local_addr().unwrap());
    let location=format!("http://{}/v1/automation-runs/complete",redirected.local_addr().unwrap());
    let account=Account::new(&base);
    let transport=NativeAutomationsBackendTransport::for_current_account(&base,account.auth.clone()).unwrap();
    let finished=Arc::new(AtomicBool::new(false)); let server_finished=finished.clone();
    let foreign=thread::spawn(move||{
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
        while !server_finished.load(Ordering::Acquire) && std::time::Instant::now()<deadline {
            match redirected.accept() {
                Ok((mut stream,_))=>{read_request(&mut stream);reply(&mut stream,200,&completed().to_string());return true;},
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>thread::sleep(std::time::Duration::from_millis(5)),
                Err(error)=>panic!("redirect probe failed: {error}"),
            }
        }
        false
    });
    let origin=thread::spawn(move||{
        let(mut stream,_)=initial.accept().unwrap(); read_request(&mut stream);
        write!(stream,"HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let result=transport.post_json("/sand/automation-runs/complete",&json!({"runUuid":"run-1","status":"succeeded"}));
    finished.store(true,Ordering::Release);
    origin.join().unwrap();
    assert!(!foreign.join().unwrap(),"native transport contacted redirect destination");
    assert!(matches!(result,Err(AutomationsBackendError::Status{status:307,..})));
}

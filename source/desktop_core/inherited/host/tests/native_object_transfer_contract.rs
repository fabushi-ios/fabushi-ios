//! HTTP codec fixture only; real SQLite CAS evidence is Backend NAS-R6.
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::thread;

use base64::Engine;
use mahayana_host_runtime::extensions::auth::auth_service::{HostAuthEnvironment, HostAuthService, HostAuthServiceOptions, FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV};
use mahayana_host_runtime::extensions::box_store_sync::native_account_store::{NativeAccountStoreClient, NativeWritePrecondition, NativeWriteOutcome};
use mahayana_host_runtime::extensions::box_store_sync::native_object_transfer::{NativeObjectTransfer, NATIVE_CHUNK_BYTES};
use mahayana_host_runtime::extensions::box_store_sync::native_box_object_store::NativeBoxObjectStoreProvider;
use mahayana_host_runtime::extensions::box_store_sync::box_object_store::{BoxObjectStoreProvider, BoxObjectStoreProviderDependencies, resolve_box_object_store_provider};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
struct Fixture {
    root: std::path::PathBuf,
    auth: Arc<HostAuthService>,
    transfer: NativeObjectTransfer,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}
fn read_request(stream: &mut TcpStream) -> (String, Option<Value>) {
    // macOS can inherit the listener's O_NONBLOCK on accepted sockets.
    // The listener is polled; each accepted fixture request uses bounded blocking I/O.
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0; 65536]; let n = stream.read(&mut buffer).unwrap(); assert!(n > 0);
        bytes.extend_from_slice(&buffer[..n]); assert!(bytes.len() < 12 * 1024 * 1024);
        if let Some(i) = bytes.windows(4).position(|v|v==b"\r\n\r\n") {
            let headers = String::from_utf8(bytes[..i].to_vec()).unwrap(); let lower = headers.to_ascii_lowercase();
            let length = lower.lines().find_map(|v|v.strip_prefix("content-length:").map(|s|s.trim().parse::<usize>().unwrap())).unwrap_or(0);
            if bytes.len() >= i+4+length {
                assert!(lower.contains("authorization: bearer native-chunk-fixture"));
                assert!(!lower.contains("x-cursor"));
                return (headers.lines().next().unwrap().to_string(), if length==0 {None} else {Some(serde_json::from_slice(&bytes[i+4..i+4+length]).unwrap())});
            }
        }
    }
}
impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap(); listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}",listener.local_addr().unwrap());
        let root = std::env::temp_dir().join(format!("native-chunks-{}",uuid::Uuid::new_v4())); fs::create_dir_all(&root).unwrap();
        let credential = root.join("credential.json");
        fs::write(&credential,json!({"version":1,"issuer":base,"accessToken":"native-chunk-fixture","expiresAtMs":600000,
            "sessionId":"chunk-session","deviceId":"chunk-device","userId":"chunk-account"}).to_string()).unwrap();
        let auth = Arc::new(HostAuthService::new(HostAuthServiceOptions {
            environment: HostAuthEnvironment::from_pairs(BTreeMap::from([(FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV.into(),credential.to_string_lossy().into_owned())])),
            backend_url: Some(base.clone()), backend: None, machine_id: Arc::new(||Ok("chunk-device".into())),
            log: Arc::new(|_|{}), now_ms: Arc::new(||10_000),
        }).unwrap());
        let transfer = NativeObjectTransfer::new(Arc::new(NativeAccountStoreClient::for_current_account(&base,auth.clone()).unwrap()),"agent-a".into());
        let files = Arc::new(Mutex::new(HashMap::<String,Vec<u8>>::new())); let store = files.clone();
        let stop = Arc::new(AtomicBool::new(false)); let stopped = stop.clone();
        let server = thread::spawn(move|| {
            while !stopped.load(Ordering::Acquire) {
                let (mut stream,_) = match listener.accept() {
                    Ok(pair)=>pair, Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>{thread::sleep(std::time::Duration::from_millis(2));continue;}, Err(e)=>panic!("fixture accept: {e}"),
                };
                let (line,body) = read_request(&mut stream); let fields = line.split_whitespace().collect::<Vec<_>>();
                let url = url::Url::parse(&format!("http://fixture{}",fields[1])).unwrap();
                assert_eq!(url.path(),"/v1/account/agents/agent-a/store");
                let mut files = store.lock().unwrap();
                let (status,value) = if fields[0]=="GET" {
                    if let Some((_,path))=url.query_pairs().find(|(k,_)|k=="path") {
                    let path=path.into_owned();match files.get(&path) {
                        Some(bytes)=>{let digest=sha(bytes);(200,json!({"agentId":"agent-a","path":path,"dataBase64":base64::engine::general_purpose::STANDARD.encode(bytes),"sizeBytes":bytes.len(),"sha256":digest,"blobId":digest,"etag":format!("sha256:{digest}")}))},
                        None=>(404,json!({"error":"agent_store_object_not_found","agentId":"agent-a","path":path})),
                    }
                    } else {
                        let prefix=url.query_pairs().find(|(k,_)|k=="prefix").map(|(_,v)|v.into_owned()).unwrap_or_default();
                        (200,json!({"agentId":"agent-a","files":files.iter().filter(|(key,_)|key.starts_with(&prefix)).map(|(key,bytes)|json!({"path":key,"etag":format!("sha256:{}",sha(bytes))})).collect::<Vec<_>>()}))
                    }
                } else if fields[0]=="DELETE" {
                    let path=url.query_pairs().find(|(k,_)|k=="path").unwrap().1.into_owned();
                    let expected=url.query_pairs().find(|(k,_)|k=="baseEtag").unwrap().1.into_owned();
                    match files.get(&path).map(|v|format!("sha256:{}",sha(v))) {
                        Some(etag) if etag!=expected=>(200,json!({"deleted":false,"conflict":true,"agentId":"agent-a","path":path,"etag":etag})),
                        Some(_)=>{files.remove(&path);(200,json!({"deleted":true,"agentId":"agent-a","path":path}))},
                        None=>(200,json!({"deleted":false,"agentId":"agent-a","path":path})),
                    }
                } else {
                    assert_eq!(fields[0],"PUT"); let body=body.unwrap(); let path=body["path"].as_str().unwrap().to_string();
                    let bytes=base64::engine::general_purpose::STANDARD.decode(body["dataBase64"].as_str().unwrap()).unwrap();
                    let prior=files.get(&path).map(|v|format!("sha256:{}",sha(v)));
                    if (body["expectAbsent"]==true && prior.is_some()) || body.get("baseEtag").is_some_and(|e|e.as_str()!=prior.as_deref()) {
                        let copy=format!(".conflicts/fixture/{path}"); files.insert(copy.clone(),bytes);
                        (409,json!({"outcome":"conflict","path":path,"baseEtag":prior,"conflictPath":copy}))
                    } else {
                        let digest=sha(&bytes);files.insert(path.clone(),bytes);
                        (200,json!({"outcome":"written","agentId":"agent-a","path":path,"sha256":digest,"blobId":digest,"etag":format!("sha256:{digest}")}))
                    }
                };
                drop(files); let body=value.to_string();
                write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        Self { root, auth, transfer, files, stop, server:Some(server) }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true,Ordering::Release);let joined=self.server.take().unwrap().join();
        self.auth.dispose();let _=fs::remove_dir_all(&self.root);
        if !thread::panicking() { joined.expect("native HTTP fixture server failed"); }
    }
}
struct BoundedReader(Cursor<Vec<u8>>);
impl Read for BoundedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        assert!(buffer.len() <= NATIVE_CHUNK_BYTES);
        let limit=buffer.len().min(65536); self.0.read(&mut buffer[..limit])
    }
}
#[test]
fn native_large_objects_stream_above_eight_mib_and_publish_only_with_cas() {
    let fixture=Fixture::new(); let bytes=(0..(9*1024*1024)).map(|i|(i%251) as u8).collect::<Vec<_>>();
    let outcome=fixture.transfer.upload("snapshots/store.db",&mut BoundedReader(Cursor::new(bytes.clone())),NativeWritePrecondition::ExpectAbsent).unwrap();
    assert!(matches!(outcome,NativeWriteOutcome::Written{..}));
    let head=fixture.transfer.head("snapshots/store.db").unwrap().unwrap();assert_eq!(head.size(),bytes.len() as u64);assert_eq!(head.sha256(),sha(&bytes));
    let mut restored=Vec::new();assert_eq!(fixture.transfer.download(&head,&mut restored,bytes.len() as u64).unwrap(),bytes.len() as u64);assert_eq!(restored,bytes);
    assert!(fixture.transfer.download(&head,&mut Vec::new(),1).is_err());
    let conflict=fixture.transfer.upload("snapshots/store.db",&mut Cursor::new(b"replacement"),NativeWritePrecondition::ExpectAbsent).unwrap();
    assert!(matches!(conflict,NativeWriteOutcome::Conflict{..}));
    assert_eq!(fixture.transfer.head("snapshots/store.db").unwrap().unwrap().etag,head.etag);
    assert!(fixture.files.lock().unwrap().values().all(|v|v.len()<=NATIVE_CHUNK_BYTES));
}
#[test]
fn native_large_objects_reject_missing_corrupt_chunks_and_do_not_publish_failed_uploads() {
    let fixture=Fixture::new();fixture.transfer.upload("object",&mut Cursor::new(b"original"),NativeWritePrecondition::ExpectAbsent).unwrap();
    let head=fixture.transfer.head("object").unwrap().unwrap();
    for (path,bytes) in fixture.files.lock().unwrap().iter_mut() {if path.starts_with(".fabushi-chunks-v1/"){bytes[0]^=1;}}
    assert!(fixture.transfer.download(&head,&mut Vec::new(),100).is_err());
    fixture.files.lock().unwrap().retain(|path,_|!path.starts_with(".fabushi-chunks-v1/"));
    assert!(fixture.transfer.download(&head,&mut Vec::new(),100).is_err());
    struct Broken(bool);
    impl Read for Broken {fn read(&mut self,b:&mut[u8])->std::io::Result<usize>{if self.0{return Err(std::io::Error::other("fixture read failed"));}self.0=true;b.fill(17);Ok(b.len())}}
    assert!(fixture.transfer.upload("not-published",&mut Broken(false),NativeWritePrecondition::ExpectAbsent).is_err());
    assert!(fixture.transfer.head("not-published").unwrap().is_none());
    fixture.transfer.upload("empty",&mut Cursor::new(b""),NativeWritePrecondition::ExpectAbsent).unwrap();
    let empty=fixture.transfer.head("empty").unwrap().unwrap();assert_eq!(fixture.transfer.download(&empty,&mut Vec::new(),0).unwrap(),0);
}

#[test]
fn native_box_object_store_port_keeps_old_baselines_and_streams_private_staging_files() {
    let fixture=Fixture::new();
    let credential=fixture.auth.get_fabushi_core_credential().unwrap();
    let provider=NativeBoxObjectStoreProvider::new(credential.issuer(),fixture.auth.clone()).unwrap();
    let first=provider.for_store("agent-a");let second=provider.for_store("agent-a");
    first.put("config",b"one").unwrap();assert_eq!(first.get("config").unwrap().unwrap(),b"one");assert_eq!(second.get("config").unwrap().unwrap(),b"one");
    first.put("config",b"two").unwrap();assert!(second.put("config",b"stale").is_err());
    assert_eq!(second.get("config").unwrap().unwrap(),b"two");
    assert!(!second.put_if_unchanged("config",Some(b"one"),b"wrong").unwrap());
    assert!(second.put_if_unchanged("config",Some(b"two"),b"three").unwrap());
    assert_eq!(first.list("conf").unwrap(),vec!["config"]);

    let bytes=vec![29;9*1024*1024];let source=fixture.root.join("source.bin");fs::write(&source,&bytes).unwrap();
    first.put_from_file("blobs/large",&source).unwrap();first.put_from_file("blobs/large",&source).unwrap();
    let destination=fixture.root.join("download.part");
    assert_eq!(first.get_to_file("blobs/large",&destination,Some(bytes.len() as u64)).unwrap(),Some(bytes.len() as u64));
    assert_eq!(fs::read(&destination).unwrap(),bytes);
    assert!(first.get_to_file("blobs/large",&destination,None).is_err());
    assert_eq!(fs::read(&destination).unwrap(),bytes,"existing staging bytes must not be truncated");
    #[cfg(unix)] {
        use std::os::unix::fs::{symlink,PermissionsExt};
        assert_eq!(fs::metadata(&destination).unwrap().permissions().mode()&0o777,0o600);
        let link=fixture.root.join("linked.part");symlink(&source,&link).unwrap();
        assert!(first.get_to_file("blobs/large",&link,None).is_err());
        assert!(first.put_from_file("blobs/symlink",&link).is_err());
        assert_eq!(fs::read(&source).unwrap(),bytes);
    }
    first.get("config").unwrap();first.delete("config").unwrap();assert!(first.get("config").unwrap().is_none());
    assert!(first.list("conf").unwrap().is_empty());
    assert!(fixture.files.lock().unwrap().keys().any(|key|key.starts_with(".fabushi-chunks-v1/")),"logical deletion must retain immutable chunks for separate GC");
}

#[test]
fn native_box_mode_selects_existing_provider_and_never_uses_legacy_credentials() {
    use mahayana_host_runtime::r#box::box_store_backend_policy::{BoxStoreBackendKind,resolve_box_store_backend_policy};
    let fixture=Fixture::new();let credential=fixture.auth.get_fabushi_core_credential().unwrap();
    let mut env=BTreeMap::from([("FABUSHI_NATIVE_BOX_STORE".into(),"1".into()),("SAND_BOX_STORE_BACKEND".into(),"v2".into())]);
    assert_eq!(resolve_box_store_backend_policy(&env).kind,BoxStoreBackendKind::Native);
    let provider=resolve_box_object_store_provider(&env,BoxObjectStoreProviderDependencies {
        backend_url:Some(credential.issuer().into()),native_credentials:Some(fixture.auth.clone()),
        get_access_token:Some(Arc::new(||panic!("native mode used legacy token"))),get_machine_id:Some(Arc::new(||panic!("native mode used legacy machine resolver"))),
    }).unwrap();
    provider.for_store("agent-a").put("native-mode",b"ok").unwrap();
    assert!(resolve_box_object_store_provider(&env,BoxObjectStoreProviderDependencies::default()).is_err());
    env.insert("SAND_BOX_STORE_LOCAL_DIR".into(),fixture.root.to_string_lossy().into_owned());
    assert_eq!(resolve_box_store_backend_policy(&env).kind,BoxStoreBackendKind::LocalFs);
}

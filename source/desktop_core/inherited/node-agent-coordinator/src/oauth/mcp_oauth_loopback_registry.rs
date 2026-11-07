use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::protocol::Failure;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackRedirect {
    pub origin: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn parse_loopback_redirect(value: &str) -> Result<LoopbackRedirect, Failure> {
    let rest = value.strip_prefix("http://").ok_or_else(|| {
        Failure::new(
            "MCP_OAUTH_INVALID_REDIRECT",
            "MCP OAuth redirect_uri must be a localhost HTTP URL",
        )
    })?;
    let authority_end = rest
        .find(|character| matches!(character, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.contains('@') {
        return Err(Failure::new(
            "MCP_OAUTH_INVALID_REDIRECT",
            "MCP OAuth redirect_uri must not contain user information",
        ));
    }
    let (host, port_text) = authority.rsplit_once(':').ok_or_else(|| {
        Failure::new(
            "MCP_OAUTH_INVALID_REDIRECT",
            "MCP OAuth redirect_uri must include a port",
        )
    })?;
    if host != "127.0.0.1" && host != "localhost" {
        return Err(Failure::new(
            "MCP_OAUTH_INVALID_REDIRECT",
            "MCP OAuth redirect_uri must use localhost or 127.0.0.1",
        ));
    }
    let port = port_text.parse::<u16>().ok().filter(|port| *port > 0).ok_or_else(|| {
        Failure::new(
            "MCP_OAUTH_INVALID_REDIRECT",
            "MCP OAuth redirect_uri must include a valid non-zero port",
        )
    })?;
    let suffix = &rest[authority_end..];
    if suffix.contains('#') {
        return Err(Failure::new(
            "MCP_OAUTH_INVALID_REDIRECT",
            "MCP OAuth redirect_uri must not contain a fragment",
        ));
    }
    let path = suffix
        .split('?')
        .next()
        .filter(|path| !path.is_empty())
        .unwrap_or("/")
        .to_string();
    Ok(LoopbackRedirect {
        origin: format!("http://{host}:{port}"),
        host: host.to_string(),
        port,
        path,
    })
}

pub fn loopback_bind_hosts(redirect_host: &str) -> Vec<String> {
    if redirect_host == "localhost" {
        vec!["127.0.0.1".into(), "::1".into()]
    } else {
        vec![redirect_host.to_string()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackLease {
    pub origin: String,
    pub lease_id: u64,
    pub port: u16,
    pub bind_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackHttpResponse {
    pub status_code: u16,
    pub content_type: String,
    pub body: String,
}

impl LoopbackHttpResponse {
    pub fn html(status_code: u16, body: impl Into<String>) -> Self {
        Self {
            status_code,
            content_type: "text/html; charset=utf-8".into(),
            body: body.into(),
        }
    }
}

pub type McpOAuthLoopbackHandler =
    Arc<dyn Fn(&str) -> Option<LoopbackHttpResponse> + Send + Sync + 'static>;

struct LoopbackRuntime {
    stop: Arc<AtomicBool>,
    handlers: Arc<Mutex<BTreeMap<u64, McpOAuthLoopbackHandler>>>,
    threads: Vec<JoinHandle<()>>,
    listener_count: usize,
}

impl LoopbackRuntime {
    fn bind(port: u16, hosts: &[String]) -> Result<Self, Failure> {
        let stop = Arc::new(AtomicBool::new(false));
        let handlers = Arc::new(Mutex::new(BTreeMap::new()));
        let mut threads = Vec::new();
        let mut errors = Vec::new();

        for host in hosts {
            let address = if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            };
            match TcpListener::bind(&address) {
                Ok(listener) => {
                    if let Err(error) = listener.set_nonblocking(true) {
                        errors.push(error.to_string());
                        continue;
                    }
                    let thread_stop = Arc::clone(&stop);
                    let thread_handlers = Arc::clone(&handlers);
                    threads.push(thread::spawn(move || {
                        loop {
                            if thread_stop.load(Ordering::Acquire) {
                                break;
                            }
                            match listener.accept() {
                                Ok((stream, _)) => {
                                    let _ = serve_request(stream, &thread_handlers);
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    thread::sleep(Duration::from_millis(10));
                                }
                                Err(_) => {
                                    if thread_stop.load(Ordering::Acquire) {
                                        break;
                                    }
                                    thread::sleep(Duration::from_millis(10));
                                }
                            }
                        }
                    }));
                }
                Err(error) => errors.push(format!("{address}: {error}")),
            }
        }

        if threads.is_empty() {
            return Err(Failure::new(
                "MCP_OAUTH_LOOPBACK_BIND_FAILED",
                errors
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| format!("could not bind OAuth callback port {port}")),
            ));
        }

        let listener_count = threads.len();
        Ok(Self {
            stop,
            handlers,
            threads,
            listener_count,
        })
    }

    fn install(&self, lease_id: u64, handler: McpOAuthLoopbackHandler) -> Result<(), Failure> {
        self.handlers
            .lock()
            .map_err(|_| Failure::new(
                "MCP_OAUTH_LOOPBACK_POISONED",
                "OAuth loopback handler registry is unavailable",
            ))?
            .insert(lease_id, handler);
        Ok(())
    }

    fn remove(&self, lease_id: u64) {
        if let Ok(mut handlers) = self.handlers.lock() {
            handlers.remove(&lease_id);
        }
    }
}

impl Drop for LoopbackRuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

fn serve_request(
    mut stream: TcpStream,
    handlers: &Arc<Mutex<BTreeMap<u64, McpOAuthLoopbackHandler>>>,
) -> std::io::Result<()> {
    // The listener is non-blocking so its accept loop can observe shutdown.
    // Accepted sockets may inherit that mode on some platforms (notably the
    // macOS CI runner), which can make the first read return WouldBlock and
    // close the callback connection before the browser finishes writing it.
    // Each accepted OAuth HTTP exchange is bounded by explicit read/write
    // timeouts, so restore blocking I/O for deterministic request handling.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    const MAX_REQUEST_BYTES: usize = 16 * 1024;
    let mut request_bytes = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 2048];
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request_bytes.extend_from_slice(&chunk[..count]);
        if request_bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request_bytes.len() >= MAX_REQUEST_BYTES {
            write_response(
                &mut stream,
                LoopbackHttpResponse::html(400, "Request headers are too large"),
            )?;
            return Ok(());
        }
    }
    if request_bytes.is_empty() {
        return Ok(());
    }
    let request = String::from_utf8_lossy(&request_bytes);
    let request_target = request
        .lines()
        .next()
        .and_then(|line| {
            let mut parts = line.split_whitespace();
            let method = parts.next()?;
            let target = parts.next()?;
            (method == "GET").then_some(target)
        })
        .unwrap_or("/");

    let callbacks = handlers
        .lock()
        .map(|entries| entries.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let response = callbacks
        .into_iter()
        .find_map(|handler| handler(request_target))
        .unwrap_or_else(|| LoopbackHttpResponse::html(404, "Not found"));
    write_response(&mut stream, response)
}

fn write_response(stream: &mut TcpStream, response: LoopbackHttpResponse) -> std::io::Result<()> {
    let reason = match response.status_code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Response",
    };
    let bytes = response.body.as_bytes();
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status_code,
        reason,
        response.content_type,
        bytes.len()
    )?;
    stream.write_all(bytes)?;
    stream.flush()
}

struct LoopbackEntry {
    port: u16,
    host: String,
    leases: HashSet<u64>,
    runtime: Option<LoopbackRuntime>,
}

#[derive(Default)]
pub struct McpOAuthLoopbackRegistry {
    origins: HashMap<String, LoopbackEntry>,
    next_lease_id: u64,
}

impl McpOAuthLoopbackRegistry {
    pub fn acquire(&mut self, redirect_url: &str) -> Result<LoopbackLease, Failure> {
        let parsed = parse_loopback_redirect(redirect_url)?;
        let entry = self.origins.entry(parsed.origin.clone()).or_insert_with(|| LoopbackEntry {
            port: parsed.port,
            host: parsed.host.clone(),
            leases: HashSet::new(),
            runtime: None,
        });
        if entry.port != parsed.port || entry.host != parsed.host {
            return Err(Failure::new(
                "MCP_OAUTH_LOOPBACK_CONFLICT",
                "loopback origin changed while it was acquired",
            ));
        }
        self.next_lease_id = self.next_lease_id.saturating_add(1).max(1);
        let lease_id = self.next_lease_id;
        entry.leases.insert(lease_id);
        Ok(LoopbackLease {
            origin: parsed.origin,
            lease_id,
            port: parsed.port,
            bind_hosts: loopback_bind_hosts(&parsed.host),
        })
    }

    pub fn acquire_with_handler(
        &mut self,
        redirect_url: &str,
        handler: McpOAuthLoopbackHandler,
    ) -> Result<LoopbackLease, Failure> {
        let lease = self.acquire(redirect_url)?;
        let install = (|| {
            let entry = self.origins.get_mut(&lease.origin).ok_or_else(|| {
                Failure::new(
                    "MCP_OAUTH_LOOPBACK_MISSING",
                    "OAuth loopback origin disappeared while acquiring it",
                )
            })?;
            if entry.runtime.is_none() {
                entry.runtime = Some(LoopbackRuntime::bind(lease.port, &lease.bind_hosts)?);
            }
            entry
                .runtime
                .as_ref()
                .expect("loopback runtime installed")
                .install(lease.lease_id, handler)
        })();
        if let Err(error) = install {
            self.release(&lease);
            return Err(error);
        }
        Ok(lease)
    }

    pub fn release(&mut self, lease: &LoopbackLease) -> bool {
        let Some(entry) = self.origins.get_mut(&lease.origin) else {
            return false;
        };
        let removed = entry.leases.remove(&lease.lease_id);
        if let Some(runtime) = entry.runtime.as_ref() {
            runtime.remove(lease.lease_id);
        }
        let empty = entry.leases.is_empty();
        if empty {
            self.origins.remove(&lease.origin);
        }
        removed
    }

    pub fn active_origin_count(&self) -> usize {
        self.origins.len()
    }

    pub fn lease_count(&self, origin: &str) -> usize {
        self.origins
            .get(origin)
            .map(|entry| entry.leases.len())
            .unwrap_or(0)
    }

    pub fn bound_listener_count(&self, origin: &str) -> usize {
        self.origins
            .get(origin)
            .and_then(|entry| entry.runtime.as_ref())
            .map(|runtime| runtime.listener_count)
            .unwrap_or(0)
    }

    pub fn dispose(&mut self) {
        self.origins.clear();
    }
}

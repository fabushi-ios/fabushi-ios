use std::env;
use std::fs;
use std::io::{self, BufReader, Cursor, Read, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

pub const GATEWAY_TLS_CERT_ENV: &str = "SAND_GATEWAY_TLS_CERT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayHttpScheme {
    Http,
    Https,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayHttpEndpoint {
    pub scheme: GatewayHttpScheme,
    pub host: String,
    pub port: u16,
    pub base_path: String,
}

impl GatewayHttpEndpoint {
    pub fn socket_addrs(&self) -> io::Result<Vec<SocketAddr>> {
        format!("{}:{}", self.host, self.port)
            .to_socket_addrs()
            .map(|addresses| addresses.collect())
    }

    pub fn connect_any(
        &self,
        sockets: &[SocketAddr],
        timeout: Duration,
    ) -> io::Result<GatewayHttpStream> {
        let mut last_error = None;
        for socket in sockets {
            match self.connect(socket, timeout) {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "Host gateway address did not resolve to a connectable socket",
            )
        }))
    }

    pub fn connect(
        &self,
        socket: &SocketAddr,
        timeout: Duration,
    ) -> io::Result<GatewayHttpStream> {
        let tcp = std::net::TcpStream::connect_timeout(socket, timeout)?;
        tcp.set_read_timeout(Some(timeout))?;
        tcp.set_write_timeout(Some(timeout))?;
        match self.scheme {
            GatewayHttpScheme::Http => Ok(GatewayHttpStream::Plain(tcp)),
            GatewayHttpScheme::Https => {
                let config = tls_client_config()?;
                let server_name = server_name(&self.host)?;
                let connection = ClientConnection::new(config, server_name)
                    .map_err(|error| io::Error::other(format!("gateway TLS client failed: {error}")))?;
                Ok(GatewayHttpStream::Tls(StreamOwned::new(connection, tcp)))
            }
        }
    }
}

pub fn parse_gateway_http_base(base_url: &str) -> io::Result<GatewayHttpEndpoint> {
    let (scheme, rest) = if let Some(rest) = base_url.strip_prefix("http://") {
        (GatewayHttpScheme::Http, rest)
    } else if let Some(rest) = base_url.strip_prefix("https://") {
        (GatewayHttpScheme::Https, rest)
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported Host gateway scheme in {base_url}"),
        ));
    };

    let (authority, base_path) = rest
        .split_once('/')
        .map_or((rest, String::new()), |(authority, path)| {
            (authority, format!("/{}", path.trim_end_matches('/')))
        });
    if authority.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Host gateway URL has no authority",
        ));
    }

    let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((host, suffix)) = ipv6.split_once(']') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid IPv6 Host gateway authority",
            ));
        };
        let port = suffix
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Host gateway URL must include a port",
                )
            })?;
        (host.to_string(), port)
    } else {
        let (host, port) = authority.rsplit_once(':').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Host gateway URL must include a port",
            )
        })?;
        let port = port.parse::<u16>().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid Host gateway port")
        })?;
        (host.to_string(), port)
    };

    if host.is_empty() || port == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Host gateway authority",
        ));
    }

    Ok(GatewayHttpEndpoint {
        scheme,
        host,
        port,
        base_path,
    })
}

fn tls_client_config() -> io::Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let Ok(path) = env::var(GATEWAY_TLS_CERT_ENV) {
        let path = path.trim();
        if !path.is_empty() {
            let bytes = fs::read(path)?;
            let mut reader = BufReader::new(Cursor::new(bytes));
            for cert in rustls_pemfile::certs(&mut reader) {
                let cert = cert.map_err(|error| {
                    io::Error::other(format!("gateway TLS trust certificate parse failed: {error}"))
                })?;
                roots.add(cert).map_err(|error| {
                    io::Error::other(format!("gateway TLS trust certificate rejected: {error}"))
                })?;
            }
        }
    }

    Ok(Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

fn server_name(host: &str) -> io::Result<ServerName<'static>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }
    ServerName::try_from(host.to_string()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid Host gateway TLS server name {host}"),
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl GatewayHttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub fn read_http_response(
    stream: &mut GatewayHttpStream,
    max_header_bytes: usize,
    max_body_bytes: usize,
) -> io::Result<GatewayHttpResponse> {
    let mut response = Vec::new();
    let mut chunk = [0_u8; 8192];
    let header_end = loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Host gateway closed before HTTP response headers completed",
            ));
        }
        response.extend_from_slice(&chunk[..count]);
        if let Some(index) = response.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if response.len() > max_header_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Host gateway HTTP response headers are too large",
            ));
        }
    };

    let header_text = std::str::from_utf8(&response[..header_end]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Host gateway HTTP response headers are not UTF-8",
        )
    })?;
    let mut lines = header_text.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Host gateway HTTP response has no valid status",
            )
        })?;

    let mut headers = Vec::new();
    let mut content_length = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            let parsed = value.parse::<usize>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Host gateway HTTP response has invalid Content-Length",
                )
            })?;
            content_length = Some(parsed);
        }
        headers.push((name.trim().to_string(), value.to_string()));
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Host gateway HTTP response is missing Content-Length",
        )
    })?;
    if content_length > max_body_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Host gateway HTTP response body is too large",
        ));
    }

    let mut body = response.split_off(header_end);
    if body.len() > content_length {
        body.truncate(content_length);
    } else if body.len() < content_length {
        let remaining = content_length - body.len();
        body.resize(content_length, 0);
        stream.read_exact(&mut body[content_length - remaining..])?;
    }

    Ok(GatewayHttpResponse {
        status,
        headers,
        body,
    })
}

pub enum GatewayHttpStream {
    Plain(std::net::TcpStream),
    Tls(StreamOwned<ClientConnection, std::net::TcpStream>),
}

impl GatewayHttpStream {
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.set_read_timeout(timeout),
            Self::Tls(stream) => stream.sock.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.set_write_timeout(timeout),
            Self::Tls(stream) => stream.sock.set_write_timeout(timeout),
        }
    }
}

impl Read for GatewayHttpStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for GatewayHttpStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

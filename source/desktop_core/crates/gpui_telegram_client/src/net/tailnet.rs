use crate::net::api_computers::{
    detect_local_host_info, detect_local_hostname, detect_real_lan_ips, execute_local_computer_op,
};
use crate::net::http_client::{HttpClient, HttpClientError};
use crate::net::http_helpers::percent_encode_rfc3986;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use ed25519_dalek::{Signer, SigningKey};
use futures::{SinkExt, StreamExt};
use gpui_telegram_proto::computer_contracts::{
    AuthorizedControllerEntry, AuthorizedControllerKind, ComputerRunPolicy,
    PendingComputerCommandDto, RemoteComputerCommandRequest, RemoteComputerCommandResult,
    RemoteComputerOp,
};
use gpui_telegram_proto::tailnet_contracts::{
    DerpClientFrame, DerpServerFrame, DiscoPayload, PeerApiFrame, TailnetEndpoint,
    TailnetEndpointKind, TailnetNetMap, TailnetNodeStatus, TailnetPeerDescriptor,
    TailnetPeerLiveState, TailnetRegisterRequest, TailnetRelayMode,
};
use hkdf::Hkdf;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};
use uuid::Uuid;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

const IDENTITY_FILE_NAME: &str = "tailnet_identity.json";
const TSWG_MAGIC: &[u8; 4] = b"TSWG";
const TSDC_MAGIC: &[u8; 4] = b"TSDC";
const TSFR_MAGIC: &[u8; 4] = b"TSFR";
const TSWG_HEADER_LEN: usize = 4 + 32 + 8 + 4;
const TSDC_HEADER_LEN: usize = 4 + 32 + 12;
const TSFR_HEADER_LEN: usize = 4 + 8 + 2 + 2;

const TSWG_DOMAIN: &[u8] = b"gpui-tailnet-wg-v1";
const TSDC_DOMAIN: &[u8] = b"gpui-tailnet-disco-v1";

pub const UDP_FRAG_CHUNK_SIZE: usize = 1100;
pub const UDP_FRAG_MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
pub const UDP_FRAG_TTL: Duration = Duration::from_secs(2);

const KEEPALIVE_REGISTER_INTERVAL: Duration = Duration::from_secs(20);
const DERP_PING_INTERVAL: Duration = Duration::from_secs(15);
const DISCO_PROBE_INTERVAL: Duration = Duration::from_secs(2);
const DIRECT_UDP_RPC_FALLBACK_TIMEOUT: Duration = Duration::from_millis(1500);
const PEER_EXEC_WAIT_TIMEOUT: Duration = Duration::from_secs(26);
const LOCAL_APPROVAL_TIMEOUT: Duration = Duration::from_secs(25);
const DIRECT_UDP_FRESH_MS: i64 = 15_000;
const MAX_PENDING_FRAG_ASSEMBLIES: usize = 256;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn random_bytes_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    if getrandom::getrandom(&mut buf).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(1);
        for (idx, b) in buf.iter_mut().enumerate() {
            *b = ((nanos >> ((idx % 16) * 4)) as u8) ^ (idx as u8).wrapping_mul(37);
        }
    }
    buf
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    if getrandom::getrandom(&mut buf).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(1);
        for (idx, b) in buf.iter_mut().enumerate() {
            *b = ((nanos >> ((idx % 16) * 4)) as u8) ^ (idx as u8).wrapping_mul(53);
        }
    }
    buf
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

fn decode_hex(raw: &str) -> Result<Vec<u8>, String> {
    let trimmed = raw.trim();
    if trimmed.len() % 2 != 0 {
        return Err("Invalid hex length".to_string());
    }
    let bytes = trimmed.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let hi = std::str::from_utf8(chunk)
            .ok()
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .ok_or_else(|| "Invalid hex character".to_string())?;
        out.push(hi);
    }
    Ok(out)
}

fn decode_hex_32(raw: &str) -> Result<[u8; 32], String> {
    let bytes = decode_hex(raw)?;
    if bytes.len() != 32 {
        return Err(format!("Expected 32-byte hex key, got {} bytes", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let b0 = input[i] as usize;
        let b1 = input[i + 1] as usize;
        let b2 = input[i + 2] as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        out.push(TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        out.push(TABLE[b2 & 0x3f] as char);
        i += 3;
    }
    match input.len() - i {
        1 => {
            let b0 = input[i] as usize;
            out.push(TABLE[b0 >> 2] as char);
            out.push(TABLE[(b0 & 0x03) << 4] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let b0 = input[i] as usize;
            let b1 = input[i + 1] as usize;
            out.push(TABLE[b0 >> 2] as char);
            out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
            out.push(TABLE[(b1 & 0x0f) << 2] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    fn val(b: u8) -> Result<u8, String> {
        match b {
            b'A'..=b'Z' => Ok(b - b'A'),
            b'a'..=b'z' => Ok(b - b'a' + 26),
            b'0'..=b'9' => Ok(b - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err("Invalid base64 byte".to_string()),
        }
    }

    let cleaned: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if cleaned.len() % 4 != 0 {
        return Err("Invalid base64 padding length".to_string());
    }

    let mut out = Vec::with_capacity((cleaned.len() / 4) * 3);
    for (idx, chunk) in cleaned.chunks_exact(4).enumerate() {
        let is_last = (idx + 1) * 4 == cleaned.len();
        let a = val(chunk[0])? as u32;
        let b = val(chunk[1])? as u32;
        if chunk[2] == b'=' {
            if !is_last || chunk[3] != b'=' {
                return Err("Invalid base64 '=' placement".to_string());
            }
            let n = (a << 18) | (b << 12);
            out.push(((n >> 16) & 0xFF) as u8);
        } else if chunk[3] == b'=' {
            if !is_last {
                return Err("Invalid base64 '=' placement".to_string());
            }
            let c = val(chunk[2])? as u32;
            let n = (a << 18) | (b << 12) | (c << 6);
            out.push(((n >> 16) & 0xFF) as u8);
            out.push(((n >> 8) & 0xFF) as u8);
        } else {
            let c = val(chunk[2])? as u32;
            let d = val(chunk[3])? as u32;
            let n = (a << 18) | (b << 12) | (c << 6) | d;
            out.push(((n >> 16) & 0xFF) as u8);
            out.push(((n >> 8) & 0xFF) as u8);
            out.push((n & 0xFF) as u8);
        }
    }
    Ok(out)
}

#[derive(Serialize, Deserialize)]
struct PersistedTailnetIdentity {
    version: u32,
    machine_secret_hex: String,
    node_secret_hex: String,
    disco_secret_hex: String,
    #[serde(default)]
    device_id: String,
}

/// Persistent WireGuard-style cryptographic node identity for the Tailnet mesh.
#[derive(Clone)]
pub struct TailnetIdentity {
    machine_key: SigningKey,
    node_key: StaticSecret,
    disco_key: StaticSecret,
    pub machine_public_bytes: [u8; 32],
    pub node_public_bytes: [u8; 32],
    pub disco_public_bytes: [u8; 32],
    pub machine_public_key_hex: String,
    pub node_public_key_hex: String,
    pub disco_public_key_hex: String,
    pub device_id: String,
}

impl std::fmt::Debug for TailnetIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TailnetIdentity")
            .field("device_id", &self.device_id)
            .field("machine_public_key_hex", &self.machine_public_key_hex)
            .field("node_public_key_hex", &self.node_public_key_hex)
            .field("disco_public_key_hex", &self.disco_public_key_hex)
            .finish()
    }
}

impl TailnetIdentity {
    /// Builds an identity from 3 raw 32-byte key seeds.
    pub fn from_seeds(machine_seed: [u8; 32], node_seed: [u8; 32], disco_seed: [u8; 32]) -> Self {
        let machine_key = SigningKey::from_bytes(&machine_seed);
        let node_key = StaticSecret::from(node_seed);
        let disco_key = StaticSecret::from(disco_seed);

        let machine_public_bytes = machine_key.verifying_key().to_bytes();
        let node_public_bytes = X25519PublicKey::from(&node_key).to_bytes();
        let disco_public_bytes = X25519PublicKey::from(&disco_key).to_bytes();

        let machine_public_key_hex = encode_hex(&machine_public_bytes);
        let node_public_key_hex = encode_hex(&node_public_bytes);
        let disco_public_key_hex = encode_hex(&disco_public_bytes);
        let device_id = format!("dev-ts-{}", &machine_public_key_hex[..16]);

        Self {
            machine_key,
            node_key,
            disco_key,
            machine_public_bytes,
            node_public_bytes,
            disco_public_bytes,
            machine_public_key_hex,
            node_public_key_hex,
            disco_public_key_hex,
            device_id,
        }
    }

    /// Generates an ephemeral in-memory identity (used when no persistent directory is bound).
    pub fn ephemeral() -> Self {
        let mut m = Zeroizing::new(random_bytes_32());
        let mut n = Zeroizing::new(random_bytes_32());
        let mut d = Zeroizing::new(random_bytes_32());
        let out = Self::from_seeds(*m, *n, *d);
        m.zeroize();
        n.zeroize();
        d.zeroize();
        out
    }

    /// Loads `tailnet_identity.json` from `dir`, or generates and atomically persists it with `0600` mode.
    pub fn load_or_create(dir: &Path) -> Self {
        let target_path = dir.join(IDENTITY_FILE_NAME);
        if target_path.is_file() {
            if let Ok(raw) = std::fs::read_to_string(&target_path) {
                if let Ok(parsed) = serde_json::from_str::<PersistedTailnetIdentity>(&raw) {
                    if let (Ok(mut m), Ok(mut n), Ok(mut d)) = (
                        decode_hex_32(&parsed.machine_secret_hex),
                        decode_hex_32(&parsed.node_secret_hex),
                        decode_hex_32(&parsed.disco_secret_hex),
                    ) {
                        let mut id = Self::from_seeds(m, n, d);
                        m.zeroize();
                        n.zeroize();
                        d.zeroize();
                        if !parsed.device_id.trim().is_empty() {
                            id.device_id = parsed.device_id.trim().to_string();
                        }
                        return id;
                    }
                }
            }
        }

        let created = Self::ephemeral();
        if let Err(err) = Self::persist_atomic(dir, &target_path, &created) {
            warn!(
                "Failed to persist tailnet identity to '{}': {}",
                target_path.display(),
                err
            );
        }
        created
    }

    fn persist_atomic(
        dir: &Path,
        target_path: &Path,
        identity: &Self,
    ) -> std::io::Result<()> {
        use std::io::Write;

        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }

        let mut m_bytes = Zeroizing::new(identity.machine_key.to_bytes());
        let mut n_bytes = Zeroizing::new(identity.node_key.to_bytes());
        let mut d_bytes = Zeroizing::new(identity.disco_key.to_bytes());

        let persisted = PersistedTailnetIdentity {
            version: 1,
            machine_secret_hex: encode_hex(&*m_bytes),
            node_secret_hex: encode_hex(&*n_bytes),
            disco_secret_hex: encode_hex(&*d_bytes),
            device_id: identity.device_id.clone(),
        };
        m_bytes.zeroize();
        n_bytes.zeroize();
        d_bytes.zeroize();

        let json = Zeroizing::new(
            serde_json::to_string_pretty(&persisted)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        );

        let tmp_name = format!(
            ".tailnet_identity.tmp.{}.{}",
            std::process::id(),
            Uuid::new_v4().simple()
        );
        let tmp_path = dir.join(tmp_name);

        let write_res = (|| -> std::io::Result<()> {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(&tmp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&tmp_path, target_path)?;
            Ok(())
        })();

        if let Err(err) = write_res {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err);
        }
        Ok(())
    }

    /// Signs `req.canonical_signing_message(owner_user_id)` with the Ed25519 MachineKey.
    pub fn sign_registration(&self, owner_user_id: u64, req: &TailnetRegisterRequest) -> String {
        let msg = req.canonical_signing_message(owner_user_id);
        let sig = self.machine_key.sign(msg.as_bytes());
        encode_hex(&sig.to_bytes())
    }
}

/// Derives `(tx_key, rx_key)` via X25519 + HKDF-SHA256 using canonical lexicographical key ordering.
pub fn derive_peer_cipher_keys(
    my_secret: &StaticSecret,
    my_pub: &[u8; 32],
    peer_pub: &[u8; 32],
    domain: &[u8],
) -> ([u8; 32], [u8; 32]) {
    let shared = my_secret.diffie_hellman(&X25519PublicKey::from(*peer_pub));
    let (lo, hi) = if my_pub <= peer_pub {
        (my_pub, peer_pub)
    } else {
        (peer_pub, my_pub)
    };

    let hk = Hkdf::<Sha256>::new(Some(domain), shared.as_bytes());

    let mut info_a_to_b = Vec::with_capacity(4 + 32 + 32);
    info_a_to_b.extend_from_slice(b"a->b");
    info_a_to_b.extend_from_slice(lo);
    info_a_to_b.extend_from_slice(hi);

    let mut info_b_to_a = Vec::with_capacity(4 + 32 + 32);
    info_b_to_a.extend_from_slice(b"b->a");
    info_b_to_a.extend_from_slice(lo);
    info_b_to_a.extend_from_slice(hi);

    let mut key_a_to_b = [0u8; 32];
    let mut key_b_to_a = [0u8; 32];
    let _ = hk.expand(&info_a_to_b, &mut key_a_to_b);
    let _ = hk.expand(&info_b_to_a, &mut key_b_to_a);

    if my_pub == peer_pub {
        (key_a_to_b, key_a_to_b)
    } else if my_pub < peer_pub {
        (key_a_to_b, key_b_to_a)
    } else {
        (key_b_to_a, key_a_to_b)
    }
}

/// WireGuard-style 64-bit sliding anti-replay sequence window.
#[derive(Debug, Clone, Default)]
pub struct ReplayWindow {
    highest_seq: u64,
    bitmap: u64,
    initialized: bool,
}

impl ReplayWindow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn would_accept(&self, seq: u64) -> bool {
        if !self.initialized {
            return true;
        }
        if seq > self.highest_seq {
            return true;
        }
        let diff = self.highest_seq - seq;
        if diff >= 64 {
            return false;
        }
        (self.bitmap & (1u64 << diff)) == 0
    }

    pub fn check_and_mark(&mut self, seq: u64) -> bool {
        if !self.initialized {
            self.initialized = true;
            self.highest_seq = seq;
            self.bitmap = 1;
            return true;
        }
        if seq > self.highest_seq {
            let shift = seq - self.highest_seq;
            if shift >= 64 {
                self.bitmap = 1;
            } else {
                self.bitmap = (self.bitmap << shift) | 1;
            }
            self.highest_seq = seq;
            return true;
        }
        let diff = self.highest_seq - seq;
        if diff >= 64 {
            return false;
        }
        let mask = 1u64 << diff;
        if (self.bitmap & mask) != 0 {
            return false;
        }
        self.bitmap |= mask;
        true
    }
}

/// Encrypts a `TSWG` WireGuard/Noise-style packet:
/// `b"TSWG" (4B) || src_node_pub (32B) || seq_u64_le (8B) || salt (4B) || ChaCha20Poly1305(plaintext)`
pub fn seal_tswg_packet(
    my_secret: &StaticSecret,
    my_pub: &[u8; 32],
    peer_pub: &[u8; 32],
    seq: u64,
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    let (mut tx_key, mut rx_key) = derive_peer_cipher_keys(my_secret, my_pub, peer_pub, TSWG_DOMAIN);
    rx_key.zeroize();

    let cipher = ChaCha20Poly1305::new_from_slice(&tx_key)
        .map_err(|e| format!("Invalid ChaCha20Poly1305 key: {e}"))?;
    tx_key.zeroize();

    let salt = random_bytes::<4>();
    let seq_le = seq.to_le_bytes();
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..8].copy_from_slice(&seq_le);
    nonce_bytes[8..12].copy_from_slice(&salt);

    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|e| format!("TSWG AEAD encrypt failed: {e}"))?;

    let mut out = Vec::with_capacity(TSWG_HEADER_LEN + ciphertext.len());
    out.extend_from_slice(TSWG_MAGIC);
    out.extend_from_slice(my_pub);
    out.extend_from_slice(&seq_le);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Verifies anti-replay and decrypts a `TSWG` packet, returning `(src_node_pub_hex, plaintext)`.
pub fn open_tswg_packet(
    my_secret: &StaticSecret,
    my_pub: &[u8; 32],
    packet: &[u8],
    replay_windows: &mut HashMap<String, ReplayWindow>,
) -> Result<(String, Vec<u8>), String> {
    if packet.len() < TSWG_HEADER_LEN + 16 || &packet[..4] != TSWG_MAGIC {
        return Err("Malformed TSWG packet".to_string());
    }
    let mut src_pub = [0u8; 32];
    src_pub.copy_from_slice(&packet[4..36]);
    let mut seq_bytes = [0u8; 8];
    seq_bytes.copy_from_slice(&packet[36..44]);
    let seq = u64::from_le_bytes(seq_bytes);

    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..8].copy_from_slice(&packet[36..44]);
    nonce_bytes[8..12].copy_from_slice(&packet[44..48]);

    let src_pub_hex = encode_hex(&src_pub);
    if !replay_windows
        .get(&src_pub_hex)
        .map_or(true, |w| w.would_accept(seq))
    {
        return Err(format!("TSWG replay/stale packet rejected (seq={seq})"));
    }

    let (mut tx_key, mut rx_key) = derive_peer_cipher_keys(my_secret, my_pub, &src_pub, TSWG_DOMAIN);
    tx_key.zeroize();

    let cipher = ChaCha20Poly1305::new_from_slice(&rx_key)
        .map_err(|e| format!("Invalid ChaCha20Poly1305 key: {e}"))?;
    rx_key.zeroize();

    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), &packet[48..])
        .map_err(|_| "TSWG AEAD authentication failed".to_string())?;

    if !replay_windows
        .entry(src_pub_hex.clone())
        .or_default()
        .check_and_mark(seq)
    {
        return Err(format!("TSWG replay rejected (seq={seq})"));
    }

    Ok((src_pub_hex, plaintext))
}

/// Encrypts a `TSDC` Disco envelope:
/// `b"TSDC" (4B) || src_disco_pub (32B) || nonce (12B) || ChaCha20Poly1305(DiscoPayload JSON)`
pub fn seal_tsdc_packet(
    my_disco_secret: &StaticSecret,
    my_disco_pub: &[u8; 32],
    peer_disco_pub: &[u8; 32],
    payload: &DiscoPayload,
) -> Result<Vec<u8>, String> {
    let json = serde_json::to_vec(payload)
        .map_err(|e| format!("Failed to serialize DiscoPayload: {e}"))?;
    let (mut tx_key, mut rx_key) =
        derive_peer_cipher_keys(my_disco_secret, my_disco_pub, peer_disco_pub, TSDC_DOMAIN);
    rx_key.zeroize();

    let cipher = ChaCha20Poly1305::new_from_slice(&tx_key)
        .map_err(|e| format!("Invalid TSDC ChaCha20Poly1305 key: {e}"))?;
    tx_key.zeroize();

    let nonce_bytes = random_bytes::<12>();
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), json.as_slice())
        .map_err(|e| format!("TSDC AEAD encrypt failed: {e}"))?;

    let mut out = Vec::with_capacity(TSDC_HEADER_LEN + ciphertext.len());
    out.extend_from_slice(TSDC_MAGIC);
    out.extend_from_slice(my_disco_pub);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypts a `TSDC` Disco envelope, returning `(src_disco_pub_hex, DiscoPayload)`.
pub fn open_tsdc_packet(
    my_disco_secret: &StaticSecret,
    my_disco_pub: &[u8; 32],
    packet: &[u8],
) -> Result<(String, DiscoPayload), String> {
    if packet.len() < TSDC_HEADER_LEN + 16 || &packet[..4] != TSDC_MAGIC {
        return Err("Malformed TSDC packet".to_string());
    }
    let mut src_disco_pub = [0u8; 32];
    src_disco_pub.copy_from_slice(&packet[4..36]);
    let nonce = Nonce::from_slice(&packet[36..48]);

    let (mut tx_key, mut rx_key) =
        derive_peer_cipher_keys(my_disco_secret, my_disco_pub, &src_disco_pub, TSDC_DOMAIN);
    tx_key.zeroize();

    let cipher = ChaCha20Poly1305::new_from_slice(&rx_key)
        .map_err(|e| format!("Invalid TSDC ChaCha20Poly1305 key: {e}"))?;
    rx_key.zeroize();

    let plaintext = cipher
        .decrypt(nonce, &packet[48..])
        .map_err(|_| "TSDC AEAD authentication failed".to_string())?;
    let payload: DiscoPayload = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("Invalid DiscoPayload JSON: {e}"))?;
    Ok((encode_hex(&src_disco_pub), payload))
}

/// Splits packets larger than `1100` bytes into `TSFR` UDP chunks (`<= 4MB` total).
pub fn fragment_udp_packet(msg_id: u64, packet: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if packet.len() <= UDP_FRAG_CHUNK_SIZE {
        return Ok(vec![packet.to_vec()]);
    }
    if packet.len() > UDP_FRAG_MAX_TOTAL_BYTES {
        return Err(format!(
            "UDP packet size {} exceeds 4MB limit",
            packet.len()
        ));
    }
    let total = packet.chunks(UDP_FRAG_CHUNK_SIZE).len();
    if total > u16::MAX as usize {
        return Err("Too many UDP fragments".to_string());
    }
    let total_u16 = total as u16;
    let mut frags = Vec::with_capacity(total);
    for (idx, chunk) in packet.chunks(UDP_FRAG_CHUNK_SIZE).enumerate() {
        let mut frame = Vec::with_capacity(TSFR_HEADER_LEN + chunk.len());
        frame.extend_from_slice(TSFR_MAGIC);
        frame.extend_from_slice(&msg_id.to_be_bytes());
        frame.extend_from_slice(&(idx as u16).to_be_bytes());
        frame.extend_from_slice(&total_u16.to_be_bytes());
        frame.extend_from_slice(chunk);
        frags.push(frame);
    }
    Ok(frags)
}

#[derive(Debug)]
struct FragAssembly {
    created_at: Instant,
    frag_total: u16,
    slots: Vec<Option<Vec<u8>>>,
    received_count: usize,
    total_bytes: usize,
}

/// Reassembles `TSFR` UDP chunks with 2s TTL cleanup and 4MB memory ceiling.
#[derive(Debug, Default)]
pub struct UdpFragmentAssembler {
    pending: HashMap<(SocketAddr, u64), FragAssembly>,
}

impl UdpFragmentAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prune_expired(&mut self) {
        let now = Instant::now();
        self.pending
            .retain(|_, entry| now.duration_since(entry.created_at) <= UDP_FRAG_TTL);
    }

    pub fn ingest(&mut self, from: SocketAddr, datagram: &[u8]) -> Option<Vec<u8>> {
        self.prune_expired();

        if datagram.len() < TSFR_HEADER_LEN || &datagram[..4] != TSFR_MAGIC {
            return None;
        }
        let mut msg_id_bytes = [0u8; 8];
        msg_id_bytes.copy_from_slice(&datagram[4..12]);
        let msg_id = u64::from_be_bytes(msg_id_bytes);
        let frag_idx = u16::from_be_bytes([datagram[12], datagram[13]]);
        let frag_total = u16::from_be_bytes([datagram[14], datagram[15]]);
        let payload = &datagram[TSFR_HEADER_LEN..];

        if frag_total == 0
            || frag_idx >= frag_total
            || payload.len() > UDP_FRAG_CHUNK_SIZE
            || (frag_total > 1 && payload.is_empty())
            || (frag_total as usize).saturating_mul(UDP_FRAG_CHUNK_SIZE)
                > UDP_FRAG_MAX_TOTAL_BYTES + UDP_FRAG_CHUNK_SIZE
        {
            return None;
        }

        let key = (from, msg_id);
        if !self.pending.contains_key(&key) {
            if self.pending.len() >= MAX_PENDING_FRAG_ASSEMBLIES {
                if let Some(oldest_key) = self
                    .pending
                    .iter()
                    .min_by_key(|(_, v)| v.created_at)
                    .map(|(k, _)| *k)
                {
                    self.pending.remove(&oldest_key);
                }
            }
            self.pending.insert(
                key,
                FragAssembly {
                    created_at: Instant::now(),
                    frag_total,
                    slots: vec![None; frag_total as usize],
                    received_count: 0,
                    total_bytes: 0,
                },
            );
        }

        let entry = self.pending.get_mut(&key)?;
        if entry.frag_total != frag_total {
            self.pending.remove(&key);
            return None;
        }

        let idx = frag_idx as usize;
        if entry.slots[idx].is_none() {
            let next_total = entry.total_bytes.saturating_add(payload.len());
            if next_total > UDP_FRAG_MAX_TOTAL_BYTES {
                self.pending.remove(&key);
                return None;
            }
            entry.slots[idx] = Some(payload.to_vec());
            entry.received_count += 1;
            entry.total_bytes = next_total;
        }

        if entry.received_count == entry.frag_total as usize {
            let completed = self.pending.remove(&key)?;
            let mut assembled = Vec::with_capacity(completed.total_bytes);
            for slot in completed.slots {
                assembled.extend_from_slice(&slot?);
            }
            return Some(assembled);
        }

        None
    }
}

#[derive(Debug, Clone)]
pub struct PeerLinkState {
    pub node_id: String,
    pub device_id: String,
    pub tailnet_ip: String,
    pub lan_ip: String,
    pub relay_mode: TailnetRelayMode,
    pub active_endpoint: Option<String>,
    pub active_socket_addr: Option<SocketAddr>,
    pub rtt_ms: Option<u32>,
    pub last_direct_pong_ms: Option<i64>,
    pub online: bool,
}

struct LocalPendingApproval {
    dto: PendingComputerCommandDto,
    decision_tx: oneshot::Sender<bool>,
}

#[derive(Clone)]
struct RuntimeConfigSnapshot {
    base_url: String,
    token: String,
    http_client: HttpClient,
    runtime_handle: Handle,
}

/// Shared pure-Rust Tailscale-like userspace mesh engine embedded inside `TelegramClient`.
pub struct TailnetEngine {
    identity: RwLock<TailnetIdentity>,
    device_id_override: RwLock<Option<String>>,
    force_derp_only: AtomicBool,
    running: AtomicBool,
    run_generation: AtomicU64,
    owner_user_id: AtomicU64,
    netmap: RwLock<Option<TailnetNetMap>>,
    peer_states: RwLock<HashMap<String, PeerLinkState>>,
    derp_connected: AtomicBool,
    derp_rtt_ms: Mutex<Option<u32>>,
    local_udp_port: AtomicU16,
    cached_stun_endpoint: RwLock<Option<TailnetEndpoint>>,
    tx_seq: AtomicU64,
    frag_msg_seq: AtomicU64,
    derp_tx: RwLock<Option<mpsc::UnboundedSender<DerpClientFrame>>>,
    udp_socket: RwLock<Option<Arc<UdpSocket>>>,
    pending_approvals: Mutex<HashMap<String, LocalPendingApproval>>,
    inflight_rpc: Mutex<HashMap<String, mpsc::UnboundedSender<PeerApiFrame>>>,
    recent_exec_requests: Mutex<HashMap<String, Instant>>,
    replay_windows: Mutex<HashMap<String, ReplayWindow>>,
    frag_assembler: Mutex<UdpFragmentAssembler>,
    disco_pings: Mutex<HashMap<String, (Instant, String)>>,
    disco_notify: Arc<Notify>,
    stop_notify: Arc<Notify>,
    runtime_cfg: RwLock<Option<RuntimeConfigSnapshot>>,
}

impl TailnetEngine {
    pub fn new(identity: TailnetIdentity) -> Arc<Self> {
        let frag_seed = u64::from_le_bytes(random_bytes::<8>()).max(1);
        Arc::new(Self {
            identity: RwLock::new(identity),
            device_id_override: RwLock::new(None),
            force_derp_only: AtomicBool::new(false),
            running: AtomicBool::new(false),
            run_generation: AtomicU64::new(0),
            owner_user_id: AtomicU64::new(0),
            netmap: RwLock::new(None),
            peer_states: RwLock::new(HashMap::new()),
            derp_connected: AtomicBool::new(false),
            derp_rtt_ms: Mutex::new(None),
            local_udp_port: AtomicU16::new(0),
            cached_stun_endpoint: RwLock::new(None),
            tx_seq: AtomicU64::new(1),
            frag_msg_seq: AtomicU64::new(frag_seed),
            derp_tx: RwLock::new(None),
            udp_socket: RwLock::new(None),
            pending_approvals: Mutex::new(HashMap::new()),
            inflight_rpc: Mutex::new(HashMap::new()),
            recent_exec_requests: Mutex::new(HashMap::new()),
            replay_windows: Mutex::new(HashMap::new()),
            frag_assembler: Mutex::new(UdpFragmentAssembler::new()),
            disco_pings: Mutex::new(HashMap::new()),
            disco_notify: Arc::new(Notify::new()),
            stop_notify: Arc::new(Notify::new()),
            runtime_cfg: RwLock::new(None),
        })
    }

    pub fn identity_snapshot(&self) -> TailnetIdentity {
        self.identity.read().clone()
    }

    pub fn bind_identity(&self, identity: TailnetIdentity) {
        *self.identity.write() = identity;
        *self.device_id_override.write() = None;
    }

    pub fn device_id(&self) -> String {
        if let Some(ref ov) = *self.device_id_override.read() {
            if !ov.trim().is_empty() {
                return ov.clone();
            }
        }
        self.identity.read().device_id.clone()
    }

    pub fn set_device_id_override(&self, id: Option<String>) {
        let clean = id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        *self.device_id_override.write() = clean;
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn set_force_derp_only(&self, force: bool) {
        self.force_derp_only.store(force, Ordering::Release);
        if force {
            let derp_up = self.derp_connected.load(Ordering::Acquire);
            let mut states = self.peer_states.write();
            for st in states.values_mut() {
                if st.relay_mode == TailnetRelayMode::DirectUdp {
                    st.relay_mode = if !st.online {
                        TailnetRelayMode::Offline
                    } else if derp_up {
                        TailnetRelayMode::DerpRelay
                    } else {
                        TailnetRelayMode::HttpFallback
                    };
                    st.active_endpoint = None;
                    st.active_socket_addr = None;
                    st.last_direct_pong_ms = None;
                }
            }
        } else {
            self.disco_notify.notify_one();
        }
    }

    pub fn build_local_endpoints(&self) -> Vec<TailnetEndpoint> {
        let udp_port = self.local_udp_port.load(Ordering::Acquire);
        let mut endpoints = Vec::new();
        let mut seen = HashSet::new();

        if udp_port > 0 {
            for ip in detect_real_lan_ips() {
                let addr = format!("{ip}:{udp_port}");
                if seen.insert(addr.clone()) {
                    endpoints.push(TailnetEndpoint {
                        addr,
                        kind: TailnetEndpointKind::Lan,
                    });
                }
            }
            let loopback = format!("127.0.0.1:{udp_port}");
            if seen.insert(loopback.clone()) {
                endpoints.push(TailnetEndpoint {
                    addr: loopback,
                    kind: TailnetEndpointKind::Loopback,
                });
            }
        }

        if let Some(stun_ep) = self.cached_stun_endpoint.read().clone() {
            if seen.insert(stun_ep.addr.clone()) {
                endpoints.push(stun_ep);
            }
        }

        endpoints
    }

    pub fn build_register_request(&self, owner_user_id: u64) -> TailnetRegisterRequest {
        let id = self.identity.read().clone();
        let effective_device_id = self.device_id();
        let (lan_ip, os_name, resolution) = detect_local_host_info();
        let display_name = detect_local_hostname();
        let endpoints = self.build_local_endpoints();

        let mut req = TailnetRegisterRequest {
            device_id: effective_device_id,
            display_name,
            lan_ip,
            os_name,
            resolution,
            machine_public_key: id.machine_public_key_hex.clone(),
            node_public_key: id.node_public_key_hex.clone(),
            disco_public_key: id.disco_public_key_hex.clone(),
            endpoints,
            timestamp_ms: now_ms(),
            signature_hex: None,
        };
        let sig = id.sign_registration(owner_user_id, &req);
        req.signature_hex = Some(sig);
        req
    }

    pub fn apply_netmap(&self, netmap: TailnetNetMap) {
        if netmap.owner_user_id > 0 {
            self.owner_user_id
                .store(netmap.owner_user_id, Ordering::Release);
        }

        let my_node_pub = self.identity.read().node_public_key_hex.clone();
        let my_device_id = self.device_id();
        let force_derp = self.force_derp_only.load(Ordering::Acquire);
        let now = now_ms();
        let mut should_probe_disco = false;

        {
            let mut states = self.peer_states.write();
            let valid_keys: HashSet<String> = netmap
                .peers
                .iter()
                .filter(|p| {
                    !p.node_public_key.is_empty()
                        && p.node_public_key != my_node_pub
                        && p.device_id != my_device_id
                })
                .map(|p| p.node_public_key.clone())
                .collect();
            states.retain(|k, _| valid_keys.contains(k));

            for peer in &netmap.peers {
                if peer.node_public_key.is_empty()
                    || peer.node_public_key == my_node_pub
                    || peer.device_id == my_device_id
                {
                    continue;
                }

                let prev = states.get(&peer.node_public_key).cloned();
                let keep_direct = !force_derp
                    && peer.online
                    && prev.as_ref().is_some_and(|st| {
                        st.relay_mode == TailnetRelayMode::DirectUdp
                            && st.active_socket_addr.is_some()
                            && st
                                .last_direct_pong_ms
                                .is_some_and(|ts| now.saturating_sub(ts) <= DIRECT_UDP_FRESH_MS)
                    });

                let next_state = if keep_direct {
                    let mut st = prev.expect("checked");
                    st.node_id = peer.node_id.clone();
                    st.device_id = peer.device_id.clone();
                    st.tailnet_ip = peer.tailnet_ip.clone();
                    st.lan_ip = peer.lan_ip.clone();
                    st.online = peer.online;
                    st
                } else {
                    if peer.online && !force_derp {
                        should_probe_disco = true;
                    }
                    let relay_mode = if !peer.online {
                        TailnetRelayMode::Offline
                    } else if peer.derp_connected && self.derp_connected.load(Ordering::Acquire) {
                        TailnetRelayMode::DerpRelay
                    } else if peer.derp_connected {
                        TailnetRelayMode::DerpRelay
                    } else {
                        TailnetRelayMode::HttpFallback
                    };
                    PeerLinkState {
                        node_id: peer.node_id.clone(),
                        device_id: peer.device_id.clone(),
                        tailnet_ip: peer.tailnet_ip.clone(),
                        lan_ip: peer.lan_ip.clone(),
                        relay_mode,
                        active_endpoint: None,
                        active_socket_addr: None,
                        rtt_ms: peer.latency_ms,
                        last_direct_pong_ms: None,
                        online: peer.online,
                    }
                };
                states.insert(peer.node_public_key.clone(), next_state);
            }
        }

        *self.netmap.write() = Some(netmap);

        if should_probe_disco && self.running.load(Ordering::Acquire) {
            self.disco_notify.notify_one();
        }
    }

    /// Synchronizes policy/controller updates returned by `PATCH /api/v1/computers/{node_id}/policy`
    /// into the local `netmap` cache immediately to avoid any WebSocket race window.
    pub fn sync_node_policy_from_dto(&self, dto: &gpui_telegram_proto::computer_contracts::ComputerNodeDto) {
        let mut guard = self.netmap.write();
        if let Some(ref mut nm) = *guard {
            if let Some(ref mut sp) = nm.self_peer {
                if sp.node_id == dto.node_id {
                    sp.run_policy = dto.run_policy;
                    sp.authorized_controllers = dto.authorized_controllers.clone();
                    sp.display_name = dto.display_name.clone();
                }
            }
            for peer in &mut nm.peers {
                if peer.node_id == dto.node_id {
                    peer.run_policy = dto.run_policy;
                    peer.authorized_controllers = dto.authorized_controllers.clone();
                    peer.display_name = dto.display_name.clone();
                }
            }
        }
    }

    pub fn status(&self) -> TailnetNodeStatus {
        let running = self.running.load(Ordering::Acquire);
        let id = self.identity.read().clone();
        let device_id = self.device_id();
        let netmap = self.netmap.read().clone();
        let force_derp = self.force_derp_only.load(Ordering::Acquire);
        let derp_connected = self.derp_connected.load(Ordering::Acquire);
        let local_udp_port = self.local_udp_port.load(Ordering::Acquire);
        let derp_rtt_ms = *self.derp_rtt_ms.lock();

        let tailnet_ip = netmap
            .as_ref()
            .and_then(|nm| nm.self_peer.as_ref())
            .map(|p| p.tailnet_ip.clone())
            .unwrap_or_default();

        let lan_ip = netmap
            .as_ref()
            .and_then(|nm| nm.self_peer.as_ref())
            .map(|p| p.lan_ip.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| detect_local_host_info().0);

        let netmap_seq = netmap.as_ref().map(|nm| nm.seq).unwrap_or(0);

        let mut peers: Vec<TailnetPeerLiveState> = self
            .peer_states
            .read()
            .values()
            .map(|st| TailnetPeerLiveState {
                node_id: st.node_id.clone(),
                device_id: st.device_id.clone(),
                tailnet_ip: st.tailnet_ip.clone(),
                lan_ip: st.lan_ip.clone(),
                relay_mode: st.relay_mode,
                active_endpoint: st.active_endpoint.clone(),
                rtt_ms: st.rtt_ms,
                online: st.online,
            })
            .collect();
        peers.sort_by(|a, b| a.node_id.cmp(&b.node_id));

        let any_direct = !force_derp
            && peers
                .iter()
                .any(|p| p.online && p.relay_mode == TailnetRelayMode::DirectUdp);

        let relay_mode = if !running {
            TailnetRelayMode::Offline
        } else if any_direct {
            TailnetRelayMode::DirectUdp
        } else if derp_connected {
            TailnetRelayMode::DerpRelay
        } else {
            TailnetRelayMode::HttpFallback
        };

        TailnetNodeStatus {
            running,
            device_id,
            tailnet_ip,
            lan_ip,
            machine_public_key: id.machine_public_key_hex,
            node_public_key: id.node_public_key_hex,
            local_udp_port,
            derp_connected,
            relay_mode,
            derp_rtt_ms,
            netmap_seq,
            peers,
        }
    }

    pub fn pending_approvals(&self) -> Vec<PendingComputerCommandDto> {
        let mut items: Vec<PendingComputerCommandDto> = self
            .pending_approvals
            .lock()
            .values()
            .map(|entry| entry.dto.clone())
            .collect();
        items.sort_by_key(|d| d.created_at_ms);
        items
    }

    pub fn decide_approval(
        &self,
        command_id: &str,
        approved: bool,
    ) -> Option<std::result::Result<(), String>> {
        let entry = self.pending_approvals.lock().remove(command_id)?;
        if entry.decision_tx.send(approved).is_err() {
            return Some(Err("Pending approval waiter already expired".to_string()));
        }
        Some(Ok(()))
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
        self.run_generation.fetch_add(1, Ordering::AcqRel);
        self.stop_notify.notify_waiters();
        self.derp_connected.store(false, Ordering::Release);
        self.local_udp_port.store(0, Ordering::Release);
        self.owner_user_id.store(0, Ordering::Release);
        *self.derp_rtt_ms.lock() = None;
        *self.derp_tx.write() = None;
        *self.udp_socket.write() = None;
        *self.runtime_cfg.write() = None;
        *self.netmap.write() = None;
        *self.cached_stun_endpoint.write() = None;
        self.peer_states.write().clear();
        self.pending_approvals.lock().clear();
        self.inflight_rpc.lock().clear();
        self.recent_exec_requests.lock().clear();
        self.replay_windows.lock().clear();
        self.frag_assembler.lock().pending.clear();
        self.disco_pings.lock().clear();
    }

    /// Starts the embedded Tailnet engine (UDP Magicsock, STUN, signed registration,
    /// 20s keepalive, DERP WebSocket relay, and Disco NAT hole punching).
    pub fn start(
        self: &Arc<Self>,
        base_url: String,
        token: String,
        user_id: Option<u64>,
        http_client: HttpClient,
        runtime_handle: Handle,
    ) {
        let clean_base = base_url.trim().trim_end_matches('/').to_string();
        let clean_token = token.trim().to_string();
        if clean_base.is_empty() || clean_token.is_empty() {
            return;
        }

        if self.running.load(Ordering::Acquire) {
            let same_cfg = self.runtime_cfg.read().as_ref().is_some_and(|cfg| {
                cfg.base_url == clean_base && cfg.token == clean_token
            });
            if same_cfg {
                return;
            }
        }

        let my_gen = self.run_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.stop_notify.notify_waiters();
        self.running.store(true, Ordering::Release);
        if let Some(uid) = user_id.filter(|&u| u > 0) {
            self.owner_user_id.store(uid, Ordering::Release);
        }

        *self.runtime_cfg.write() = Some(RuntimeConfigSnapshot {
            base_url: clean_base.clone(),
            token: clean_token.clone(),
            http_client: http_client.clone(),
            runtime_handle: runtime_handle.clone(),
        });

        let engine = Arc::clone(self);
        runtime_handle.clone().spawn(async move {
            engine
                .run_supervisor(my_gen, clean_base, clean_token, user_id, http_client, runtime_handle)
                .await;
        });
    }

    fn is_gen_active(&self, my_gen: u64) -> bool {
        self.running.load(Ordering::Acquire) && self.run_generation.load(Ordering::Acquire) == my_gen
    }

    async fn run_supervisor(
        self: Arc<Self>,
        my_gen: u64,
        base_url: String,
        token: String,
        initial_user_id: Option<u64>,
        http_client: HttpClient,
        runtime_handle: Handle,
    ) {
        // 1. Bind Magicsock UDP socket on ephemeral port.
        match UdpSocket::bind("0.0.0.0:0").await {
            Ok(sock) => {
                let port = sock.local_addr().map(|a| a.port()).unwrap_or(0);
                let arc_sock = Arc::new(sock);
                self.local_udp_port.store(port, Ordering::Release);
                *self.udp_socket.write() = Some(Arc::clone(&arc_sock));
                info!("Tailnet Magicsock UDP bound on 0.0.0.0:{}", port);

                let engine_udp = Arc::clone(&self);
                let sock_rx = Arc::clone(&arc_sock);
                runtime_handle.spawn(async move {
                    engine_udp.run_udp_recv_loop(my_gen, sock_rx).await;
                });
            }
            Err(err) => {
                warn!("Tailnet UDP bind failed (continuing with DERP/HTTP): {}", err);
                self.local_udp_port.store(0, Ordering::Release);
                *self.udp_socket.write() = None;
            }
        }

        if !self.is_gen_active(my_gen) {
            return;
        }

        // 2. Refresh STUN reflexive endpoint (best effort).
        self.refresh_stun_endpoint(&http_client, &token).await;

        // 3. Resolve owner_user_id if needed for Ed25519 registration signature.
        let mut resolved_user_id = initial_user_id
            .filter(|&u| u > 0)
            .unwrap_or_else(|| self.owner_user_id.load(Ordering::Acquire));
        if resolved_user_id == 0 {
            if let Ok(me) = http_client.get_me(&token).await {
                if me.user_id > 0 {
                    resolved_user_id = me.user_id;
                    self.owner_user_id.store(me.user_id, Ordering::Release);
                }
            }
        }

        // 4. Initial signed registration (falling back to /api/v1/computers/local/register on 404/405).
        self.perform_registration(&http_client, &token, resolved_user_id)
            .await;

        if !self.is_gen_active(my_gen) {
            return;
        }

        // 5. Spawn DERP WebSocket loop.
        {
            let engine_derp = Arc::clone(&self);
            let base = base_url.clone();
            let tok = token.clone();
            runtime_handle.spawn(async move {
                engine_derp.run_derp_ws_loop(my_gen, base, tok).await;
            });
        }

        // 6. Spawn 20s keepalive & endpoint refresh loop.
        {
            let engine_keepalive = Arc::clone(&self);
            let http = http_client.clone();
            let tok = token.clone();
            runtime_handle.spawn(async move {
                engine_keepalive
                    .run_keepalive_register_loop(my_gen, http, tok)
                    .await;
            });
        }

        // 7. Spawn Disco NAT hole-punching loop.
        {
            let engine_disco = Arc::clone(&self);
            runtime_handle.spawn(async move {
                engine_disco.run_disco_loop(my_gen).await;
            });
        }
    }

    async fn refresh_stun_endpoint(&self, http_client: &HttpClient, token: &str) {
        if let Ok(Ok(stun)) =
            tokio::time::timeout(Duration::from_secs(2), http_client.get_tailnet_stun(token)).await
        {
            let trimmed = stun.observed_ip.trim();
            if !trimmed.is_empty() && stun.observed_port > 0 {
                if let Ok(ip) = trimmed.parse::<IpAddr>() {
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        *self.cached_stun_endpoint.write() = Some(TailnetEndpoint {
                            addr: format!("{ip}:{}", stun.observed_port),
                            kind: TailnetEndpointKind::StunReflexive,
                        });
                    }
                }
            }
        }
    }

    async fn perform_registration(&self, http_client: &HttpClient, token: &str, user_id: u64) {
        let req = self.build_register_request(user_id);
        match http_client.register_tailnet(token, &req).await {
            Ok(netmap) => {
                self.apply_netmap(netmap);
            }
            Err(HttpClientError::Api {
                status: 404 | 405, ..
            }) => {
                debug!("Tailnet register endpoint unavailable (404/405); using HTTP fallback");
                let _ = http_client.register_current_host_computer(token).await;
            }
            Err(err) => {
                debug!("Tailnet registration attempt failed: {}", err);
            }
        }
    }

    async fn run_keepalive_register_loop(
        self: Arc<Self>,
        my_gen: u64,
        http_client: HttpClient,
        token: String,
    ) {
        let mut interval = tokio::time::interval(KEEPALIVE_REGISTER_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await; // initial registration already ran

        while self.is_gen_active(my_gen) {
            tokio::select! {
                _ = interval.tick() => {}
                _ = self.stop_notify.notified() => break,
            }
            if !self.is_gen_active(my_gen) {
                break;
            }
            self.refresh_stun_endpoint(&http_client, &token).await;
            let mut uid = self.owner_user_id.load(Ordering::Acquire);
            if uid == 0 {
                if let Ok(me) = http_client.get_me(&token).await {
                    if me.user_id > 0 {
                        uid = me.user_id;
                        self.owner_user_id.store(uid, Ordering::Release);
                    }
                }
            }
            self.perform_registration(&http_client, &token, uid).await;
        }
    }

    fn format_derp_ws_url(&self, base_url: &str, token: &str) -> String {
        let derp_path = self
            .netmap
            .read()
            .as_ref()
            .map(|nm| nm.derp_ws_path.trim().to_string())
            .filter(|p| !p.is_empty() && p.starts_with('/'))
            .unwrap_or_else(|| "/api/v1/tailnet/derp".to_string());

        let clean = base_url.trim().trim_end_matches('/');
        let ws_base = if clean.starts_with("https://") {
            clean.replacen("https://", "wss://", 1)
        } else if clean.starts_with("http://") {
            clean.replacen("http://", "ws://", 1)
        } else {
            format!("ws://{}", clean)
        };
        format!(
            "{}{}?token={}",
            ws_base,
            derp_path,
            percent_encode_rfc3986(token)
        )
    }

    async fn run_derp_ws_loop(self: Arc<Self>, my_gen: u64, base_url: String, token: String) {
        let mut backoff_ms = 250u64;

        while self.is_gen_active(my_gen) {
            let ws_url = self.format_derp_ws_url(&base_url, &token);
            match connect_async(&ws_url).await {
                Ok((ws_stream, _)) => {
                    info!("Tailnet DERP WebSocket connected: {}", ws_url);
                    backoff_ms = 250;
                    self.derp_connected.store(true, Ordering::Release);

                    let (mut ws_write, mut ws_read) = ws_stream.split();
                    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<DerpClientFrame>();
                    *self.derp_tx.write() = Some(out_tx.clone());

                    let id = self.identity.read().clone();
                    let endpoints = self.build_local_endpoints();
                    let hello = DerpClientFrame::Hello {
                        device_id: self.device_id(),
                        node_public_key: id.node_public_key_hex,
                        disco_public_key: id.disco_public_key_hex,
                        endpoints: endpoints.clone(),
                    };
                    if let Ok(json) = serde_json::to_string(&hello) {
                        if ws_write.send(Message::Text(json.into())).await.is_err() {
                            self.derp_connected.store(false, Ordering::Release);
                            *self.derp_tx.write() = None;
                            continue;
                        }
                    }

                    // Send an immediate KeepAlive ping so `derp_rtt_ms` is measured immediately.
                    let init_ka = DerpClientFrame::KeepAlive {
                        client_time_ms: now_ms(),
                        endpoints,
                    };
                    if let Ok(json) = serde_json::to_string(&init_ka) {
                        let _ = ws_write.send(Message::Text(json.into())).await;
                    }

                    let mut ping_interval = tokio::time::interval(DERP_PING_INTERVAL);
                    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    ping_interval.tick().await;

                    while self.is_gen_active(my_gen) {
                        tokio::select! {
                            _ = self.stop_notify.notified() => break,
                            _ = ping_interval.tick() => {
                                let ka = DerpClientFrame::KeepAlive {
                                    client_time_ms: now_ms(),
                                    endpoints: self.build_local_endpoints(),
                                };
                                if let Ok(json) = serde_json::to_string(&ka) {
                                    if ws_write.send(Message::Text(json.into())).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            maybe_out = out_rx.recv() => {
                                match maybe_out {
                                    Some(frame) => {
                                        if let Ok(json) = serde_json::to_string(&frame) {
                                            if ws_write.send(Message::Text(json.into())).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                    None => break,
                                }
                            }
                            maybe_in = ws_read.next() => {
                                match maybe_in {
                                    Some(Ok(Message::Text(text))) => {
                                        match serde_json::from_str::<DerpServerFrame>(&text) {
                                            Ok(frame) => {
                                                self.handle_derp_server_frame(my_gen, frame, &out_tx);
                                            }
                                            Err(err) => {
                                                debug!("Failed to parse DerpServerFrame: {}", err);
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Ping(data))) => {
                                        let _ = ws_write.send(Message::Pong(data)).await;
                                    }
                                    Some(Ok(Message::Close(_))) | None => break,
                                    Some(Err(err)) => {
                                        debug!("Tailnet DERP WebSocket read error: {}", err);
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    self.derp_connected.store(false, Ordering::Release);
                    *self.derp_tx.write() = None;
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(resp))
                    if resp.status()
                        == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED =>
                {
                    warn!("Tailnet DERP handshake rejected (401 Unauthorized); stopping loop");
                    self.derp_connected.store(false, Ordering::Release);
                    break;
                }
                Err(err) => {
                    debug!("Tailnet DERP WebSocket connect error: {}", err);
                    self.derp_connected.store(false, Ordering::Release);
                }
            }

            if !self.is_gen_active(my_gen) {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                _ = self.stop_notify.notified() => break,
            }
            backoff_ms = (backoff_ms * 2).min(15_000);
        }
    }

    fn handle_derp_server_frame(
        self: &Arc<Self>,
        my_gen: u64,
        frame: DerpServerFrame,
        out_tx: &mpsc::UnboundedSender<DerpClientFrame>,
    ) {
        match frame {
            DerpServerFrame::Welcome { netmap, .. } => {
                self.apply_netmap(netmap);
            }
            DerpServerFrame::NetMapUpdate { netmap } => {
                self.apply_netmap(netmap);
            }
            DerpServerFrame::KeepAliveAck { client_time_ms, .. } => {
                let rtt = now_ms().saturating_sub(client_time_ms).max(1) as u32;
                *self.derp_rtt_ms.lock() = Some(rtt);
            }
            DerpServerFrame::RecvPacket {
                src_node_key: _,
                packet_b64,
            } => {
                if let Ok(bytes) = decode_base64(&packet_b64) {
                    self.handle_wire_packet(my_gen, &bytes, None);
                }
            }
            DerpServerFrame::ServerCommandDispatch { command } => {
                if command.requires_user_approval && !command.approved {
                    // Server-queued commands needing approval are surfaced via ComputerListResponse.pending_approvals.
                    return;
                }
                let engine = Arc::clone(self);
                let derp_sender = out_tx.clone();
                let handle = self
                    .runtime_cfg
                    .read()
                    .as_ref()
                    .map(|c| c.runtime_handle.clone())
                    .or_else(|| Handle::try_current().ok());
                if let Some(rt) = handle {
                    rt.spawn(async move {
                        let res = execute_local_computer_op(
                            &command.node_id,
                            &command.command_id,
                            &command.operation,
                        )
                        .await;
                        let frame = DerpClientFrame::CommandResult {
                            command_id: command.command_id.clone(),
                            result: res.clone(),
                        };
                        if derp_sender.send(frame).is_err() {
                            let cfg = engine.runtime_cfg.read().clone();
                            if let Some(c) = cfg {
                                let _ = c
                                    .http_client
                                    .complete_computer_command(&c.token, &command.command_id, &res)
                                    .await;
                            }
                        }
                    });
                }
            }
            DerpServerFrame::Error { code, message } => {
                debug!("DERP server error ({}): {}", code, message);
            }
        }
    }

    async fn run_udp_recv_loop(self: Arc<Self>, my_gen: u64, sock: Arc<UdpSocket>) {
        let mut buf = vec![0u8; 65_535];
        while self.is_gen_active(my_gen) {
            tokio::select! {
                _ = self.stop_notify.notified() => break,
                recv_res = sock.recv_from(&mut buf) => {
                    match recv_res {
                        Ok((n, from_addr)) => {
                            if n == 0 {
                                continue;
                            }
                            if self.force_derp_only.load(Ordering::Acquire) {
                                continue;
                            }
                            let datagram = &buf[..n];
                            if datagram.len() >= 4 && &datagram[..4] == TSFR_MAGIC {
                                if let Some(assembled) = self.frag_assembler.lock().ingest(from_addr, datagram) {
                                    self.handle_wire_packet(my_gen, &assembled, Some(from_addr));
                                }
                            } else {
                                self.handle_wire_packet(my_gen, datagram, Some(from_addr));
                            }
                        }
                        Err(err) => {
                            debug!("Tailnet UDP recv_from error: {}", err);
                            tokio::time::sleep(Duration::from_millis(50)).await;
                        }
                    }
                }
            }
        }
    }

    fn handle_wire_packet(
        self: &Arc<Self>,
        my_gen: u64,
        bytes: &[u8],
        from_addr: Option<SocketAddr>,
    ) {
        if bytes.len() < 4 {
            return;
        }
        if &bytes[..4] == TSDC_MAGIC {
            let id = self.identity.read().clone();
            if let Ok((src_disco_pub_hex, disco)) =
                open_tsdc_packet(&id.disco_key, &id.disco_public_bytes, bytes)
            {
                self.handle_disco_payload(my_gen, &src_disco_pub_hex, disco, from_addr);
            }
            return;
        }
        if &bytes[..4] == TSWG_MAGIC {
            let id = self.identity.read().clone();
            let decrypted = {
                let mut windows = self.replay_windows.lock();
                open_tswg_packet(&id.node_key, &id.node_public_bytes, bytes, &mut windows)
            };
            if let Ok((src_node_pub_hex, plaintext)) = decrypted {
                if let Ok(frame) = serde_json::from_slice::<PeerApiFrame>(&plaintext) {
                    self.handle_peer_api_frame(my_gen, &src_node_pub_hex, frame, from_addr);
                }
            }
        }
    }

    fn handle_disco_payload(
        self: &Arc<Self>,
        _my_gen: u64,
        src_disco_pub_hex: &str,
        disco: DiscoPayload,
        from_addr: Option<SocketAddr>,
    ) {
        let peer_opt = self.find_peer_by_disco_key(src_disco_pub_hex);
        let Some(peer) = peer_opt else {
            return;
        };

        match disco {
            DiscoPayload::Ping {
                tx_id,
                sent_at_ms,
                src_node_key: _,
            } => {
                if self.force_derp_only.load(Ordering::Acquire) {
                    return;
                }
                let Some(addr) = from_addr else {
                    return;
                };
                let id = self.identity.read().clone();
                let Ok(peer_disco_pub) = decode_hex_32(&peer.disco_public_key) else {
                    return;
                };
                let pong = DiscoPayload::Pong {
                    tx_id,
                    sent_at_ms,
                    observed_addr: addr.to_string(),
                    src_node_key: id.node_public_key_hex.clone(),
                };
                if let Ok(pkt) =
                    seal_tsdc_packet(&id.disco_key, &id.disco_public_bytes, &peer_disco_pub, &pong)
                {
                    self.spawn_udp_send(addr, pkt);
                }
            }
            DiscoPayload::Pong {
                tx_id,
                sent_at_ms,
                observed_addr: _,
                src_node_key: _,
            } => {
                if self.force_derp_only.load(Ordering::Acquire) {
                    return;
                }
                let Some(addr) = from_addr else {
                    return;
                };
                let rtt_ms = {
                    let mut pings = self.disco_pings.lock();
                    pings.retain(|_, (ts, _)| ts.elapsed() < Duration::from_secs(10));
                    if let Some((sent_inst, expected_node_key)) = pings.remove(&tx_id) {
                        if expected_node_key == peer.node_public_key {
                            sent_inst.elapsed().as_millis().max(1) as u32
                        } else {
                            now_ms().saturating_sub(sent_at_ms).max(1) as u32
                        }
                    } else {
                        now_ms().saturating_sub(sent_at_ms).max(1) as u32
                    }
                };

                let mut states = self.peer_states.write();
                let entry = states
                    .entry(peer.node_public_key.clone())
                    .or_insert_with(|| PeerLinkState {
                        node_id: peer.node_id.clone(),
                        device_id: peer.device_id.clone(),
                        tailnet_ip: peer.tailnet_ip.clone(),
                        lan_ip: peer.lan_ip.clone(),
                        relay_mode: TailnetRelayMode::DirectUdp,
                        active_endpoint: None,
                        active_socket_addr: None,
                        rtt_ms: None,
                        last_direct_pong_ms: None,
                        online: true,
                    });
                entry.relay_mode = TailnetRelayMode::DirectUdp;
                entry.active_endpoint = Some(addr.to_string());
                entry.active_socket_addr = Some(addr);
                entry.rtt_ms = Some(rtt_ms);
                entry.last_direct_pong_ms = Some(now_ms());
                entry.online = true;
            }
            DiscoPayload::CallMeMaybe {
                src_node_key: _,
                my_endpoints,
            } => {
                if self.force_derp_only.load(Ordering::Acquire) {
                    return;
                }
                self.probe_peer_udp_endpoints(&peer, &my_endpoints);
            }
        }
    }

    fn find_peer_by_disco_key(&self, disco_pub_hex: &str) -> Option<TailnetPeerDescriptor> {
        self.netmap.read().as_ref().and_then(|nm| {
            nm.peers
                .iter()
                .find(|p| p.disco_public_key == disco_pub_hex)
                .cloned()
        })
    }

    fn find_peer_by_node_key(&self, node_pub_hex: &str) -> Option<TailnetPeerDescriptor> {
        self.netmap.read().as_ref().and_then(|nm| {
            nm.peers
                .iter()
                .find(|p| p.node_public_key == node_pub_hex)
                .cloned()
        })
    }

    fn find_peer_by_node_id(&self, node_id: &str) -> Option<TailnetPeerDescriptor> {
        let my_node_pub = self.identity.read().node_public_key_hex.clone();
        let my_device_id = self.device_id();
        self.netmap.read().as_ref().and_then(|nm| {
            nm.peers
                .iter()
                .find(|p| {
                    p.node_id == node_id
                        && !p.node_public_key.is_empty()
                        && p.node_public_key != my_node_pub
                        && p.device_id != my_device_id
                })
                .cloned()
        })
    }

    async fn run_disco_loop(self: Arc<Self>, my_gen: u64) {
        let mut interval = tokio::time::interval(DISCO_PROBE_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        while self.is_gen_active(my_gen) {
            tokio::select! {
                _ = interval.tick() => {}
                _ = self.disco_notify.notified() => {}
                _ = self.stop_notify.notified() => break,
            }
            if !self.is_gen_active(my_gen) {
                break;
            }
            if self.force_derp_only.load(Ordering::Acquire) {
                continue;
            }
            self.send_disco_probes_to_online_peers();
        }
    }

    fn send_disco_probes_to_online_peers(&self) {
        let my_endpoints = self.build_local_endpoints();
        let peers: Vec<TailnetPeerDescriptor> = {
            let my_node_pub = self.identity.read().node_public_key_hex.clone();
            let my_device_id = self.device_id();
            self.netmap
                .read()
                .as_ref()
                .map(|nm| {
                    nm.peers
                        .iter()
                        .filter(|p| {
                            p.online
                                && !p.node_public_key.is_empty()
                                && !p.disco_public_key.is_empty()
                                && p.node_public_key != my_node_pub
                                && p.device_id != my_device_id
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };

        if peers.is_empty() {
            return;
        }

        let id = self.identity.read().clone();
        for peer in peers {
            let Ok(peer_disco_pub) = decode_hex_32(&peer.disco_public_key) else {
                continue;
            };
            // 1. Send CallMeMaybe over DERP so peer probes back.
            if !my_endpoints.is_empty() {
                let cmm = DiscoPayload::CallMeMaybe {
                    src_node_key: id.node_public_key_hex.clone(),
                    my_endpoints: my_endpoints.clone(),
                };
                if let Ok(pkt) = seal_tsdc_packet(
                    &id.disco_key,
                    &id.disco_public_bytes,
                    &peer_disco_pub,
                    &cmm,
                ) {
                    let _ = self.send_derp_packet(&peer.node_public_key, &pkt);
                }
            }

            // 2. Send UDP Ping probes to all peer endpoints.
            self.probe_peer_udp_endpoints(&peer, &[]);
        }
    }

    fn probe_peer_udp_endpoints(
        &self,
        peer: &TailnetPeerDescriptor,
        extra_endpoints: &[TailnetEndpoint],
    ) {
        if self.force_derp_only.load(Ordering::Acquire) {
            return;
        }
        let Some(sock) = self.udp_socket.read().clone() else {
            return;
        };
        let Ok(peer_disco_pub) = decode_hex_32(&peer.disco_public_key) else {
            return;
        };
        let id = self.identity.read().clone();

        let mut addrs: Vec<SocketAddr> = Vec::new();
        let mut seen = HashSet::new();
        for ep in extra_endpoints.iter().chain(peer.endpoints.iter()) {
            if let Ok(addr) = ep.addr.trim().parse::<SocketAddr>() {
                if addr.is_ipv4() && addr.port() > 0 && seen.insert(addr) {
                    addrs.push(addr);
                }
            }
        }
        if let Some(active) = self
            .peer_states
            .read()
            .get(&peer.node_public_key)
            .and_then(|st| st.active_socket_addr)
        {
            if active.is_ipv4() && seen.insert(active) {
                addrs.push(active);
            }
        }

        if addrs.is_empty() {
            return;
        }

        let handle = self
            .runtime_cfg
            .read()
            .as_ref()
            .map(|c| c.runtime_handle.clone())
            .or_else(|| Handle::try_current().ok());
        let Some(rt) = handle else {
            return;
        };

        let mut probes = Vec::with_capacity(addrs.len());
        {
            let mut pings = self.disco_pings.lock();
            pings.retain(|_, (ts, _)| ts.elapsed() < Duration::from_secs(10));
            for addr in addrs {
                let tx_id = Uuid::new_v4().to_string();
                let ping = DiscoPayload::Ping {
                    tx_id: tx_id.clone(),
                    sent_at_ms: now_ms(),
                    src_node_key: id.node_public_key_hex.clone(),
                };
                if let Ok(pkt) = seal_tsdc_packet(
                    &id.disco_key,
                    &id.disco_public_bytes,
                    &peer_disco_pub,
                    &ping,
                ) {
                    pings.insert(tx_id, (Instant::now(), peer.node_public_key.clone()));
                    probes.push((addr, pkt));
                }
            }
        }

        if !probes.is_empty() {
            rt.spawn(async move {
                for (addr, pkt) in probes {
                    let _ = sock.send_to(&pkt, addr).await;
                }
            });
        }
    }

    fn spawn_udp_send(&self, addr: SocketAddr, packet: Vec<u8>) {
        let Some(sock) = self.udp_socket.read().clone() else {
            return;
        };
        let handle = self
            .runtime_cfg
            .read()
            .as_ref()
            .map(|c| c.runtime_handle.clone())
            .or_else(|| Handle::try_current().ok());
        let Some(rt) = handle else {
            return;
        };
        let msg_id = self.frag_msg_seq.fetch_add(1, Ordering::Relaxed);
        rt.spawn(async move {
            let _ = send_udp_datagrams(&sock, addr, msg_id, &packet).await;
        });
    }

    fn send_derp_packet(&self, dst_node_key: &str, packet: &[u8]) -> Result<(), String> {
        let tx = self
            .derp_tx
            .read()
            .clone()
            .ok_or_else(|| "DERP relay is not connected".to_string())?;
        let frame = DerpClientFrame::SendPacket {
            dst_node_key: dst_node_key.to_string(),
            packet_b64: encode_base64(packet),
        };
        tx.send(frame)
            .map_err(|_| "DERP outbound channel closed".to_string())
    }

    fn seal_peer_api_frame(
        &self,
        peer_node_pub_hex: &str,
        frame: &PeerApiFrame,
    ) -> Result<Vec<u8>, String> {
        let peer_pub = decode_hex_32(peer_node_pub_hex)?;
        let plaintext = serde_json::to_vec(frame)
            .map_err(|e| format!("Failed to serialize PeerApiFrame: {e}"))?;
        let id = self.identity.read().clone();
        let seq = self.tx_seq.fetch_add(1, Ordering::Relaxed);
        seal_tswg_packet(&id.node_key, &id.node_public_bytes, &peer_pub, seq, &plaintext)
    }

    fn handle_peer_api_frame(
        self: &Arc<Self>,
        my_gen: u64,
        src_node_pub_hex: &str,
        frame: PeerApiFrame,
        from_addr: Option<SocketAddr>,
    ) {
        match frame {
            PeerApiFrame::ExecResponse { ref request_id, .. }
            | PeerApiFrame::ApprovalPending { ref request_id, .. } => {
                let sender = self.inflight_rpc.lock().get(request_id).cloned();
                if let Some(tx) = sender {
                    let _ = tx.send(frame);
                }
            }
            PeerApiFrame::ExecRequest {
                request_id,
                caller_device_id,
                caller_node_key,
                caller_label,
                operation,
            } => {
                // Verify claimed caller_node_key matches authenticated TSWG sender key.
                if caller_node_key != src_node_pub_hex {
                    return;
                }
                {
                    let mut recent = self.recent_exec_requests.lock();
                    recent.retain(|_, ts| ts.elapsed() < Duration::from_secs(60));
                    if recent.contains_key(&request_id) {
                        return;
                    }
                    recent.insert(request_id.clone(), Instant::now());
                }

                let engine = Arc::clone(self);
                let handle = self
                    .runtime_cfg
                    .read()
                    .as_ref()
                    .map(|c| c.runtime_handle.clone())
                    .or_else(|| Handle::try_current().ok());
                if let Some(rt) = handle {
                    rt.spawn(async move {
                        engine
                            .process_incoming_exec_request(
                                my_gen,
                                request_id,
                                caller_device_id,
                                caller_node_key,
                                caller_label,
                                operation,
                                from_addr,
                            )
                            .await;
                    });
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_incoming_exec_request(
        self: Arc<Self>,
        my_gen: u64,
        request_id: String,
        caller_device_id: String,
        caller_node_key: String,
        caller_label: String,
        operation: RemoteComputerOp,
        from_addr: Option<SocketAddr>,
    ) {
        if !self.is_gen_active(my_gen) {
            return;
        }

        // Refresh netmap once if caller is missing/offline or self_peer missing.
        let mut caller_opt = self.find_peer_by_node_key(&caller_node_key);
        let mut self_peer_opt = self
            .netmap
            .read()
            .as_ref()
            .and_then(|nm| nm.self_peer.clone());

        let mut refreshed_netmap = false;
        if caller_opt.as_ref().map_or(true, |p| !p.online) || self_peer_opt.is_none() {
            let cfg = self.runtime_cfg.read().clone();
            if let Some(c) = cfg {
                if let Ok(fresh_map) = c.http_client.get_tailnet_netmap(&c.token).await {
                    self.apply_netmap(fresh_map);
                    refreshed_netmap = true;
                    caller_opt = self.find_peer_by_node_key(&caller_node_key);
                    self_peer_opt = self
                        .netmap
                        .read()
                        .as_ref()
                        .and_then(|nm| nm.self_peer.clone());
                }
            }
        }

        let Some(caller_peer) = caller_opt else {
            return;
        };
        if caller_peer.device_id != caller_device_id {
            return;
        }

        let Some(mut self_peer) = self_peer_opt else {
            return;
        };

        let target_node_id = self_peer.node_id.clone();
        let target_device_id = self.device_id();

        if !caller_peer.online {
            let reply = PeerApiFrame::ExecResponse {
                request_id,
                result: RemoteComputerCommandResult {
                    command_id: format!("p2p:{}", Uuid::new_v4()),
                    node_id: target_node_id,
                    ok: false,
                    exit_code: 403,
                    stdout: String::new(),
                    stderr: "Caller peer is marked offline in netmap".to_string(),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: target_device_id,
                    duration_ms: 0,
                },
            };
            self.send_peer_reply(&caller_node_key, &reply, from_addr).await;
            return;
        }

        let mut auth_eval =
            evaluate_local_p2p_authorization(&self_peer, &caller_device_id, &operation);
        if !refreshed_netmap && !matches!(auth_eval, Ok(false)) {
            let cfg = self.runtime_cfg.read().clone();
            if let Some(c) = cfg {
                if let Ok(fresh_map) = c.http_client.get_tailnet_netmap(&c.token).await {
                    self.apply_netmap(fresh_map);
                    if let Some(sp) = self
                        .netmap
                        .read()
                        .as_ref()
                        .and_then(|nm| nm.self_peer.clone())
                    {
                        self_peer = sp;
                        auth_eval = evaluate_local_p2p_authorization(
                            &self_peer,
                            &caller_device_id,
                            &operation,
                        );
                    }
                }
            }
        }

        let requires_approval = match auth_eval {
            Ok(req_app) => req_app,
            Err(reason) => {
                let reply = PeerApiFrame::ExecResponse {
                    request_id,
                    result: RemoteComputerCommandResult {
                        command_id: format!("p2p:{}", Uuid::new_v4()),
                        node_id: target_node_id,
                        ok: false,
                        exit_code: 403,
                        stdout: String::new(),
                        stderr: reason,
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: target_device_id,
                        duration_ms: 0,
                    },
                };
                self.send_peer_reply(&caller_node_key, &reply, from_addr).await;
                return;
            }
        };

        let command_id = format!("p2p:{}", Uuid::new_v4());
        if requires_approval {
            let (decision_tx, decision_rx) = oneshot::channel();
            let dto = PendingComputerCommandDto {
                command_id: command_id.clone(),
                node_id: target_node_id.clone(),
                target_device_id: target_device_id.clone(),
                caller_device_id: Some(caller_device_id),
                caller_bot_id: None,
                caller_label: if caller_label.trim().is_empty() {
                    caller_peer.display_name.clone()
                } else {
                    caller_label
                },
                requires_user_approval: true,
                approved: false,
                operation: operation.clone(),
                created_at_ms: now_ms(),
            };
            self.pending_approvals.lock().insert(
                command_id.clone(),
                LocalPendingApproval { dto, decision_tx },
            );

            // Inform the caller immediately so it suppresses the 1.5s DERP retry and awaits user approval.
            let pending_notice = PeerApiFrame::ApprovalPending {
                request_id: request_id.clone(),
                command_id: command_id.clone(),
                message: "Waiting for user approval on local device".to_string(),
            };
            self.send_peer_reply(&caller_node_key, &pending_notice, from_addr)
                .await;

            let approved_res = tokio::select! {
                _ = self.stop_notify.notified() => Err(()),
                res = tokio::time::timeout(LOCAL_APPROVAL_TIMEOUT, decision_rx) => {
                    match res {
                        Ok(Ok(val)) => Ok(val),
                        _ => Err(()),
                    }
                }
            };

            self.pending_approvals.lock().remove(&command_id);

            match approved_res {
                Ok(true) => {}
                Ok(false) => {
                    let reply = PeerApiFrame::ExecResponse {
                        request_id,
                        result: RemoteComputerCommandResult {
                            command_id,
                            node_id: target_node_id,
                            ok: false,
                            exit_code: 403,
                            stdout: String::new(),
                            stderr: "Denied by device user".to_string(),
                            entries: Vec::new(),
                            frame_data_url: None,
                            executed_by: target_device_id,
                            duration_ms: 0,
                        },
                    };
                    self.send_peer_reply(&caller_node_key, &reply, from_addr).await;
                    return;
                }
                Err(()) => {
                    let reply = PeerApiFrame::ExecResponse {
                        request_id,
                        result: RemoteComputerCommandResult {
                            command_id,
                            node_id: target_node_id,
                            ok: false,
                            exit_code: 124,
                            stdout: String::new(),
                            stderr: "Timed out waiting for local device user approval".to_string(),
                            entries: Vec::new(),
                            frame_data_url: None,
                            executed_by: target_device_id,
                            duration_ms: LOCAL_APPROVAL_TIMEOUT.as_millis() as u64,
                        },
                    };
                    self.send_peer_reply(&caller_node_key, &reply, from_addr).await;
                    return;
                }
            }
        }

        let exec_res = execute_local_computer_op(&target_node_id, &command_id, &operation).await;
        let reply = PeerApiFrame::ExecResponse {
            request_id,
            result: exec_res,
        };
        self.send_peer_reply(&caller_node_key, &reply, from_addr).await;
    }

    async fn send_peer_reply(
        &self,
        dst_node_key: &str,
        frame: &PeerApiFrame,
        preferred_udp: Option<SocketAddr>,
    ) {
        let Ok(tswg_bytes) = self.seal_peer_api_frame(dst_node_key, frame) else {
            return;
        };

        let force_derp = self.force_derp_only.load(Ordering::Acquire);
        if !force_derp {
            let udp_target = preferred_udp.or_else(|| {
                self.peer_states
                    .read()
                    .get(dst_node_key)
                    .and_then(|st| {
                        if st.relay_mode == TailnetRelayMode::DirectUdp {
                            st.active_socket_addr
                        } else {
                            None
                        }
                    })
            });
            if let Some(addr) = udp_target {
                let sock_opt = self.udp_socket.read().clone();
                if let Some(sock) = sock_opt {
                    let msg_id = self.frag_msg_seq.fetch_add(1, Ordering::Relaxed);
                    if send_udp_datagrams(&sock, addr, msg_id, &tswg_bytes)
                        .await
                        .is_ok()
                    {
                        return;
                    }
                }
            }
        }

        let _ = self.send_derp_packet(dst_node_key, &tswg_bytes);
    }

    /// Executes an operation over E2E-encrypted Tailnet Direct UDP (with automatic 1.5s DERP fallback)
    /// when `req.caller_bot_id.is_none()` and `req.node_id` matches an online peer in the netmap.
    ///
    /// Returns `None` if P2P does not apply or transport failed before reaching the peer
    /// (allowing `TelegramClient` to fall back cleanly to HTTP `/api/v1/computers/exec`).
    pub async fn execute_peer_op(
        &self,
        req: &RemoteComputerCommandRequest,
    ) -> Option<RemoteComputerCommandResult> {
        if req.caller_bot_id.is_some() || !self.running.load(Ordering::Acquire) {
            return None;
        }
        let target_id = req.node_id.trim();
        if target_id.is_empty()
            || target_id.starts_with("cloud:")
            || target_id.starts_with("sandbox:")
        {
            return None;
        }

        let mut peer_opt = self.find_peer_by_node_id(target_id);
        if peer_opt.as_ref().map_or(true, |p| !p.online) {
            let cfg = self.runtime_cfg.read().clone();
            if let Some(c) = cfg {
                if let Ok(fresh_map) = c.http_client.get_tailnet_netmap(&c.token).await {
                    self.apply_netmap(fresh_map);
                    peer_opt = self.find_peer_by_node_id(target_id);
                }
            }
        }

        let peer = peer_opt?;
        if !peer.online || peer.node_public_key.is_empty() {
            return None;
        }

        let id = self.identity.read().clone();
        let request_id = format!("p2preq-{}", Uuid::new_v4());
        let caller_device_id = req
            .caller_device_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.device_id());

        let rpc = PeerApiFrame::ExecRequest {
            request_id: request_id.clone(),
            caller_device_id,
            caller_node_key: id.node_public_key_hex.clone(),
            caller_label: req.caller_label.clone(),
            operation: req.operation.clone(),
        };

        let tswg_bytes = self.seal_peer_api_frame(&peer.node_public_key, &rpc).ok()?;

        let (tx, mut rx) = mpsc::unbounded_channel::<PeerApiFrame>();
        self.inflight_rpc.lock().insert(request_id.clone(), tx);

        struct InflightCleanup<'a> {
            map: &'a Mutex<HashMap<String, mpsc::UnboundedSender<PeerApiFrame>>>,
            key: String,
        }
        impl Drop for InflightCleanup<'_> {
            fn drop(&mut self) {
                self.map.lock().remove(&self.key);
            }
        }
        let _cleanup = InflightCleanup {
            map: &self.inflight_rpc,
            key: request_id.clone(),
        };

        let force_derp = self.force_derp_only.load(Ordering::Acquire);
        let direct_addr = if !force_derp {
            self.peer_states
                .read()
                .get(&peer.node_public_key)
                .and_then(|st| {
                    if st.relay_mode == TailnetRelayMode::DirectUdp {
                        st.active_socket_addr
                    } else {
                        None
                    }
                })
        } else {
            None
        };

        let mut peer_acknowledged = false;
        let mut sent_via_derp = false;

        if let Some(addr) = direct_addr {
            let sock_opt = self.udp_socket.read().clone();
            if let Some(sock) = sock_opt {
                let msg_id = self.frag_msg_seq.fetch_add(1, Ordering::Relaxed);
                let udp_sent = send_udp_datagrams(&sock, addr, msg_id, &tswg_bytes)
                    .await
                    .is_ok();
                if udp_sent {
                    match tokio::time::timeout(DIRECT_UDP_RPC_FALLBACK_TIMEOUT, rx.recv()).await {
                        Ok(Some(PeerApiFrame::ExecResponse { result, .. })) => {
                            return Some(result);
                        }
                        Ok(Some(PeerApiFrame::ApprovalPending { .. })) => {
                            peer_acknowledged = true;
                        }
                        _ => {
                            // Direct UDP did not reply in 1.5s; fall back to DERP SendPacket.
                            if self
                                .send_derp_packet(&peer.node_public_key, &tswg_bytes)
                                .is_ok()
                            {
                                sent_via_derp = true;
                            } else {
                                return None;
                            }
                        }
                    }
                } else if self
                    .send_derp_packet(&peer.node_public_key, &tswg_bytes)
                    .is_ok()
                {
                    sent_via_derp = true;
                } else {
                    return None;
                }
            } else if self
                .send_derp_packet(&peer.node_public_key, &tswg_bytes)
                .is_ok()
            {
                sent_via_derp = true;
            } else {
                return None;
            }
        } else if self
            .send_derp_packet(&peer.node_public_key, &tswg_bytes)
            .is_ok()
        {
            sent_via_derp = true;
        } else {
            return None;
        }

        let _ = sent_via_derp;
        let deadline = tokio::time::Instant::now() + PEER_EXEC_WAIT_TIMEOUT;
        loop {
            let now_inst = tokio::time::Instant::now();
            if now_inst >= deadline {
                break;
            }
            let remaining = deadline - now_inst;
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(PeerApiFrame::ExecResponse { result, .. })) => {
                    return Some(result);
                }
                Ok(Some(PeerApiFrame::ApprovalPending { .. })) => {
                    peer_acknowledged = true;
                    continue;
                }
                Ok(None) | Err(_) => break,
                Ok(Some(PeerApiFrame::ExecRequest { .. })) => continue,
            }
        }

        if peer_acknowledged {
            return Some(RemoteComputerCommandResult {
                command_id: request_id,
                node_id: peer.node_id,
                ok: false,
                exit_code: 124,
                stdout: String::new(),
                stderr: "Timed out waiting for local device user approval".to_string(),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by: peer.device_id,
                duration_ms: PEER_EXEC_WAIT_TIMEOUT.as_millis() as u64,
            });
        }

        None
    }
}

async fn send_udp_datagrams(
    sock: &UdpSocket,
    addr: SocketAddr,
    msg_id: u64,
    packet: &[u8],
) -> Result<(), String> {
    let chunks = fragment_udp_packet(msg_id, packet)?;
    let total = chunks.len();
    for (idx, chunk) in chunks.into_iter().enumerate() {
        sock.send_to(&chunk, addr)
            .await
            .map_err(|e| format!("UDP send_to failed: {e}"))?;
        if total > 16 && idx % 16 == 15 {
            tokio::task::yield_now().await;
        }
    }
    Ok(())
}

fn is_device_controller_authorized(
    entries: &[AuthorizedControllerEntry],
    caller_device_id: &str,
) -> bool {
    if entries.is_empty() {
        return true;
    }
    entries.iter().any(|entry| {
        entry.kind == AuthorizedControllerKind::Device
            && (entry.controller_id == "account_all_devices"
                || entry.controller_id == "*"
                || entry.controller_id == caller_device_id
                || entry.controller_id == format!("device:{caller_device_id}"))
    })
}

fn evaluate_local_p2p_authorization(
    self_peer: &TailnetPeerDescriptor,
    caller_device_id: &str,
    op: &RemoteComputerOp,
) -> Result<bool, String> {
    let is_same_device_owner = caller_device_id == self_peer.device_id;
    match self_peer.run_policy {
        ComputerRunPolicy::Disabled => {
            Err("Computer execution is disabled by node run policy".to_string())
        }
        ComputerRunPolicy::AskEveryTime => Ok(!is_same_device_owner),
        ComputerRunPolicy::AllowReadOnly => Ok(!is_same_device_owner && !op.is_read_only()),
        ComputerRunPolicy::AlwaysAllowAuthorized => {
            if is_same_device_owner
                || is_device_controller_authorized(
                    &self_peer.authorized_controllers,
                    caller_device_id,
                )
            {
                Ok(false)
            } else {
                Err("Caller is not authorized on this computer node".to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    #[test]
    fn test_tailnet_identity_persistence_and_signing() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let id1 = TailnetIdentity::load_or_create(temp_dir.path());
        assert!(id1.device_id.starts_with("dev-ts-"));
        assert_eq!(id1.device_id.len(), "dev-ts-".len() + 16);
        assert_eq!(id1.machine_public_key_hex.len(), 64);
        assert_eq!(id1.node_public_key_hex.len(), 64);
        assert_eq!(id1.disco_public_key_hex.len(), 64);

        let id_path = temp_dir.path().join(IDENTITY_FILE_NAME);
        assert!(id_path.is_file());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&id_path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "identity file must be 0600");
        }

        let id2 = TailnetIdentity::load_or_create(temp_dir.path());
        assert_eq!(id1.device_id, id2.device_id);
        assert_eq!(id1.machine_public_key_hex, id2.machine_public_key_hex);
        assert_eq!(id1.node_public_key_hex, id2.node_public_key_hex);
        assert_eq!(id1.disco_public_key_hex, id2.disco_public_key_hex);

        let req = TailnetRegisterRequest {
            device_id: id1.device_id.clone(),
            display_name: "TestMac".to_string(),
            lan_ip: "192.168.1.10".to_string(),
            os_name: "macOS".to_string(),
            resolution: "1440x900".to_string(),
            machine_public_key: id1.machine_public_key_hex.clone(),
            node_public_key: id1.node_public_key_hex.clone(),
            disco_public_key: id1.disco_public_key_hex.clone(),
            endpoints: Vec::new(),
            timestamp_ms: 1_730_000_000_000,
            signature_hex: None,
        };
        let sig_hex = id1.sign_registration(42, &req);
        let sig_bytes: [u8; 64] = decode_hex(&sig_hex)
            .expect("hex")
            .try_into()
            .expect("64B sig");
        let vk = VerifyingKey::from_bytes(&id1.machine_public_bytes).expect("vk");
        vk.verify(
            req.canonical_signing_message(42).as_bytes(),
            &Signature::from_bytes(&sig_bytes),
        )
        .expect("Ed25519 registration signature must verify");

        let eph = TailnetIdentity::ephemeral();
        assert_ne!(eph.device_id, id1.device_id);
    }

    #[test]
    fn test_x25519_chacha20poly1305_tswg_encryption_and_anti_replay() {
        let alice = TailnetIdentity::ephemeral();
        let bob = TailnetIdentity::ephemeral();

        let (a_tx, a_rx) = derive_peer_cipher_keys(
            &alice.node_key,
            &alice.node_public_bytes,
            &bob.node_public_bytes,
            TSWG_DOMAIN,
        );
        let (b_tx, b_rx) = derive_peer_cipher_keys(
            &bob.node_key,
            &bob.node_public_bytes,
            &alice.node_public_bytes,
            TSWG_DOMAIN,
        );
        assert_eq!(a_tx, b_rx);
        assert_eq!(a_rx, b_tx);
        assert_ne!(a_tx, a_rx);

        let pkt1 = seal_tswg_packet(
            &alice.node_key,
            &alice.node_public_bytes,
            &bob.node_public_bytes,
            1,
            b"cmd-one",
        )
        .expect("seal 1");
        let pkt2 = seal_tswg_packet(
            &alice.node_key,
            &alice.node_public_bytes,
            &bob.node_public_bytes,
            2,
            b"cmd-two",
        )
        .expect("seal 2");
        let pkt3 = seal_tswg_packet(
            &alice.node_key,
            &alice.node_public_bytes,
            &bob.node_public_bytes,
            3,
            b"cmd-three",
        )
        .expect("seal 3");

        let mut bob_windows = HashMap::new();

        // Out-of-order within 64-packet window succeeds
        let (src3, pt3) = open_tswg_packet(
            &bob.node_key,
            &bob.node_public_bytes,
            &pkt3,
            &mut bob_windows,
        )
        .expect("open 3");
        assert_eq!(src3, alice.node_public_key_hex);
        assert_eq!(pt3, b"cmd-three");

        let (_, pt1) = open_tswg_packet(
            &bob.node_key,
            &bob.node_public_bytes,
            &pkt1,
            &mut bob_windows,
        )
        .expect("open 1");
        assert_eq!(pt1, b"cmd-one");

        let (_, pt2) = open_tswg_packet(
            &bob.node_key,
            &bob.node_public_bytes,
            &pkt2,
            &mut bob_windows,
        )
        .expect("open 2");
        assert_eq!(pt2, b"cmd-two");

        // Replay must be rejected
        assert!(open_tswg_packet(
            &bob.node_key,
            &bob.node_public_bytes,
            &pkt2,
            &mut bob_windows,
        )
        .is_err());

        // Tampered packet with high seq must fail AEAD WITHOUT advancing the replay window
        let pkt100 = seal_tswg_packet(
            &alice.node_key,
            &alice.node_public_bytes,
            &bob.node_public_bytes,
            100,
            b"cmd-hundred",
        )
        .expect("seal 100");
        let mut tampered_100 = pkt100.clone();
        let last = tampered_100.len() - 1;
        tampered_100[last] ^= 0xFF;
        assert!(open_tswg_packet(
            &bob.node_key,
            &bob.node_public_bytes,
            &tampered_100,
            &mut bob_windows,
        )
        .is_err());

        // Legitimate seq 100 must still succeed after tampered packet failed
        let (_, pt100) = open_tswg_packet(
            &bob.node_key,
            &bob.node_public_bytes,
            &pkt100,
            &mut bob_windows,
        )
        .expect("open legit 100");
        assert_eq!(pt100, b"cmd-hundred");

        // Seq 4 is now 96 behind highest_seq (100 >= 64) -> rejected as stale
        let pkt4 = seal_tswg_packet(
            &alice.node_key,
            &alice.node_public_bytes,
            &bob.node_public_bytes,
            4,
            b"stale",
        )
        .expect("seal 4");
        assert!(open_tswg_packet(
            &bob.node_key,
            &bob.node_public_bytes,
            &pkt4,
            &mut bob_windows,
        )
        .is_err());
    }

    #[test]
    fn test_tsdc_disco_envelope_roundtrip() {
        let alice = TailnetIdentity::ephemeral();
        let bob = TailnetIdentity::ephemeral();
        let mallory = TailnetIdentity::ephemeral();

        let ping = DiscoPayload::Ping {
            tx_id: "tx-abc".to_string(),
            sent_at_ms: 123456,
            src_node_key: alice.node_public_key_hex.clone(),
        };
        let pkt = seal_tsdc_packet(
            &alice.disco_key,
            &alice.disco_public_bytes,
            &bob.disco_public_bytes,
            &ping,
        )
        .expect("seal tsdc");

        let (src_hex, decoded) =
            open_tsdc_packet(&bob.disco_key, &bob.disco_public_bytes, &pkt).expect("open tsdc");
        assert_eq!(src_hex, alice.disco_public_key_hex);
        assert_eq!(decoded, ping);

        assert!(open_tsdc_packet(&mallory.disco_key, &mallory.disco_public_bytes, &pkt).is_err());
    }

    #[test]
    fn test_tsfr_fragmentation_and_reassembly() {
        let small = vec![42u8; 900];
        let small_frags = fragment_udp_packet(1, &small).expect("small");
        assert_eq!(small_frags.len(), 1);
        assert_eq!(small_frags[0], small);

        let large: Vec<u8> = (0..5500).map(|i| (i % 251) as u8).collect();
        let mut frags = fragment_udp_packet(999, &large).expect("fragment 5500B");
        assert_eq!(frags.len(), 5);
        for f in &frags {
            assert!(f.starts_with(TSFR_MAGIC));
            assert!(f.len() <= TSFR_HEADER_LEN + UDP_FRAG_CHUNK_SIZE);
        }

        let peer_addr: SocketAddr = "127.0.0.1:41641".parse().unwrap();
        let mut assembler = UdpFragmentAssembler::new();

        // Out-of-order delivery + duplicate fragment
        assert!(assembler.ingest(peer_addr, &frags[4]).is_none());
        assert!(assembler.ingest(peer_addr, &frags[1]).is_none());
        assert!(assembler.ingest(peer_addr, &frags[1]).is_none()); // duplicate ignored
        assert!(assembler.ingest(peer_addr, &frags[3]).is_none());
        assert!(assembler.ingest(peer_addr, &frags[0]).is_none());
        let reassembled = assembler
            .ingest(peer_addr, &frags[2])
            .expect("final fragment completes packet");
        assert_eq!(reassembled, large);

        // Oversized (> 4MB) must fail
        let too_big = vec![0u8; UDP_FRAG_MAX_TOTAL_BYTES + 1];
        assert!(fragment_udp_packet(2, &too_big).is_err());

        // TTL expiry cleanup check
        frags.truncate(2);
        let expired_key = (peer_addr, 7777u64);
        assembler.pending.insert(
            expired_key,
            FragAssembly {
                created_at: Instant::now() - Duration::from_secs(3),
                frag_total: 2,
                slots: vec![Some(vec![1, 2, 3]), None],
                received_count: 1,
                total_bytes: 3,
            },
        );
        assembler.prune_expired();
        assert!(!assembler.pending.contains_key(&expired_key));
    }
}

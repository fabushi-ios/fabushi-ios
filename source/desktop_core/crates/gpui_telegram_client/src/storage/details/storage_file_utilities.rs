//! Target Symbol: StorageFileUtilitiesDetails
//! Telegram Desktop 1:1 Submodule: `StorageFileUtilitiesDetails`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::Mutex;
use sha2::{Digest, Sha512};
use sha1::Sha1;
use md5::{Md5};
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes256;

pub const TDF_MAGIC: [u8; 4] = [b'T', b'D', b'F', b'$'];
pub const APP_VERSION: i32 = 5_000_000;
pub const PASSCODE_KDF_ARGON2ID: u32 = 1;
pub const PASSCODE_KDF_SCRYPT: u32 = 2;
pub const PASSCODE_SALT_MIN_SIZE: usize = 8;
pub const STRONG_ITERATIONS_COUNT: u32 = 100_000;
pub const LOCAL_ENCRYPT_ITER_COUNT: u32 = 4000;
pub const LOCAL_ENCRYPT_NO_PWD_ITER_COUNT: u32 = 4;

pub type FileKey = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasscodeKdf {
    pub kind: u32,
    pub memory: u32,
    pub time: u32,
    pub parallel: u32,
}

impl Default for PasscodeKdf {
    fn default() -> Self {
        Self {
            kind: PASSCODE_KDF_ARGON2ID,
            memory: 65_536,
            time: 8,
            parallel: 1,
        }
    }
}

impl PasscodeKdf {
    pub fn valid(&self) -> bool {
        if self.time == 0 || self.parallel == 0 {
            return false;
        }
        if self.kind == PASSCODE_KDF_ARGON2ID {
            self.memory >= 8 * self.parallel
        } else if self.kind == PASSCODE_KDF_SCRYPT {
            self.memory >= 2 && (self.memory & (self.memory - 1)) == 0
        } else {
            false
        }
    }

    pub fn cost_within_limits(&self) -> bool {
        if self.kind == PASSCODE_KDF_ARGON2ID {
            self.memory <= 262_144 && self.time <= 64 && self.parallel <= 16
        } else {
            true
        }
    }
}

pub fn to_file_part(mut val: FileKey) -> String {
    let mut result = String::with_capacity(16);
    let hex_chars = b"0123456789ABCDEF";
    for _ in 0..16 {
        let v = (val & 0x0F) as usize;
        result.push(hex_chars[v] as char);
        val >>= 4;
    }
    result
}

pub fn key_already_used(base_path: &str, name: &str) -> bool {
    let mut p0 = PathBuf::from(base_path);
    p0.push(format!("{}0", name));
    if p0.exists() {
        return true;
    }
    let mut p1 = PathBuf::from(base_path);
    p1.push(format!("{}1", name));
    if p1.exists() {
        return true;
    }
    let mut ps = PathBuf::from(base_path);
    ps.push(format!("{}s", name));
    ps.exists()
}

pub fn generate_key(base_path: &str) -> FileKey {
    let mut rng_buf = [0u8; 8];
    loop {
        let _ = getrandom::getrandom(&mut rng_buf);
        let key = u64::from_le_bytes(rng_buf);
        if key != 0 && !key_already_used(base_path, &to_file_part(key)) {
            return key;
        }
    }
}

pub fn clear_key(key: FileKey, base_path: &str) {
    let part = to_file_part(key);
    for ext in &["0", "1", "s"] {
        let mut p = PathBuf::from(base_path);
        p.push(format!("{}{}", part, ext));
        let _ = fs::remove_file(p);
    }
}

pub fn create_local_key(passcode: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update(salt);
    hasher.update(passcode);
    hasher.update(salt);
    let prehash = hasher.finalize();

    let iterations = if passcode.is_empty() {
        1
    } else {
        STRONG_ITERATIONS_COUNT
    };

    let mut out = [0u8; 32];
    let _ = pbkdf2::pbkdf2_hmac::<Sha512>(&prehash, salt, iterations, &mut out);
    out
}

pub fn create_passcode_key(
    passcode: &[u8],
    salt: &[u8],
    kdf: &PasscodeKdf,
) -> Result<[u8; 32], String> {
    if passcode.is_empty() || salt.len() < PASSCODE_SALT_MIN_SIZE {
        return Err("Passcode empty or salt too short".to_string());
    }
    if !kdf.valid() || !kdf.cost_within_limits() {
        return Err("Invalid or out-of-bounds KDF parameters".to_string());
    }

    if kdf.kind == PASSCODE_KDF_ARGON2ID {
        let params = argon2::Params::new(
            kdf.memory,
            kdf.time,
            kdf.parallel,
            Some(32),
        ).map_err(|e| format!("Argon2 params error: {e}"))?;
        let argon = argon2::Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            params,
        );
        let mut out = [0u8; 32];
        argon.hash_password_into(passcode, salt, &mut out)
            .map_err(|e| format!("Argon2 derivation error: {e}"))?;
        Ok(out)
    } else {
        // Fallback for Scrypt or other kind to PBKDF2-HMAC-SHA512
        Ok(create_local_key(passcode, salt))
    }
}

pub fn aes_ige_encrypt(plaintext: &[u8], key: &[u8; 32], iv: &[u8; 32]) -> Vec<u8> {
    assert_eq!(plaintext.len() % 16, 0);
    let cipher = Aes256::new(aes::cipher::generic_array::GenericArray::from_slice(key));
    let mut ciphertext = vec![0u8; plaintext.len()];
    let mut iv1 = [0u8; 16];
    let mut iv2 = [0u8; 16];
    iv1.copy_from_slice(&iv[..16]);
    iv2.copy_from_slice(&iv[16..32]);

    for (p_chunk, c_chunk) in plaintext.chunks_exact(16).zip(ciphertext.chunks_exact_mut(16)) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = p_chunk[i] ^ iv1[i];
        }
        let mut gen_block = *aes::cipher::generic_array::GenericArray::from_slice(&block);
        cipher.encrypt_block(&mut gen_block);
        for i in 0..16 {
            c_chunk[i] = gen_block[i] ^ iv2[i];
        }
        iv1.copy_from_slice(c_chunk);
        iv2.copy_from_slice(p_chunk);
    }
    ciphertext
}

pub fn aes_ige_decrypt(ciphertext: &[u8], key: &[u8; 32], iv: &[u8; 32]) -> Vec<u8> {
    assert_eq!(ciphertext.len() % 16, 0);
    let cipher = Aes256::new(aes::cipher::generic_array::GenericArray::from_slice(key));
    let mut plaintext = vec![0u8; ciphertext.len()];
    let mut iv1 = [0u8; 16];
    let mut iv2 = [0u8; 16];
    iv1.copy_from_slice(&iv[..16]);
    iv2.copy_from_slice(&iv[16..32]);

    for (c_chunk, p_chunk) in ciphertext.chunks_exact(16).zip(plaintext.chunks_exact_mut(16)) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = c_chunk[i] ^ iv2[i];
        }
        let mut gen_block = *aes::cipher::generic_array::GenericArray::from_slice(&block);
        cipher.decrypt_block(&mut gen_block);
        for i in 0..16 {
            p_chunk[i] = gen_block[i] ^ iv1[i];
        }
        iv1.copy_from_slice(c_chunk);
        iv2.copy_from_slice(p_chunk);
    }
    plaintext
}

fn derive_ige_iv(sha1_key128: &[u8; 16], local_key: &[u8; 32]) -> [u8; 32] {
    let mut iv = [0u8; 32];
    iv[..16].copy_from_slice(sha1_key128);
    for i in 0..16 {
        iv[16 + i] = sha1_key128[i] ^ local_key[i];
    }
    iv
}

pub fn prepare_encrypted(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
    let original_size = data.len() as u32;
    let mut padded = Vec::with_capacity(4 + data.len() + 16);
    padded.extend_from_slice(&original_size.to_le_bytes());
    padded.extend_from_slice(data);

    let rem = padded.len() % 16;
    if rem != 0 {
        let pad_len = 16 - rem;
        let mut pad = vec![0u8; pad_len];
        let _ = getrandom::getrandom(&mut pad);
        padded.extend_from_slice(&pad);
    }

    let mut sha1_hasher = Sha1::new();
    sha1_hasher.update(&padded);
    let sha1_result = sha1_hasher.finalize();
    let mut key128 = [0u8; 16];
    key128.copy_from_slice(&sha1_result[..16]);

    let iv = derive_ige_iv(&key128, key);
    let encrypted = aes_ige_encrypt(&padded, key, &iv);

    let mut result = Vec::with_capacity(16 + encrypted.len());
    result.extend_from_slice(&key128);
    result.extend_from_slice(&encrypted);
    result
}

pub fn decrypt_local(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if encrypted.len() <= 16 || (encrypted.len() - 16) % 16 != 0 {
        return Err(format!("Bad encrypted payload size: {}", encrypted.len()));
    }
    let mut key128 = [0u8; 16];
    key128.copy_from_slice(&encrypted[..16]);
    let cipher_part = &encrypted[16..];

    let iv = derive_ige_iv(&key128, key);
    let decrypted = aes_ige_decrypt(cipher_part, key, &iv);

    let mut sha1_hasher = Sha1::new();
    sha1_hasher.update(&decrypted);
    let sha1_result = sha1_hasher.finalize();
    if sha1_result[..16] != key128 {
        return Err("Checksum mismatch: bad decryption key or corrupted ciphertext".to_string());
    }

    if decrypted.len() < 4 {
        return Err("Decrypted data too short".to_string());
    }
    let original_size = u32::from_le_bytes(decrypted[..4].try_into().unwrap()) as usize;
    if original_size > decrypted.len() - 4 {
        return Err("Invalid decoded size header in decrypted payload".to_string());
    }
    Ok(decrypted[4..4 + original_size].to_vec())
}

#[derive(Debug, Clone, Default)]
pub struct FileReadDescriptor {
    pub version: i32,
    pub data: Vec<u8>,
    pub position: usize,
}

impl FileReadDescriptor {
    pub fn new(version: i32, data: Vec<u8>) -> Self {
        Self {
            version,
            data,
            position: 0,
        }
    }

    pub fn read_bytes(&mut self, len: usize) -> Option<&[u8]> {
        if self.position + len <= self.data.len() {
            let slice = &self.data[self.position..self.position + len];
            self.position += len;
            Some(slice)
        } else {
            None
        }
    }

    pub fn read_u32(&mut self) -> Option<u32> {
        let bytes = self.read_bytes(4)?;
        Some(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn read_i32(&mut self) -> Option<i32> {
        let bytes = self.read_bytes(4)?;
        Some(i32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn read_byte_array(&mut self) -> Option<Vec<u8>> {
        let len = self.read_u32()? as usize;
        let bytes = self.read_bytes(len)?;
        Some(bytes.to_vec())
    }
}

pub fn read_file(name: &str, base_path: &str) -> Result<FileReadDescriptor, String> {
    let mut base = PathBuf::from(base_path);
    base.push(name);

    let paths_to_try = [
        PathBuf::from(format!("{}s", base.display())),
        PathBuf::from(format!("{}0", base.display())),
        PathBuf::from(format!("{}1", base.display())),
    ];

    for path in &paths_to_try {
        if !path.exists() {
            continue;
        }
        let Ok(mut file) = File::open(path) else {
            continue;
        };
        let mut buffer = Vec::new();
        if file.read_to_end(&mut buffer).is_err() {
            continue;
        }
        if buffer.len() < 4 + 4 + 16 {
            continue;
        }

        if buffer[..4] != TDF_MAGIC {
            continue;
        }

        let version = i32::from_le_bytes(buffer[4..8].try_into().unwrap());
        if version > APP_VERSION {
            continue;
        }

        let payload_len = buffer.len() - 16;
        let data_size = (payload_len - 8) as i32;

        let mut md5_hasher = Md5::new();
        md5_hasher.update(&buffer[8..payload_len]);
        md5_hasher.update(&data_size.to_le_bytes());
        md5_hasher.update(&version.to_le_bytes());
        md5_hasher.update(&TDF_MAGIC);
        let computed_md5 = md5_hasher.finalize();

        if computed_md5[..] != buffer[payload_len..] {
            continue;
        }

        let data = buffer[8..payload_len].to_vec();
        return Ok(FileReadDescriptor::new(version, data));
    }

    Err(format!("Could not read valid TDF file for '{}'", name))
}

pub fn read_encrypted_file(
    name: &str,
    base_path: &str,
    key: &[u8; 32],
) -> Result<FileReadDescriptor, String> {
    let mut descriptor = read_file(name, base_path)?;
    let encrypted_payload = descriptor.read_byte_array()
        .ok_or_else(|| "Failed to read encrypted byte array from file".to_string())?;
    let decrypted = decrypt_local(&encrypted_payload, key)?;
    Ok(FileReadDescriptor::new(descriptor.version, decrypted))
}

#[derive(Debug, Clone)]
pub struct WriteEntry {
    pub base_path: String,
    pub name: String,
    pub data: Vec<u8>,
}

pub struct FileWriteDescriptor {
    base_path: String,
    name: String,
    sync: bool,
    buffer: Vec<u8>,
}

impl FileWriteDescriptor {
    pub fn new(name: impl Into<String>, base_path: impl Into<String>, sync: bool) -> Self {
        Self {
            base_path: base_path.into(),
            name: name.into(),
            sync,
            buffer: Vec::new(),
        }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    pub fn write_u32(&mut self, val: u32) {
        self.buffer.extend_from_slice(&val.to_le_bytes());
    }

    pub fn write_i32(&mut self, val: i32) {
        self.buffer.extend_from_slice(&val.to_le_bytes());
    }

    pub fn write_byte_array(&mut self, bytes: &[u8]) {
        self.write_u32(bytes.len() as u32);
        self.write_bytes(bytes);
    }

    pub fn write_encrypted(&mut self, plaintext: &[u8], key: &[u8; 32]) {
        let encrypted = prepare_encrypted(plaintext, key);
        self.write_byte_array(&encrypted);
    }

    pub fn finish(self) -> Result<(), String> {
        let entry = WriteEntry {
            base_path: self.base_path,
            name: self.name,
            data: self.buffer,
        };
        let manager = WriteManager::new();
        if self.sync {
            manager.write_sync(entry)
        } else {
            manager.write(entry);
            Ok(())
        }
    }
}

pub struct WriteManager {
    scheduled: Arc<Mutex<Vec<WriteEntry>>>,
}

impl Default for WriteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteManager {
    pub fn new() -> Self {
        Self {
            scheduled: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn write(&self, entry: WriteEntry) {
        let mut guard = self.scheduled.lock();
        if let Some(pos) = guard.iter().position(|e| e.name == entry.name) {
            guard[pos] = entry;
        } else {
            guard.push(entry);
        }
    }

    pub fn write_sync(&self, entry: WriteEntry) -> Result<(), String> {
        let dir = Path::new(&entry.base_path);
        if !dir.exists() {
            fs::create_dir_all(dir).map_err(|e| format!("Mkdir error: {e}"))?;
        }

        let mut safe_path = dir.to_path_buf();
        safe_path.push(format!("{}s", entry.name));

        let mut temp_path = dir.to_path_buf();
        temp_path.push(format!("{}s.tmp", entry.name));

        let data_size = entry.data.len() as i32;
        let mut md5_hasher = Md5::new();
        md5_hasher.update(&entry.data);
        md5_hasher.update(&data_size.to_le_bytes());
        md5_hasher.update(&APP_VERSION.to_le_bytes());
        md5_hasher.update(&TDF_MAGIC);
        let md5_digest = md5_hasher.finalize();

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|e| format!("Create temp file error: {e}"))?;

        file.write_all(&TDF_MAGIC).map_err(|e| format!("Write magic error: {e}"))?;
        file.write_all(&APP_VERSION.to_le_bytes()).map_err(|e| format!("Write version error: {e}"))?;
        file.write_all(&entry.data).map_err(|e| format!("Write data error: {e}"))?;
        file.write_all(&md5_digest).map_err(|e| format!("Write md5 error: {e}"))?;
        file.sync_all().map_err(|e| format!("Sync file error: {e}"))?;
        drop(file);

        fs::rename(&temp_path, &safe_path).map_err(|e| format!("Rename error: {e}"))?;

        // Cleanup legacy 0 and 1
        let mut p0 = dir.to_path_buf();
        p0.push(format!("{}0", entry.name));
        let _ = fs::remove_file(p0);

        let mut p1 = dir.to_path_buf();
        p1.push(format!("{}1", entry.name));
        let _ = fs::remove_file(p1);

        Ok(())
    }

    pub fn write_sync_all(&self) -> Result<(), String> {
        let entries = {
            let mut guard = self.scheduled.lock();
            std::mem::take(&mut *guard)
        };
        for entry in entries {
            self.write_sync(entry)?;
        }
        Ok(())
    }
}

pub struct AsyncWriteManager {
    manager: WriteManager,
}

impl Default for AsyncWriteManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncWriteManager {
    pub fn new() -> Self {
        Self {
            manager: WriteManager::new(),
        }
    }

    pub fn write(&self, entry: WriteEntry) {
        self.manager.write(entry);
    }

    pub fn write_sync(&self, entry: WriteEntry) -> Result<(), String> {
        self.manager.write_sync(entry)
    }

    pub fn sync(&self) -> Result<(), String> {
        self.manager.write_sync_all()
    }
}

#[derive(Debug, Clone, Default)]
pub struct StorageFileUtilitiesDetails {
    pub manager_initialized: bool,
    pub default_kdf: PasscodeKdf,
}

impl StorageFileUtilitiesDetails {
    pub fn new() -> Self {
        Self {
            manager_initialized: true,
            default_kdf: PasscodeKdf::default(),
        }
    }

    pub fn to_part(&self, key: FileKey) -> String {
        to_file_part(key)
    }

    pub fn generate_key(&self, base_path: &str) -> FileKey {
        generate_key(base_path)
    }
}

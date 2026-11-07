use sha2::{Digest, Sha256};

pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(data.as_ref());
    format!("{digest:x}")
}

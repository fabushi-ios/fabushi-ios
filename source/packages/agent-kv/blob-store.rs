use crate::package_agent_kv_serde::to_hex;
use sha2::{Digest,Sha256};
use std::collections::HashMap;

pub trait BlobStore {
    fn get_blob(&self, blob_id:&[u8])->Option<Vec<u8>>;
    fn set_blob(&mut self, blob_id:&[u8], blob_data:&[u8]);
    fn set_blob_locally_only(&mut self, blob_id:&[u8], blob_data:&[u8]) {
        self.set_blob(blob_id,blob_data);
    }
    fn flush(&mut self) {}
    fn is_blob_durable(&self, _blob_id:&[u8])->Option<bool> { None }
}

pub fn is_blob_durable<Store:BlobStore>(blob_store:&Store, blob_id:&[u8])->bool {
    blob_store.is_blob_durable(blob_id).unwrap_or(true)
}

pub fn to_uint8_array(value:&[u8])->Vec<u8> {
    // Rust byte slices cannot carry JavaScript SharedArrayBuffer aliasing,
    // so the native equivalent is an owned byte vector.
    value.to_vec()
}

pub fn get_blob_id(blob_data:&[u8])->Vec<u8> {
    let owned=to_uint8_array(blob_data);
    Sha256::digest(&owned).to_vec()
}

#[derive(Debug,Default)]
pub struct InMemoryBlobStore {
    blobs: HashMap<String,Vec<u8>>,
}

impl BlobStore for InMemoryBlobStore {
    fn get_blob(&self, blob_id:&[u8])->Option<Vec<u8>> {
        self.blobs.get(&to_hex(blob_id)).cloned()
    }
    fn set_blob(&mut self, blob_id:&[u8], blob_data:&[u8]) {
        self.blobs.insert(to_hex(blob_id),blob_data.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_owned_bytes_with_sha256() {
        assert_eq!(
            crate::package_agent_kv_serde::to_hex(&get_blob_id(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn in_memory_store_round_trips_and_defaults_to_durable() {
        let mut store=InMemoryBlobStore::default();
        store.set_blob(&[1,2],b"value");
        assert_eq!(store.get_blob(&[1,2]),Some(b"value".to_vec()));
        assert!(is_blob_durable(&store,&[1,2]));
        store.set_blob_locally_only(&[3],b"local");
        assert_eq!(store.get_blob(&[3]),Some(b"local".to_vec()));
        store.flush();
    }

    #[test]
    fn to_uint8_array_returns_an_owned_copy() {
        let source=vec![1,2,3];
        let copy=to_uint8_array(&source);
        assert_eq!(copy,source);
    }
}

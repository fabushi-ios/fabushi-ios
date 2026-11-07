//! Target Symbol: SerializePeer
//! Telegram Desktop 1:1 Submodule: `SerializePeer`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use super::serialize_common::{ByteArrayReader, ByteArrayWriter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerType {
    User = 0,
    Chat = 1,
    Channel = 2,
}

impl Default for PeerType {
    fn default() -> Self {
        Self::User
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SerializedPeer {
    pub peer_type: PeerType,
    pub id: u64,
    pub access_hash: u64,
    pub name: String,
    pub username: String,
    pub phone: String,
}

impl SerializedPeer {
    pub fn write_to(&self, writer: &mut ByteArrayWriter) {
        writer.write_i32(self.peer_type as i32);
        writer.write_u64(self.id);
        writer.write_u64(self.access_hash);
        writer.write_string(&self.name);
        writer.write_string(&self.username);
        writer.write_string(&self.phone);
    }

    pub fn read_from(reader: &mut ByteArrayReader) -> Option<Self> {
        let type_raw = reader.read_i32();
        let peer_type = match type_raw {
            0 => PeerType::User,
            1 => PeerType::Chat,
            2 => PeerType::Channel,
            _ => return None,
        };
        let id = reader.read_u64();
        let access_hash = reader.read_u64();
        let name = reader.read_string();
        let username = reader.read_string();
        let phone = reader.read_string();

        if reader.ok() {
            Some(Self {
                peer_type,
                id,
                access_hash,
                name,
                username,
                phone,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageLocation {
    pub dc_id: i32,
    pub volume_id: u64,
    pub local_id: i32,
    pub secret: u64,
    pub file_reference: Vec<u8>,
}

impl ImageLocation {
    pub fn write_to(&self, writer: &mut ByteArrayWriter) {
        writer.write_i32(self.dc_id);
        writer.write_u64(self.volume_id);
        writer.write_i32(self.local_id);
        writer.write_u64(self.secret);
        writer.write_bytes(&self.file_reference);
    }

    pub fn read_from(reader: &mut ByteArrayReader) -> Option<Self> {
        let dc_id = reader.read_i32();
        let volume_id = reader.read_u64();
        let local_id = reader.read_i32();
        let secret = reader.read_u64();
        let file_reference = reader.read_bytes();

        if reader.ok() {
            Some(Self {
                dc_id,
                volume_id,
                local_id,
                secret,
                file_reference,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SerializePeer {
    pub peers_serialized: usize,
}

impl SerializePeer {
    pub fn new() -> Self {
        Self {
            peers_serialized: 0,
        }
    }

    pub fn serialize_peer(&mut self, peer: &SerializedPeer) -> Vec<u8> {
        let mut writer = ByteArrayWriter::new();
        peer.write_to(&mut writer);
        self.peers_serialized += 1;
        writer.finish()
    }

    pub fn deserialize_peer(&self, data: &[u8]) -> Option<SerializedPeer> {
        let mut reader = ByteArrayReader::new(data);
        SerializedPeer::read_from(&mut reader)
    }

    pub fn serialize_image_location(&self, loc: &ImageLocation) -> Vec<u8> {
        let mut writer = ByteArrayWriter::new();
        loc.write_to(&mut writer);
        writer.finish()
    }

    pub fn deserialize_image_location(&self, data: &[u8]) -> Option<ImageLocation> {
        let mut reader = ByteArrayReader::new(data);
        ImageLocation::read_from(&mut reader)
    }
}

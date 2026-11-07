//! Target Symbol: SerializeDocument
//! Telegram Desktop 1:1 Submodule: `SerializeDocument`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use super::serialize_common::{ByteArrayReader, ByteArrayWriter};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StickerSetInfo {
    pub set_id: u64,
    pub access_hash: u64,
    pub short_name: String,
}

impl StickerSetInfo {
    pub fn new(set_id: u64, access_hash: u64, short_name: impl Into<String>) -> Self {
        Self {
            set_id,
            access_hash,
            short_name: short_name.into(),
        }
    }

    pub fn write_to(&self, writer: &mut ByteArrayWriter) {
        writer.write_u64(self.set_id);
        writer.write_u64(self.access_hash);
        writer.write_string(&self.short_name);
    }

    pub fn read_from(reader: &mut ByteArrayReader) -> Option<Self> {
        let set_id = reader.read_u64();
        let access_hash = reader.read_u64();
        let short_name = reader.read_string();
        if reader.ok() {
            Some(Self {
                set_id,
                access_hash,
                short_name,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SerializedDocument {
    pub id: u64,
    pub access_hash: u64,
    pub date: i32,
    pub name: String,
    pub mime_type: String,
    pub size: i64,
    pub dc_id: i32,
    pub thumbnail_data: Vec<u8>,
    pub sticker_info: Option<StickerSetInfo>,
}

impl SerializedDocument {
    pub fn write_to(&self, writer: &mut ByteArrayWriter) {
        writer.write_u64(self.id);
        writer.write_u64(self.access_hash);
        writer.write_i32(self.date);
        writer.write_string(&self.name);
        writer.write_string(&self.mime_type);
        writer.write_i64(self.size);
        writer.write_i32(self.dc_id);
        writer.write_bytes(&self.thumbnail_data);
        if let Some(info) = &self.sticker_info {
            writer.write_bool(true);
            info.write_to(writer);
        } else {
            writer.write_bool(false);
        }
    }

    pub fn read_from(reader: &mut ByteArrayReader) -> Option<Self> {
        let id = reader.read_u64();
        let access_hash = reader.read_u64();
        let date = reader.read_i32();
        let name = reader.read_string();
        let mime_type = reader.read_string();
        let size = reader.read_i64();
        let dc_id = reader.read_i32();
        let thumbnail_data = reader.read_bytes();
        let has_sticker_info = reader.read_bool();
        let sticker_info = if has_sticker_info {
            StickerSetInfo::read_from(reader)
        } else {
            None
        };

        if reader.ok() {
            Some(Self {
                id,
                access_hash,
                date,
                name,
                mime_type,
                size,
                dc_id,
                thumbnail_data,
                sticker_info,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SerializeDocument {
    pub count: usize,
}

impl SerializeDocument {
    pub fn new() -> Self {
        Self { count: 0 }
    }

    pub fn serialize(&mut self, doc: &SerializedDocument) -> Vec<u8> {
        let mut writer = ByteArrayWriter::new();
        doc.write_to(&mut writer);
        self.count += 1;
        writer.finish()
    }

    pub fn deserialize(&self, data: &[u8]) -> Option<SerializedDocument> {
        let mut reader = ByteArrayReader::new(data);
        SerializedDocument::read_from(&mut reader)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobType {
    pub kind: &'static str,
    pub type_name: Option<&'static str>,
    pub mime_type: Option<&'static str>,
}

pub trait Serde<T> {
    fn serialize(&self, value: &T) -> Vec<u8>;
    fn deserialize(&self, blob: &[u8]) -> T;
    fn get_blob_type(&self) -> Option<BlobType> { None }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Utf8Serde;

impl Serde<String> for Utf8Serde {
    fn serialize(&self, value: &String) -> Vec<u8> { value.as_bytes().to_vec() }
    fn deserialize(&self, blob: &[u8]) -> String { String::from_utf8_lossy(blob).into_owned() }
    fn get_blob_type(&self) -> Option<BlobType> {
        Some(BlobType { kind: "string", type_name: None, mime_type: None })
    }
}

pub const UTF8_SERDE: Utf8Serde = Utf8Serde;

pub fn to_hex(value: &[u8]) -> String {
    const HEX: &[u8;16]=b"0123456789abcdef";
    let mut out=String::with_capacity(value.len()*2);
    for byte in value {
        out.push(HEX[(byte>>4) as usize] as char);
        out.push(HEX[(byte&0x0f) as usize] as char);
    }
    out
}

pub fn from_hex(hex: &str) -> Result<Vec<u8>, String> {
    let clean=hex.trim().to_ascii_lowercase();
    if clean.len()%2!=0 { return Err("Invalid hex string length".into()); }
    let mut output=Vec::with_capacity(clean.len()/2);
    for index in (0..clean.len()).step_by(2) {
        // Pinned JS Number.parseInt(..., 16) yields NaN for an invalid pair;
        // assignment into Uint8Array coerces NaN to 0 rather than throwing.
        output.push(u8::from_str_radix(&clean[index..index+2],16).unwrap_or(0));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_serde_round_trips_and_reports_string_blob_type() {
        let value="佛 Fabushi".to_owned();
        let encoded=UTF8_SERDE.serialize(&value);
        assert_eq!(UTF8_SERDE.deserialize(&encoded),value);
        assert_eq!(UTF8_SERDE.get_blob_type().unwrap().kind,"string");
    }

    #[test]
    fn hex_helpers_preserve_reference_coercion_behavior() {
        assert_eq!(to_hex(&[0x00,0xab,0xff]),"00abff");
        assert_eq!(from_hex(" 00ABff ").unwrap(),vec![0x00,0xab,0xff]);
        assert_eq!(from_hex("zz").unwrap(),vec![0]);
        assert_eq!(from_hex("0").unwrap_err(),"Invalid hex string length");
    }
}

use std::error::Error;
use std::fmt::{Display, Formatter};

pub const CONVERSATION_DATA_MISSING_MESSAGE: &str =
    "This conversation's data is missing and can't be restored. Start a new chat to continue.";
const MAX_MESSAGE_BLOB_ID_HEXES: usize = 3;
const MESSAGE_BLOB_ID_HEX_LENGTH: usize = 12;
const MAX_CAUSE_CHAIN_DEPTH: usize = 10;

pub fn format_missing_blob_suffix(blob_id_hexes: &[String]) -> String {
    if blob_id_hexes.is_empty() { return String::new(); }
    let shown=blob_id_hexes.iter().take(MAX_MESSAGE_BLOB_ID_HEXES)
        .map(|hex| hex.chars().take(MESSAGE_BLOB_ID_HEX_LENGTH).collect::<String>())
        .collect::<Vec<_>>();
    let label=if blob_id_hexes.len()==1 {
        format!("missing blob {}",shown[0])
    } else {
        format!(
            "{} missing blobs: {}{}",
            blob_id_hexes.len(),
            shown.join(", "),
            if blob_id_hexes.len()>shown.len() { ", …" } else { "" }
        )
    };
    format!(" ({label})")
}

#[derive(Debug)]
pub struct BlobNotFoundError {
    pub is_usage_error: bool,
    pub is_blob_not_found: bool,
    pub blob_id_hexes: Vec<String>,
    message: String,
    cause: Option<Box<dyn Error + Send + Sync>>,
}

impl BlobNotFoundError {
    pub fn new(blob_id_hexes: Vec<String>) -> Self {
        Self::with_cause(blob_id_hexes,None)
    }

    pub fn with_cause(
        blob_id_hexes: Vec<String>,
        cause: Option<Box<dyn Error + Send + Sync>>,
    ) -> Self {
        let message=format!("{CONVERSATION_DATA_MISSING_MESSAGE}{}",format_missing_blob_suffix(&blob_id_hexes));
        Self {
            is_usage_error:true,
            is_blob_not_found:true,
            blob_id_hexes,
            message,
            cause,
        }
    }
}

impl Display for BlobNotFoundError {
    fn fmt(&self, formatter:&mut Formatter<'_>)->std::fmt::Result { formatter.write_str(&self.message) }
}
impl Error for BlobNotFoundError {
    fn source(&self)->Option<&(dyn Error+'static)> {
        self.cause.as_deref().map(|source| source as &(dyn Error+'static))
    }
}

pub fn find_blob_not_found_error<'a>(error:&'a (dyn Error+'static))->Option<&'a BlobNotFoundError> {
    let mut current=Some(error);
    for _ in 0..MAX_CAUSE_CHAIN_DEPTH {
        let node=current?;
        if let Some(found)=node.downcast_ref::<BlobNotFoundError>() { return Some(found); }
        current=node.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Wrapper { source: Box<dyn Error+Send+Sync> }
    impl Display for Wrapper { fn fmt(&self,f:&mut Formatter<'_>)->std::fmt::Result { f.write_str("wrapper") } }
    impl Error for Wrapper { fn source(&self)->Option<&(dyn Error+'static)> { Some(self.source.as_ref()) } }

    #[test]
    fn formats_single_and_truncated_multi_blob_suffixes() {
        let one=BlobNotFoundError::new(vec!["1234567890abcdef".into()]);
        assert!(one.to_string().ends_with("(missing blob 1234567890ab)"));
        let many=BlobNotFoundError::new(vec![
            "aaaaaaaaaaaa1111".into(),"bbbbbbbbbbbb2222".into(),"cccccccccccc3333".into(),"dddddddddddd4444".into()
        ]);
        assert!(many.to_string().ends_with("(4 missing blobs: aaaaaaaaaaaa, bbbbbbbbbbbb, cccccccccccc, …)"));
        assert!(many.is_usage_error && many.is_blob_not_found);
    }

    #[test]
    fn finds_typed_blob_error_through_cause_chain() {
        let wrapped=Wrapper{source:Box::new(BlobNotFoundError::new(vec!["abc".into()]))};
        let found=find_blob_not_found_error(&wrapped).unwrap();
        assert_eq!(found.blob_id_hexes,vec!["abc".to_owned()]);
    }
}

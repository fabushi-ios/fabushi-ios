use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackgroundWorkMetadata {
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub start_time_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    String(String),
    Other,
}

pub fn encode_background_work_metadata(
    metadata: &BackgroundWorkMetadata,
) -> Option<BTreeMap<String, String>> {
    let mut encoded = BTreeMap::new();

    if let Some(title) = metadata.title.as_ref().filter(|value| !value.trim().is_empty()) {
        encoded.insert("title".to_owned(), title.clone());
    }
    if let Some(cwd) = metadata.cwd.as_ref().filter(|value| !value.trim().is_empty()) {
        encoded.insert("cwd".to_owned(), cwd.clone());
    }
    if let Some(start_time_ms) = metadata
        .start_time_ms
        .filter(|value| value.is_finite() && *value > 0.0)
    {
        encoded.insert("startTimeMs".to_owned(), start_time_ms.floor().to_string());
    }

    (!encoded.is_empty()).then_some(encoded)
}

pub fn decode_background_work_metadata(
    metadata: Option<&BTreeMap<String, MetadataValue>>,
) -> BackgroundWorkMetadata {
    let mut decoded = BackgroundWorkMetadata::default();
    let Some(metadata) = metadata else {
        return decoded;
    };

    if let Some(MetadataValue::String(title)) = metadata.get("title") {
        if !title.trim().is_empty() {
            decoded.title = Some(title.clone());
        }
    }
    if let Some(MetadataValue::String(cwd)) = metadata.get("cwd") {
        if !cwd.trim().is_empty() {
            decoded.cwd = Some(cwd.clone());
        }
    }
    if let Some(MetadataValue::String(raw)) = metadata.get("startTimeMs") {
        if !raw.is_empty() {
            if let Ok(parsed) = raw.parse::<f64>() {
                if parsed.is_finite() && parsed > 0.0 {
                    decoded.start_time_ms = Some(parsed.floor());
                }
            }
        }
    }

    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_preserves_nonblank_strings_and_floors_positive_finite_time() {
        let encoded = encode_background_work_metadata(&BackgroundWorkMetadata {
            title: Some("  build ".to_owned()),
            cwd: Some("/tmp/project".to_owned()),
            start_time_ms: Some(1234.9),
        })
        .unwrap();

        assert_eq!(encoded.get("title").map(String::as_str), Some("  build "));
        assert_eq!(encoded.get("cwd").map(String::as_str), Some("/tmp/project"));
        assert_eq!(encoded.get("startTimeMs").map(String::as_str), Some("1234"));
    }

    #[test]
    fn encode_omits_blank_or_nonpositive_nonfinite_fields() {
        assert_eq!(
            encode_background_work_metadata(&BackgroundWorkMetadata {
                title: Some("   ".to_owned()),
                cwd: None,
                start_time_ms: Some(f64::NAN),
            }),
            None
        );
        assert_eq!(
            encode_background_work_metadata(&BackgroundWorkMetadata {
                title: None,
                cwd: Some("\t".to_owned()),
                start_time_ms: Some(0.0),
            }),
            None
        );
    }

    #[test]
    fn decode_accepts_only_nonblank_strings_and_positive_finite_time() {
        let mut values = BTreeMap::new();
        values.insert("title".to_owned(), MetadataValue::String("Work".to_owned()));
        values.insert("cwd".to_owned(), MetadataValue::Other);
        values.insert(
            "startTimeMs".to_owned(),
            MetadataValue::String("42.9".to_owned()),
        );

        assert_eq!(
            decode_background_work_metadata(Some(&values)),
            BackgroundWorkMetadata {
                title: Some("Work".to_owned()),
                cwd: None,
                start_time_ms: Some(42.0),
            }
        );

        values.insert(
            "startTimeMs".to_owned(),
            MetadataValue::String("Infinity".to_owned()),
        );
        assert_eq!(
            decode_background_work_metadata(Some(&values)).start_time_ms,
            None
        );
        assert_eq!(
            decode_background_work_metadata(None),
            BackgroundWorkMetadata::default()
        );
    }
}

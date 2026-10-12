use std::collections::BTreeSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    Canvas,
}

impl Capability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Canvas => "canvas",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityParseError {
    value: String,
}

impl fmt::Display for CapabilityParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported capability: {}", self.value)
    }
}

impl std::error::Error for CapabilityParseError {}

pub fn normalize_capabilities<'a, I>(values: I) -> Result<Vec<Capability>, CapabilityParseError>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut capabilities = BTreeSet::new();
    for value in values {
        match value {
            "canvas" => {
                capabilities.insert(Capability::Canvas);
            }
            other => {
                return Err(CapabilityParseError {
                    value: other.to_owned(),
                });
            }
        }
    }
    Ok(capabilities.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_deduplicates_and_sorts_capabilities() {
        let values = normalize_capabilities(["canvas", "canvas"]).unwrap();
        assert_eq!(values, vec![Capability::Canvas]);
        assert_eq!(values[0].as_str(), "canvas");
        assert!(normalize_capabilities(["other"]).is_err());
    }
}

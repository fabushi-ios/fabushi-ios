//! Node Host Gateway protocol-range negotiation.
//!
//! This module owns only the version admission rule. It does not build the
//! challenge-bound connect envelope, sign device metadata, or open a Gateway
//! transport.

use std::fmt;

pub const CURRENT_PROTOCOL_MIN: u16 = 3;
pub const CURRENT_PROTOCOL_MAX: u16 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum NodeHostProtocol {
    V3 = 3,
    V4 = 4,
}

impl TryFrom<u16> for NodeHostProtocol {
    type Error = ProtocolNegotiationError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            3 => Ok(Self::V3),
            4 => Ok(Self::V4),
            other => Err(ProtocolNegotiationError::UnsupportedServerVersion(other)),
        }
    }
}

impl fmt::Display for NodeHostProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "V{}", *self as u16)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfferedProtocolRange {
    pub minimum: u16,
    pub maximum: u16,
}

impl OfferedProtocolRange {
    pub fn new(minimum: u16, maximum: u16) -> Result<Self, ProtocolNegotiationError> {
        if minimum > maximum {
            return Err(ProtocolNegotiationError::InvalidOfferedRange { minimum, maximum });
        }
        if minimum < CURRENT_PROTOCOL_MIN || maximum > CURRENT_PROTOCOL_MAX {
            return Err(ProtocolNegotiationError::UnsupportedOfferedRange { minimum, maximum });
        }
        Ok(Self { minimum, maximum })
    }
}

pub fn negotiate_protocol(
    server_version: u16,
    offered: OfferedProtocolRange,
) -> Result<NodeHostProtocol, ProtocolNegotiationError> {
    if server_version < offered.minimum || server_version > offered.maximum {
        return Err(ProtocolNegotiationError::ServerOutsideOfferedRange {
            server_version,
            minimum: offered.minimum,
            maximum: offered.maximum,
        });
    }
    let selected = server_version.min(offered.maximum);
    NodeHostProtocol::try_from(selected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolNegotiationError {
    #[error("offered protocol range is inverted: {minimum}..{maximum}")]
    InvalidOfferedRange { minimum: u16, maximum: u16 },
    #[error("offered protocol range is unsupported: {minimum}..{maximum}")]
    UnsupportedOfferedRange { minimum: u16, maximum: u16 },
    #[error("server protocol {server_version} is outside offered range {minimum}..{maximum}")]
    ServerOutsideOfferedRange {
        server_version: u16,
        minimum: u16,
        maximum: u16,
    },
    #[error("server protocol {0} is unsupported")]
    UnsupportedServerVersion(u16),
}

#[cfg(test)]
mod tests {
    use super::{
        NodeHostProtocol, OfferedProtocolRange, ProtocolNegotiationError, negotiate_protocol,
    };

    #[test]
    fn selects_server_version_within_the_offered_v3_v4_range() {
        let offered = OfferedProtocolRange::new(3, 4).expect("range is valid");
        assert_eq!(negotiate_protocol(3, offered), Ok(NodeHostProtocol::V3));
        assert_eq!(negotiate_protocol(4, offered), Ok(NodeHostProtocol::V4));
    }

    #[test]
    fn rejects_versions_outside_the_offered_range() {
        let offered = OfferedProtocolRange::new(4, 4).expect("range is valid");
        assert_eq!(
            negotiate_protocol(3, offered),
            Err(ProtocolNegotiationError::ServerOutsideOfferedRange {
                server_version: 3,
                minimum: 4,
                maximum: 4,
            })
        );
    }

    #[test]
    fn rejects_inverted_and_future_ranges() {
        assert!(OfferedProtocolRange::new(4, 3).is_err());
        assert!(OfferedProtocolRange::new(3, 5).is_err());
        assert!(OfferedProtocolRange::new(2, 4).is_err());
    }
}

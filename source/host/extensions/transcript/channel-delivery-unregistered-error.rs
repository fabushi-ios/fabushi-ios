use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandChannelDeliveryUnregisteredError;

impl SandChannelDeliveryUnregisteredError {
    pub const NAME: &'static str = "SandChannelDeliveryUnregisteredError";
    pub const MESSAGE: &'static str = "No channel delivery mechanism is registered.";
}
impl fmt::Display for SandChannelDeliveryUnregisteredError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(Self::MESSAGE) }
}
impl std::error::Error for SandChannelDeliveryUnregisteredError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_reference_message() {
        assert_eq!(SandChannelDeliveryUnregisteredError.to_string(), "No channel delivery mechanism is registered.");
    }
}

#[cfg(test)]
mod tests {
    use gpui_telegram_core::auth::GenerationFence;
    use gpui_telegram_media::waveform::{decode_waveform_5bit, encode_waveform_5bit};

    #[test]
    fn test_generation_fence() {
        let fence = GenerationFence::new(1);
        assert_eq!(fence.current(), 1);
        assert!(fence.assert_valid(1).is_ok());
        assert_eq!(fence.bump(), 2);
        assert!(fence.assert_valid(1).is_err());
        assert!(fence.assert_valid(2).is_ok());
    }

    #[test]
    fn test_waveform_roundtrip() {
        let input: Vec<u8> = (0..100).map(|i| (i % 32) as u8).collect();
        let packed = encode_waveform_5bit(&input);
        assert!(packed.len() <= 64);
        let unpacked = decode_waveform_5bit(&packed);
        assert!(unpacked.len() >= 100);
        assert_eq!(&unpacked[..100], input.as_slice());
    }
}

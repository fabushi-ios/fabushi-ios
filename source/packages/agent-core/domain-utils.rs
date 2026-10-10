// The pinned Grok module retains these module-initializer values privately.
// Its public/type surface was tree-shaken, so the native counterpart keeps the
// same numeric constants without inventing a platform API.
#[allow(dead_code)]
const BIGINT_ZERO: i128 = 0;
#[allow(dead_code)]
const BIGINT_EIGHT: i128 = 8;
#[allow(dead_code)]
const BIGINT_SIXTEEN: i128 = 16;
#[allow(dead_code)]
const IPV4_LOW_HEXTET_MASK: i128 = 65_535;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_pinned_initializer_values() {
        assert_eq!(BIGINT_ZERO, 0);
        assert_eq!(BIGINT_EIGHT, 8);
        assert_eq!(BIGINT_SIXTEEN, 16);
        assert_eq!(IPV4_LOW_HEXTET_MASK, 65_535);
    }
}

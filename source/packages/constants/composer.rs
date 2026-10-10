pub const MAX_TEXT_SIZE: usize = 8 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_reference_text_limit() { assert_eq!(MAX_TEXT_SIZE, 8_388_608); }
}

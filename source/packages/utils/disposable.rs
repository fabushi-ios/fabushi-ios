#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Disposable;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_a_zero_sized_marker_like_the_reference_empty_class() {
        assert_eq!(std::mem::size_of::<Disposable>(), 0);
        assert_eq!(Disposable, Disposable);
    }
}

pub const SAND_MONITOR_WIDTH: u32 = 1280;
pub const SAND_MONITOR_HEIGHT: u32 = 800;

pub fn display_space_sentence(width: Option<u32>, height: Option<u32>) -> String {
    let width = width.unwrap_or(SAND_MONITOR_WIDTH).max(1);
    let height = height.unwrap_or(SAND_MONITOR_HEIGHT).max(1);
    format!(
        "Display is {width}×{height}. Computer click/move/scroll x,y are pixels in that space (origin top-left); never emit coordinates outside 0..{} × 0..{}.",
        width - 1,
        height - 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_reference_default_coordinate_contract() {
        assert_eq!(
            display_space_sentence(None, None),
            "Display is 1280×800. Computer click/move/scroll x,y are pixels in that space (origin top-left); never emit coordinates outside 0..1279 × 0..799."
        );
    }

    #[test]
    fn clamps_zero_sized_inputs_to_a_valid_coordinate_space() {
        assert!(display_space_sentence(Some(0), Some(0)).contains("0..0 × 0..0"));
    }
}

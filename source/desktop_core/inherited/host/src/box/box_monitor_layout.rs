pub const SAND_MONITOR_WIDTH: u32 = 1280;
pub const SAND_MONITOR_HEIGHT: u32 = 800;

pub fn display_space_sentence(width: Option<u32>, height: Option<u32>) -> String {
    let width = width.unwrap_or(SAND_MONITOR_WIDTH);
    let height = height.unwrap_or(SAND_MONITOR_HEIGHT);
    format!(
        "Display is {width}×{height}. Computer click/move/scroll x,y are pixels in that space (origin top-left); never emit coordinates outside 0..{} × 0..{}.",
        width.saturating_sub(1),
        height.saturating_sub(1)
    )
}

/// Telegram Desktop cross-multiplication aspect-fit geometry calculation (`shrinkToKeepAspect`).
///
/// Upstream reference: `Telegram/SourceFiles/ui/image/image_location.h` lines 653–663:
/// ```cpp
/// inline QSize shrinkToKeepAspect(int32 width, int32 height, int32 towidth, int32 toheight) {
///     int32 w = std::max(width, 1), h = std::max(height, 1);
///     if (w * toheight > h * towidth) {
///         h = int(base::SafeRound(h * towidth / float64(w)));
///         w = towidth;
///     } else {
///         w = int(base::SafeRound(w * toheight / float64(h)));
///         h = toheight;
///     }
///     return QSize(std::max(w, 1), std::max(h, 1));
/// }
/// ```
pub fn shrink_to_keep_aspect(width: u32, height: u32, to_width: u32, to_height: u32) -> (u32, u32) {
    let w = width.max(1) as u64;
    let h = height.max(1) as u64;
    let to_w = to_width.max(1) as u64;
    let to_h = to_height.max(1) as u64;

    // Cross-multiplication prevents division-by-zero and floating-point drift:
    // w / h > to_w / to_h  <=>  w * to_h > h * to_w
    if w * to_h > h * to_w {
        let new_h = ((h * to_w + w / 2) / w).max(1) as u32;
        (to_width.max(1), new_h)
    } else {
        let new_w = ((w * to_h + h / 2) / h).max(1) as u32;
        (new_w, to_height.max(1))
    }
}

/// Downscales an image only if it exceeds the specified maximum bounding box.
/// Images that already fit inside (max_w, max_h) preserve their original dimensions.
pub fn fit_dimensions_downscale_only(
    orig_w: u32,
    orig_h: u32,
    max_w: u32,
    max_h: u32,
) -> (u32, u32) {
    if orig_w <= max_w && orig_h <= max_h {
        (orig_w.max(1), orig_h.max(1))
    } else {
        shrink_to_keep_aspect(orig_w, orig_h, max_w, max_h)
    }
}

//! Pure-Rust shared Mini App core for 全球法布施 (Global Dharma Offering & Zen Room).
//!
//! Default features compile cleanly to `wasm32-unknown-unknown` with zero OS/DB dependencies.
//! Enable the `server` feature for SQLite storage (`storage::*`) and static asset serving
//! (`app_wasm_bytes`, `hosted_bundle_html`, `asset_bytes`).

pub mod achievements;
pub mod countries;
pub mod scene_math;
pub mod state;
pub mod sutras;

#[cfg(feature = "server")]
pub mod storage;

/// Cosmic dark glass design tokens ported from `lib/core/design_system/colors.dart` & `app_theme.dart`.
pub mod theme {
    pub const SPACE_DEEP_BLUE: &str = "#0B0E14";
    pub const SPACE_BLUE: &str = "#1B263B";
    pub const STARLIGHT_WHITE: &str = "#E8EAF6";
    pub const NEBULA_PURPLE: &str = "#7B1FA2";
    pub const NEBULA_PINK: &str = "#E91E63";
    pub const COSMIC_GOLD: &str = "#FFD700";
    pub const ZEN_GOLD: &str = "#D4AF37";
    pub const GLASS_BORDER: &str = "rgba(255, 255, 255, 0.15)";
    pub const GLASS_SURFACE: &str = "rgba(255, 255, 255, 0.10)";
}

#[cfg(feature = "server")]
pub fn app_wasm_bytes() -> &'static [u8] {
    include_bytes!("../assets/fabushi_miniapp.wasm")
}

#[cfg(feature = "server")]
pub fn hosted_bundle_html() -> &'static str {
    include_str!("../assets/hosted_bundle.html")
}

#[cfg(feature = "server")]
pub fn asset_bytes(name: &str) -> Option<(&'static [u8], &'static str)> {
    let trimmed = name.trim().trim_start_matches('/');
    match trimmed {
        "earth_texture.jpg" => Some((include_bytes!("../assets/earth_texture.jpg"), "image/jpeg")),
        "home_world_lightfield.webp" | "images/home_world_lightfield.webp" => Some((
            include_bytes!("../assets/home_world_lightfield.webp"),
            "image/webp",
        )),
        "zen_room_backdrop_2d.webp" | "images/zen_room_backdrop_2d.webp" => Some((
            include_bytes!("../assets/zen_room_backdrop_2d.webp"),
            "image/webp",
        )),
        "concap.csv" | "data/concap.csv" => Some((
            include_bytes!("../assets/concap.csv"),
            "text/csv; charset=utf-8",
        )),
        "r2-files-list.json" | "data/r2-files-list.json" => Some((
            include_bytes!("../assets/r2-files-list.json"),
            "application/json",
        )),
        "fabushi_miniapp.wasm" | "app.wasm" => Some((app_wasm_bytes(), "application/wasm")),
        "hosted_bundle.html" => Some((hosted_bundle_html().as_bytes(), "text/html; charset=utf-8")),
        _ => None,
    }
}

#[cfg(all(test, feature = "server"))]
mod server_asset_tests {
    use super::*;

    #[test]
    fn embedded_server_assets_resolve_with_expected_headers() {
        assert!(app_wasm_bytes().starts_with(b"\0asm\x01\0\0\0"));
        assert!(hosted_bundle_html().contains("<html"));
        assert_eq!(asset_bytes("earth_texture.jpg").unwrap().1, "image/jpeg");
        assert_eq!(asset_bytes("home_world_lightfield.webp").unwrap().1, "image/webp");
        assert_eq!(asset_bytes("zen_room_backdrop_2d.webp").unwrap().1, "image/webp");
        assert!(asset_bytes("nonexistent.png").is_none());
    }
}

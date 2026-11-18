# Fabushi iOS

Standalone native iOS application (`Fabushi`, bundle ID `com.ombhrum.fabushi`) built on the reused pure-Rust domain/protocol engine of `bhrum/gpui-telegram@main` with pixel-, gesture-, and animation-faithful native iOS UI/UX modeled directly on `TelegramMessenger/Telegram-iOS`.

- Live functional & Rust upstream: `bhrum/gpui-telegram@main`
- Native iOS UI/UX reference authority: `TelegramMessenger/Telegram-iOS` (`master` @ `24638bc69bd083981b178dd1e7f921eebbf7fc46`)
- Canonical migration Spec: `docs/specs/gpui-telegram-main-ios-parity.md`
- Machine authority lock: `manifests/desktop-main-authority.json`
- Current status: milestone implementation in progress; exact-main source completeness and all native release gates (`IOS-MAIN-AC-01..21`) must pass on final `HEAD`.

Only GitHub Actions or a user-approved remote runtime runs executable build/test/validation. All Rust and Swift build/runtime inputs are self-contained inside this repository (`source/desktop_core/`, `source/host/`, `mobile/ios/`).

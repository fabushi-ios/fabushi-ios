fn main() {
    gpui_telegram_test_harness::tier3_headless::HeadlessTranscriptDriver::run_gpui_headless_app_shell_child(
        "http://127.0.0.1:18080",
    );
}

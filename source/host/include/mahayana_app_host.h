#ifndef MAHAYANA_APP_HOST_H
#define MAHAYANA_APP_HOST_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// ============================================================================
// 1. C Callback Function Pointer for Push Event Streaming
// ============================================================================

/**
 * Function pointer type for receiving asynchronous push events emitted by the
 * desktop Rust core (e.g. new messages, PTS difference updates, reactions, presence).
 *
 * @param event_type    NUL-terminated UTF-8 string indicating event category (e.g., "newMessage", "ptsUpdate").
 * @param payload_json  NUL-terminated UTF-8 JSON string containing event details.
 * @param user_context  Opaque pointer passed during callback registration (e.g., Unmanaged Swift instance pointer).
 *
 * Safety & Threading Contract:
 * - The callback is invoked from an internal Rust Tokio background thread.
 * - String pointers (event_type, payload_json) are valid ONLY for the synchronous duration of the invocation.
 * - Callers MUST synchronously copy the string contents and must not free or retain the raw pointers.
 * - The callback implementation MUST NOT block or deadlock.
 */
typedef void (*TelegramBridgeEventCallback)(
    const char *event_type,
    const char *payload_json,
    void *user_context
);

// ============================================================================
// 2. Telegram Bridge C-ABI Exports (Primary API)
// ============================================================================

/**
 * Creates an instance of the Telegram App Host runtime.
 *
 * @param app_data_dir        NUL-terminated path to the sandbox storage directory. If NULL, defaults to app sandbox.
 * @param storage_passphrase  NUL-terminated secure passphrase retrieved from iOS Keychain for SQLite encryption.
 * @return                    Opaque pointer to TelegramAppHost on success, or NULL on failure.
 */
void *telegram_app_host_create(
    const char *app_data_dir,
    const char *storage_passphrase
);

/**
 * Creates an instance of the Telegram App Host in deterministic test mode for UI/unit test harnesses.
 *
 * @param app_data_dir  NUL-terminated path to the sandbox storage directory.
 * @return              Opaque pointer to TelegramAppHost on success, or NULL on failure.
 */
void *telegram_app_host_create_test(
    const char *app_data_dir
);

/**
 * Returns the last initialization error message recorded on this thread, if any.
 *
 * @return  Borrowed pointer to NUL-terminated C string, or NULL if no error occurred.
 *          Caller must copy immediately and MUST NOT free this pointer.
 */
const char *telegram_app_host_last_error(void);

/**
 * Synchronously dispatches a JSON-RPC request to the Telegram App Host runtime.
 *
 * @param host          Live opaque handle returned by telegram_app_host_create.
 * @param request_json  NUL-terminated UTF-8 JSON request string (e.g., {"command":"telegram.sendMessage","params":{...}}).
 * @return              Newly allocated NUL-terminated UTF-8 JSON response string.
 *                      Caller MUST free this string using `telegram_app_host_free_string`.
 */
char *telegram_app_host_dispatch(
    void *host,
    const char *request_json
);

/**
 * Alias for telegram_app_host_dispatch.
 */
char *telegram_app_host_dispatch_with_handle(
    void *host,
    const char *request_json
);

/**
 * Registers a push event callback with the Telegram App Host runtime.
 *
 * @param host          Live opaque handle returned by telegram_app_host_create.
 * @param callback      C function pointer to be invoked on real-time events. Pass NULL to unregister.
 * @param user_context  Opaque context pointer forwarded to the callback on every invocation.
 */
void telegram_bridge_register_event_callback(
    void *host,
    TelegramBridgeEventCallback callback,
    void *user_context
);

/**
 * Releases memory for a string allocated by the Rust runtime.
 *
 * @param pointer  Pointer returned by telegram_app_host_dispatch or legacy dispatch functions.
 *                 Safe to pass NULL (no-op).
 */
void telegram_app_host_free_string(
    char *pointer
);

/**
 * Destroys a Telegram App Host instance and reclaims all associated resources.
 *
 * @param host  Opaque handle returned by telegram_app_host_create. Safe to pass NULL (no-op).
 */
void telegram_app_host_destroy(
    void *host
);

// ============================================================================
// 3. Backward-Compatibility Aliases (Legacy mahayana_* Symbols)
// ============================================================================

void *mahayana_app_host_create(const char *app_data_dir);
void *mahayana_app_host_create_with_storage_passphrase(const char *app_data_dir, const char *storage_passphrase);
void *mahayana_app_host_create_test(const char *app_data_dir);
const char *mahayana_app_host_last_error(void);
char *mahayana_app_host_dispatch_with_handle(void *host, const char *request_json);
char *mahayana_app_host_dispatch(const char *request_json);
void mahayana_app_host_free_string(char *pointer);
void mahayana_app_host_destroy(void *host);
char *mahayana_cloud_agent_trace_rows(const unsigned char *bytes, size_t len);
char *mahayana_spreadsheet_parse_file(const char *path, size_t max_bytes, size_t max_rows);

// ============================================================================
// 4. Sliding Replay Window C-ABI Exports (Feature 19: Secret Chats E2EE)
// ============================================================================

/**
 * Creates an instance of the 128-bit sliding replay window.
 *
 * @return Opaque pointer to SlidingReplayWindow128 on success, or NULL on panic.
 */
void *telegram_replay_window_create(void);

/**
 * Checks if a sequence number is within the valid 128-bit sliding window and not previously seen.
 * If valid, marks the sequence number as seen and advances the window if seq > highest.
 *
 * @param window  Live opaque handle returned by telegram_replay_window_create.
 * @param seq     64-bit sequence counter (must be >= 1).
 * @return        true if sequence was valid and admitted; false if duplicate, stale (>128 behind), or 0.
 */
bool telegram_replay_window_check_and_add(void *window, uint64_t seq);

/**
 * Destroys a sliding replay window instance and frees associated memory.
 *
 * @param window  Opaque handle returned by telegram_replay_window_create. Safe to pass NULL.
 */
void telegram_replay_window_destroy(void *window);

/**
 * Returns the maximum permissible MTProto packet size in bytes (16MB = 16,777,216).
 */
size_t telegram_proto_max_packet_size(void);

#ifdef __cplusplus
}
#endif

#endif // MAHAYANA_APP_HOST_H

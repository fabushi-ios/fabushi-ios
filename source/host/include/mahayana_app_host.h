#ifndef MAHAYANA_APP_HOST_H
#define MAHAYANA_APP_HOST_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// ============================================================================
// Legacy Mahayana App Host C-ABI Compatibility Layer
// ============================================================================

void *mahayana_app_host_create(const char *app_data_dir);
void *mahayana_app_host_create_test(const char *app_data_dir);
const char *mahayana_app_host_last_error(void);
char *mahayana_app_host_dispatch(void *host, const char *request_json);
char *mahayana_spreadsheet_parse_file(const char *file_path, uint32_t max_rows, uint32_t max_columns);
char *mahayana_cloud_agent_trace_rows(const char *file_path, const char *filter_json);
void mahayana_app_host_free_string(char *pointer);
void mahayana_app_host_destroy(void *host);

// ============================================================================
// Native iOS Telegram Core C-ABI Bridge (v1.2.0)
// ============================================================================

/**
 * Callback signature for asynchronous push notifications from Rust to Swift.
 * Passing `NULL` to `telegram_bridge_register_event_callback` unregisters any active callback.
 *
 * @param event_type   Null-terminated UTF-8 string identifying the event type
 * @param payload_json Null-terminated UTF-8 JSON payload string
 * @param user_context Opaque pointer supplied during registration (e.g., Unmanaged Swift reference)
 */
typedef void (*TelegramBridgeEventCallback)(
    const char *event_type,
    const char *payload_json,
    void *user_context
);

/**
 * Creates a production TelegramAppHost instance backed by SQLite WAL and network transport.
 */
void *telegram_app_host_create(const char *app_data_dir, const char *storage_passphrase);

/**
 * Creates a deterministic isolated test TelegramAppHost instance.
 */
void *telegram_app_host_create_test(const char *app_data_dir);

/**
 * Returns the most recent thread-local C-ABI error message, or NULL if no error occurred.
 */
const char *telegram_app_host_last_error(void);

/**
 * Synchronously dispatches a JSON-RPC request envelope across the FFI boundary.
 * Caught by `std::panic::catch_unwind` to guarantee zero Rust panics cross into Swift.
 * Caller MUST free the returned string via `telegram_app_host_free_string`.
 */
char *telegram_app_host_dispatch(void *host, const char *request_json);

/**
 * Dispatches a JSON-RPC request envelope carrying an optional cancellation handle ID.
 * Caller MUST free the returned string via `telegram_app_host_free_string`.
 */
char *telegram_app_host_dispatch_with_handle(void *host, const char *request_json);

/**
 * Registers (or unregisters when `callback == NULL`) an asynchronous push event callback.
 */
void telegram_bridge_register_event_callback(
    void *host,
    TelegramBridgeEventCallback callback,
    void *user_context
);

/**
 * Frees a heap-allocated C string returned by `telegram_app_host_dispatch*`.
 */
void telegram_app_host_free_string(char *pointer);

/**
 * Cleans up event callbacks, background workers, and destroys the TelegramAppHost instance.
 */
void telegram_app_host_destroy(void *host);

// ============================================================================
// Replay Window & Protocol Constants C-ABI Exports
// ============================================================================

void *telegram_replay_window_create(void);
bool telegram_replay_window_check_and_add(void *window, uint64_t seq);
void telegram_replay_window_destroy(void *window);
uint32_t telegram_proto_max_packet_size(void);

#ifdef __cplusplus
}
#endif

#endif

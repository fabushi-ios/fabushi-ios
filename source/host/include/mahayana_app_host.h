#ifndef MAHAYANA_APP_HOST_H
#define MAHAYANA_APP_HOST_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * Push-event callback invoked asynchronously on Rust runtime threads.
 * Pass NULL to telegram_bridge_register_event_callback to unregister.
 */
typedef void (*TelegramBridgeEventCallback)(
    const char * _Nullable event_type,
    const char * _Nullable payload_json,
    void * _Nullable user_context
);

void * _Nullable telegram_app_host_create(
    const char * _Nullable app_data_dir,
    const char * _Nullable storage_passphrase
);

void * _Nullable telegram_app_host_create_test(
    const char * _Nullable app_data_dir
);

const char * _Nullable telegram_app_host_last_error(void);

char * _Nullable telegram_app_host_dispatch(
    void * _Nullable host,
    const char * _Nullable request_json
);

char * _Nullable telegram_app_host_dispatch_with_handle(
    void * _Nullable host,
    const char * _Nullable request_json
);

void telegram_bridge_register_event_callback(
    void * _Nullable host,
    TelegramBridgeEventCallback _Nullable callback,
    void * _Nullable user_context
);

void telegram_app_host_free_string(char * _Nullable s);

void telegram_app_host_destroy(void * _Nullable host);

/* Replay window C-ABI (128-bit sliding replay protection) */
void * _Nullable telegram_replay_window_create(void);
bool telegram_replay_window_check_and_add(void * _Nullable window, uint64_t seq);
void telegram_replay_window_destroy(void * _Nullable window);

size_t telegram_proto_max_packet_size(void);

/* Legacy backward-compatibility C-ABI exports called by MahayanaHostRuntime.swift & NativeSpreadsheetPreview.swift */
void * _Nullable mahayana_app_host_create(const char * _Nullable app_data_dir);
void * _Nullable mahayana_app_host_create_with_storage_passphrase(
    const char * _Nullable app_data_dir,
    const char * _Nullable storage_passphrase
);
void * _Nullable mahayana_app_host_create_test(const char * _Nullable app_data_dir);
const char * _Nullable mahayana_app_host_last_error(void);
char * _Nullable mahayana_app_host_dispatch_with_handle(
    void * _Nullable host,
    const char * _Nullable request_json
);
char * _Nullable mahayana_app_host_dispatch(const char * _Nullable request_json);
void mahayana_app_host_free_string(char * _Nullable s);
void mahayana_app_host_destroy(void * _Nullable host);

char * _Nullable mahayana_cloud_agent_trace_rows(
    const unsigned char * _Nullable bytes,
    size_t len
);

char * _Nullable mahayana_spreadsheet_parse_file(
    const char * _Nullable path,
    size_t max_bytes,
    size_t max_rows
);

#ifdef __cplusplus
}
#endif

#endif /* MAHAYANA_APP_HOST_H */

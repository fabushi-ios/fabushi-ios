#ifndef MAHAYANA_APP_HOST_H
#define MAHAYANA_APP_HOST_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

void *mahayana_app_host_create(const char *app_data_dir);
void *mahayana_app_host_create_with_storage_passphrase(const char *app_data_dir, const char *storage_passphrase);
void *mahayana_app_host_create_test(const char *app_data_dir);
char *mahayana_app_host_dispatch_with_handle(void *host, const char *request_json);
void mahayana_app_host_destroy(void *host);
void mahayana_app_host_free_string(char *pointer);

#ifdef __cplusplus
}
#endif

#endif

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <assert.h>
#include "../../source/host/include/mahayana_app_host.h"

static int g_events_received = 0;
static pthread_mutex_t g_event_lock = PTHREAD_MUTEX_INITIALIZER;

static void test_event_cb(const char *event_type, const char *payload_json, void *user_context) {
    pthread_mutex_lock(&g_event_lock);
    g_events_received++;
    pthread_mutex_unlock(&g_event_lock);
    (void)user_context;
}

#define NUM_THREADS 8
#define REQS_PER_THREAD 50

struct ThreadArg {
    void *host;
    int thread_id;
    int success_count;
};

void *worker_fn(void *arg) {
    struct ThreadArg *t = (struct ThreadArg *)arg;
    for (int i = 0; i < REQS_PER_THREAD; i++) {
        const char *cmd = "{\"command\":\"telegram.ping\",\"params\":{\"nonce\":100}}";
        if (i % 3 == 1) {
            cmd = "{\"command\":\"telegram.sendMessage\",\"params\":{\"chat_id\":42,\"text\":\"pthread msg\"}}";
        } else if (i % 3 == 2) {
            cmd = "{\"command\":\"telegram.getMessages\",\"params\":{\"chat_id\":42}}";
        }
        char *resp = telegram_app_host_dispatch(t->host, cmd);
        if (resp != NULL && strstr(resp, "\"ok\":true") != NULL) {
            t->success_count++;
        }
        telegram_app_host_free_string(resp);
    }
    return NULL;
}

int main(void) {
    printf("=== C-ABI Empirical Verification Harness ===\n");

    // 1. Host Creation
    void *host = telegram_app_host_create_test("/tmp/c_abi_test_dir");
    assert(host != NULL);
    printf("[PASS] telegram_app_host_create_test\n");

    // 2. Register C Callback
    telegram_bridge_register_event_callback(host, test_event_cb, NULL);
    printf("[PASS] telegram_bridge_register_event_callback\n");

    // 3. Dispatch Valid Command & Event Trigger
    char *res1 = telegram_app_host_dispatch(host, "{\"command\":\"telegram.sendMessage\",\"params\":{\"chat_id\":1,\"text\":\"hello from C\"}}");
    assert(res1 != NULL);
    assert(strstr(res1, "\"ok\":true") != NULL);
    telegram_app_host_free_string(res1);
    printf("[PASS] telegram_app_host_dispatch valid message\n");

    // Verify event callback fired
    pthread_mutex_lock(&g_event_lock);
    assert(g_events_received > 0);
    pthread_mutex_unlock(&g_event_lock);
    printf("[PASS] push event callback fired (%d events)\n", g_events_received);

    // Test Unregister Callback with NULL
    telegram_bridge_register_event_callback(host, NULL, NULL);
    char *res_unreg = telegram_app_host_dispatch(host, "{\"command\":\"telegram.sendMessage\",\"params\":{\"chat_id\":2,\"text\":\"after unreg\"}}");
    assert(res_unreg != NULL);
    assert(strstr(res_unreg, "\"ok\":true") != NULL);
    telegram_app_host_free_string(res_unreg);
    printf("[PASS] unregister callback with NULL\n");

    // 4. Panic Isolation Verification
    char *res_panic = telegram_app_host_dispatch(host, "{\"command\":\"debug.trigger_panic\"}");
    assert(res_panic != NULL);
    assert(strstr(res_panic, "\"code\":\"HOST_PANIC\"") != NULL);
    telegram_app_host_free_string(res_panic);
    printf("[PASS] panic isolation: process survived deliberate Rust panic\n");

    // 5. NULL Pointer Resilience
    char *res_null1 = telegram_app_host_dispatch(NULL, "{\"command\":\"ping\"}");
    assert(res_null1 != NULL);
    assert(strstr(res_null1, "\"code\":\"INVALID_ARGUMENTS\"") != NULL);
    telegram_app_host_free_string(res_null1);

    char *res_null2 = telegram_app_host_dispatch(host, NULL);
    assert(res_null2 != NULL);
    assert(strstr(res_null2, "\"code\":\"INVALID_ARGUMENTS\"") != NULL);
    telegram_app_host_free_string(res_null2);

    telegram_app_host_free_string(NULL);
    telegram_app_host_destroy(NULL);
    telegram_bridge_register_event_callback(NULL, test_event_cb, NULL);
    printf("[PASS] NULL pointer resilience across entry points\n");

    // 6. Multi-Threaded Pthread Hammering
    pthread_t threads[NUM_THREADS];
    struct ThreadArg args[NUM_THREADS];
    for (int i = 0; i < NUM_THREADS; i++) {
        args[i].host = host;
        args[i].thread_id = i;
        args[i].success_count = 0;
        pthread_create(&threads[i], NULL, worker_fn, &args[i]);
    }
    for (int i = 0; i < NUM_THREADS; i++) {
        pthread_join(threads[i], NULL);
        assert(args[i].success_count == REQS_PER_THREAD);
    }
    printf("[PASS] Multi-threaded pthread concurrency (%d threads x %d reqs = %d total)\n",
           NUM_THREADS, REQS_PER_THREAD, NUM_THREADS * REQS_PER_THREAD);

    // 7. Cleanup
    telegram_app_host_destroy(host);
    printf("[PASS] telegram_app_host_destroy\n");

    printf("=== ALL C-ABI NATIVE VERIFICATION TESTS PASSED ===\n");
    return 0;
}

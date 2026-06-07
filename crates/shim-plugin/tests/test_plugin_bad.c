#include <stddef.h>

typedef const char* (*NameFn)(void);
typedef int (*InitFn)(const char* config_json);
typedef int (*VoidFn)(void);
typedef const char* (*MetricsFn)(void);

struct PluginVTable {
    NameFn name;
    InitFn init;
    VoidFn start;
    VoidFn stop;
    MetricsFn metrics;
};

static const char* bad_name(void) { return "bad_test_plugin"; }
static int bad_init(const char* config_json) { (void)config_json; return 1; }
static int bad_start(void) { return 2; }
static int bad_stop(void) { return 3; }
static const char* bad_metrics(void) { return "[]"; }

static struct PluginVTable bad_vtable = {
    .name = bad_name,
    .init = bad_init,
    .start = bad_start,
    .stop = bad_stop,
    .metrics = bad_metrics,
};

struct PluginVTable* shim_plugin_init(void) {
    return &bad_vtable;
}

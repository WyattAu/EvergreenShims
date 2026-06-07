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

static const char* good_name(void) { return "good_test_plugin"; }
static int good_init(const char* config_json) { (void)config_json; return 0; }
static int good_start(void) { return 0; }
static int good_stop(void) { return 0; }
static const char* good_metrics(void) { return "[]"; }

static struct PluginVTable good_vtable = {
    .name = good_name,
    .init = good_init,
    .start = good_start,
    .stop = good_stop,
    .metrics = good_metrics,
};

struct PluginVTable* shim_plugin_init(void) {
    return &good_vtable;
}

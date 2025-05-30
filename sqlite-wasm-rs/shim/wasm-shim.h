#include <stddef.h>
#include <stdint.h>
#include <time.h>

void *rust_sqlite_wasm_shim_malloc(size_t size);
void *rust_sqlite_wasm_shim_realloc(void *ptr, size_t size);
void rust_sqlite_wasm_shim_free(void *ptr);
void *rust_sqlite_wasm_shim_calloc(size_t num, size_t size);

double rust_sqlite_wasm_shim_emscripten_get_now(void);
void rust_sqlite_wasm_shim_localtime_js(time_t t, struct tm *__restrict__ tm);
void rust_sqlite_wasm_shim_tzset_js(long *timezone, int *daylight,
                                    char *std_name, char *dst_name);

uint16_t rust_sqlite_wasm_shim_wasi_random_get(uint8_t *buf, size_t buf_len);

void rust_sqlite_wasm_shim_exit(int code);

void rust_sqlite_wasm_shim_abort_js();

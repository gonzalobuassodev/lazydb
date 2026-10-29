#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

const char *duckdb_library_version(void) {
    return "v1.5.6";
}

void duckdb_free(void *pointer) {
    (void)pointer;
}

bool duckdb_open(const char *path, void **database) {
    (void)path;
    if (database != NULL) {
        *database = NULL;
    }
    return false;
}

void duckdb_close(void **database) {
    if (database != NULL) {
        *database = NULL;
    }
}

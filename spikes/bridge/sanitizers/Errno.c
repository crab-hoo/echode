// Exercise Darwin's per-thread errno lifetime without Echode, Rust, or hardware.
#include <assert.h>
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

// Counts and path buffers bound only this standalone regression fixture.
static const char directory[] = ".";
static const char missing[] = "/dev/null/echode-errno-missing-entry";

static void verify_runtime(const char *expected) {
    Dl_info info;
    assert(dladdr(dlsym(RTLD_DEFAULT, "__tsan_read4"), &info));
    char actual_path[PATH_MAX], expected_path[PATH_MAX];
    assert(realpath(info.dli_fname, actual_path));
    assert(realpath(expected, expected_path));
    assert(strcmp(actual_path, expected_path) == 0);
    unsigned count = 0;
    for (uint32_t i = 0; i < _dyld_image_count(); ++i)
        if (strstr(_dyld_get_image_name(i), "libclang_rt.tsan_")) ++count;
    assert(count == 1);
    printf("PASS single pinned TSan runtime: %s\n", actual_path);
}

static void *reuse_errno(void *argument) {
    (void)argument;
    for (int i = 0; i < 8; ++i) {
        struct stat status;
        errno = 0;
        assert(stat(missing, &status) == -1);
        assert(errno == ENOTDIR);
        DIR *entries = opendir(directory);
        assert(entries);
        errno = 0;
        while (readdir(entries)) {}
        assert(errno == 0);
        assert(closedir(entries) == 0);
    }
    return NULL;
}

static void *create_threads(void *argument) {
    (void)argument;
    for (int i = 0; i < 300; ++i) {
        pthread_t child;
        assert(pthread_create(&child, NULL, reuse_errno, NULL) == 0);
        assert(pthread_join(child, NULL) == 0);
    }
    return NULL;
}

__attribute__((noinline)) static void *foreign_errno_write(void *argument) {
    // The owner is alive. Both workers deliberately write its errno, not theirs.
    volatile int *foreign = argument;
    for (int i = 0; i < 100000; ++i) *foreign = i;
    return NULL;
}

int main(int argc, char **argv) {
    assert(argc == 3);
    verify_runtime(argv[2]);
    if (strcmp(argv[1], "race") == 0) {
        int *foreign = &errno;
        pthread_t first, second;
        assert(pthread_create(&first, NULL, foreign_errno_write, foreign) == 0);
        assert(pthread_create(&second, NULL, foreign_errno_write, foreign) == 0);
        assert(pthread_join(first, NULL) == 0);
        assert(pthread_join(second, NULL) == 0);
        return 0;
    }
    assert(strcmp(argv[1], "reuse") == 0);
    pthread_t parents[8];
    for (int i = 0; i < 8; ++i)
        assert(pthread_create(&parents[i], NULL, create_threads, NULL) == 0);
    for (int i = 0; i < 8; ++i) assert(pthread_join(parents[i], NULL) == 0);
    puts("PASS errno lifetime across 2400 thread creations");
}

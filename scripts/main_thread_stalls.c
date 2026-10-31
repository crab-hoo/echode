// Diagnostic only: inserted into a sanitizer test process by main_thread_stalls.py.
// Records how long the main queue stays silent, samples the main thread during
// each silence, and names who triggers runtime framework loads.
#include <dispatch/dispatch.h>
#include <dlfcn.h>
#include <execinfo.h>
#include <fcntl.h>
#include <mach-o/dyld.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <unistd.h>

// Sizes are scoped to this diagnostic: a longer stack or stall is truncated, not lost.
enum { FRAMES = 96, SAMPLES = 64 };
static const uint64_t SILENCE_NS = 500000000, SAMPLE_NS = 250000000, BURST_GAP_NS = 200000000;

static int log_fd = -1;
static pthread_t main_pthread;
static mach_port_t main_port;
static mach_timebase_info_data_t timebase;
static uint64_t start_ns, last_image_ns;
static _Atomic uint64_t last_pong;
static _Atomic int ping_outstanding, served;

static uint64_t now_ns(void) {
    return mach_absolute_time() * timebase.numer / timebase.denom;
}

__attribute__((format(printf, 1, 2))) static void say(const char *format, ...) {
    char line[2048];
    int used = snprintf(line, sizeof line, "%9.3f ", (double)(now_ns() - start_ns) / 1e9);
    va_list arguments;
    va_start(arguments, format);
    int more = vsnprintf(line + used, sizeof line - (size_t)used - 1, format, arguments);
    va_end(arguments);
    size_t length = (size_t)used + (more < 0 ? 0 : (size_t)more);
    if (length > sizeof line - 2) length = sizeof line - 2;
    line[length] = '\n';
    (void)!write(log_fd, line, length + 1);
}

static void describe(uintptr_t address, const char *prefix) {
    Dl_info info;
    memset(&info, 0, sizeof info);
    dladdr((void *)address, &info);
    const char *image = info.dli_fname ? strrchr(info.dli_fname, '/') : NULL;
    say("%s %s %s", prefix, image ? image + 1 : "?", info.dli_sname ? info.dli_sname : "?");
}

static void on_image(const struct mach_header *header, intptr_t slide) {
    (void)slide;
    if (!atomic_load(&served)) return;  // Runtime loads only, not process launch.
    uint64_t now = now_ns();
    int on_main = pthread_equal(pthread_self(), main_pthread);
    describe((uintptr_t)header, on_main ? "IMAGE main" : "IMAGE other");
    if (now - last_image_ns > BURST_GAP_NS) {
        void *frames[FRAMES];
        int count = backtrace(frames, FRAMES);
        say("BURST %s", on_main ? "main" : "other");
        for (int i = 0; i < count; i++) describe((uintptr_t)frames[i], "frame");
    }
    last_image_ns = now;
}

static void pong(void *context) {
    (void)context;
    atomic_store(&last_pong, now_ns());
    atomic_store(&ping_outstanding, 0);
    atomic_store(&served, 1);
}

// Nothing allocates between suspend and resume: the main thread may hold the malloc lock.
static int sample_main(uintptr_t *frames) {
    if (thread_suspend(main_port) != KERN_SUCCESS) return 0;
    arm_thread_state64_t state;
    mach_msg_type_number_t count = ARM_THREAD_STATE64_COUNT;
    int used = 0;
    if (thread_get_state(main_port, ARM_THREAD_STATE64, (thread_state_t)&state, &count) == KERN_SUCCESS) {
        frames[used++] = (uintptr_t)arm_thread_state64_get_pc(state);
        uintptr_t frame = (uintptr_t)arm_thread_state64_get_fp(state);
        while (frame && used < FRAMES && (frame & 7) == 0) {
            uintptr_t pair[2];
            vm_size_t got = 0;
            if (vm_read_overwrite(mach_task_self(), frame, sizeof pair, (vm_address_t)pair, &got) != KERN_SUCCESS) break;
            uintptr_t link = pair[1] & 0x0000000FFFFFFFFFULL;  // Strip the pointer signature.
            if (!link) break;
            frames[used++] = link;
            if (pair[0] <= frame) break;
            frame = pair[0];
        }
    }
    thread_resume(main_port);
    return used;
}

static void *watch(void *context) {
    (void)context;
    static uintptr_t samples[SAMPLES][FRAMES];
    static int depths[SAMPLES];
    int stored = 0, stalled = 0;
    uint64_t stall_start = 0, next_sample = 0;
    for (;;) {
        usleep(50000);
        uint64_t now = now_ns();
        if (!atomic_exchange(&ping_outstanding, 1)) dispatch_async_f(dispatch_get_main_queue(), NULL, pong);
        if (!atomic_load(&served)) continue;
        uint64_t pong_at = atomic_load(&last_pong);
        if (now > pong_at && now - pong_at > SILENCE_NS) {
            if (!stalled) {
                stalled = 1;
                stall_start = pong_at;
                stored = 0;
                next_sample = now;
            }
            if (now >= next_sample && stored < SAMPLES) {
                depths[stored] = sample_main(samples[stored]);
                stored++;
                next_sample = now + SAMPLE_NS;
            }
        } else if (stalled) {
            // Symbolicate only now: dladdr would wait on a main thread that is inside dlopen.
            stalled = 0;
            say("STALL %.3f", (double)(pong_at - stall_start) / 1e9);
            for (int sample = 0; sample < stored; sample++) {
                say("SAMPLE");
                for (int i = 0; i < depths[sample]; i++) describe(samples[sample][i], "frame");
            }
        }
    }
    return NULL;
}

__attribute__((constructor)) static void start(void) {
    const char *program = getprogname();
    const char *path = getenv("ECHODE_MAIN_THREAD_STALLS_LOG");
    if (!path || !program || strcmp(program, "swiftpm-testing-helper") != 0) return;
    log_fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0644);
    if (log_fd < 0) return;
    mach_timebase_info(&timebase);
    start_ns = now_ns();
    main_pthread = pthread_self();
    main_port = pthread_mach_thread_np(main_pthread);
    atomic_store(&last_pong, start_ns);
    struct timeval wall;
    gettimeofday(&wall, NULL);
    say("START %.3f", (double)wall.tv_sec + (double)wall.tv_usec / 1e6);
    _dyld_register_func_for_add_image(on_image);
    pthread_t thread;
    if (pthread_create(&thread, NULL, watch, NULL) == 0) pthread_detach(thread);
}

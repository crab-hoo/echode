// Diagnostic only: inserted into a sanitizer test process by main_thread_stalls.py.
// Records how long the main queue stays silent, samples the main thread during
// each silence, what the thread and the process did with that time, and who
// triggers runtime framework loads.
#include <dispatch/dispatch.h>
#include <dlfcn.h>
#include <execinfo.h>
#include <fcntl.h>
#include <libproc.h>
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
#include <sys/resource.h>
#include <sys/time.h>
#include <unistd.h>

// Sizes are scoped to this diagnostic: a longer stack or stall is truncated, not lost.
enum { FRAMES = 96, SAMPLES = 64, PROFILE_BATCH = 128 };
static const uint64_t SILENCE_NS = 500000000, SAMPLE_NS = 250000000, BURST_GAP_NS = 200000000;

static int log_fd = -1, profile_fd = -1;
static __thread int thread_fd;  // A thread that writes elsewhere than log_fd.
static pthread_t main_pthread;
static mach_port_t main_port;
static mach_timebase_info_data_t timebase;
static uint64_t start_ns, last_image_ns;
static _Atomic uint64_t last_pong;
static _Atomic int ping_outstanding, served, load_slot;

// What the main thread and the process had used by a moment, in nanoseconds.
typedef struct {
    uint64_t main_user, main_system, process, runnable, read_bytes, pageins;
} Load;
// One pong writes the slot the watcher is not reading, so no read is ever torn:
// only the watcher posts pongs, one at a time.
static Load loads[2];

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
    (void)!write(thread_fd ? thread_fd : log_fd, line, length + 1);
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

static uint64_t ticks_ns(uint64_t ticks) {
    return ticks * timebase.numer / timebase.denom;
}

static uint64_t time_ns(time_value_t time) {
    return (uint64_t)time.seconds * 1000000000 + (uint64_t)time.microseconds * 1000;
}

static int main_info(thread_basic_info_data_t *info) {
    mach_msg_type_number_t count = THREAD_BASIC_INFO_COUNT;
    return thread_info(main_port, THREAD_BASIC_INFO, (thread_info_t)info, &count) == KERN_SUCCESS;
}

static Load load_now(void) {
    Load load;
    memset(&load, 0, sizeof load);
    thread_basic_info_data_t info;
    if (main_info(&info)) {
        load.main_user = time_ns(info.user_time);
        load.main_system = time_ns(info.system_time);
    }
    struct rusage_info_v6 usage;
    if (proc_pid_rusage(getpid(), RUSAGE_INFO_V6, (rusage_info_t *)&usage) == 0) {
        load.process = ticks_ns(usage.ri_user_time + usage.ri_system_time);
        load.runnable = ticks_ns(usage.ri_runnable_time);
        load.read_bytes = usage.ri_diskio_bytesread;
        load.pageins = usage.ri_pageins;
    }
    return load;
}

static void pong(void *context) {
    (void)context;
    int next = !atomic_load(&load_slot);
    loads[next] = load_now();
    atomic_store(&load_slot, next);
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

// With ECHODE_MAIN_THREAD_PROFILE, every pass samples the main thread, stalled or not.
// A batch keeps symbols out of the sampling: dladdr waits while dyld loads a framework.
// Its own file keeps the watcher from waiting here, which would read as a stall.
static void *profile(void *context) {
    (void)context;
    thread_fd = profile_fd;
    static uintptr_t frames[PROFILE_BATCH][FRAMES];
    static int depths[PROFILE_BATCH], states[PROFILE_BATCH];
    static uint64_t times[PROFILE_BATCH];
    for (int stored = 0;;) {
        usleep(50000);
        thread_basic_info_data_t info;
        times[stored] = now_ns();
        states[stored] = main_info(&info) ? info.run_state : 0;
        depths[stored] = sample_main(frames[stored]);
        if (++stored < PROFILE_BATCH) continue;
        for (int sample = 0; sample < stored; sample++) {
            say("PROFILE %.3f %d", (double)(times[sample] - start_ns) / 1e9, states[sample]);
            for (int i = 0; i < depths[sample]; i++) describe(frames[sample][i], "frame");
        }
        stored = 0;
    }
    return NULL;
}

static void *watch(void *context) {
    (void)context;
    static uintptr_t samples[SAMPLES][FRAMES];
    static int depths[SAMPLES];
    int stored = 0, stalled = 0;
    int states[TH_STATE_HALTED + 1] = {0};
    uint64_t stall_start = 0, next_sample = 0, last_pass = now_ns(), longest_gap = 0, longest_sample = 0;
    Load before;
    memset(&before, 0, sizeof before);
    for (;;) {
        usleep(50000);
        uint64_t now = now_ns();
        uint64_t gap = now - last_pass;
        last_pass = now;
        if (!atomic_exchange(&ping_outstanding, 1)) dispatch_async_f(dispatch_get_main_queue(), NULL, pong);
        if (!atomic_load(&served)) continue;
        uint64_t pong_at = atomic_load(&last_pong);
        if (now > pong_at && now - pong_at > SILENCE_NS) {
            if (!stalled) {
                stalled = 1;
                stall_start = pong_at;
                stored = 0;
                next_sample = now;
                before = loads[atomic_load(&load_slot)];
                memset(states, 0, sizeof states);
                longest_gap = longest_sample = 0;
            }
            if (gap > longest_gap) longest_gap = gap;
            thread_basic_info_data_t info;
            if (main_info(&info) && info.run_state >= 0 && info.run_state <= TH_STATE_HALTED) states[info.run_state]++;
            if (now >= next_sample && stored < SAMPLES) {
                depths[stored] = sample_main(samples[stored]);
                stored++;
                uint64_t sampled = now_ns();
                if (sampled - now > longest_sample) longest_sample = sampled - now;
                next_sample = sampled + SAMPLE_NS;
            }
        } else if (stalled) {
            // Symbolicate only now: dladdr would wait on a main thread that is inside dlopen.
            stalled = 0;
            Load after = loads[atomic_load(&load_slot)];
            // The pong that ended the silence wrote `after`. Whether the main thread had
            // the CPU, wanted it, or was blocked follows from these and from its states.
            say("STALL %.3f main_user=%.3f main_system=%.3f process=%.3f runnable=%.3f read_mb=%.1f pageins=%llu "
                "running=%d waiting=%d uninterruptible=%d stopped=%d watch_gap=%.3f sample_block=%.3f",
                (double)(pong_at - stall_start) / 1e9,
                (double)(after.main_user - before.main_user) / 1e9, (double)(after.main_system - before.main_system) / 1e9,
                (double)(after.process - before.process) / 1e9, (double)(after.runnable - before.runnable) / 1e9,
                (double)(after.read_bytes - before.read_bytes) / 1e6, after.pageins - before.pageins,
                states[TH_STATE_RUNNING], states[TH_STATE_WAITING], states[TH_STATE_UNINTERRUPTIBLE], states[TH_STATE_STOPPED],
                (double)longest_gap / 1e9, (double)longest_sample / 1e9);
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
    loads[0] = load_now();
    struct timeval wall;
    gettimeofday(&wall, NULL);
    say("START %.3f", (double)wall.tv_sec + (double)wall.tv_usec / 1e6);
    _dyld_register_func_for_add_image(on_image);
    pthread_t thread;
    if (pthread_create(&thread, NULL, watch, NULL) == 0) pthread_detach(thread);
    const char *profile_path = getenv("ECHODE_MAIN_THREAD_PROFILE");
    if (profile_path) profile_fd = open(profile_path, O_WRONLY | O_CREAT | O_APPEND, 0644);
    if (profile_fd >= 0 && pthread_create(&thread, NULL, profile, NULL) == 0) pthread_detach(thread);
}

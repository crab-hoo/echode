#include <os/workgroup.h>
#include <AudioToolbox/AudioWorkInterval.h>

// A public OS workgroup for headless lifecycle tests, without opening audio IO.
void *echode_test_workgroup_create(void) {
    return AudioWorkIntervalCreate("echode-headless-regression", OS_CLOCK_MACH_ABSOLUTE_TIME, NULL);
}

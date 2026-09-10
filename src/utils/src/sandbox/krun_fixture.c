#include <stdbool.h>
#include <stdint.h>
#include <string.h>

static int failure, step, started, freed, invalid;

void joe_fault(int value) {
    failure = value;
    step = started = freed = invalid = 0;
}

int joe_state(int field) {
    return field == 1 ? started : field == 2 ? freed : invalid;
}

static int next(bool valid) {
    invalid += !valid;
    step += 1;
    return invalid || failure == step ? -1 : 0;
}

int32_t krun_create_ctx(void) {
    return 7;
}

int32_t krun_free_ctx(uint32_t id) {
    freed += 1;
    invalid += id != 7;
    return 0;
}

int32_t krun_set_vm_config(uint32_t id, uint8_t cpus, uint32_t memory) {
    return next(id == 7 && cpus > 0 && memory > 0);
}

int32_t krun_disable_implicit_vsock(uint32_t id) {
    return next(id == 7 && step == 1);
}

int32_t krun_disable_implicit_console(uint32_t id) {
    return next(id == 7 && step == 2);
}

int32_t krun_add_virtio_console_default(uint32_t id, int input, int output, int error) {
    return next(id == 7 && step == 3 && input == 0 && output == 1 && error == 2);
}

int32_t krun_add_virtiofs3(uint32_t id, const char *tag, const char *path, uint64_t dax, bool read_only) {
    bool root = !strcmp(tag, "/dev/root") && !strcmp(path, "/rootfs") && read_only;
    bool workspace = !strcmp(tag, "joe-workspace") && !strcmp(path, "/project") && !read_only;
    return next(id == 7 && dax == 0 && ((step == 4 && root) || (step == 5 && workspace)));
}

int32_t krun_set_exec(uint32_t id, const char *program, const char *const *args, const char *const *env) {
    return next(id == 7 && step == 6 && !strcmp(program, "/bin/sh") &&
        !strcmp(args[0], "/usr/local/libexec/joe-guest") && !strcmp(args[1], "00000000-0000-0000-0000-000000000000") &&
        args[2] == 0 && env != 0 && env[0] == 0);
}

int32_t krun_start_enter(uint32_t id) {
    started += 1;
    invalid += id != 7 || step != 7;
    return -1;
}

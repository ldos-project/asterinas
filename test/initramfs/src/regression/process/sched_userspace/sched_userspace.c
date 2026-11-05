// SPDX-License-Identifier: MPL-2.0

// Checks that the `userspace` CPU-selection policy moves this thread to the
// CPU written to the policy's OQueue. It needs `sched.select_cpu=userspace` and
// at least two CPUs; run it with `make run_kernel AUTO_TEST=sched_userspace
// SMP=4`.

#define _GNU_SOURCE
#include "../../common/test.h"

#include <fcntl.h>
#include <stdint.h>
#include <sys/syscall.h>
#include <unistd.h>

#define PRODUCE_PATH "/oqueues/sched/select_cpu/produce"

static int produce_fd;
static int nr_cpus;

FN_SETUP(open_produce_file)
{
	produce_fd = CHECK(open(PRODUCE_PATH, O_WRONLY));
	nr_cpus = CHECK_WITH(sysconf(_SC_NPROCESSORS_ONLN), _ret >= 2);
}
END_SETUP()

// Writes `cpu` to the policy's OQueue as a CBOR unsigned integer. Supports CPU
// numbers below 256.
static ssize_t request_cpu(int cpu)
{
	uint8_t buf[2] = { 0x18, cpu };

	if (cpu < 24)
		return write(produce_fd, &buf[1], 1);
	return write(produce_fd, buf, 2);
}

static int current_cpu(void)
{
	unsigned int cpu;

	// Use `syscall()` because `getcpu()` from glibc may not use the system
	// call.
	if (syscall(SYS_getcpu, &cpu, NULL, NULL) < 0)
		return -1;
	return cpu;
}

FN_TEST(migrate_to_requested_cpu)
{
	// Visit every CPU twice, from the highest down, so that each request
	// after the first moves the thread to another CPU.
	for (int round = 0; round < 2; round++) {
		for (int cpu = nr_cpus - 1; cpu >= 0; cpu--) {
			TEST_SUCC(request_cpu(cpu) > 0 ? 0 : -1);
			// A wake-up always asks the policy for a CPU. A yield
			// does not when nothing else is queued on this CPU.
			TEST_SUCC(usleep(1000));
			TEST_RES(current_cpu(), _ret == cpu);
		}
	}
}
END_TEST()

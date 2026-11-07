// SPDX-License-Identifier: MPL-2.0

// Tests the `userspace` CPU-selection policy.
//
// With `sched.select_cpu=userspace`, the kernel places every task on the CPU
// that userspace most recently wrote to the policy's OQueue. For each CPU, this
// test writes the CPU number, sleeps so that its wake-up asks the policy where
// to run, and checks that it now runs on that CPU.
//
// Run it with `make run_kernel AUTO_TEST=sched_userspace SMP=4`.

#define _GNU_SOURCE
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <unistd.h>

// The file that userspace writes the requested CPU to.
#define PRODUCE_PATH "/oqueues/scheduler/select_cpu/produce"

// How many times to visit every CPU.
#define ROUNDS 20

// How long to sleep after each request. 
#define SLEEP_US 1000

// Expect less than 24 CPUs. 
// Or it make the CBOR logic below more complicated, which is not necessary 
// for a simple migration test. 
#define MAX_CPUS 24

// Asks the policy to place tasks on `cpu`. Returns 0 on success.
static int request_cpu(int fd, int cpu)
{
	uint8_t byte = cpu;

	if (write(fd, &byte, 1) != 1)
		return -1;
	return 0;
}

// Returns the CPU this thread is running on, or -1 on failure.
static int current_cpu(void)
{
	unsigned int cpu;

	if (syscall(SYS_getcpu, &cpu, NULL, NULL) < 0)
		return -1;
	return cpu;
}

int main(void)
{
	int fd = open(PRODUCE_PATH, O_WRONLY);
	if (fd < 0) {
		perror("cannot open " PRODUCE_PATH
		       " (was the kernel booted with sched.select_cpu=userspace?)");
		return EXIT_FAILURE;
	}

	int nr_cpus = sysconf(_SC_NPROCESSORS_ONLN);
	if (nr_cpus < 2 || nr_cpus > MAX_CPUS) {
		printf("this test needs 2 to %d CPUs, but found %d\n", MAX_CPUS,
		       nr_cpus);
		return EXIT_FAILURE;
	}

	int failures = 0;
	for (int round = 0; round < ROUNDS; round++) {
		for (int cpu = nr_cpus - 1; cpu >= 0; cpu--) {
			if (request_cpu(fd, cpu) != 0) {
				perror("cannot write to " PRODUCE_PATH);
				return EXIT_FAILURE;
			}
			usleep(SLEEP_US);

			int now = current_cpu();
			if (now == cpu) {
				printf("requested CPU %d, running on CPU %d: ok\n",
				       cpu, now);
			} else {
				printf("requested CPU %d, running on CPU %d: FAILED\n",
				       cpu, now);
				failures++;
			}
		}
	}

	close(fd);
	if (failures > 0) {
		printf("%d of %d requests failed\n", failures,
		       ROUNDS * nr_cpus);
		return EXIT_FAILURE;
	}
	return EXIT_SUCCESS;
}

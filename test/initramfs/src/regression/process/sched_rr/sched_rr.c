// SPDX-License-Identifier: MPL-2.0

// Workloads for checking CPU migration with `sched.select_cpu=round_robin`.
//
//   sched_rr sleep <iters>             one thread that sleeps <iters> times
//   sched_rr spin <threads> <seconds>  CPU-bound threads that never sleep
//
// Each thread prints its TID so that the scheduler capture can be filtered by
// it. This is not part of `run_test.sh`, because it needs a specific boot
// parameter and a host-side check of the capture.
//
// This test program should be used with the round_robin task placement policy.
// The round robin policy always place the task at (last_cpu + 1) % (#cpu) cpu
// everytime a "select_cpu" is called for that task.
//
// For a specific task created in this script, if you see the CPU that the task
// corresponding to your task ID appears something like 0,1,2,3,0,1,2,3 then
// that means the policy works and the task is migrated between the CPUs correctly.

#define _GNU_SOURCE
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static void *spin(void *arg)
{
	time_t end = time(NULL) + *(int *)arg;

	printf("spin tid=%d\n", gettid());
	while (time(NULL) < end)
		;
	return NULL;
}

int main(int argc, char **argv)
{
	setvbuf(stdout, NULL, _IONBF, 0);

	if (argc == 3 && strcmp(argv[1], "sleep") == 0) {
		int iters = atoi(argv[2]);

		printf("sleep tid=%d\n", gettid());
		// Every wake-up goes through `select_cpu`.
		for (int i = 0; i < iters; i++)
			usleep(5000);
		return 0;
	}

	if (argc == 4 && strcmp(argv[1], "spin") == 0) {
		int n = atoi(argv[2]), secs = atoi(argv[3]);
		pthread_t threads[n];

		for (int i = 0; i < n; i++)
			pthread_create(&threads[i], NULL, spin, &secs);
		for (int i = 0; i < n; i++)
			pthread_join(threads[i], NULL);
		return 0;
	}

	fprintf(stderr,
		"usage: sched_rr sleep <iters> | spin <threads> <seconds>\n");
	return 1;
}

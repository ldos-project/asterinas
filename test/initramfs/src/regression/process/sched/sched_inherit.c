// SPDX-License-Identifier: MPL-2.0

/*
 * Tests that a new thread or process starts with its parent's CPU affinity and
 * scheduling policy, as on Linux.
 */

#define _GNU_SOURCE

#include <linux/sched/types.h>
#include <sched.h>
#include <sys/syscall.h>
#include <sys/sysinfo.h>
#include <sys/wait.h>
#include <unistd.h>

#include "../../common/test.h"

static int sys_sched_setattr(pid_t pid, struct sched_attr *attr,
			     unsigned int flags)
{
	return syscall(SYS_sched_setattr, pid, attr, flags);
}

static int sys_sched_getattr(pid_t pid, struct sched_attr *attr,
			     unsigned int size, unsigned int flags)
{
	return syscall(SYS_sched_getattr, pid, attr, size, flags);
}

static void *run_check(void *check)
{
	return ((int (*)(void))check)() ? check : NULL;
}

/*
 * Runs `check` in a new thread and in a forked child. Returns the number of
 * them in which it returned false. A check prints what it saw if it fails.
 */
static int count_failures_in_children(int (*check)(void))
{
	int failures = 0;

	pthread_t thread;
	void *thread_ok;
	if (pthread_create(&thread, NULL, run_check, check) != 0 ||
	    pthread_join(thread, &thread_ok) != 0 || thread_ok == NULL)
		failures++;

	pid_t pid = fork();
	if (pid == 0)
		_exit(check() ? 0 : 1);
	int status;
	if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status) ||
	    WEXITSTATUS(status) != 0)
		failures++;

	return failures;
}

static int last_cpu;

static int on_last_cpu_only(void)
{
	cpu_set_t mask;
	if (sched_getaffinity(0, sizeof(mask), &mask) != 0)
		return 0;
	// glibc's `sched_getcpu` may not use the system call.
	unsigned int cpu;
	if (syscall(SYS_getcpu, &cpu, NULL) != 0)
		return 0;
	if (CPU_COUNT(&mask) == 1 && CPU_ISSET(last_cpu, &mask) &&
	    cpu == (unsigned int)last_cpu)
		return 1;
	fprintf(stderr, "%d CPUs allowed, running on CPU %u\n",
		CPU_COUNT(&mask), cpu);
	return 0;
}

FN_TEST(cpu_affinity)
{
	SKIP_TEST_IF(get_nprocs() < 2);
	last_cpu = get_nprocs() - 1;

	cpu_set_t old_mask, mask;
	TEST_SUCC(sched_getaffinity(0, sizeof(old_mask), &old_mask));
	CPU_ZERO(&mask);
	CPU_SET(last_cpu, &mask);
	TEST_SUCC(sched_setaffinity(0, sizeof(mask), &mask));

	TEST_RES(count_failures_in_children(on_last_cpu_only), _ret == 0);

	TEST_SUCC(sched_setaffinity(0, sizeof(old_mask), &old_mask));
}
END_TEST()

static int is_fifo_20(void)
{
	struct sched_param param = { .sched_priority = -1 };
	int policy = sched_getscheduler(0);
	sched_getparam(0, &param);
	if (policy == SCHED_FIFO && param.sched_priority == 20)
		return 1;
	fprintf(stderr, "policy %d, priority %d\n", policy,
		param.sched_priority);
	return 0;
}

FN_TEST(real_time_policy)
{
	struct sched_param param = { .sched_priority = 20 };
	TEST_SUCC(sched_setscheduler(0, SCHED_FIFO, &param));

	TEST_RES(count_failures_in_children(is_fifo_20), _ret == 0);

	param.sched_priority = 0;
	TEST_SUCC(sched_setscheduler(0, SCHED_OTHER, &param));
}
END_TEST()

static int has_nice_5(void)
{
	struct sched_attr attr = { 0 };
	sys_sched_getattr(0, &attr, sizeof(attr), 0);
	if (attr.sched_policy == SCHED_OTHER && attr.sched_nice == 5)
		return 1;
	fprintf(stderr, "policy %u, nice %d\n", attr.sched_policy,
		attr.sched_nice);
	return 0;
}

FN_TEST(fair_nice)
{
	struct sched_attr attr = {
		.size = sizeof(attr),
		.sched_policy = SCHED_OTHER,
		.sched_nice = 5,
	};
	TEST_SUCC(sys_sched_setattr(0, &attr, 0));

	TEST_RES(count_failures_in_children(has_nice_5), _ret == 0);

	attr.sched_nice = 0;
	TEST_SUCC(sys_sched_setattr(0, &attr, 0));
}
END_TEST()

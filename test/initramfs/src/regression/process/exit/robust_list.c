// SPDX-License-Identifier: MPL-2.0

/*
 * Tests the robust list walk at thread exit (Linux's `exit_robust_list`).
 *
 * Each test forks a child that registers its own robust list head with
 * `set_robust_list`, changes the list *after* registering it, and exits with a
 * raw `exit` system call so that no user-space code touches the list. The locks
 * live in shared memory, so the parent can check what the kernel did to them.
 */

#define _GNU_SOURCE

#include <linux/futex.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include "../../common/test.h"

struct robust_lock {
	struct robust_list list;
	uint32_t futex;
};

struct shared {
	struct robust_list_head head;
	struct robust_lock locks[3];
};

static struct shared *shared;

FN_SETUP(shared_memory)
{
	shared = CHECK_WITH(mmap(NULL, sizeof(*shared), PROT_READ | PROT_WRITE,
				 MAP_SHARED | MAP_ANONYMOUS, -1, 0),
			    _ret != MAP_FAILED);
}
END_SETUP()

#define PI_BIT ((uintptr_t)1)

static struct robust_list *entry(int i, uintptr_t tag)
{
	return (struct robust_list *)((uintptr_t)&shared->locks[i].list | tag);
}

static void reset_shared(void)
{
	memset(shared, 0, sizeof(*shared));
	shared->head.list.next = &shared->head.list;
	shared->head.futex_offset = offsetof(struct robust_lock, futex) -
				    offsetof(struct robust_lock, list);
}

static pid_t gettid_raw(void)
{
	return syscall(SYS_gettid);
}

/*
 * Runs `body` in a child that registers `shared->head` first, then waits for
 * the child to exit. Returns the child's TID, which is its PID.
 */
static pid_t run_child(void (*body)(pid_t tid))
{
	pid_t pid = fork();
	if (pid == 0) {
		if (syscall(SYS_set_robust_list, &shared->head,
			    sizeof(shared->head)) != 0)
			syscall(SYS_exit, 1);
		body(gettid_raw());
		syscall(SYS_exit, 0);
	}
	if (pid < 0)
		return -1;
	int status;
	if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) ||
	    WEXITSTATUS(status) != 0)
		return -1;
	return pid;
}

/* Two locks are added to the list after the head was registered. */
static void hold_two_locks(pid_t tid)
{
	shared->locks[0].futex = tid;
	shared->locks[1].futex = tid;
	shared->locks[0].list.next = entry(1, 0);
	shared->locks[1].list.next = &shared->head.list;
	shared->head.list.next = entry(0, 0);
}

FN_TEST(list_changed_after_registration)
{
	reset_shared();
	TEST_RES(run_child(hold_two_locks), _ret > 0);
	TEST_RES(shared->locks[0].futex, _ret == FUTEX_OWNER_DIED);
	TEST_RES(shared->locks[1].futex, _ret == FUTEX_OWNER_DIED);
}
END_TEST()

/* A lock held by another thread must be left alone. */
static void list_lock_of_other_owner(pid_t tid)
{
	shared->locks[0].futex = tid + 1;
	shared->locks[0].list.next = &shared->head.list;
	shared->head.list.next = entry(0, 0);
}

FN_TEST(other_owner_untouched)
{
	reset_shared();
	pid_t tid = TEST_RES(run_child(list_lock_of_other_owner), _ret > 0);
	TEST_RES(shared->locks[0].futex, _ret == (uint32_t)tid + 1);
}
END_TEST()

/*
 * User space tags PI futexes by setting bit 0 of the entry pointer. The walk
 * must strip the tag and keep going past such an entry.
 */
static void hold_pi_lock_then_normal_lock(pid_t tid)
{
	shared->locks[0].futex = tid;
	shared->locks[1].futex = tid;
	shared->locks[0].list.next = entry(1, 0);
	shared->locks[1].list.next = &shared->head.list;
	shared->head.list.next = entry(0, PI_BIT);
}

FN_TEST(pi_tagged_entry)
{
	reset_shared();
	TEST_RES(run_child(hold_pi_lock_then_normal_lock), _ret > 0);
	TEST_RES(shared->locks[0].futex, _ret == FUTEX_OWNER_DIED);
	TEST_RES(shared->locks[1].futex, _ret == FUTEX_OWNER_DIED);
}
END_TEST()

/*
 * `list_op_pending` names a lock being acquired or released; it is set after
 * the head was registered, so the kernel must read it at exit.
 */
static void acquiring_lock(pid_t tid)
{
	shared->head.list_op_pending = entry(0, 0);
	shared->locks[0].futex = tid;
}

FN_TEST(pending_lock)
{
	reset_shared();
	TEST_RES(run_child(acquiring_lock), _ret > 0);
	TEST_RES(shared->locks[0].futex, _ret == FUTEX_OWNER_DIED);
}
END_TEST()

/* A pending lock that is also on the list must only be handled once. */
static void pending_lock_on_list(pid_t tid)
{
	shared->locks[0].futex = tid | FUTEX_WAITERS;
	shared->locks[0].list.next = &shared->head.list;
	shared->head.list.next = entry(0, 0);
	shared->head.list_op_pending = entry(0, 0);
}

FN_TEST(pending_lock_on_list)
{
	reset_shared();
	TEST_RES(run_child(pending_lock_on_list), _ret > 0);
	TEST_RES(shared->locks[0].futex,
		 _ret == (FUTEX_OWNER_DIED | FUTEX_WAITERS));
}
END_TEST()

/*
 * A thread that dies after releasing a non-PI lock in user space, but before
 * waking a waiter and clearing `list_op_pending`, must have a waiter woken.
 */
static void released_lock_without_wake(pid_t tid)
{
	/* Give the parent time to block on the futex. */
	usleep(200 * 1000);
	shared->head.list_op_pending = entry(0, 0);
	shared->locks[0].futex = 0;
}

static void *wait_for_child(void *arg)
{
	*(pid_t *)arg = run_child(released_lock_without_wake);
	return NULL;
}

FN_TEST(pending_released_lock_wakes_waiter)
{
	reset_shared();
	pthread_t waiter;
	pid_t child = 0;
	TEST_SUCC(pthread_create(&waiter, NULL, wait_for_child, &child));

	struct timespec timeout = { .tv_sec = 2 };
	TEST_RES(syscall(SYS_futex, &shared->locks[0].futex, FUTEX_WAIT, 0,
			 &timeout, NULL, 0),
		 _ret == 0);

	TEST_SUCC(pthread_join(waiter, NULL));
	TEST_RES(child, _ret > 0);
}
END_TEST()

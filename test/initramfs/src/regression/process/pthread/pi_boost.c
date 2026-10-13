// SPDX-License-Identifier: MPL-2.0

// Checks that Asterinas boosts the owner of a contended PI futex (the default
// `lockprio.policy=inherit`), and that this resolves a priority inversion. The
// tests are skipped if boosting is disabled with `lockprio.policy=none`.

#define _GNU_SOURCE

#include <fcntl.h>
#include <linux/futex.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include "../../common/test.h"

#define DECISIONS_PATH "/oqueues/lockprio/decisions/strong_observe"

static pid_t gettid_(void)
{
	return syscall(SYS_gettid);
}

static long futex_pi(_Atomic uint32_t *word, int op,
		     const struct timespec *timeout)
{
	return syscall(SYS_futex, word, op, 0, timeout, NULL, 0);
}

static void sleep_ms(long ms)
{
	struct timespec ts = { .tv_sec = ms / 1000,
			       .tv_nsec = (ms % 1000) * 1000000 };
	nanosleep(&ts, NULL);
}

// Returns whether the kernel command line contains `param`.
static int has_kernel_param(const char *param)
{
	static char cmdline[4096];
	ssize_t len;
	int fd;

	fd = open("/proc/cmdline", O_RDONLY);
	if (fd < 0)
		return 0;
	len = read(fd, cmdline, sizeof(cmdline) - 1);
	close(fd);
	if (len < 0)
		return 0;
	cmdline[len] = '\0';
	return strstr(cmdline, param) != NULL;
}

static int set_policy(int policy, int prio)
{
	struct sched_param param = { .sched_priority = prio };

	return sched_setscheduler(0, policy, &param);
}

// Appends the CBOR encoding of the text string `key` followed by the unsigned
// integer `value`, i.e., one entry of an encoded map.
static size_t encode_entry(uint8_t *buf, const char *key, uint32_t value)
{
	size_t len = strlen(key), n = 0;

	buf[n++] = 0x60 | len;
	memcpy(buf + n, key, len);
	n += len;
	if (value < 24) {
		buf[n++] = value;
	} else if (value <= 0xff) {
		buf[n++] = 0x18;
		buf[n++] = value;
	} else if (value <= 0xffff) {
		buf[n++] = 0x19;
		buf[n++] = value >> 8;
		buf[n++] = value;
	} else {
		buf[n++] = 0x1a;
		for (int shift = 24; shift >= 0; shift -= 8)
			buf[n++] = value >> shift;
	}
	return n;
}

static uint8_t decisions[64 * 1024];
static size_t decisions_len;

// Returns whether some decision about the owner `tid` contains `text` in its
// `from` (if `to` is 0) or `to` policy. A decision is encoded as
// `{time_ns, owner, from, to, contended_futexes}`.
static int has_decision(pid_t tid, const char *text, int to)
{
	uint8_t needle[16];
	size_t needle_len = encode_entry(needle, "owner", tid);
	const char *name = to ? "to" : "from";
	uint8_t field[8];
	size_t field_len = strlen(name) + 1;
	uint8_t *pos = decisions, *end = decisions + decisions_len;

	// The CBOR text-string header of the field name, then the name.
	field[0] = 0x60 | strlen(name);
	memcpy(field + 1, name, strlen(name));

	while ((pos = memmem(pos, end - pos, needle, needle_len)) != NULL) {
		pos += needle_len;
		size_t window = end - pos < 96 ? end - pos : 96;
		uint8_t *policy = memmem(pos, window, field, field_len);
		if (policy == NULL)
			continue;
		policy += field_len;
		window = end - policy < 16 ? end - policy : 16;
		if (memmem(policy, window, text, strlen(text)) != NULL)
			return 1;
	}
	return 0;
}

static void read_decisions(int fd)
{
	ssize_t n;

	while ((n = read(fd, decisions + decisions_len,
			 sizeof(decisions) - decisions_len)) > 0)
		decisions_len += n;
}

// ============================================================================
// The owner is boosted while a real-time thread waits, and then restored
// ============================================================================

static _Atomic uint32_t word;
static _Atomic int owner_locked, owner_release;
static _Atomic pid_t owner_tid;
static _Atomic int owner_policy_after_unlock;

static void *own_until_released(void *arg)
{
	(void)arg;
	if (set_policy(SCHED_OTHER, 0) < 0)
		return (void *)(long)errno;
	atomic_store(&owner_tid, gettid_());
	if (futex_pi(&word, FUTEX_TRYLOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	atomic_store(&owner_locked, 1);
	while (!atomic_load(&owner_release))
		sleep_ms(1);
	if (futex_pi(&word, FUTEX_UNLOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	atomic_store(&owner_policy_after_unlock, sched_getscheduler(0));
	return NULL;
}

static void *lock_and_unlock(void *arg)
{
	(void)arg;
	if (set_policy(SCHED_FIFO, 50) < 0)
		return (void *)(long)errno;
	if (futex_pi(&word, FUTEX_LOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	if (futex_pi(&word, FUTEX_UNLOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	return NULL;
}

FN_TEST(boost_and_restore)
{
	struct sched_param param;
	pthread_t owner, waiter;
	void *ret;
	int fd;

	fd = open(DECISIONS_PATH, O_RDONLY | O_NONBLOCK);
	SKIP_TEST_IF(fd < 0);

	atomic_store(&word, 0);
	TEST_SUCC(pthread_create(&owner, NULL, own_until_released, NULL));
	while (!atomic_load(&owner_locked))
		sleep_ms(1);
	TEST_SUCC(pthread_create(&waiter, NULL, lock_and_unlock, NULL));
	while (!(atomic_load(&word) & FUTEX_WAITERS))
		sleep_ms(1);
	sleep_ms(20);

	// The owner now runs with the waiter's policy, ...
	pid_t tid = atomic_load(&owner_tid);
	TEST_RES(sched_getscheduler(tid), _ret == SCHED_FIFO);
	TEST_RES(sched_getparam(tid, &param), param.sched_priority == 50);

	// ... until it hands the lock to the waiter.
	atomic_store(&owner_release, 1);
	TEST_RES(pthread_join(owner, &ret), ret == NULL);
	TEST_RES(atomic_load(&owner_policy_after_unlock), _ret == SCHED_OTHER);
	TEST_RES(pthread_join(waiter, &ret), ret == NULL);

	read_decisions(fd);
	TEST_SUCC(close(fd));
	TEST_RES(has_decision(tid, "RealTime", 1), _ret);
	TEST_RES(has_decision(tid, "RealTime", 0), _ret);
	TEST_RES(has_decision(tid, "Fair", 1), _ret);
}
END_TEST()

// ============================================================================
// A thread created by a boosted owner starts with the owner's own policy
// ============================================================================

static _Atomic int spawn_child;
static _Atomic int child_policy = -1;

static void *record_policy(void *arg)
{
	(void)arg;
	atomic_store(&child_policy, sched_getscheduler(0));
	return NULL;
}

static void *spawn_while_boosted(void *arg)
{
	pthread_t child;

	(void)arg;
	if (set_policy(SCHED_OTHER, 0) < 0)
		return (void *)(long)errno;
	atomic_store(&owner_tid, gettid_());
	if (futex_pi(&word, FUTEX_TRYLOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	atomic_store(&owner_locked, 1);
	while (!atomic_load(&spawn_child))
		sleep_ms(1);
	if (pthread_create(&child, NULL, record_policy, NULL) != 0 ||
	    pthread_join(child, NULL) != 0)
		return (void *)(long)EAGAIN;
	if (futex_pi(&word, FUTEX_UNLOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	return NULL;
}

FN_TEST(boost_is_not_inherited)
{
	pthread_t owner, waiter;
	void *ret;

	SKIP_TEST_IF(has_kernel_param("lockprio.policy=none"));

	atomic_store(&word, 0);
	atomic_store(&owner_locked, 0);
	TEST_SUCC(pthread_create(&owner, NULL, spawn_while_boosted, NULL));
	while (!atomic_load(&owner_locked))
		sleep_ms(1);
	TEST_SUCC(pthread_create(&waiter, NULL, lock_and_unlock, NULL));
	while (!(atomic_load(&word) & FUTEX_WAITERS))
		sleep_ms(1);
	sleep_ms(20);
	TEST_RES(sched_getscheduler(atomic_load(&owner_tid)),
		 _ret == SCHED_FIFO);

	// Like Linux, the child starts with the owner's own policy.
	atomic_store(&spawn_child, 1);
	TEST_RES(pthread_join(owner, &ret), ret == NULL);
	TEST_RES(atomic_load(&child_policy), _ret == SCHED_OTHER);
	TEST_RES(pthread_join(waiter, &ret), ret == NULL);
}
END_TEST()

// ============================================================================
// Boosting lets a high-priority waiter get past a medium-priority hog
// ============================================================================

// This test also runs with `lockprio.policy=none`, where it checks that the
// inversion does happen without boosting.

// The work the owner does in its critical section once the hog is running.
#define CRITICAL_SECTION_ITERATIONS 20000000UL

static _Atomic int hog_stop, hog_running;

static void *hold_lock_for_a_while(void *arg)
{
	(void)arg;
	if (futex_pi(&word, FUTEX_TRYLOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	atomic_store(&owner_locked, 1);
	// Stay runnable until the hog starts, so that the hog preempts us.
	while (!atomic_load(&hog_running))
		;
	for (volatile unsigned long i = 0; i < CRITICAL_SECTION_ITERATIONS; i++)
		;
	if (futex_pi(&word, FUTEX_UNLOCK_PI, NULL) < 0)
		return (void *)(long)errno;
	return NULL;
}

static void *hog_cpu(void *arg)
{
	(void)arg;
	if (set_policy(SCHED_FIFO, 10) < 0)
		return (void *)(long)errno;
	atomic_store(&hog_running, 1);
	while (!atomic_load(&hog_stop))
		;
	return NULL;
}

FN_TEST(inversion_is_resolved)
{
	struct timespec deadline;
	int boosting = !has_kernel_param("lockprio.policy=none");
	pthread_t owner, hog;
	cpu_set_t cpus;
	void *ret;

	// Run everything on one CPU, so that the hog starves the owner unless the
	// owner is boosted.
	CPU_ZERO(&cpus);
	CPU_SET(0, &cpus);
	TEST_SUCC(sched_setaffinity(0, sizeof(cpus), &cpus));

	atomic_store(&word, 0);
	atomic_store(&owner_locked, 0);
	TEST_SUCC(pthread_create(&owner, NULL, hold_lock_for_a_while, NULL));
	while (!atomic_load(&owner_locked))
		sleep_ms(1);

	TEST_SUCC(set_policy(SCHED_FIFO, 50));
	TEST_SUCC(pthread_create(&hog, NULL, hog_cpu, NULL));
	while (!atomic_load(&hog_running))
		sleep_ms(1);

	// Without a boost, the owner never runs again and this times out.
	TEST_SUCC(clock_gettime(CLOCK_REALTIME, &deadline));
	deadline.tv_sec += 3;
	long locked = futex_pi(&word, FUTEX_LOCK_PI, &deadline);
	int lock_errno = errno;
	atomic_store(&hog_stop, 1);
	TEST_RES(locked == 0 ? 0 : lock_errno,
		 boosting ? _ret == 0 : _ret == ETIMEDOUT);
	if (locked == 0)
		TEST_SUCC(futex_pi(&word, FUTEX_UNLOCK_PI, NULL));

	TEST_RES(pthread_join(hog, &ret), ret == NULL);
	TEST_RES(pthread_join(owner, &ret), ret == NULL);
	TEST_SUCC(set_policy(SCHED_OTHER, 0));
}
END_TEST()

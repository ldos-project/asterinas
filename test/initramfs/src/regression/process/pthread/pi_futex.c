// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE

#include <linux/futex.h>
#include <sched.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include "../../common/test.h"

#define FUTEX_TID(word) ((word) & FUTEX_TID_MASK)

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

// Waits until the kernel has queued a waiter on `word`, which it signals by
// setting FUTEX_WAITERS.
static void wait_for_waiters(_Atomic uint32_t *word)
{
	while (!(atomic_load(word) & FUTEX_WAITERS))
		sleep_ms(1);
	// Give a second waiter that may be racing in time to queue up as well.
	sleep_ms(20);
}

// ============================================================================
// Raw futex operations
// ============================================================================

static _Atomic uint32_t word;

static void *unlock_from_other_thread(void *arg)
{
	(void)arg;
	return (void *)(long)(futex_pi(&word, FUTEX_UNLOCK_PI, NULL) < 0 ?
				      errno :
				      0);
}

FN_TEST(trylock_unlock)
{
	pid_t tid = gettid_();
	pthread_t thread;
	void *ret;

	atomic_store(&word, 0);
	TEST_RES(futex_pi(&word, FUTEX_TRYLOCK_PI, NULL),
		 _ret == 0 && atomic_load(&word) == (uint32_t)tid);
	TEST_ERRNO(futex_pi(&word, FUTEX_TRYLOCK_PI, NULL), EDEADLK);
	TEST_ERRNO(futex_pi(&word, FUTEX_LOCK_PI, NULL), EDEADLK);

	TEST_SUCC(
		pthread_create(&thread, NULL, unlock_from_other_thread, NULL));
	TEST_RES(pthread_join(thread, &ret), (long)ret == EPERM);

	TEST_RES(futex_pi(&word, FUTEX_UNLOCK_PI, NULL),
		 _ret == 0 && atomic_load(&word) == 0);
	TEST_ERRNO(futex_pi(&word, FUTEX_UNLOCK_PI, NULL), EPERM);
	TEST_ERRNO(futex_pi(&word, FUTEX_LOCK_PI | FUTEX_CLOCK_REALTIME, NULL),
		   ENOSYS);
}
END_TEST()

// ============================================================================
// Contention: the owner hands the futex over to a blocked waiter
// ============================================================================

struct waiter {
	pthread_t thread;
	_Atomic pid_t tid;
	int policy;
	int error;
	_Atomic uint32_t word_after_lock;
};

static void *lock_and_unlock(void *arg)
{
	struct waiter *waiter = arg;

	atomic_store(&waiter->tid, gettid_());
	if (waiter->policy != SCHED_OTHER) {
		struct sched_param param = { .sched_priority = 10 };
		if (sched_setscheduler(0, waiter->policy, &param) < 0) {
			waiter->error = errno;
			return NULL;
		}
	}

	if (futex_pi(&word, FUTEX_LOCK_PI, NULL) < 0) {
		waiter->error = errno;
		return NULL;
	}
	atomic_store(&waiter->word_after_lock, atomic_load(&word));
	if (futex_pi(&word, FUTEX_UNLOCK_PI, NULL) < 0)
		waiter->error = errno;
	return NULL;
}

FN_TEST(handoff)
{
	pid_t tid = gettid_();
	struct waiter waiter = { .policy = SCHED_OTHER };

	atomic_store(&word, tid);
	TEST_SUCC(
		pthread_create(&waiter.thread, NULL, lock_and_unlock, &waiter));
	wait_for_waiters(&word);
	TEST_RES(atomic_load(&word), _ret == (tid | FUTEX_WAITERS));

	// The waiter becomes the owner. Like Linux, FUTEX_WAITERS stays set so
	// that its release also goes through the kernel.
	TEST_SUCC(futex_pi(&word, FUTEX_UNLOCK_PI, NULL));
	TEST_SUCC(pthread_join(waiter.thread, NULL));
	TEST_RES(waiter.error, _ret == 0);
	TEST_RES(atomic_load(&waiter.word_after_lock),
		 _ret == ((uint32_t)atomic_load(&waiter.tid) | FUTEX_WAITERS));
	TEST_RES(atomic_load(&word), _ret == 0);
}
END_TEST()

static _Atomic int release_holder;

static void *hold_word(void *arg)
{
	(void)arg;
	futex_pi(&word, FUTEX_TRYLOCK_PI, NULL);
	while (!atomic_load(&release_holder))
		sleep_ms(1);
	futex_pi(&word, FUTEX_UNLOCK_PI, NULL);
	return NULL;
}

FN_TEST(timeout)
{
	struct timespec deadline;
	pthread_t holder;

	atomic_store(&word, 0);
	atomic_store(&release_holder, 0);
	TEST_SUCC(pthread_create(&holder, NULL, hold_word, NULL));
	while (atomic_load(&word) == 0)
		sleep_ms(1);

	// FUTEX_LOCK_PI takes an absolute CLOCK_REALTIME deadline.
	TEST_SUCC(clock_gettime(CLOCK_REALTIME, &deadline));
	deadline.tv_nsec += 50 * 1000000;
	if (deadline.tv_nsec >= 1000000000) {
		deadline.tv_sec++;
		deadline.tv_nsec -= 1000000000;
	}
	TEST_ERRNO(futex_pi(&word, FUTEX_LOCK_PI, &deadline), ETIMEDOUT);

	atomic_store(&release_holder, 1);
	TEST_SUCC(pthread_join(holder, NULL));
	TEST_RES(atomic_load(&word), _ret == 0);
}
END_TEST()

FN_TEST(owner_does_not_exist)
{
	// The largest TID is never allocated, so nobody can release this lock.
	atomic_store(&word, FUTEX_TID_MASK);
	TEST_ERRNO(futex_pi(&word, FUTEX_LOCK_PI, NULL), ESRCH);
	TEST_ERRNO(futex_pi(&word, FUTEX_TRYLOCK_PI, NULL), ESRCH);
	atomic_store(&word, 0);
}
END_TEST()

FN_TEST(realtime_waiter_first)
{
	pid_t tid = gettid_();
	struct waiter normal = { .policy = SCHED_OTHER };
	struct waiter realtime = { .policy = SCHED_FIFO };
	struct sched_param param = { .sched_priority = 10 };

	// Raising a thread to SCHED_FIFO needs CAP_SYS_NICE.
	SKIP_TEST_IF(sched_setscheduler(0, SCHED_FIFO, &param) < 0);
	param.sched_priority = 0;
	TEST_SUCC(sched_setscheduler(0, SCHED_OTHER, &param));

	atomic_store(&word, tid);
	// The normal waiter queues first, so FIFO order alone would pick it.
	TEST_SUCC(
		pthread_create(&normal.thread, NULL, lock_and_unlock, &normal));
	wait_for_waiters(&word);
	TEST_SUCC(pthread_create(&realtime.thread, NULL, lock_and_unlock,
				 &realtime));
	while (atomic_load(&realtime.tid) == 0)
		sleep_ms(1);
	sleep_ms(20);

	TEST_SUCC(futex_pi(&word, FUTEX_UNLOCK_PI, NULL));
	TEST_SUCC(pthread_join(realtime.thread, NULL));
	TEST_SUCC(pthread_join(normal.thread, NULL));
	TEST_RES(realtime.error | normal.error, _ret == 0);
	// The real-time waiter queued last but was handed the lock first.
	TEST_RES(atomic_load(&realtime.word_after_lock),
		 _ret == ((uint32_t)atomic_load(&realtime.tid) |
			  FUTEX_WAITERS));
	TEST_RES(atomic_load(&normal.word_after_lock),
		 _ret == ((uint32_t)atomic_load(&normal.tid) | FUTEX_WAITERS));
}
END_TEST()

// ============================================================================
// glibc PTHREAD_PRIO_INHERIT mutexes
// ============================================================================

#define NTHREADS 4
#define ITERATIONS 20000

static pthread_mutex_t pi_mutex;
static unsigned long counter;

static void *increment(void *arg)
{
	(void)arg;
	for (int i = 0; i < ITERATIONS; i++) {
		pthread_mutex_lock(&pi_mutex);
		counter++;
		pthread_mutex_unlock(&pi_mutex);
	}
	return NULL;
}

FN_SETUP(pi_mutex)
{
	pthread_mutexattr_t attr;

	CHECK(pthread_mutexattr_init(&attr));
	CHECK(pthread_mutexattr_setprotocol(&attr, PTHREAD_PRIO_INHERIT));
	CHECK(pthread_mutex_init(&pi_mutex, &attr));
	CHECK(pthread_mutexattr_destroy(&attr));
}
END_SETUP()

FN_TEST(mutex_counter)
{
	pthread_t threads[NTHREADS];

	for (int i = 0; i < NTHREADS; i++)
		TEST_SUCC(pthread_create(&threads[i], NULL, increment, NULL));
	for (int i = 0; i < NTHREADS; i++)
		TEST_SUCC(pthread_join(threads[i], NULL));
	TEST_RES(counter, _ret == NTHREADS * ITERATIONS);
}
END_TEST()

static _Atomic int handled_signals;

static void count_signal(int sig)
{
	(void)sig;
	atomic_fetch_add(&handled_signals, 1);
}

static void *lock_mutex(void *arg)
{
	int *result = arg;

	*result = pthread_mutex_lock(&pi_mutex);
	if (*result == 0)
		pthread_mutex_unlock(&pi_mutex);
	return NULL;
}

FN_TEST(signal_does_not_break_lock)
{
	// No SA_RESTART: a blocked FUTEX_LOCK_PI must still restart rather
	// than return EINTR, which glibc would mistake for success.
	struct sigaction action = { .sa_handler = count_signal };
	pthread_t thread;
	int result = -1;

	TEST_SUCC(sigaction(SIGUSR1, &action, NULL));
	TEST_RES(pthread_mutex_lock(&pi_mutex), _ret == 0);
	TEST_SUCC(pthread_create(&thread, NULL, lock_mutex, &result));
	wait_for_waiters((_Atomic uint32_t *)&pi_mutex.__data.__lock);

	TEST_RES(pthread_kill(thread, SIGUSR1), _ret == 0);
	while (atomic_load(&handled_signals) == 0)
		sleep_ms(1);
	sleep_ms(20);
	// The waiter must still be blocked, not holding the lock.
	TEST_RES(FUTEX_TID(pi_mutex.__data.__lock), _ret == gettid_());

	TEST_RES(pthread_mutex_unlock(&pi_mutex), _ret == 0);
	TEST_SUCC(pthread_join(thread, NULL));
	TEST_RES(result, _ret == 0);
}
END_TEST()

static pthread_mutex_t robust_mutex;
static _Atomic int robust_locked;

static void *lock_robust_and_exit(void *arg)
{
	(void)arg;
	pthread_mutex_lock(&robust_mutex);
	atomic_store(&robust_locked, 1);
	sleep_ms(50);
	// Exit while still holding the mutex.
	return NULL;
}

FN_TEST(robust_owner_died)
{
	pthread_mutexattr_t attr;
	pthread_t thread;

	TEST_RES(pthread_mutexattr_init(&attr), _ret == 0);
	TEST_RES(pthread_mutexattr_setprotocol(&attr, PTHREAD_PRIO_INHERIT),
		 _ret == 0);
	TEST_RES(pthread_mutexattr_setrobust(&attr, PTHREAD_MUTEX_ROBUST),
		 _ret == 0);
	TEST_RES(pthread_mutex_init(&robust_mutex, &attr), _ret == 0);

	TEST_SUCC(pthread_create(&thread, NULL, lock_robust_and_exit, NULL));
	while (!atomic_load(&robust_locked))
		sleep_ms(1);
	// Block on the mutex before the owner exits, so the owner's death must
	// wake this waiter.
	TEST_RES(pthread_mutex_lock(&robust_mutex), _ret == EOWNERDEAD);
	TEST_RES(pthread_mutex_consistent(&robust_mutex), _ret == 0);
	TEST_RES(pthread_mutex_unlock(&robust_mutex), _ret == 0);
	TEST_SUCC(pthread_join(thread, NULL));
}
END_TEST()

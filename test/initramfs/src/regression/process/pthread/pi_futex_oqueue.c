// SPDX-License-Identifier: MPL-2.0

// Checks the events that Asterinas reports through the `futex.pi.events`
// OQueue when threads contend on a PI futex.

#define _GNU_SOURCE

#include <fcntl.h>
#include <linux/futex.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include "../../common/test.h"

#define EVENTS_PATH "/oqueues/futex/pi/events/strong_observe"

static pid_t gettid_(void)
{
	return syscall(SYS_gettid);
}

static long futex_pi(_Atomic uint32_t *word, int op)
{
	return syscall(SYS_futex, word, op, 0, NULL, NULL, 0);
}

static void sleep_ms(long ms)
{
	struct timespec ts = { .tv_sec = ms / 1000,
			       .tv_nsec = (ms % 1000) * 1000000 };
	nanosleep(&ts, NULL);
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

static uint8_t events[64 * 1024];
static size_t events_len;

// Returns whether the events contain the map entry `key: value`.
static int has_entry(const char *key, uint32_t value)
{
	uint8_t needle[64];
	size_t len = encode_entry(needle, key, value);

	return memmem(events, events_len, needle, len) != NULL;
}

static int has_text(const char *text)
{
	return memmem(events, events_len, text, strlen(text)) != NULL;
}

static _Atomic uint32_t word;
static _Atomic pid_t waiter_tid;

static void *lock_and_unlock(void *arg)
{
	(void)arg;
	atomic_store(&waiter_tid, gettid_());
	if (futex_pi(&word, FUTEX_LOCK_PI) < 0)
		return (void *)(long)errno;
	if (futex_pi(&word, FUTEX_UNLOCK_PI) < 0)
		return (void *)(long)errno;
	return NULL;
}

FN_TEST(handoff_events)
{
	pid_t tid = gettid_();
	pthread_t thread;
	void *ret;
	ssize_t n;
	int fd;

	fd = open(EVENTS_PATH, O_RDONLY | O_NONBLOCK);
	SKIP_TEST_IF(fd < 0);

	atomic_store(&word, tid);
	TEST_SUCC(pthread_create(&thread, NULL, lock_and_unlock, NULL));
	while (!(atomic_load(&word) & FUTEX_WAITERS))
		sleep_ms(1);
	TEST_SUCC(futex_pi(&word, FUTEX_UNLOCK_PI));
	TEST_RES(pthread_join(thread, &ret), ret == NULL);

	// Only events produced after `open` are observed, so all of these come
	// from this test.
	while ((n = read(fd, events + events_len,
			 sizeof(events) - events_len)) > 0)
		events_len += n;
	int read_errno = errno;
	TEST_RES(read_errno, _ret == EAGAIN && events_len > 0);
	TEST_SUCC(close(fd));

	pid_t waiter = atomic_load(&waiter_tid);
	if ((uintptr_t)&word <= UINT32_MAX)
		TEST_RES(has_entry("futex_addr", (uintptr_t)&word), _ret);
	// The waiter blocked behind this thread, ...
	TEST_RES(has_text("Block"), _ret);
	TEST_RES(has_entry("waiter", waiter), _ret);
	TEST_RES(has_entry("owner", tid), _ret);
	// ... this thread handed the lock to it, ...
	TEST_RES(has_text("Handoff"), _ret);
	TEST_RES(has_entry("from", tid), _ret);
	TEST_RES(has_entry("to", waiter), _ret);
	TEST_RES(has_entry("waiters_left", 0), _ret);
	// ... and so its wait ended with the lock.
	TEST_RES(has_text("WaitEnd"), _ret);
	TEST_RES(has_text("Acquired"), _ret);
}
END_TEST()

// SPDX-License-Identifier: MPL-2.0

// Checks the events that Asterinas reports through the `scheduler.events`
// OQueue over the lifetime of a thread.

#define _GNU_SOURCE

#include <fcntl.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include "../../common/test.h"

#define EVENTS_PATH "/oqueues/scheduler/events/strong_observe"

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

static uint8_t events[256 * 1024];
static size_t events_len;

// Returns whether some event about the thread `tid` contains `text`.
//
// An event is encoded as `{task: {id, tid}, cpu, kind}`, so the kind follows
// the task's TID within a few bytes.
static int has_event(pid_t tid, const char *text)
{
	uint8_t needle[16];
	size_t needle_len = encode_entry(needle, "tid", tid);
	size_t text_len = strlen(text);
	uint8_t *pos = events, *end = events + events_len;

	while ((pos = memmem(pos, end - pos, needle, needle_len)) != NULL) {
		pos += needle_len;
		size_t window = end - pos < 48 ? end - pos : 48;
		if (memmem(pos, window, text, text_len) != NULL)
			return 1;
	}
	return 0;
}

static int pipe_fds[2];
static _Atomic pid_t child_tid;

static void *block_on_pipe(void *arg)
{
	char c;

	(void)arg;
	atomic_store(&child_tid, syscall(SYS_gettid));
	if (read(pipe_fds[0], &c, 1) != 1)
		return (void *)(long)errno;
	return NULL;
}

FN_TEST(thread_lifecycle_events)
{
	pthread_t thread;
	void *ret;
	ssize_t n;
	int fd;

	fd = open(EVENTS_PATH, O_RDONLY | O_NONBLOCK);
	SKIP_TEST_IF(fd < 0);

	TEST_SUCC(pipe(pipe_fds));
	TEST_SUCC(pthread_create(&thread, NULL, block_on_pipe, NULL));
	// Give the thread time to block on the empty pipe.
	sleep_ms(50);
	TEST_RES(write(pipe_fds[1], "x", 1), _ret == 1);
	TEST_RES(pthread_join(thread, &ret), ret == NULL);

	// Only events produced after `open` are observed, so the child's events
	// all come from this test.
	while ((n = read(fd, events + events_len,
			 sizeof(events) - events_len)) > 0)
		events_len += n;
	int read_errno = errno;
	TEST_RES(read_errno, _ret == EAGAIN && events_len > 0);
	TEST_SUCC(close(fd));
	TEST_SUCC(close(pipe_fds[0]));
	TEST_SUCC(close(pipe_fds[1]));

	pid_t tid = atomic_load(&child_tid);
	// The thread was spawned, ...
	TEST_RES(has_event(tid, "Spawn"), _ret);
	TEST_RES(has_event(tid, "Schedule"), _ret);
	// ... blocked on the pipe, ...
	TEST_RES(has_event(tid, "Blocked"), _ret);
	// ... was woken by the write, ...
	TEST_RES(has_event(tid, "Wakeup"), _ret);
	// ... and exited.
	TEST_RES(has_event(tid, "Exited"), _ret);
}
END_TEST()

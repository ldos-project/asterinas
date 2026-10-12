// SPDX-License-Identifier: MPL-2.0

// Checks the samples that Asterinas reports through the `pmu.task_counters`
// OQueue. The OQueue only exists if the kernel is booted with the
// `pmu.task_counters` parameter on a CPU whose PMU supports it, so the test is
// skipped otherwise.

#define _GNU_SOURCE

#include <fcntl.h>
#include <stdint.h>
#include <string.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include "../../common/test.h"

#define SAMPLES_PATH "/oqueues/pmu/task_counters/strong_observe"

// The number of loop iterations to run before blocking. Each iteration retires
// at least one instruction.
#define SPIN_ITERATIONS 10000000UL

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

// Decodes the CBOR unsigned integer at `pos`, or returns 0 if there is none.
static uint64_t decode_uint(const uint8_t *pos, const uint8_t *end)
{
	static const int extra_bytes[] = { 1, 2, 4, 8 };
	uint64_t value = 0;

	if (pos >= end || (*pos >> 5) != 0)
		return 0;
	if ((*pos & 0x1f) < 24)
		return *pos & 0x1f;
	if ((*pos & 0x1f) > 27)
		return 0;
	int n = extra_bytes[(*pos & 0x1f) - 24];
	if (end - pos < 1 + n)
		return 0;
	for (int i = 1; i <= n; i++)
		value = (value << 8) | pos[i];
	return value;
}

static uint8_t samples[256 * 1024];
static size_t samples_len;

// Returns the largest value of the field `key` over the samples for the thread
// `tid`.
//
// A sample is encoded as `{task, tid, cpu, tsc_ticks, cycles, instructions,
// ...}`, so the other fields follow the thread's TID within a few dozen bytes.
static uint64_t max_field(pid_t tid, const char *key)
{
	uint8_t needle[16];
	size_t needle_len = encode_entry(needle, "tid", tid);
	uint8_t key_enc[32];
	size_t key_len = strlen(key);
	uint8_t *pos = samples, *end = samples + samples_len;
	uint64_t max = 0;

	key_enc[0] = 0x60 | key_len;
	memcpy(key_enc + 1, key, key_len);

	while ((pos = memmem(pos, end - pos, needle, needle_len)) != NULL) {
		pos += needle_len;
		size_t window = end - pos < 96 ? end - pos : 96;
		uint8_t *field = memmem(pos, window, key_enc, key_len + 1);
		if (field == NULL)
			continue;
		uint64_t value = decode_uint(field + key_len + 1, end);
		if (value > max)
			max = value;
	}
	return max;
}

FN_TEST(spinning_thread_is_counted)
{
	pid_t tid = syscall(SYS_gettid);
	struct timespec ms = { .tv_sec = 0, .tv_nsec = 1000000 };
	ssize_t n;
	int fd;

	fd = open(SAMPLES_PATH, O_RDONLY | O_NONBLOCK);
	SKIP_TEST_IF(fd < 0);

	// Spin, then block so that the counted instructions are reported.
	for (volatile unsigned long i = 0; i < SPIN_ITERATIONS; i++)
		;
	TEST_SUCC(nanosleep(&ms, NULL));

	while ((n = read(fd, samples + samples_len,
			 sizeof(samples) - samples_len)) > 0)
		samples_len += n;
	int read_errno = errno;
	TEST_RES(read_errno, _ret == EAGAIN && samples_len > 0);
	TEST_SUCC(close(fd));

	// The spin may be split across several samples if this thread was
	// preempted, but one of them should be large.
	TEST_RES(max_field(tid, "instructions"), _ret >= SPIN_ITERATIONS / 4);
	TEST_RES(max_field(tid, "cycles"), _ret > 0);
	TEST_RES(max_field(tid, "tsc_ticks"), _ret > 0);
}
END_TEST()

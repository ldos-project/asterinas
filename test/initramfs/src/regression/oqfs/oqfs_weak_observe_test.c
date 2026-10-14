// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "oqfs_test_common.h"

#define WEAK_OBSERVE_PATH "/oqueues/oqfs_test/observe/weak_observe"
#define TRIGGER_PRODUCE_PATH "/oqueues/oqfs_test/trigger/produce"

static int write_all(int fd, const void *buf, size_t len)
{
	size_t off = 0;
	while (off < len) {
		ssize_t n = write(fd, (const char *)buf + off, len - off);
		if (n < 0)
			return -1;
		off += (size_t)n;
		if (n == 0)
			break;
	}
	return off == len ? 0 : -1;
}

int main(void)
{
	unsigned char buf[BATCH];
	/* CBOR encoding of the trigger value `1`: a single byte `0x01`. */
	unsigned char trigger = 1;
	int wfd, tfd, n, attempt;

	/*
	 * Open the weak observer BEFORE triggering so that the whole batch is retained in its
	 * newest-first snapshot.
	 */
	wfd = open(WEAK_OBSERVE_PATH, O_RDONLY);
	if (wfd < 0) {
		fprintf(stderr, "open %s: %s\n", WEAK_OBSERVE_PATH,
			strerror(errno));
		return -1;
	}

	tfd = open(TRIGGER_PRODUCE_PATH, O_WRONLY);
	if (tfd < 0) {
		fprintf(stderr, "open %s: %s\n", TRIGGER_PRODUCE_PATH,
			strerror(errno));
		close(wfd);
		return -1;
	}
	if (write_all(tfd, &trigger, 1) < 0) {
		fprintf(stderr, "write %s: %s\n", TRIGGER_PRODUCE_PATH,
			strerror(errno));
		close(tfd);
		close(wfd);
		return -1;
	}
	close(tfd);

	/*
	 * The weak-observation file is offset-0-only and non-consuming: a plain read() would advance
	 * the file offset past zero, making the next positional read fail with EINVAL. Use a
	 * positional pread() at offset 0 in a retry loop until the full batch (BATCH values) is
	 * retained, since the kernel producer may still be producing.
	 */
	n = 0;
	for (attempt = 0; attempt < 100000 && n != BATCH; attempt++)
		n = (int)pread(wfd, buf, BATCH, 0);
	close(wfd);

	if (n < 0) {
		fprintf(stderr, "pread %s: %s\n", WEAK_OBSERVE_PATH,
			strerror(errno));
		return -1;
	}
	if (n != BATCH) {
		fprintf(stderr,
			"oqfs weak_observe: timed out waiting for %d values (got %d)\n",
			BATCH, n);
		return -1;
	}

	if (check_newest_first(buf, BATCH) < 0) {
		fprintf(stderr,
			"oqfs weak_observe: snapshot is not newest-first (BATCH-1..0)\n");
		return -1;
	}

	printf("oqfs weak_observe test passed\n");
	return 0;
}

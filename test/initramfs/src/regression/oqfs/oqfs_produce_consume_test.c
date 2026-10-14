// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "oqfs_test_common.h"

#define PRODUCE_PATH "/oqueues/oqfs_test/io/produce"
#define CONSUME_PATH "/oqueues/oqfs_test/io/consume"

/* Writes `len` bytes from `buf` to `fd`, retrying on short writes. Returns 0 on success. */
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

/* Reads exactly `len` bytes into `buf` from `fd`, blocking for the rest as needed. */
static int read_all(int fd, unsigned char *buf, size_t len)
{
	size_t off = 0;
	while (off < len) {
		ssize_t n = read(fd, buf + off, len - off);
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
	int pfd, cfd;

	encode_batch_in_order(buf);

	/* Attach the consumer before producing; values have nowhere to land otherwise. */
	cfd = open(CONSUME_PATH, O_RDONLY);
	if (cfd < 0) {
		fprintf(stderr, "open %s: %s\n", CONSUME_PATH, strerror(errno));
		return -1;
	}

	/* Produce the batch into the OQueue. */
	pfd = open(PRODUCE_PATH, O_WRONLY);
	if (pfd < 0) {
		fprintf(stderr, "open %s: %s\n", PRODUCE_PATH, strerror(errno));
		close(cfd);
		return -1;
	}
	if (write_all(pfd, buf, BATCH) < 0) {
		fprintf(stderr, "write %s: %s\n", PRODUCE_PATH,
			strerror(errno));
		close(pfd);
		close(cfd);
		return -1;
	}
	close(pfd);

	/* Consume the same batch back out of the OQueue. */
	if (read_all(cfd, buf, BATCH) < 0) {
		fprintf(stderr, "read %s: %s\n", CONSUME_PATH, strerror(errno));
		close(cfd);
		return -1;
	}
	close(cfd);

	if (check_in_order(buf, BATCH) < 0) {
		fprintf(stderr,
			"oqfs produce/consume: consumed values are not 0..BATCH in order\n");
		return -1;
	}

	printf("oqfs produce/consume test passed\n");
	return 0;
}

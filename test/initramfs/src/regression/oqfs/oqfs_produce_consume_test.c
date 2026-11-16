// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "oqfs_test_common.h"

#define PRODUCE_PATH "/oqueues/oqfs_test/userspace_produce_consume/produce"
#define CONSUME_PATH "/oqueues/oqfs_test/userspace_produce_consume/consume"

int main(void)
{
	unsigned char buf[BATCH];
	int pfd, cfd;

	encode_batch_in_order(buf);

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

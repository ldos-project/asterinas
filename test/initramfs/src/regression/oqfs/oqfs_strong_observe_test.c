// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "oqfs_test_common.h"

#define STRONG_OBSERVE_PATH "/oqueues/oqfs_test/observe/strong_observe"
#define TRIGGER_PRODUCE_PATH "/oqueues/oqfs_test/trigger/produce"

int main(void)
{
	unsigned char buf[BATCH];
	/* CBOR encoding of the trigger value `1`: a single byte `0x01`. */
	unsigned char trigger = 1;
	int ofd, tfd;

	ofd = open(STRONG_OBSERVE_PATH, O_RDONLY);
	if (ofd < 0) {
		fprintf(stderr, "open %s: %s\n", STRONG_OBSERVE_PATH,
			strerror(errno));
		return -1;
	}

	tfd = open(TRIGGER_PRODUCE_PATH, O_WRONLY);
	if (tfd < 0) {
		fprintf(stderr, "open %s: %s\n", TRIGGER_PRODUCE_PATH,
			strerror(errno));
		close(ofd);
		return -1;
	}
	/* Trigger the kernel thread to produce values. */
	if (write_all(tfd, &trigger, 1) < 0) {
		fprintf(stderr, "write %s: %s\n", TRIGGER_PRODUCE_PATH,
			strerror(errno));
		close(tfd);
		close(ofd);
		return -1;
	}
	close(tfd);

	/* Read all the values. */
	if (read_all(ofd, buf, BATCH) < 0) {
		fprintf(stderr, "read %s: %s\n", STRONG_OBSERVE_PATH,
			strerror(errno));
		close(ofd);
		return -1;
	}
	close(ofd);

	if (check_in_order(buf, BATCH) < 0) {
		fprintf(stderr,
			"oqfs strong_observe: observed values are not 0..BATCH in order\n");
		return -1;
	}

	printf("oqfs strong_observe test passed\n");
	return 0;
}

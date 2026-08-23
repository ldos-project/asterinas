// SPDX-License-Identifier: MPL-2.0

/*
 * Userspace side of the mariposa_data_capture regression smoke test.
 *
 * The kernel writes a tiny capture file (path regression.smoke) with the
 * known u32 samples 42, 100, 200 when data_capture.regression_smoke=1.
 * This program opens the capture block device, finds the Mariposa magic,
 * and checks that those CBOR-encoded samples are present.
 *
 * Device naming: QEMU attaches the capture image last among the standard
 * virtio-blk disks (see tools/qemu_args.sh), so it appears as /dev/vdg and
 * is selected via data_capture.device=vdg in OSDK.toml.
 */

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "../common/test.h"

#define CAPTURE_DEV "/dev/vdg"
#define BLOCK_SIZE 4096
/* Directory is one block; smoke file starts at the next block. */
#define SMOKE_FILE_OFFSET BLOCK_SIZE
#define READ_LEN (BLOCK_SIZE * 2)

static const unsigned char MAGIC[] = "MARIPOSALDOSDATA"; /* plus trailing NUL in file */
/* CBOR major type 0, additional 24, then one-byte value: 42, 100, 200 */
static const unsigned char EXPECTED_SAMPLES[] = { 0x18, 0x2a, 0x18, 0x64, 0x18, 0xc8 };

static int memmem_idx(const unsigned char *hay, size_t hay_len,
		      const unsigned char *needle, size_t needle_len)
{
	size_t i;

	if (needle_len > hay_len)
		return -1;
	for (i = 0; i + needle_len <= hay_len; i++) {
		if (memcmp(hay + i, needle, needle_len) == 0)
			return (int)i;
	}
	return -1;
}

FN_TEST(capture_device_has_magic_and_samples)
{
	int fd;
	unsigned char *buf;
	ssize_t n;
	int magic_at;
	int samples_at;

	buf = NULL;
	if (posix_memalign((void **)&buf, BLOCK_SIZE, READ_LEN) != 0) {
		fprintf(stderr, "posix_memalign failed\\n");
		exit(EXIT_FAILURE);
	}

	fd = open(CAPTURE_DEV, O_RDONLY);
	if (fd < 0) {
		if (errno == ENOENT || errno == ENODEV || errno == ENXIO) {
			fprintf(stderr,
				"mariposa_data_capture tests skipped: open('%s'): %s\\n",
				CAPTURE_DEV, strerror(errno));
			free(buf);
			exit(EXIT_SUCCESS);
		}
		TEST_SUCC(fd);
		free(buf);
		return;
	}

	/* Read the directory block plus the first capture-file block. */
	n = pread(fd, buf, READ_LEN, 0);
	TEST_RES(n, n == (ssize_t)READ_LEN);
	close(fd);

	magic_at = memmem_idx(buf, (size_t)n, MAGIC, sizeof(MAGIC) - 1);
	if (magic_at < 0) {
		/*
		 * Smoke capture not present (kernel not built with the kcmd
		 * flag). Skip rather than fail so the suite stays usable
		 * outside AUTO_TEST=regression.
		 */
		fprintf(stderr,
			"mariposa_data_capture tests skipped: no MARIPOSALDOSDATA magic on %s\\n",
			CAPTURE_DEV);
		free(buf);
		exit(EXIT_SUCCESS);
	}

	/* Magic should land in the file region (after the directory block). */
	TEST_RES(magic_at, magic_at >= (int)SMOKE_FILE_OFFSET);

	samples_at = memmem_idx(buf, (size_t)n, EXPECTED_SAMPLES,
				sizeof(EXPECTED_SAMPLES));
	TEST_RES(samples_at, samples_at > magic_at);

	printf("mariposa_data_capture: found magic at %d and samples at %d\\n",
	       magic_at, samples_at);
	free(buf);
}
END_TEST()

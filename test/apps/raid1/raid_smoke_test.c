// SPDX-License-Identifier: MPL-2.0

#define _GNU_SOURCE

#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <unistd.h>
#include <string.h>
#include <errno.h>
#include <sys/stat.h>

// Define the file size for the test (e.g., one block = 4096 bytes)
#define RAID1_TEST_BLOCK_SIZE 8192
#define RAID1_TEST_FILENAME "/raid1/raid_smoke_file"

// Returns 0 on success, -1 on error
int main()
{
	int fd;
	ssize_t n;
	unsigned char *pattern, *read_back;
	posix_memalign((void**)&pattern, 4096, RAID1_TEST_BLOCK_SIZE);
	posix_memalign((void**)&read_back, 4096, RAID1_TEST_BLOCK_SIZE);
	int i, ret = 0;

	// Fill pattern buffer with known values
	for (i = 0; i < RAID1_TEST_BLOCK_SIZE; i++) {
		pattern[i] = (unsigned char)i;
	}

	// Create the file and write the pattern to it
	fd = open(RAID1_TEST_FILENAME, O_DIRECT | O_CREAT | O_WRONLY | O_TRUNC,
		  0666);
	if (fd < 0) {
		fprintf(stderr, "[raid-test] Failed to create file '%s': %s\n",
			RAID1_TEST_FILENAME, strerror(errno));
		return -1;
	}

	n = write(fd, pattern, RAID1_TEST_BLOCK_SIZE);
	if (n != RAID1_TEST_BLOCK_SIZE) {
		fprintf(stderr, "[raid-test] Failed to write test data: %s\n",
			(n < 0) ? strerror(errno) : "Incomplete write");
		close(fd);
		return -1;
	}
	fsync(fd);
	// After write and fsync, before close:
	struct stat st;
	fstat(fd, &st);
	printf("file size after O_DIRECT write: %ld\n", (long)st.st_size);
	close(fd);

	// Reopen the file for reading
	fd = open(RAID1_TEST_FILENAME, O_DIRECT | O_RDONLY);
	if (fd < 0) {
		fprintf(stderr,
			"[raid-test] Failed to open file for reading: %s\n",
			strerror(errno));
		return -1;
	}

	memset(read_back, 0, RAID1_TEST_BLOCK_SIZE);
	n = read(fd, read_back, RAID1_TEST_BLOCK_SIZE);
	if (n != RAID1_TEST_BLOCK_SIZE) {
		fprintf(stderr, "[raid-test] Failed to read test data: %s\n",
			(n < 0) ? strerror(errno) : "Incomplete read");
		close(fd);
		return -1;
	}
	close(fd);

	// Print the read content
	printf("first 8 bytes read: %02x %02x %02x %02x %02x %02x %02x %02x\n",
    read_back[0], read_back[1], read_back[2], read_back[3],
    read_back[4], read_back[5], read_back[6], read_back[7]);
	printf("expected:           00 01 02 03 04 05 06 07\n");

	// Compare written and read data
	if (memcmp(pattern, read_back, RAID1_TEST_BLOCK_SIZE) == 0) {
		printf("[raid-test] read/write verification succeeded\n");
		ret = 0;
	} else {
		fprintf(stderr,
			"[raid-test] data mismatch detected during RAID-1 smoke test\n");
		ret = -1;
	}

	// dump all the data
	printf("full read data:\n");
	for (i = 0; i < RAID1_TEST_BLOCK_SIZE; i++) {
		printf("%02x ", read_back[i]);
		if ((i + 1) % 16 == 0)
			printf("\n");
	}

	// Optionally, remove test file
	// unlink(RAID1_TEST_FILENAME);

	return ret;
}
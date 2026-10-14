// SPDX-License-Identifier: MPL-2.0
#ifndef OQFS_TEST_COMMON_H
#define OQFS_TEST_COMMON_H

/* Number of values in one batch produced by the kernel test OQueue. */
#define BATCH 16

/*
 * The test OQueues carry `u32` values `0..BATCH`. For unsigned integers `0..23` the CBOR encoding
 * is a single byte equal to the value, so the on-wire form of value `i` is just the byte `i`.
 */

/* Fills `buf` with the CBOR encoding of the values `0, 1, ..., BATCH-1` in order. */
static inline void encode_batch_in_order(unsigned char *buf)
{
	for (int i = 0; i < BATCH; i++)
		buf[i] = (unsigned char)i;
}

/* Returns `0` if `buf` (of `len` bytes) is the in-order encoding `0,1,...,BATCH-1`, else `-1`. */
static inline int check_in_order(const unsigned char *buf, int len)
{
	if (len != BATCH)
		return -1;
	for (int i = 0; i < BATCH; i++)
		if (buf[i] != (unsigned char)i)
			return -1;
	return 0;
}

/* Returns `0` if `buf` (of `len` bytes) is the newest-first encoding
 * `BATCH-1,...,1,0`, else `-1`. */
static inline int check_newest_first(const unsigned char *buf, int len)
{
	if (len != BATCH)
		return -1;
	for (int i = 0; i < BATCH; i++)
		if (buf[i] != (unsigned char)(BATCH - 1 - i))
			return -1;
	return 0;
}

#endif

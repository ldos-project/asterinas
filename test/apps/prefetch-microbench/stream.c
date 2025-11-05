#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <pthread.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static int verbose = 0;

typedef struct {
	const char *filename;
	size_t thread_id;
	size_t offset;
	size_t block_size;
	size_t stride;
	size_t count;
	size_t sleep_time;
} ThreadArgs;

void *reader_thread(void *arg)
{
	ThreadArgs *args = (ThreadArgs *)arg;
	FILE *file = fopen(args->filename, "rb");
	if (!file) {
		fprintf(stderr, "Failed to open file %s\n", args->filename);
		return NULL;
	}
	printf("Thread %zu starting with filename %s, offset %zu (pages: %zu), block size %zu (pages: %zu), stride %zu (pages: %zu), count %zu, sleep time %zu\n",
	       args->thread_id, args->filename, args->offset,
	       args->offset / 4096, args->block_size, args->block_size / 4096,
	       args->stride, args->stride / 4096, args->count,
	       args->sleep_time);

	// Jump to offset in file.
	fseek(file, args->offset, SEEK_CUR);

	char buffer[args->block_size];
	size_t bytes_read;

	// The number of blocks that have been read.
	int i = 0;

	sched_yield();

	// Read until either we reach the end of file or we have read args->count blocks
	while ((bytes_read = fread(buffer, 1, args->block_size, file)) > 0 &&
	       (++i) < args->count) {
		if (verbose) {
			long int read_start_pos = ftell(file) - bytes_read;
			printf("Thread reading %zu from %s: Bytes read: %zu (pages: %zu), Read position: %ld (page: %ld)\n",
			       args->thread_id, args->filename, bytes_read,
			       bytes_read / 4096, read_start_pos,
			       read_start_pos / 4096);
		}

		// Move forward to the next stride
		fseek(file, args->stride - bytes_read, SEEK_CUR);

		if (args->sleep_time > 0) {
			struct timespec time = { args->sleep_time, 0 };
			nanosleep(&time, NULL);
		}
	}

	if (ferror(file)) {
		perror("Error reading file");
	}

	fclose(file);
	return NULL;
}

void usage(char *argv[])
{
	fprintf(stderr,
		"Usage: %s [-v] <file> <offset1>,<block_size1>,<stride1>,<count1>,<sleep_time1> [<offset2>,<block_size2>,<stride2>,<count2>,<sleep_time2> ...]\n"
		"\n"
		"sleep_time is in ns\n",
		argv[0]);
}

int main(int argc, char *argv[])
{
	int next_arg_i = 1;

	if (strcmp(argv[next_arg_i], "-v") == 0) {
		verbose = 1;
		next_arg_i++;
	}

	const char *filename = argv[next_arg_i];
	next_arg_i++;

	int num_threads = argc - next_arg_i;

	if (num_threads < 1) {
		usage(argv);
		return 1;
	}

	pthread_t thread_ids[num_threads];

	for (int i = 0; i < num_threads; i++) {
		char *config = argv[next_arg_i];
		next_arg_i++;

		char *offset_str, *block_size_str, *stride_str, *count_str,
			*sleep_time_str;

		offset_str = strtok(config, ",");
		block_size_str = strtok(NULL, ",");
		stride_str = strtok(NULL, ",");
		count_str = strtok(NULL, ",");
		sleep_time_str = strtok(NULL, ",");

		if (!offset_str || !block_size_str || !stride_str ||
		    !count_str || !sleep_time_str) {
			fprintf(stderr, "Invalid configuration for thread %d\n",
				i);
			return 1;
		}

		ThreadArgs *args = (ThreadArgs *)malloc(sizeof(ThreadArgs));
		if (!args) {
			fprintf(stderr,
				"Failed to allocate memory for ThreadArgs\n");
			return 1;
		}
		args->filename = filename;
		args->thread_id = i;
		args->offset = atoi(offset_str);
		args->block_size = atoi(block_size_str);
		args->stride = atoi(stride_str);
		args->count = atoi(count_str);
		args->sleep_time = atoi(sleep_time_str);

		if (pthread_create(&thread_ids[i], NULL, reader_thread, args) !=
		    0) {
			fprintf(stderr, "Failed to create thread %d\n", i);
			free(args);
			return 1;
		}
	}

	for (int j = 0; j < num_threads; j++) {
		pthread_join(thread_ids[j], NULL);
	}

	return 0;
}

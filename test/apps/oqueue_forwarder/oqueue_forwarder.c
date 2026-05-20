// SPDX-License-Identifier: MPL-2.0

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

static int send_all(int sock, const char *buf, ssize_t len)
{
	ssize_t off = 0;

	while (off < len) {
		ssize_t sent = send(sock, buf + off, len - off, 0);
		if (sent < 0) {
			perror("send");
			return -1;
		}
		if (sent == 0) {
			fprintf(stderr, "send returned 0\n");
			return -1;
		}
		off += sent;
	}

	return 0;
}

int main(int argc, char **argv)
{
	if (argc != 4) {
		fprintf(stderr, "usage: %s <proc_path> <host> <port>\n", argv[0]);
		return 2;
	}

	const char *proc_path = argv[1];
	const char *host = argv[2];
	int port = atoi(argv[3]);

	int fd = open(proc_path, O_RDONLY);
	if (fd < 0) {
		perror("open");
		return 1;
	}

	int sock = socket(AF_INET, SOCK_STREAM, 0);
	if (sock < 0) {
		perror("socket");
		close(fd);
		return 1;
	}

	struct sockaddr_in sa;
	memset(&sa, 0, sizeof(sa));
	sa.sin_family = AF_INET;
	sa.sin_port = htons(port);
	if (inet_pton(AF_INET, host, &sa.sin_addr) != 1) {
		fprintf(stderr, "bad host: %s\n", host);
		close(sock);
		close(fd);
		return 1;
	}

	if (connect(sock, (struct sockaddr *)&sa, sizeof(sa)) < 0) {
		perror("connect");
		close(sock);
		close(fd);
		return 1;
	}

	char buf[4096];
	for (;;) {
		ssize_t nread = read(fd, buf, sizeof(buf));
		if (nread < 0) {
			if (errno == EINTR) {
				continue;
			}
			perror("read");
			close(sock);
			close(fd);
			return 1;
		}
		if (nread == 0) {
			break;
		}
		if (send_all(sock, buf, nread) < 0) {
			close(sock);
			close(fd);
			return 1;
		}
	}

	close(sock);
	close(fd);
	return 0;
}

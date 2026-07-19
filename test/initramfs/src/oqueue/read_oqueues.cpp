// SPDX-License-Identifier: MPL-2.0
//
// Decode and display an OQueue filesystem (`/oqueues`) CBOR stream.
//
// Each observation file (e.g. `.../strong_observe`) emits a self-delimiting CBOR byte stream of
// one record per observed value. Records are concatenated with no framing, so they decode
// back-to-back regardless of read boundaries. Reading a live file blocks for new records (like
// `tail -f`) and ends (a zero-length read) when this reader is dropped for falling too far behind
// the queue. Descriptive information about the queue is in the sibling `metadata.yaml`, not in
// this stream.
//
// Decoding is delegated to libcbor. The kernel emits only definite-length unsigned/negative
// integers, byte/text strings, arrays, maps, and the `null`/`true`/`false` simple values; each
// record is printed in a JSON-like form.
//
// This tool is written against the C standard library and libcbor only (no C++ stdlib), so it
// links statically into the initramfs like the other in-guest tools.
//
// Usage:
//   read_oqueues PATH      e.g. /oqueues/scheduler/events/strong_observe

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <fcntl.h>
#include <unistd.h>

#include <cbor.h>

namespace {

// Prints one decoded CBOR item in a JSON-like form. Children are borrowed from `item`, so only the
// top-level item the caller loaded needs to be freed.
void print_item(cbor_item_t* item) {
    switch (cbor_typeof(item)) {
        case CBOR_TYPE_UINT:
            printf("%llu", static_cast<unsigned long long>(cbor_get_int(item)));
            break;
        case CBOR_TYPE_NEGINT:
            // CBOR encodes a negative integer as the unsigned value `-1 - n`.
            printf("-%llu", static_cast<unsigned long long>(cbor_get_int(item)) + 1);
            break;
        case CBOR_TYPE_BYTESTRING: {  // printed as hex
            unsigned char* data = cbor_bytestring_handle(item);
            size_t len = cbor_bytestring_length(item);
            for (size_t i = 0; i < len; i++) {
                printf("%02x", data[i]);
            }
            break;
        }
        case CBOR_TYPE_STRING: {  // printed as-is
            unsigned char* data = cbor_string_handle(item);
            size_t len = cbor_string_length(item);
            putchar('"');
            for (size_t i = 0; i < len; i++) {
                putchar(static_cast<int>(data[i]));
            }
            putchar('"');
            break;
        }
        case CBOR_TYPE_ARRAY: {
            size_t size = cbor_array_size(item);
            cbor_item_t** elems = cbor_array_handle(item);
            putchar('[');
            for (size_t i = 0; i < size; i++) {
                if (i != 0) printf(", ");
                print_item(elems[i]);
            }
            putchar(']');
            break;
        }
        case CBOR_TYPE_MAP: {
            size_t size = cbor_map_size(item);
            struct cbor_pair* pairs = cbor_map_handle(item);
            putchar('{');
            for (size_t i = 0; i < size; i++) {
                if (i != 0) printf(", ");
                print_item(pairs[i].key);
                printf(": ");
                print_item(pairs[i].value);
            }
            putchar('}');
            break;
        }
        case CBOR_TYPE_FLOAT_CTRL:  // the kernel only emits null/true/false here
            if (cbor_is_null(item)) {
                printf("null");
            } else if (cbor_is_bool(item)) {
                printf(cbor_get_bool(item) ? "true" : "false");
            } else {
                printf("<simple>");
            }
            break;
        default:  // tags are not emitted by the kernel
            printf("<unsupported>");
            break;
    }
}

}  // namespace

int main(int argc, char** argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: %s PATH\n", argv[0]);
        return 1;
    }
    const char* path = argv[1];

    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        fprintf(stderr, "error: cannot open %s\n", path);
        return 1;
    }

    size_t cap = 4096;
    unsigned char* buf = static_cast<unsigned char*>(malloc(cap));
    if (buf == nullptr) {
        fprintf(stderr, "error: out of memory\n");
        close(fd);
        return 1;
    }
    size_t len = 0;  // Bytes buffered in `buf`.
    size_t pos = 0;  // Decode cursor into `buf`.

    for (;;) {
        // Try to decode one complete record from the bytes already buffered.
        struct cbor_load_result result;
        cbor_item_t* item = cbor_load(buf + pos, len - pos, &result);

        if (result.error.code == CBOR_ERR_NONE) {
            print_item(item);
            putchar('\n');
            fflush(stdout);
            cbor_decref(&item);
            pos += result.read;
            continue;
        }

        if (item != nullptr) {
            cbor_decref(&item);
        }
        // A truncated record just means more bytes are on the way; anything else is fatal.
        if (result.error.code != CBOR_ERR_NOTENOUGHDATA &&
            result.error.code != CBOR_ERR_NODATA) {
            fprintf(stderr, "error: malformed CBOR at offset %zu\n",
                    pos + result.error.position);
            break;
        }

        // Compact the consumed prefix, grow if a single record fills the buffer, then read more.
        if (pos > 0) {
            memmove(buf, buf + pos, len - pos);
            len -= pos;
            pos = 0;
        }
        if (len == cap) {
            cap *= 2;
            unsigned char* grown = static_cast<unsigned char*>(realloc(buf, cap));
            if (grown == nullptr) {
                fprintf(stderr, "error: out of memory\n");
                break;
            }
            buf = grown;
        }
        ssize_t n = read(fd, buf + len, cap - len);
        if (n <= 0) {
            break;  // End of stream.
        }
        len += static_cast<size_t>(n);
    }

    free(buf);
    close(fd);
    return 0;
}

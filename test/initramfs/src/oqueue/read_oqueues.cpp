// SPDX-License-Identifier: MPL-2.0
//
// Decode and display an OQueue filesystem (`/oqueues`) CBOR stream.
//
// Each observation file (e.g. `.../strong_observe`) emits a self-delimiting CBOR byte stream of
// one `{seq, value}` record map per observed value. Records are concatenated with no framing, so
// they decode back-to-back regardless of read boundaries. Reading a live file blocks for new
// records (like `tail -f`) and ends when the queue is unregistered or this reader is dropped for
// falling too far behind. Descriptive information about the queue is in the sibling
// `metadata.yaml`, not in this stream.
//
// This tool is intentionally written against the C standard library only (no C++ stdlib), so it
// links statically into the initramfs like the other in-guest tools. It supports exactly the CBOR
// subset the kernel emits: definite-length unsigned/negative integers, byte/text strings, arrays,
// maps, and the `null`/`true`/`false` simple values.
//
// Usage:
//   read_oqueues [PATH]      PATH defaults to /oqueues/scheduler/events/strong_observe

#include <cstdint>
#include <cstdio>

#include <fcntl.h>
#include <unistd.h>

namespace {

// A buffered reader over a file descriptor. It blocks for more data on a live file and reports EOF
// once the underlying `read` returns 0.
struct Reader {
    int fd;
    unsigned char buf[4096];
    size_t len;
    size_t pos;
    bool eof;
};

// Reads the next raw byte, returning false at end of stream.
bool read_byte(Reader& r, uint8_t& out) {
    if (r.pos >= r.len) {
        if (r.eof) {
            return false;
        }
        ssize_t n = read(r.fd, r.buf, sizeof(r.buf));
        if (n <= 0) {
            r.eof = true;
            return false;
        }
        r.len = static_cast<size_t>(n);
        r.pos = 0;
    }
    out = r.buf[r.pos++];
    return true;
}

// Reads the CBOR argument encoded by an item's `info` (low 5 bits of the initial byte).
bool read_arg(Reader& r, uint8_t info, uint64_t& out) {
    if (info < 24) {
        out = info;
        return true;
    }
    int nbytes;
    switch (info) {
        case 24: nbytes = 1; break;
        case 25: nbytes = 2; break;
        case 26: nbytes = 4; break;
        case 27: nbytes = 8; break;
        default: return false;  // indefinite lengths / reserved are not emitted by the kernel
    }
    uint64_t value = 0;
    for (int i = 0; i < nbytes; i++) {
        uint8_t b;
        if (!read_byte(r, b)) {
            return false;
        }
        value = (value << 8) | b;
    }
    out = value;
    return true;
}

// Reads one CBOR item and prints it in a JSON-like form. Returns false at end of stream.
bool print_value(Reader& r) {
    uint8_t initial;
    if (!read_byte(r, initial)) {
        return false;
    }
    uint8_t major = initial >> 5;
    uint8_t info = initial & 0x1f;
    uint64_t arg;

    switch (major) {
        case 0:  // unsigned integer
            if (!read_arg(r, info, arg)) return false;
            printf("%llu", static_cast<unsigned long long>(arg));
            return true;
        case 1:  // negative integer
            if (!read_arg(r, info, arg)) return false;
            printf("-%llu", static_cast<unsigned long long>(arg) + 1);
            return true;
        case 2:    // byte string (printed as hex)
        case 3: {  // text string (printed as-is)
            if (!read_arg(r, info, arg)) return false;
            if (major == 3) putchar('"');
            for (uint64_t i = 0; i < arg; i++) {
                uint8_t b;
                if (!read_byte(r, b)) return false;
                if (major == 3) {
                    putchar(static_cast<int>(b));
                } else {
                    printf("%02x", b);
                }
            }
            if (major == 3) putchar('"');
            return true;
        }
        case 4: {  // array
            if (!read_arg(r, info, arg)) return false;
            putchar('[');
            for (uint64_t i = 0; i < arg; i++) {
                if (i != 0) printf(", ");
                if (!print_value(r)) return false;
            }
            putchar(']');
            return true;
        }
        case 5: {  // map
            if (!read_arg(r, info, arg)) return false;
            putchar('{');
            for (uint64_t i = 0; i < arg; i++) {
                if (i != 0) printf(", ");
                if (!print_value(r)) return false;  // key
                printf(": ");
                if (!print_value(r)) return false;  // value
            }
            putchar('}');
            return true;
        }
        case 7:  // simple values (the kernel only emits null/true/false)
            switch (info) {
                case 20: printf("false"); return true;
                case 21: printf("true"); return true;
                case 22: printf("null"); return true;
                default:
                    if (!read_arg(r, info, arg)) return false;
                    printf("<simple>");
                    return true;
            }
        default:
            return false;
    }
}

}  // namespace

int main(int argc, char** argv) {
    const char* path =
        (argc > 1) ? argv[1] : "/oqueues/scheduler/events/strong_observe";

    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        fprintf(stderr, "error: cannot open %s\n", path);
        return 1;
    }

    Reader r{fd, {}, 0, 0, false};

    // Each item is one `{seq, value}` record; print one per line as it arrives.
    while (print_value(r)) {
        putchar('\n');
        fflush(stdout);
    }

    close(fd);
    return 0;
}

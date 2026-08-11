// SPDX-License-Identifier: MPL-2.0

//! `oqbench_server`: the userspace peer of the kernel -> user -> kernel OQFS round-trip
//! microbenchmark (see `kernel/comps/oqueue_roundtrip_bench`). For each request (a CBOR sequence
//! number) it stamps `t1`, optionally burns compute, stamps `t2`, and writes the reply
//! `[seq, t1, t2]`. When the kernel signals the end of measurement it receives the stored samples
//! over the dump OQueue and writes them to a plain-text results file. Runs as root because the
//! OQueue stream files are root-only.

use std::{
    fs::{self, File},
    io::{BufWriter, Read, Write},
    thread,
    time::Duration,
};

/// Request stream path (kernel -> user); overridable with `--request-path`.
const DEFAULT_REQUEST_PATH: &str = "/oqueues/oqbench/request/strong_observe";
/// Reply produce file path (user -> kernel); overridable with `--reply-path`.
const DEFAULT_REPLY_PATH: &str = "/oqueues/oqbench/reply/produce";
/// Dump stream path (kernel -> user), carrying the stored samples; overridable with `--dump-path`.
const DEFAULT_DUMP_PATH: &str = "/oqueues/oqbench/dump/strong_observe";
/// Results file the samples are written to; overridable with `--output`.
const DEFAULT_OUTPUT_PATH: &str = "/tmp/oqbench-samples.csv";

/// Read buffer size when draining a stream file.
const READ_CHUNK_SIZE: usize = 4096;

/// Request value marking the end of measurement; the peer then switches to receiving the dump. Must
/// match `DUMP_SENTINEL` in `kernel/comps/oqueue_roundtrip_bench`.
const DUMP_SENTINEL: u64 = u64::MAX;
/// Ack the kernel after every this-many samples written (and once at the end), pacing the kernel so
/// its revocable observer never fills. Must match `DUMP_ACK_EVERY` in the kernel component.
const DUMP_ACK_EVERY: u64 = 1024;

/// Max attempts before giving up waiting for an OQueue path to appear.
const MAX_RETRY_ATTEMPTS: u32 = 100;
/// Delay between retries when waiting for an OQueue path to appear.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Reads the CPU timestamp counter -- the userspace half of the shared guest TSC time base.
fn rdtsc() -> u64 {
    // SAFETY: `_rdtsc` is always available on x86-64 and has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Effective configuration parsed from argv.
struct Config {
    compute: u64,
    verbose: bool,
    request_path: String,
    reply_path: String,
    dump_path: String,
    output_path: String,
}

const USAGE: &str = "\
oqbench_server -- userspace peer for the OQFS round-trip microbenchmark

USAGE:
    oqbench_server [OPTIONS]

OPTIONS:
    --compute <N>          Synthetic compute iterations to burn per request (default: 0).
    --request-path <PATH>  Request stream to read (default: /oqueues/oqbench/request/strong_observe).
    --reply-path <PATH>    Reply produce file to write (default: /oqueues/oqbench/reply/produce).
    --dump-path <PATH>     Dump stream to read the samples from (default: /oqueues/oqbench/dump/strong_observe).
    --output <PATH>        Results file to write the samples to (default: /tmp/oqbench-samples.csv).
    --verbose              Log each request/reply to stderr (do not use for a real timing run).
    -h, --help             Print this help and exit.
";

/// Parses argv; exits with a message on an unknown flag or a missing value.
fn parse_args() -> Config {
    let mut config = Config {
        compute: 0,
        verbose: false,
        request_path: DEFAULT_REQUEST_PATH.to_string(),
        reply_path: DEFAULT_REPLY_PATH.to_string(),
        dump_path: DEFAULT_DUMP_PATH.to_string(),
        output_path: DEFAULT_OUTPUT_PATH.to_string(),
    };

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--verbose" => config.verbose = true,
            "--compute" => {
                let value = args
                    .next()
                    .unwrap_or_else(|| fatal("--compute requires a value"));
                config.compute = value
                    .parse()
                    .unwrap_or_else(|_| fatal("--compute value must be a non-negative integer"));
            }
            "--request-path" => {
                config.request_path = args
                    .next()
                    .unwrap_or_else(|| fatal("--request-path requires a value"));
            }
            "--reply-path" => {
                config.reply_path = args
                    .next()
                    .unwrap_or_else(|| fatal("--reply-path requires a value"));
            }
            "--dump-path" => {
                config.dump_path = args
                    .next()
                    .unwrap_or_else(|| fatal("--dump-path requires a value"));
            }
            "--output" => {
                config.output_path = args
                    .next()
                    .unwrap_or_else(|| fatal("--output requires a value"));
            }
            other => fatal(&format!("unknown argument '{other}' (try --help)")),
        }
    }
    config
}

/// Prints an error and exits non-zero.
fn fatal(message: &str) -> ! {
    eprintln!("oqbench_server: {message}");
    std::process::exit(2);
}

/// Burns `iterations` of synthetic compute the optimizer cannot elide.
fn burn_compute(iterations: u64) {
    let mut acc: u64 = 0;
    for i in 0..iterations {
        acc = acc
            .wrapping_mul(6364136223846793005)
            .wrapping_add((i | 1) as u64);
        std::hint::black_box(acc);
    }
    std::hint::black_box(acc);
}

/// Decodes one CBOR unsigned integer from the front of `bytes`, returning `(value, consumed)`, or
/// `None` if `bytes` does not yet hold a complete integer.
fn decode_request(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut decoder = minicbor::decode::Decoder::new(bytes);
    let value = decoder.u64().ok()?;
    Some((value, decoder.position()))
}

/// Encodes one reply as the fixed 3-element CBOR array `[seq, t1, t2]` into `out` (cleared first).
/// A dump ack reuses this as `[consumed, 0, 0]`.
fn encode_reply(out: &mut Vec<u8>, seq: u64, t1: u64, t2: u64) {
    out.clear();
    minicbor::encode([seq, t1, t2], &mut *out)
        .expect("encoding a 3-element array of u64 into a Vec cannot fail");
}

/// Decodes one dump record -- a fixed 4-element CBOR array of `u64` -- from the front of `bytes`,
/// returning `(record, consumed)`, or `None` if `bytes` does not yet hold a complete record.
fn decode_sample(bytes: &[u8]) -> Option<([u64; 4], usize)> {
    let mut decoder = minicbor::decode::Decoder::new(bytes);
    if decoder.array().ok()? != Some(4) {
        fatal("dump record was not a 4-element array");
    }
    let mut record = [0u64; 4];
    for field in &mut record {
        *field = decoder.u64().ok()?;
    }
    Some((record, decoder.position()))
}

/// Opens `path`, retrying briefly while the OQueue registry is still being populated at boot.
fn open_with_retry(path: &str, write: bool) -> File {
    let mut attempt = 0;
    loop {
        let opened = if write {
            fs::OpenOptions::new().write(true).open(path)
        } else {
            File::open(path)
        };
        match opened {
            Ok(file) => return file,
            Err(err) if attempt < MAX_RETRY_ATTEMPTS => {
                attempt += 1;
                eprintln!("oqbench_server: waiting for {path} ({err}), retrying...");
                thread::sleep(RETRY_INTERVAL);
            }
            Err(err) => {
                fatal(&format!("failed to open {path}: {err}"));
            }
        }
    }
}

fn main() {
    let config = parse_args();
    eprintln!(
        "oqbench_server: starting (compute={}, request={}, reply={}, output={})",
        config.compute, config.request_path, config.reply_path, config.output_path
    );

    let mut request_file = open_with_retry(&config.request_path, false);
    let mut reply_file = open_with_retry(&config.reply_path, true);
    // Open the dump stream now so its observer is attached before the kernel starts the transfer.
    let mut dump_file = open_with_retry(&config.dump_path, false);
    eprintln!("oqbench_server: attached to request, reply, and dump streams");

    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_SIZE];
    let mut reply = Vec::new();
    let mut replied_first = false;

    loop {
        // `t1` is stamped the instant the read completing the request record returns.
        let (seq, t1) = loop {
            if let Some((seq, consumed)) = decode_request(&pending) {
                let t1 = rdtsc();
                pending.drain(..consumed);
                break (seq, t1);
            }
            let read = match request_file.read(&mut chunk) {
                Ok(0) => {
                    eprintln!("oqbench_server: request stream ended; exiting");
                    return;
                }
                Ok(n) => n,
                Err(err) => fatal(&format!("error reading a request: {err}")),
            };
            pending.extend_from_slice(&chunk[..read]);
        };

        // The kernel ends the measurement phase with the sentinel; switch to receiving the dump.
        if seq == DUMP_SENTINEL {
            break;
        }

        burn_compute(config.compute);

        // `t2` is stamped the instant before the reply is written.
        let t2 = rdtsc();
        encode_reply(&mut reply, seq, t1, t2);
        if let Err(err) = reply_file.write_all(&reply) {
            fatal(&format!("failed to write a reply: {err}"));
        }

        if config.verbose {
            eprintln!("oqbench_server: seq={seq} t1={t1} t2={t2}");
        }
        if !replied_first {
            eprintln!("oqbench_server: replied to first request (seq={seq})");
            replied_first = true;
        }
    }

    receive_dump(&config, &mut dump_file, &mut reply_file);
}

/// Reads the sample count and then that many sample records from the dump stream, writing each as
/// one CSV line to the results file and acking progress on the reply stream. An early end of the
/// dump stream (a revoked observer) is a truncation and is fatal, so a short file is never left
/// behind as if it were whole.
fn receive_dump(config: &Config, dump_file: &mut File, reply_file: &mut File) {
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_SIZE];

    let read_record = |dump_file: &mut File, pending: &mut Vec<u8>, chunk: &mut [u8], have: u64, want: u64| -> [u64; 4] {
        loop {
            if let Some((record, consumed)) = decode_sample(pending) {
                pending.drain(..consumed);
                return record;
            }
            match dump_file.read(chunk) {
                Ok(0) => fatal(&format!("dump stream ended after {have} of {want} samples")),
                Ok(n) => pending.extend_from_slice(&chunk[..n]),
                Err(err) => fatal(&format!("error reading the dump stream: {err}")),
            }
        }
    };

    // The first record is the header `[count, 0, 0, 0]`.
    let count = read_record(dump_file, &mut pending, &mut chunk, 0, 0)[0];
    eprintln!("oqbench_server: receiving {count} samples into {}", config.output_path);

    let output = File::create(&config.output_path)
        .unwrap_or_else(|err| fatal(&format!("cannot create {}: {err}", config.output_path)));
    let mut writer = BufWriter::new(output);
    let mut ack = Vec::new();

    let mut written: u64 = 0;
    while written < count {
        let record = read_record(dump_file, &mut pending, &mut chunk, written, count);
        writeln!(writer, "{},{},{},{}", record[0], record[1], record[2], record[3])
            .unwrap_or_else(|err| fatal(&format!("cannot write to {}: {err}", config.output_path)));
        written += 1;

        // Ack only after flushing, so the kernel learns of received samples only once they are in
        // the file; the final ack therefore certifies a complete, durable results file.
        if written % DUMP_ACK_EVERY == 0 || written == count {
            writer
                .flush()
                .unwrap_or_else(|err| fatal(&format!("cannot flush {}: {err}", config.output_path)));
            encode_reply(&mut ack, written, 0, 0);
            if let Err(err) = reply_file.write_all(&ack) {
                fatal(&format!("failed to write a dump ack: {err}"));
            }
        }
    }

    eprintln!("oqbench_server: wrote {count} samples to {}", config.output_path);
}

// SPDX-License-Identifier: MPL-2.0

//! `oqbench_server`: the userspace peer of the kernel -> user -> kernel OQFS round-trip
//! microbenchmark (see `kernel/comps/mariposa_benchmark`). For each request (a CBOR sequence number)
//! it stamps `t1`, optionally spins for a fixed number of cycles, stamps `t2`, and writes the reply
//! `[seq, t1, t2]`. A request of `null` means the measurement is over and the peer exits; the kernel
//! writes the samples to the data capture device itself. Runs as root because the OQueue stream
//! files are root-only.

use std::{
    fs::{self, File},
    io::{Read, Write},
    thread,
    time::Duration,
};

/// Request stream path (kernel -> user); overridable with `--request-path`.
const DEFAULT_REQUEST_PATH: &str = "/oqueues/oqbench/request/strong_observe";
/// Reply produce file path (user -> kernel); overridable with `--reply-path`.
const DEFAULT_REPLY_PATH: &str = "/oqueues/oqbench/reply/produce";

/// Read buffer size when draining a stream file.
const READ_CHUNK_SIZE: usize = 4096;

/// Max attempts before giving up waiting for an OQueue path to appear.
const MAX_RETRY_ATTEMPTS: u32 = 100;
/// Delay between retries when waiting for an OQueue path to appear.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Reads the CPU timestamp counter, the same time base the kernel side stamps with.
fn rdtsc() -> u64 {
    // SAFETY: `_rdtsc` is always available on x86-64 and has no preconditions.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Effective configuration parsed from argv.
struct Config {
    compute_cycles: u64,
    verbose: bool,
    request_path: String,
    reply_path: String,
}

const USAGE: &str = "\
oqbench_server -- userspace peer for the OQFS round-trip microbenchmark

USAGE:
    oqbench_server [OPTIONS]

OPTIONS:
    --compute <CYCLES>     TSC cycles to spin for per request before replying (default: 0).
    --request-path <PATH>  Request stream to read (default: /oqueues/oqbench/request/strong_observe).
    --reply-path <PATH>    Reply produce file to write (default: /oqueues/oqbench/reply/produce).
    --verbose              Log each request/reply to stderr (do not use for a real timing run).
    -h, --help             Print this help and exit.
";

/// Parses argv; exits with a message on an unknown flag or a missing value.
fn parse_args() -> Config {
    let mut config = Config {
        compute_cycles: 0,
        verbose: false,
        request_path: DEFAULT_REQUEST_PATH.to_string(),
        reply_path: DEFAULT_REPLY_PATH.to_string(),
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
                config.compute_cycles = value
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

/// Spins until the timestamp counter has advanced by `cycles`.
///
/// This waits for a measured amount of time rather than for an amount of arithmetic, whose duration
/// would depend on how well the hardware happened to pipeline it.
fn spin_for_cycles(cycles: u64) {
    if cycles == 0 {
        return;
    }
    let deadline = rdtsc().wrapping_add(cycles);
    while rdtsc().wrapping_sub(deadline) > u64::MAX / 2 {
        std::hint::spin_loop();
    }
}

/// Decodes one CBOR request from the front of `bytes`, returning `(request, consumed)`, or `None` if
/// `bytes` does not yet hold a complete request. A CBOR `null` is the kernel's `None`, meaning the
/// measurement is over.
fn decode_request(bytes: &[u8]) -> Option<(Option<u64>, usize)> {
    let mut decoder = minicbor::decode::Decoder::new(bytes);
    let request = match decoder.datatype().ok()? {
        minicbor::data::Type::Null => {
            decoder.null().ok()?;
            None
        }
        _ => Some(decoder.u64().ok()?),
    };
    Some((request, decoder.position()))
}

/// Encodes one reply as the fixed 3-element CBOR array `[seq, t1, t2]` into `out` (cleared first).
fn encode_reply(out: &mut Vec<u8>, seq: u64, t1: u64, t2: u64) {
    out.clear();
    minicbor::encode([seq, t1, t2], &mut *out)
        .expect("encoding a 3-element array of u64 into a Vec cannot fail");
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
        "oqbench_server: starting (compute={} cycles, request={}, reply={})",
        config.compute_cycles, config.request_path, config.reply_path
    );

    let mut request_file = open_with_retry(&config.request_path, false);
    let mut reply_file = open_with_retry(&config.reply_path, true);
    eprintln!("oqbench_server: attached to the request and reply streams");

    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_SIZE];
    let mut reply = Vec::new();
    let mut replied_first = false;

    loop {
        // `t1` is stamped the instant the read completing the request record returns.
        let (request, t1) = loop {
            if let Some((request, consumed)) = decode_request(&pending) {
                let t1 = rdtsc();
                pending.drain(..consumed);
                break (request, t1);
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

        let Some(seq) = request else {
            eprintln!("oqbench_server: measurement finished; exiting");
            return;
        };

        spin_for_cycles(config.compute_cycles);

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
}

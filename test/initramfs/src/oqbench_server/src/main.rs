// SPDX-License-Identifier: MPL-2.0

//! `oqbench_server`: the userspace peer of the kernel -> user -> kernel OQFS round-trip
//! microbenchmark (see `kernel/comps/mariposa_benchmark`). For each `Measure` request it stamps `t1`,
//! optionally spins for a fixed number of cycles, stamps `t2`, and writes the reply `[seq, t1, t2]`.
//! The kernel writes the samples to the data capture device itself and then ends the run with either
//! `Finished` or `Failed`, which this peer turns into its own exit status.
//!
//! The kernel never stops the machine, so this process ending is what lets `init` power the guest
//! off. That also means every wait here needs a bound: a kernel side that never starts must not leave
//! the guest hanging with no diagnostic.
//!
//! Runs as root because the OQueue stream files are root-only.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use snafu::Snafu;

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
/// How long to wait for the kernel's first request before giving up. The kernel side can decline to
/// start at all (no data capture device, for instance) and it can no longer stop the machine to say
/// so, so this bound is what keeps such a boot from hanging.
const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Why the peer stopped early.
///
/// The variant chooses the process's exit status, so a run that failed can never leave the same trace
/// as one that finished.
#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("the run failed; the kernel console has the reason"))]
    RunFailed,

    #[snafu(display("the request stream ended before the run reported a result"))]
    StreamEnded,

    #[snafu(display(
        "no request within {}s; the kernel side never started",
        timeout.as_secs()
    ))]
    NoRequest { timeout: Duration },

    #[snafu(display("{message} (try --help)"))]
    Usage { message: String },

    #[snafu(display("malformed request: {reason}"))]
    MalformedRequest { reason: String },

    #[snafu(display("could not {action}: {source}"))]
    Io { action: String, source: io::Error },
}

impl Error {
    /// The process exit status that reports this failure.
    fn code(&self) -> u8 {
        match self {
            Error::RunFailed | Error::StreamEnded => 1,
            Error::Usage { .. } => 2,
            Error::NoRequest { .. } => 3,
            Error::MalformedRequest { .. } | Error::Io { .. } => 4,
        }
    }
}

/// Builds an [`Error::Io`] for the operation that failed.
fn io_error(action: &str) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        action: action.to_string(),
        source,
    }
}

/// What the kernel is asking for. Mirrors `Request` in the kernel component.
enum Request {
    /// Time this round trip.
    Measure(u64),
    /// Every sample is captured; the run succeeded.
    Finished,
    /// The run failed; the reason is on the kernel console.
    Failed,
}

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

/// Parses argv. `--help` prints the usage and exits before returning.
fn parse_args() -> Result<Config, Error> {
    let mut config = Config {
        compute_cycles: 0,
        verbose: false,
        request_path: DEFAULT_REQUEST_PATH.to_string(),
        reply_path: DEFAULT_REPLY_PATH.to_string(),
    };
    let usage_error = |message: &str| Error::Usage {
        message: message.to_string(),
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
                    .ok_or_else(|| usage_error("--compute requires a value"))?;
                config.compute_cycles = value
                    .parse()
                    .map_err(|_| usage_error("--compute value must be a non-negative integer"))?;
            }
            "--request-path" => {
                config.request_path = args
                    .next()
                    .ok_or_else(|| usage_error("--request-path requires a value"))?;
            }
            "--reply-path" => {
                config.reply_path = args
                    .next()
                    .ok_or_else(|| usage_error("--reply-path requires a value"))?;
            }
            other => return Err(usage_error(&format!("unknown argument '{other}'"))),
        }
    }
    Ok(config)
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

/// Decodes one request from the front of `bytes`, returning `(request, consumed)`, or `Ok(None)` if
/// `bytes` does not yet hold a complete one.
///
/// The kernel serializes `Request` with serde, which gives the tuple variant a single-entry map
/// (`{"Measure": 7}`) and the unit variants a plain string (`"Finished"`).
fn decode_request(bytes: &[u8]) -> Result<Option<(Request, usize)>, Error> {
    let malformed = |reason: &str| Error::MalformedRequest {
        reason: reason.to_string(),
    };
    let mut decoder = minicbor::decode::Decoder::new(bytes);
    let Ok(datatype) = decoder.datatype() else {
        return Ok(None);
    };
    let request = match datatype {
        minicbor::data::Type::Map => {
            let Ok(entries) = decoder.map() else {
                return Ok(None);
            };
            if entries != Some(1) {
                return Err(malformed("expected a single-entry map"));
            }
            let Ok(variant) = decoder.str() else {
                return Ok(None);
            };
            if variant != "Measure" {
                return Err(malformed(&format!("unknown variant '{variant}'")));
            }
            let Ok(seq) = decoder.u64() else {
                return Ok(None);
            };
            Request::Measure(seq)
        }
        minicbor::data::Type::String => {
            let Ok(variant) = decoder.str() else {
                return Ok(None);
            };
            match variant {
                "Finished" => Request::Finished,
                "Failed" => Request::Failed,
                other => return Err(malformed(&format!("unknown request '{other}'"))),
            }
        }
        other => return Err(malformed(&format!("unexpected CBOR type {other:?}"))),
    };
    Ok(Some((request, decoder.position())))
}

/// Encodes one reply as the fixed 3-element CBOR array `[seq, t1, t2]` into `out` (cleared first).
fn encode_reply(out: &mut Vec<u8>, seq: u64, t1: u64, t2: u64) {
    out.clear();
    minicbor::encode([seq, t1, t2], &mut *out)
        .expect("encoding a 3-element array of u64 into a Vec cannot fail");
}

/// Opens `path`, retrying briefly while the OQueue registry is still being populated at boot.
fn open_with_retry(path: &str, write: bool) -> Result<File, Error> {
    let mut attempt = 0;
    loop {
        let opened = if write {
            fs::OpenOptions::new().write(true).open(path)
        } else {
            File::open(path)
        };
        match opened {
            Ok(file) => return Ok(file),
            Err(err) if attempt < MAX_RETRY_ATTEMPTS => {
                attempt += 1;
                eprintln!("oqbench_server: waiting for {path} ({err}), retrying...");
                thread::sleep(RETRY_INTERVAL);
            }
            Err(err) => return Err(io_error(&format!("open {path}"))(err)),
        }
    }
}

/// Exits the process if the kernel has not asked for anything within [`FIRST_REQUEST_TIMEOUT`].
///
/// This runs on its own thread because the main thread sits in a blocking read that nothing else can
/// interrupt, so it reports and exits directly rather than returning an error.
fn spawn_first_request_watchdog(got_request: Arc<AtomicBool>) {
    thread::spawn(move || {
        thread::sleep(FIRST_REQUEST_TIMEOUT);
        if !got_request.load(Ordering::Relaxed) {
            let error = Error::NoRequest {
                timeout: FIRST_REQUEST_TIMEOUT,
            };
            eprintln!("oqbench_server: {error}");
            std::process::exit(error.code().into());
        }
    });
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oqbench_server: {error}");
            ExitCode::from(error.code())
        }
    }
}

/// Serves requests until the kernel ends the run, returning `Ok` only for a run that finished.
fn run() -> Result<(), Error> {
    let config = parse_args()?;
    eprintln!(
        "oqbench_server: starting (compute={} cycles, request={}, reply={})",
        config.compute_cycles, config.request_path, config.reply_path
    );

    let mut request_file = open_with_retry(&config.request_path, false)?;
    let mut reply_file = open_with_retry(&config.reply_path, true)?;
    eprintln!("oqbench_server: attached to the request and reply streams");

    // Attaching is what starts the run on the kernel side, so only arm the watchdog once we are
    // attached; before that, slow boots are expected rather than a fault.
    let got_request = Arc::new(AtomicBool::new(false));
    spawn_first_request_watchdog(got_request.clone());

    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; READ_CHUNK_SIZE];
    let mut reply = Vec::new();
    let mut replied_first = false;

    loop {
        // `t1` is stamped the instant the read completing the request record returns.
        let (request, t1) = loop {
            if let Some((request, consumed)) = decode_request(&pending)? {
                let t1 = rdtsc();
                pending.drain(..consumed);
                got_request.store(true, Ordering::Relaxed);
                break (request, t1);
            }
            let read = request_file
                .read(&mut chunk)
                .map_err(io_error("read a request"))?;
            // The stream ending without a verdict means the kernel side went away mid-run, which is
            // a failed run rather than a finished one.
            if read == 0 {
                return Err(Error::StreamEnded);
            }
            pending.extend_from_slice(&chunk[..read]);
        };

        let seq = match request {
            Request::Measure(seq) => seq,
            Request::Finished => {
                eprintln!("oqbench_server: the run finished; exiting");
                return Ok(());
            }
            Request::Failed => return Err(Error::RunFailed),
        };

        spin_for_cycles(config.compute_cycles);

        // `t2` is stamped the instant before the reply is written.
        let t2 = rdtsc();
        encode_reply(&mut reply, seq, t1, t2);
        reply_file
            .write_all(&reply)
            .map_err(io_error("write a reply"))?;

        if config.verbose {
            eprintln!("oqbench_server: seq={seq} t1={t1} t2={t2}");
        }
        if !replied_first {
            eprintln!("oqbench_server: replied to first request (seq={seq})");
            replied_first = true;
        }
    }
}

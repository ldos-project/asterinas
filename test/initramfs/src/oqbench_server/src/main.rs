// SPDX-License-Identifier: MPL-2.0

//! `oqbench_server`: the userspace peer of the kernel -> user -> kernel OQFS round-trip
//! microbenchmark (see `kernel/comps/mariposa_benchmark`). For each `Measure` request it stamps `t1`,
//! optionally spins for a fixed number of cycles, stamps `t2`, and writes the reply `[seq, t1, t2]`.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    process::ExitCode,
    thread,
    time::Duration,
};

use clap::Parser;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, Snafu};

/// Request stream path (kernel -> user); overridable with `--request-path`.
const DEFAULT_REQUEST_PATH: &str = "/oqueues/oqbench/request/strong_observe";
/// Reply produce file path (user -> kernel); overridable with `--reply-path`.
const DEFAULT_REPLY_PATH: &str = "/oqueues/oqbench/reply/produce";
/// Control produce file path (user -> kernel), carrying this peer's lifecycle signals; overridable
/// with `--control-path`.
const DEFAULT_CONTROL_PATH: &str = "/oqueues/oqbench/control/produce";

/// Read buffer size when draining a stream file.
const READ_CHUNK_SIZE: usize = 4096;

/// Max attempts before giving up waiting for an OQueue path to appear.
const MAX_RETRY_ATTEMPTS: u32 = 100;
/// Delay between retries when waiting for an OQueue path to appear.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// What this peer tells the kernel about its own lifecycle. Mirrors `PeerSignal` in the kernel
/// component.
#[derive(Clone, Copy, Debug, Serialize)]
enum Signal {
    Ready,
    Done,
}

/// Every way this peer can stop other than by the run finishing.
#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("the run failed; refer the error trace from the kernel console"))]
    RunFailed,

    #[snafu(display("the request stream ended before the run reported a result"))]
    StreamEnded,

    #[snafu(display("could not decode a request: {source}"))]
    DecodeRequest {
        source: minicbor_serde::error::DecodeError,
    },

    #[snafu(display("could not open {path}: {source}"))]
    OpenStream { path: String, source: io::Error },

    #[snafu(display("could not send the {signal:?} signal: {source}"))]
    SendSignal { signal: Signal, source: io::Error },

    #[snafu(display("could not read a request: {source}"))]
    ReadRequest { source: io::Error },

    #[snafu(display("could not write a reply: {source}"))]
    WriteReply { source: io::Error },
}

/// What the kernel is asking for. Mirrors `Request` in the kernel component, which serde puts on
/// the wire; the derive here is what keeps the two ends in step.
#[derive(Debug, Deserialize)]
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
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Effective configuration parsed from argv.
#[derive(Parser)]
#[command(about = "Userspace peer for the OQFS round-trip microbenchmark")]
struct Config {
    /// TSC cycles to spin for per request before replying.
    #[arg(long = "compute", value_name = "CYCLES", default_value_t = 0)]
    compute_cycles: u64,

    /// Log each request and reply to stderr. Do not use for a real timing run.
    #[arg(long)]
    verbose: bool,

    /// Request stream to read, kernel to user.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_REQUEST_PATH)]
    request_path: String,

    /// Reply produce file to write, user to kernel.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_REPLY_PATH)]
    reply_path: String,

    /// Control produce file carrying this peer's lifecycle signals.
    #[arg(long, value_name = "PATH", default_value = DEFAULT_CONTROL_PATH)]
    control_path: String,
}

/// Spins until the timestamp counter has advanced by `cycles`.
fn spin_for_cycles(cycles: u64) {
    if cycles == 0 {
        return;
    }
    let start = rdtsc();
    while rdtsc().wrapping_sub(start) < cycles {
        std::hint::spin_loop();
    }
}

/// Decodes one request from the front of `bytes`, returning `(request, consumed)`, or `Ok(None)` if
/// `bytes` does not yet hold a complete one.
fn decode_request(bytes: &[u8]) -> Result<Option<(Request, usize)>, Error> {
    // Skipping fails while `bytes` holds only part of a record, which is not an error: it means the
    // rest has yet to be read. Only a whole record is handed to serde, so its errors are real ones.
    let mut probe = minicbor::decode::Decoder::new(bytes);
    if probe.skip().is_err() {
        return Ok(None);
    }
    let consumed = probe.position();
    let request = minicbor_serde::from_slice(&bytes[..consumed]).context(DecodeRequestSnafu)?;
    Ok(Some((request, consumed)))
}

/// Encodes one reply as the fixed 3-element CBOR array `[seq, t1, t2]` into `out` (cleared first).
fn encode_reply(out: &mut Vec<u8>, seq: u64, t1: u64, t2: u64) {
    out.clear();
    minicbor::encode([seq, t1, t2], &mut *out)
        .expect("encoding a 3-element array of u64 into a Vec cannot fail");
}

/// Tells the kernel where this peer is in its lifecycle.
fn signal(control_file: &mut File, signal: Signal) -> Result<(), Error> {
    let encoded = minicbor_serde::to_vec(signal).expect("encoding a unit variant cannot fail");
    control_file
        .write_all(&encoded)
        .context(SendSignalSnafu { signal })
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
            Err(source) => return Err(source).context(OpenStreamSnafu { path }),
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oqbench_server: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Serves requests until the kernel ends the run, returning `Ok` only for a run that finished.
fn run() -> Result<(), Error> {
    let config = Config::parse();
    eprintln!(
        "oqbench_server: starting (compute={} cycles, request={}, reply={})",
        config.compute_cycles, config.request_path, config.reply_path
    );

    let mut request_file = open_with_retry(&config.request_path, false)?;
    let mut reply_file = open_with_retry(&config.reply_path, true)?;
    let mut control_file = open_with_retry(&config.control_path, true)?;
    eprintln!("oqbench_server: attached to the request, reply and control streams");

    // Only now, with every stream open, is this peer able to serve. Saying so is what starts the run.
    signal(&mut control_file, Signal::Ready)?;

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
                break (request, t1);
            }
            let read = request_file.read(&mut chunk).context(ReadRequestSnafu)?;
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
                signal(&mut control_file, Signal::Done)?;
                return Ok(());
            }
            Request::Failed => {
                // Report done even on a failure, so the kernel stops holding the queues open for us.
                signal(&mut control_file, Signal::Done)?;
                return Err(Error::RunFailed);
            }
        };

        spin_for_cycles(config.compute_cycles);

        // `t2` is stamped the instant before the reply is written.
        let t2 = rdtsc();
        encode_reply(&mut reply, seq, t1, t2);
        reply_file.write_all(&reply).context(WriteReplySnafu)?;

        if config.verbose {
            eprintln!("oqbench_server: seq={seq} t1={t1} t2={t2}");
        }
        if !replied_first {
            eprintln!("oqbench_server: replied to first request (seq={seq})");
            replied_first = true;
        }
    }
}

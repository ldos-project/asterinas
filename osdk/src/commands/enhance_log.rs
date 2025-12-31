// SPDX-License-Identifier: MPL-2.0

use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    process::Stdio,
    sync::LazyLock,
};

use regex::Regex;

use crate::{cli::EnhanceLogArgs, util::new_command_checked_exists};

use rustc_demangle::demangle;

/// Process a log file by adding additional lines with useful information.
///
/// Currently:
///  * Symbolized versions of stack traces produced by panicking or the CaptureStacktrace Display
///    implementation.
pub fn execute_enhance_log(args: &EnhanceLogArgs) {
    // Start addr2line process
    let mut addr2line = new_command_checked_exists("addr2line")
        .args(["-f", "-e"])
        .arg(args.bin_file.clone())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to start addr2line");

    {
        let mut addr2line_stdin = addr2line
            .stdin
            .as_mut()
            .expect("failed to open addr2line stdin");
        let mut addr2line_stdout = BufReader::new(
            addr2line
                .stdout
                .as_mut()
                .expect("failed to open addr2line stdout"),
        );

        let reader: Box<dyn Read> = if let Some(input) = &args.input {
            Box::new(File::open(input).expect("Failed to open input file"))
        } else {
            Box::new(std::io::stdin())
        };

        let mut writer: Box<dyn Write> = if let Some(output) = &args.output {
            Box::new(File::create(output).expect("Failed to open output file"))
        } else {
            Box::new(std::io::stdout())
        };

        let mut stderr = std::io::stderr();

        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else {
                writeln!(stderr, "ERROR: Failed to read non-UTF-8 line.").expect("Write failed");
                continue;
            };
            writeln!(writer, "{}", line).expect("Write failed");

            process_single_line_stacktrace(
                &line,
                &mut addr2line_stdin,
                &mut addr2line_stdout,
                &mut writer,
            );
            process_single_frame(
                &line,
                &mut addr2line_stdin,
                &mut addr2line_stdout,
                &mut writer,
            );
        }
    }

    addr2line.wait().expect("Failed to shutdown addr2line");
}

/// Symbolize and write out a line for the frame information provided.
fn write_stack_frame_line(
    addr2line_stdin: &mut impl Write,
    addr2line_stdout: &mut impl BufRead,
    writer: &mut impl Write,
    frame_number: usize,
    frame: u64,
) -> Option<()> {
    writeln!(addr2line_stdin, "0x{:x}", frame).ok()?;
    addr2line_stdin.flush().ok()?;
    let mut function_line = String::new();
    addr2line_stdout.read_line(&mut function_line).ok()?;
    let function_line = function_line.trim();
    let demangled = demangle(function_line).to_string();
    let mut file_line = String::new();
    addr2line_stdout.read_line(&mut file_line).ok()?;
    let file_line = file_line.trim();
    writeln!(writer, "{frame_number:4}: {demangled} ({file_line})").ok()?;
    Some(())
}

static STACKTRACE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r".*stacktrace\[((?:(?:0x[0-9a-fA-F]+)(?:, )?)+)\].*").unwrap());

/// Process a line from the log printing stack frames if a stacktrace is found.
fn process_single_line_stacktrace(
    line: &str,
    addr2line_stdin: &mut impl Write,
    addr2line_stdout: &mut impl BufRead,
    writer: &mut impl Write,
) -> Option<()> {
    let captures = STACKTRACE_PATTERN.captures(line)?;
    let addresses = captures.get(1).unwrap().as_str();
    let frames = addresses
        .split(", ")
        .map(|s| -> Option<u64> { u64::from_str_radix(s.strip_prefix("0x")?, 16).ok() })
        .collect::<Option<Vec<u64>>>()?;

    for (frame_number, frame) in frames.into_iter().enumerate() {
        write_stack_frame_line(
            addr2line_stdin,
            addr2line_stdout,
            writer,
            frame_number,
            frame,
        )?;
    }
    Some(())
}

static SINGLE_FRAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(\d+): .* pc (0x[0-9a-fA-F]+) .*").unwrap());

/// Process single frame lines produced by panicking.
fn process_single_frame(
    line: &str,
    addr2line_stdin: &mut impl Write,
    addr2line_stdout: &mut impl BufRead,
    writer: &mut impl Write,
) -> Option<()> {
    let captures = SINGLE_FRAME_REGEX.captures(line)?;

    let number: usize = captures.get(1).unwrap().as_str().parse().ok()?;
    let pc = u64::from_str_radix(captures.get(2).unwrap().as_str().strip_prefix("0x")?, 16).ok()?;

    write_stack_frame_line(addr2line_stdin, addr2line_stdout, writer, number, pc)?;

    Some(())
}

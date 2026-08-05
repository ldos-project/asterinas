// SPDX-License-Identifier: MPL-2.0

//! `raid_policy_supervisor`: swaps the running RAID-1 selection-policy PROCESS while the kernel keeps
//! serving I/O.
//!
//! It starts a default policy (`avg_latency`) at boot, then polls `/tmp/raid_policy` for a policy
//! name. On a change it terminates the current child, waits for it to exit, and spawns the new one;
//! on an UNKNOWN name it logs loudly and keeps the current child running (a default is for an UNSET
//! value, not an INVALID one).
//!
//! Terminate-then-spawn (not spawn-then-terminate) is deliberate: an OQFS `produce` open attaches a
//! fresh producer, and the outgoing process must release its attachment before the incoming one can
//! acquire it. The gap is kept short and the policy programs' open-retry covers it. During the gap
//! no policy is attached, so the kernel's `UserspacePolicy` hits its 200ms reply timeout and falls
//! back to round-robin for those reads — correct behaviour, just briefly slower.
//!
//! A policy name is valid iff the binary `raid_policy_<name>` exists in [`POLICY_BIN_DIR`]. That is
//! the entire plugin registry: adding a new policy means installing a new binary, with NO edit here.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::Duration,
};

/// The policy the supervisor starts at boot.
const DEFAULT_POLICY: &str = "avg_latency";

/// File the supervisor polls for the desired policy name (written by whoever drives the swap).
const REQUEST_PATH: &str = "/tmp/raid_policy";

/// File the supervisor writes the currently-active policy name to, so a swap can be confirmed to
/// have taken effect (rather than slept-and-hoped).
const ACTIVE_PATH: &str = "/tmp/raid_policy_active";

/// Directory holding the `raid_policy_<name>` binaries.
const POLICY_BIN_DIR: &str = "/usr/bin";

/// How often to poll [`REQUEST_PATH`].
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The binary path for a policy `name` (`/usr/bin/raid_policy_<name>`).
fn policy_binary(name: &str) -> PathBuf {
    Path::new(POLICY_BIN_DIR).join(format!("raid_policy_{name}"))
}

/// Spawns the policy program for `name`, inheriting stdio so its logs reach the console.
fn spawn_policy(name: &str) -> std::io::Result<Child> {
    Command::new(policy_binary(name)).spawn()
}

/// Records the active policy name so a swap can be confirmed by polling.
fn set_active(name: &str) {
    if let Err(err) = fs::write(ACTIVE_PATH, name) {
        eprintln!("raid_policy_supervisor: failed to record active policy at {ACTIVE_PATH}: {err}");
    }
}

/// Reads the requested policy name, trimmed. `None` if the file is missing or empty (an UNSET
/// value, which the supervisor ignores — it keeps whatever is already running).
fn read_requested() -> Option<String> {
    let contents = fs::read_to_string(REQUEST_PATH).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn main() {
    eprintln!("raid_policy_supervisor: starting default policy '{DEFAULT_POLICY}'");
    let mut current_name = DEFAULT_POLICY.to_string();
    let mut child = spawn_policy(&current_name)
        .unwrap_or_else(|err| panic!("failed to start default policy '{DEFAULT_POLICY}': {err}"));
    set_active(&current_name);

    // Track the last value seen so unknown names are logged once, not every poll, and so writing the
    // already-active name doesn't trigger a needless swap.
    let mut last_seen = current_name.clone();

    loop {
        thread::sleep(POLL_INTERVAL);

        let Some(requested) = read_requested() else {
            continue;
        };
        if requested == last_seen {
            continue;
        }
        last_seen = requested.clone();

        if requested == current_name {
            // Re-affirming the running policy; nothing to do (active file already correct).
            continue;
        }

        if !policy_binary(&requested).exists() {
            // UNKNOWN name: log loudly and KEEP the current child. Do NOT fall back to a default.
            eprintln!(
                "raid_policy_supervisor: !!! UNKNOWN policy '{requested}' \
                 (no {POLICY_BIN_DIR}/raid_policy_{requested}); keeping '{current_name}' running !!!"
            );
            continue;
        }

        eprintln!("raid_policy_supervisor: swapping '{current_name}' -> '{requested}'");
        // Terminate-then-spawn: release the outgoing OQFS producer before the incoming one attaches.
        if let Err(err) = child.kill() {
            eprintln!("raid_policy_supervisor: failed to signal '{current_name}': {err}");
        }
        if let Err(err) = child.wait() {
            eprintln!("raid_policy_supervisor: failed to reap '{current_name}': {err}");
        }

        match spawn_policy(&requested) {
            Ok(new_child) => {
                child = new_child;
                current_name = requested;
                set_active(&current_name);
                eprintln!("raid_policy_supervisor: now running '{current_name}'");
            }
            Err(err) => {
                // Could not start the requested policy. Restart the previous one so I/O keeps being
                // served by a real policy rather than only the kernel's fallback.
                eprintln!(
                    "raid_policy_supervisor: failed to start '{requested}': {err}; \
                     restarting '{current_name}'"
                );
                child = spawn_policy(&current_name)
                    .unwrap_or_else(|err| panic!("failed to restart '{current_name}': {err}"));
                set_active(&current_name);
            }
        }
    }
}

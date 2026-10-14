// SPDX-License-Identifier: MPL-2.0

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::FileExt,
};

use oqfs_regression_tests::{BATCH_SIZE, check_newest_first, encoded_batch, encoded_trigger};

const WEAK_OBSERVE_PATH: &str = "/oqueues/oqfs_test/observe/weak_observe";
const TRIGGER_PRODUCE_PATH: &str = "/oqueues/oqfs_test/trigger/produce";
const MAX_READ_ATTEMPTS: usize = 100_000;

fn main() -> io::Result<()> {
    // Attach before triggering so the whole batch is retained in the snapshot.
    let observer = File::open(WEAK_OBSERVE_PATH)?;
    let mut trigger = OpenOptions::new().write(true).open(TRIGGER_PRODUCE_PATH)?;
    trigger.write_all(&encoded_trigger())?;
    drop(trigger);

    let mut buffer = vec![0; encoded_batch().len()];
    let mut bytes_read = 0;
    for _ in 0..MAX_READ_ATTEMPTS {
        // Weak observation is offset-0-only and non-consuming, so every retry must use `pread`.
        bytes_read = observer.read_at(&mut buffer, 0)?;
        if bytes_read == buffer.len() {
            break;
        }
    }

    if bytes_read != buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for {BATCH_SIZE} values (got {bytes_read})"),
        ));
    }
    check_newest_first(&buffer)?;

    println!("oqfs weak_observe test passed");
    Ok(())
}

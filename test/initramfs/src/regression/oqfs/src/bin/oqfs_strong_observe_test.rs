// SPDX-License-Identifier: MPL-2.0

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
};

use oqfs_regression_tests::{check_in_order, encoded_batch, encoded_trigger};

const STRONG_OBSERVE_PATH: &str = "/oqueues/oqfs_test/observe/strong_observe";
const TRIGGER_PRODUCE_PATH: &str = "/oqueues/oqfs_test/trigger/produce";
fn main() -> io::Result<()> {
    // Attach before triggering so the observer receives every value in the batch.
    let mut observer = File::open(STRONG_OBSERVE_PATH)?;
    let mut trigger = OpenOptions::new().write(true).open(TRIGGER_PRODUCE_PATH)?;
    trigger.write_all(&encoded_trigger())?;
    drop(trigger);

    let mut buffer = vec![0; encoded_batch().len()];
    observer.read_exact(&mut buffer)?;
    check_in_order(&buffer)?;

    println!("oqfs strong_observe test passed");
    Ok(())
}

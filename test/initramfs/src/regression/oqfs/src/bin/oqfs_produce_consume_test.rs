// SPDX-License-Identifier: MPL-2.0

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
};

use oqfs_regression_tests::{check_in_order, encoded_batch};

const PRODUCE_PATH: &str = "/oqueues/oqfs_test/io/produce";
const CONSUME_PATH: &str = "/oqueues/oqfs_test/io/consume";

fn main() -> io::Result<()> {
    let encoded = encoded_batch();

    // Values have nowhere to land unless a consumer is attached first.
    let mut consumer = File::open(CONSUME_PATH)?;
    let mut producer = OpenOptions::new().write(true).open(PRODUCE_PATH)?;
    producer.write_all(&encoded)?;
    drop(producer);

    let mut buffer = vec![0; encoded.len()];
    consumer.read_exact(&mut buffer)?;
    check_in_order(&buffer)?;

    println!("oqfs produce/consume test passed");
    Ok(())
}

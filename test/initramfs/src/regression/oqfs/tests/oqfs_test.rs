// SPDX-License-Identifier: MPL-2.0

use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
};

use oqfs_regression_tests::{check_in_order, encoded_batch, encoded_trigger};

const PRODUCE_PATH: &str = "/oqueues/oqfs_test/produce_consume/produce";
const CONSUME_PATH: &str = "/oqueues/oqfs_test/produce_consume/consume";
const STRONG_OBSERVE_PATH: &str = "/oqueues/oqfs_test/observe/strong_observe";
const TRIGGER_PRODUCE_PATH: &str = "/oqueues/oqfs_test/trigger/produce";

fn has_oqueues() -> bool {
    let filesystems = BufReader::new(File::open("/proc/filesystems").unwrap());
    filesystems
        .lines()
        .any(|line| line.is_ok_and(|line| line.contains("oqueues")))
}

#[test]
fn produce_consume() {
    if !has_oqueues() {
        return;
    }

    let encoded = encoded_batch();

    // Values have nowhere to land unless a consumer is attached first.
    let mut consumer = File::open(CONSUME_PATH).unwrap();
    let mut producer = OpenOptions::new().write(true).open(PRODUCE_PATH).unwrap();
    producer.write_all(&encoded).unwrap();
    drop(producer);

    let mut buffer = vec![0; encoded.len()];
    consumer.read_exact(&mut buffer).unwrap();
    check_in_order(&buffer);
}

#[test]
fn strong_observe() {
    if !has_oqueues() {
        return;
    }

    // Attach before triggering so the observer receives every value in the batch.
    let mut observer = File::open(STRONG_OBSERVE_PATH).unwrap();
    trigger_production();

    let mut buffer = vec![0; encoded_batch().len()];
    observer.read_exact(&mut buffer).unwrap();
    check_in_order(&buffer);
}

fn trigger_production() {
    let mut trigger = OpenOptions::new()
        .write(true)
        .open(TRIGGER_PRODUCE_PATH)
        .unwrap();
    trigger.write_all(&encoded_trigger()).unwrap();
}

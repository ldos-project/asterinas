// SPDX-License-Identifier: MPL-2.0

//! Shared values and checks for the OQFS regression tests.

use std::io;

use serde::{Deserialize, Serialize};

/// Number of values in one batch produced by the kernel test OQueue.
pub const BATCH_SIZE: usize = 16;

/// Returns the CBOR encoding of the values `0..BATCH_SIZE` in order.
pub fn encoded_batch() -> Vec<u8> {
    encode_values(0..BATCH_SIZE as u32)
}

/// Returns the CBOR encoding of the value that triggers kernel-side production.
pub fn encoded_trigger() -> Vec<u8> {
    encode_values([1])
}

fn encode_values(values: impl IntoIterator<Item = u32>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut serializer = minicbor_serde::Serializer::new(&mut bytes);
    for value in values {
        value
            .serialize(&mut serializer)
            .expect("encoding a u32 into a Vec cannot fail");
    }
    bytes
}

fn decode_values(bytes: &[u8]) -> Vec<u32> {
    let mut deserializer = minicbor_serde::Deserializer::new(bytes);
    let mut values = Vec::new();
    while deserializer.decoder().position() < bytes.len() {
        values.push(u32::deserialize(&mut deserializer).unwrap());
    }
    values
}

/// Checks that `bytes` contains the encoded batch in order.
pub fn check_in_order(bytes: &[u8]) {
    let values = decode_values(bytes);
    let expected = (0..BATCH_SIZE as u32).collect::<Vec<_>>();
    assert_eq!(values, expected);
}

/// Checks that `bytes` contains the encoded batch in newest-first order.
pub fn check_newest_first(bytes: &[u8]) {
    let values = decode_values(bytes);
    let expected = (0..BATCH_SIZE as u32).rev().collect::<Vec<_>>();
    assert_eq!(values, expected);
}

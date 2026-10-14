// SPDX-License-Identifier: MPL-2.0

//! Shared values and checks for the OQFS regression-test binaries.

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

fn decode_values(bytes: &[u8]) -> io::Result<Vec<u32>> {
    let mut deserializer = minicbor_serde::Deserializer::new(bytes);
    let mut values = Vec::new();
    while deserializer.decoder().position() < bytes.len() {
        values.push(u32::deserialize(&mut deserializer).map_err(io::Error::other)?);
    }
    Ok(values)
}

/// Checks that `bytes` contains the encoded batch in order.
pub fn check_in_order(bytes: &[u8]) -> io::Result<()> {
    let values = decode_values(bytes)?;
    if values.into_iter().eq(0..BATCH_SIZE as u32) {
        Ok(())
    } else {
        Err(io::Error::other(
            "values are not the expected batch in order",
        ))
    }
}

/// Checks that `bytes` contains the encoded batch in newest-first order.
pub fn check_newest_first(bytes: &[u8]) -> io::Result<()> {
    let values = decode_values(bytes)?;
    if values.into_iter().eq((0..BATCH_SIZE as u32).rev()) {
        Ok(())
    } else {
        Err(io::Error::other(
            "values are not the expected batch in newest-first order",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_checks_accept_expected_orders() {
        let in_order = encoded_batch();
        let newest_first = encode_values((0..BATCH_SIZE as u32).rev());

        assert!(check_in_order(&in_order).is_ok());
        assert!(check_newest_first(&newest_first).is_ok());
    }

    #[test]
    fn batch_checks_reject_wrong_orders() {
        let in_order = encoded_batch();
        let newest_first = encode_values((0..BATCH_SIZE as u32).rev());

        assert!(check_in_order(&newest_first).is_err());
        assert!(check_newest_first(&in_order).is_err());
    }

    #[test]
    fn batch_checks_reject_invalid_cbor() {
        assert!(check_in_order(&[0xff]).is_err());
        assert!(check_newest_first(&[0xff]).is_err());
    }
}

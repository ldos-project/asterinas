// SPDX-License-Identifier: MPL-2.0

#![allow(deprecated)]

use aes_gcm::{
    Aes128Gcm,
    aead::{AeadInPlace, Key, NewAead, Nonce, Tag},
    aes::Aes128,
};
use ctr::cipher::{NewCipher, StreamCipher};

use crate::InKernelFpuSection;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidInput,
    EncryptionFailed,
    DecryptionFailed,
}

pub fn aead_encrypt(
    _section: &InKernelFpuSection,
    input: &[u8],
    key: &[u8],
    iv: &[u8],
    aad: &[u8],
    output: &mut [u8],
) -> Result<[u8; 16], Error> {
    if key.len() != 16 || iv.len() != 12 || output.len() != input.len() {
        return Err(Error::InvalidInput);
    }

    let key = Key::<Aes128Gcm>::from_slice(key);
    let nonce = Nonce::<Aes128Gcm>::from_slice(iv);
    let cipher = Aes128Gcm::new(key);
    output.copy_from_slice(input);
    let tag = cipher
        .encrypt_in_place_detached(nonce, aad, output)
        .map_err(|_| Error::EncryptionFailed)?;
    Ok(tag.into())
}

pub fn aead_decrypt(
    _section: &InKernelFpuSection,
    input: &[u8],
    key: &[u8],
    iv: &[u8],
    aad: &[u8],
    mac: &[u8],
    output: &mut [u8],
) -> Result<(), Error> {
    if key.len() != 16 || iv.len() != 12 || mac.len() != 16 || output.len() != input.len() {
        return Err(Error::InvalidInput);
    }

    let key = Key::<Aes128Gcm>::from_slice(key);
    let nonce = Nonce::<Aes128Gcm>::from_slice(iv);
    let tag = Tag::<Aes128Gcm>::from_slice(mac);
    let cipher = Aes128Gcm::new(key);
    output.copy_from_slice(input);
    cipher
        .decrypt_in_place_detached(nonce, aad, output, tag)
        .map_err(|_| Error::DecryptionFailed)
}

pub fn skcipher_encrypt(
    _section: &InKernelFpuSection,
    input: &[u8],
    key: &[u8],
    iv: &[u8],
    output: &mut [u8],
) -> Result<(), Error> {
    skcipher_apply(input, key, iv, output)
}

pub fn skcipher_decrypt(
    _section: &InKernelFpuSection,
    input: &[u8],
    key: &[u8],
    iv: &[u8],
    output: &mut [u8],
) -> Result<(), Error> {
    skcipher_apply(input, key, iv, output)
}

fn skcipher_apply(input: &[u8], key: &[u8], iv: &[u8], output: &mut [u8]) -> Result<(), Error> {
    if key.len() != 16 || iv.len() != 16 || output.len() != input.len() {
        return Err(Error::InvalidInput);
    }

    let mut cipher =
        ctr::Ctr128LE::<Aes128>::new_from_slices(key, iv).map_err(|_| Error::InvalidInput)?;
    output.copy_from_slice(input);
    cipher.apply_keystream(output);
    Ok(())
}

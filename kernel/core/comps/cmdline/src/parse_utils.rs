// SPDX-License-Identifier: MPL-2.0

//! Functions for parsing parameter values which appear both in early and later parameters.

use ostd::log::LevelFilter;

/// Strips Linux-style surrounding quotes from a token or value.
pub(crate) const fn strip_linux_double_quotes(bytes: &[u8], start: usize, end: usize) -> &[u8] {
    let (new_start, new_end) = if start < end && bytes[start] == b'"' {
        let new_start = start + 1;
        if new_start < end && bytes[end - 1] == b'"' {
            (new_start, end - 1)
        } else {
            (new_start, end)
        }
    } else {
        (start, end)
    };
    bytes.split_at(new_start).1.split_at(new_end - new_start).0
}

/// Parses a `loglevel` value.
///
/// Accepts `0..=8` or lowercase level names. Returns `None` for malformed values so they do not
/// override earlier valid settings.
pub const fn parse_loglevel_at(bytes: &[u8]) -> Option<LevelFilter> {
    let bytes = strip_linux_double_quotes(bytes, 0, bytes.len());

    if bytes.is_empty() {
        return None;
    }

    if bytes[0].is_ascii_digit() {
        if let Ok(result) = u8::from_ascii_radix(bytes, 10)
            && result <= 8
        {
            return Some(LevelFilter::from_u8(result));
        }
        return None;
    }

    parse_loglevel_name_at(bytes)
}

/// Parses lowercase textual loglevel names.
const fn parse_loglevel_name_at(bytes: &[u8]) -> Option<LevelFilter> {
    match bytes {
        b"off" => Some(LevelFilter::Off),
        b"emerg" => Some(LevelFilter::Emerg),
        b"alert" => Some(LevelFilter::Alert),
        b"crit" => Some(LevelFilter::Crit),
        b"error" | b"err" => Some(LevelFilter::Error),
        b"warning" | b"warn" => Some(LevelFilter::Warning),
        b"notice" => Some(LevelFilter::Notice),
        b"info" => Some(LevelFilter::Info),
        b"debug" => Some(LevelFilter::Debug),
        _ => None,
    }
}

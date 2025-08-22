// SPDX-License-Identifier: MPL-2.0

// TODO(arthurp): Remove and fix the underlying issues.
#![expect(clippy::needless_borrows_for_generic_args)]
#![expect(clippy::needless_borrow)]
#![expect(clippy::map_flatten)]
#![expect(clippy::ineffective_open_options)]

//! This module contains tests that invokes the `osdk` binary and checks the output.
//! Please be sure the the `osdk` binary is built and available in the `target/debug`
//! directory before running these tests.

mod cli;
mod commands;
mod examples_in_book;
mod util;

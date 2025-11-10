// SPDX-License-Identifier: MPL-2.0

//! A minimal early-boot test kernel.
//!
//! This should include tests which cannot be ktests because they must executing before the ostd
//! kernel (or other components) are fully initialized.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use ostd::{
    arch::qemu::{QemuExitCode, exit_qemu},
    orpc::{framework::errors::RPCError, orpc_impl, orpc_server, orpc_trait},
    prelude::println,
};

/// The methods used for testing the ORPC framework in early-boot context.
#[orpc_trait]
trait TestTrait {
    fn f(&self) -> Result<usize, RPCError>;
}

/// A server implementing `TestTrait` for testing.
#[orpc_server(TestTrait)]
struct TestServer {}

#[orpc_impl]
impl TestTrait for TestServer {
    fn f(&self) -> Result<usize, RPCError> {
        Ok(42)
    }
}

#[ostd::main]
fn main() {
    println!("Early-boot tests...");

    let server = TestServer::new_with(|orpc_internal, _| TestServer { orpc_internal });

    assert_eq!(server.f().unwrap(), 42);

    println!("Tests passed.");

    exit_qemu(QemuExitCode::Success);
}

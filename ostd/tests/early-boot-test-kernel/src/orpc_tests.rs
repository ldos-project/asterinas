// SPDX-License-Identifier: MPL-2.0

use ostd::orpc::{errors::RPCError, orpc_impl, orpc_server, orpc_trait, path::Path};

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

pub(crate) fn test_early_boot_server() {
    let server = TestServer::new_with(Path::test(), |orpc_internal, _| TestServer {
        orpc_internal,
    });
    assert_eq!(server.f().unwrap(), 42);
}

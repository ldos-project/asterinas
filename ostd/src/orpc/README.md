# ORPC

This module contains a framework for Observable RPC, built on top of `OQueue`s. This enables
subsystems to produce and consume values, but also introduces observers, which can observe messages
without influencing consumers.

Using `ORPC` between subsystems will allow developers to write code without having to think about the
underlying transport abstractions, using ordinary traits and function calls which transparently
publish values to an `OQueue` when needed.

For an object to be an `OQueue`, it must implement `OQueue` trait.

To build an `ORPC` server, see the `orpc_macros` crate which provides an easy way to
annotate traits and structs implementing the trait to be automatically transformed to use `ORPC` as
the mechanism for calls.

# Some History of Mariposa

This provides some historical information about how Mariposa came to be. It is not documentation for
the current system.

## Why not a micro-kernel?

A micro-kernel design has several challenges:

**Performance:** Micro-kernels can be fast, but it requires very high-performance communication
primitives and limits how communication happens. For example, high-performance microkernels
generally don't support asynchronous messaging. They use "rendezvous channels" where send and
receive are a barrier that both threads must reach to continue. This is quite inflexible and does
not directly support observability any more than a method call.

**Modularity:** Micro-kernels do not generally create fine-grained modularity. For example, they
generally have a single server that represents the entire memory pager. This does not provide an
easy way to replace the paging policy. In theory, we could use lots of smaller servers, but this has
performance problems. Rust provides a language level memory safety which replaces some of the safety
features of a micro-kernel (though certainly not all).

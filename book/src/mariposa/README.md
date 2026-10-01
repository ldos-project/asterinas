<<<<<<< conflict 1 of 1
+++++++ otmstxwy a057c34a "Merged documentation" (no terminating newline)

# Mariposa

*(This document assumes the you have a basic understanding of Rust and kernel development. For
Mariposa specific or otherwise important terms, there is a [glossary](glossary.md).)*

Mariposa a variant of Asterinas and is the "clean-slate" kernel of the [LDOS project]. It is
designed as a tool for research and development. Specifically, for exploring new operating system
policies based on machine learning and AI.

Mariposa has three main design goals:

1. **Observability:** The kernel should expose a wide range of events and state for use by policies.
2. **Reconfigurability:** The kernel should allow components to be replace and reconfigured easily.
   To do this it should be modular and flexible, and provide ways to configure how components
   communicate and invoke one another.
3. **Features and performance:** The kernel should provide features required for benchmarks and
   research. It should also provide good enough performance to convince reviewers that our benchmark
   results are meaningful.

The first two goals imply a clean architecture. The last implies that we should take advantage of
any new features we can, so we should track upstream changes from the Asterinas team and be careful
not to introduce our own overheads.

The approach to Mariposa is pragmatic instead of ideal. The goal is to have a system that is as good
as we can, but which recognizes that our resources are limited and that our requirements do not
include an ideal system architecture or implementation.

[LDOS project]: https://ldos.utexas.edu/

## Our approach

Due to the realities of this project we have made some choices.

**We will be maintaining merge compatibility with upstream Asterinas, when possible.** The upstream
developers are rapidly adding new features and capabilities and we need to be able to take advantage
of those.

**We will not be rearchitecting the system into a microkernel.** This was discussed in the early
days of the project. However, this introduces significant additional effort without enough benefit
to our research goals. Also, rearchitecting would make our kernel, much less compatible with
upstream improvements.

**We will be using an [Object-Oriented approach].** This matches the upstream design of smaller
objects with well defined interfaces (Rust traits), as opposed to the larger object (e.i., server)
design of micro-kernels. It also matches the Linux design of values which have a defined interface
exposed by a set of functions or a function table (such as, VFS). Unlike Linux we will be
explicitly defining these interfaces such that multiple types can implement the same
interface.

**We use [OQueues] to expose events and state to policies (or other loosely coupled components).**
OQueues are very low-overhead, with a near zero cost when there are no observers. This makes it
possible to expose a large number of event and state with the cost only paid when there are actually
observers. Even with observers, the cost can be very low depending on the needs of the specific
observers. OQueue are also used for event driven or command queue interactions. For example, a
daemon which runs after some watermark is reached, or a cache which can receive prefetch requests
asynchronously.

**We use [slots] to provide modularity for more tightly coupled objects.** Generally, this is
objects whose methods must be invoked synchronously and quickly, such as an eviction policy that
needs to be run during out of memory situations. **TODO:** Needs to be defined better. 

**We rely on Rust's language level safety guarantees to provide safety and a limited form
security.** This guarantees that code that we build, and which does not contain `unsafe` code, will
not access memory without being passed a reference to it with methods that allow the access.
Security is not a specific focus of our work at the moment, so we do not try to solve these problems
fully.

[Object-Oriented approach]: objects.md
[OQueues]: oqueues.md
[slots]: slots.md

## Why not a micro-kernel?

A micro-kernel design has several challenges:

**Performance:** Micro-kernels can be fast, but it requires very high-performance communication
primitives and limits how communication happens. For example, high-performance microkernels
generally don't support asynchronous messaging. They use "rendezvous channels" where send and
receive are a barrier that both threads must reach to continue. This is quite inflexible and does
not directly support observability any more than a method call.

**Modularity:** Counterintuitively, micro-kernels do not generally create fine-grained modularity.
For example, they generally have a single server that represents the entire memory pager. This does
not provide an easy way to replace the paging policy. In theory, we could use lots of smaller
servers, but this has performance problems and may require co-locating servers (placing them in one
address space). In the extreme, this is object-oriented programming, so we just did that instead.
Rust provides a language level memory safety which replaces some of the safety features of a
micro-kernel (though certainly not all).

In the name of pragmatism, we (read: Arthur) have decided to follow the monolithic and generally object oriented
design of Asterinas. See [Objects in Mariposa](objects.md) for more discussion of objects and how we
use them in Mariposa specifically.

<hr/>


# Mariposa (OLDER VERSION)

## Overview

Mariposa is a modified version of [OSTD](../ostd/README.md) and [Asterinas](../kernel/README.md). It
has features and architectural changes specifically designed to support observability and policy
development. Mariposa is part of the [The Learning-Directed OS project](https://ldos.utexas.edu/)
with the goal of developing the next-generation Machine Learning-based Operating System to drive
computing infrastructure toward high efficiency and performance. 

## OQueues

(This is a high-level overview. See the [OQueues page](oqueues.md) for details.)

A core component of the Mariposa system are Observable Queues (OQueues). OQueues are a form of
concurrent queue which provides additional operations to enable observation of the values in the
queue. The operations supported on an OQueue are:

* **Produce**: Enqueue a value. Multiple threads can produce.
* **Consume**: Dequeue a value. Multiple threads can consume, and each value will be received by
  exactly one consumer.
* **Strong observe**: Observe the value in the OQueues as a stream. The values are not *consumed* in
  the above sense. Each strong observer receives *all* values.
* **Weak observe**: Get specific values from the history of values still available in the OQueue.
  When a weak observer tried to get a value, the value may already have been discarded.

Strong and weak observation are not available on standard queues. Strong observers and consumers can
cause the producer to block if they do not process values fast enough. However, weak observers can
never block the queue because they are not guaranteed to see every value.

All consumers and observers must *attach* before they can perform their operations. Consumers and
observers can never see or receive values that were produced before they attached. This means that
produce does not need to actually store values if there are no attached consumers or observers. This
is very important for OQueues used for observation as it dramatically reduces the cost of OQueues
which are not being observered.

**TODO(arthurp)**: This should be cleaned up to basically give a high level idea of what OQueues are
    and why we should care. It should not be more than a couple of paragraphs.

## ORPC

(This is a high-level overview. See the [ORPC page](orpc.md) for details.)

Observable RPC (ORPC) is a framework supporting defining interfaces between objects in Mariposa and
then observing the interactions of those objects. It follows the object-oriented design of Asterinas
with additional design requirements and patterns.

# Mariposa

*(This document assumes you have a basic understanding of Rust and kernel development. For
Mariposa specific or otherwise important terms, there is a [glossary](glossary.md).)*

Mariposa is a variant of Asterinas and is the "clean-slate" kernel of the [LDOS project]. It is
designed as a platform for exploring and deploying new operating system policies using machine
learning and LLMs. We specifically target data-rich bespoke policies (designed by humans or agents)

Mariposa has three main design goals:

1. **Observability:** The kernel should expose a wide range of events and state for use by
   policies.
2. **Reconfigurability:** The kernel should allow mechanisms and policies to be replaced and
   reconfigured easily.
3. **Features and performance:** The kernel should provide features required for benchmarks and
   research. It should also provide good enough performance to demonstrate our research.

The first two goals imply a clean architecture. The last implies that we should take advantage of
any new features we can, so we should track upstream changes from the Asterinas team and be careful
to maintain decent performance.

Our approach to Mariposa is pragmatic instead of ideal. The goal is to have a system that is as good
enough, but we recognize that our resources are limited. As such, we do not expect an ideal system
architecture or implementation.

[LDOS project]: https://ldos.utexas.edu/

## Our approach

Due to the realities of this project we have made some architectural choices: (Follow the links for
more information on each.)

[**We will be using an Object-Oriented approach.**](#mariposa-objects) This matches the upstream
design of smaller objects with well defined interfaces (Rust traits), as opposed to the larger
object design of micro-kernels. For example, an object might be the memory allocator and it may use
allocation policy and page allocator objects. This contrasts with a micro-kernel which will combine
all kinds of memory management mechanisms and policies into a single object. Our small object design
aligns with the Linux design of structs which have a defined interface exposed by a set of functions
or a function table (e.g., VFS). Unlike Linux we will be explicitly defining these interfaces such
that multiple types can implement the same interface.

[**We use Observable Queues to expose events and state to policies (or other loosely coupled
components).**](#observable-queues-oqueues) OQueues are low-overhead, with a near zero cost when
there are no observers. This makes it possible to expose a large number of events and state while
only paying the cost when it is used. OQueues are also used for event-driven or command queue
interactions.

[**We use dynamic dependency injection to provide reconfigurability.**](#slots) We call the
injection points [slots](#slots). Slots are implemented as a setter method which installs an object
implementing the trait associated with the slot. The installed object is then used to perform
operations, e.g., making a policy decision.

In addition, we have made some technical and logistical choices:

**We will be maintaining merge compatibility with upstream Asterinas.** The upstream developers are
rapidly adding new features and capabilities and we want to take advantage of those.

[**We will not be rearchitecting the system into a microkernel.**](history.md#why-not-a-micro-kernel)
See [History](history.md#why-not-a-micro-kernel).

**We rely on Rust's language level safety guarantees to provide safety and limited security.** This
guarantees that code that we build, and which does not contain `unsafe`, can only perform allowed
operations on values. This makes references act as capabilities.

## Observable Queues (OQueues)

(See the [OQueues page](oqueues.md) for details about the usage and capabilities of OQueues.)

A core component of the Mariposa system are Observable Queues (OQueues). OQueues are a form of
concurrent queue which provides additional operations to enable observation of the values in the
queue. The operations supported on an OQueue are:

* **Produce**: Enqueue a value. Multiple objects can produce, potentially concurrently.
* **Consume**: Dequeue a value. Multiple objects can consume, and each value will be received by
  exactly one consumer.
* **Strong observe**: Observe the values in the OQueue as a stream. The values are not *consumed*
  in the above sense. Each strong observer receives *all* values.
* **Weak observe**: Get historical values from the OQueue. The length of the available history is
  limited.

The basic usage of an OQueue is as follows. Note that manually producing into an OQueue will be
rare. Most observation will be done using one of the tracing macros. See
[Ad-hoc OQueues](#ad-hoc-oqueues) and [Method OQueues](#method-oqueues).

```rust
let queue = OQueueRef::new(64, path!(test.oqueue));
let producer = queue.attach_ref_producer()?;
let strong_observer = queue.attach_strong_observer(Projection::new(|e| e.x))?;
let weak_observer = queue.attach_weak_observer(Projection::new(|e| e.y))?;

producer.produce(&v);

let x = strong_observer.observe();
let ys = weak_observer.weak_observe_recent(10);
```

Strong observers and consumers can cause the producer to block if they do not process values fast
enough. However, weak observers can never block the queue because they are not guaranteed to see
every value. So values can be overwritten before a weak observer sees it. Strong observers can
attach "revocably", meaning that they can be detached if they fall behind and would cause a producer
to block.

All consumers and observers must *attach* before they can perform their operations, and can never
see values produced before they attached. This means that produce does not need to actually store
values if there are no attached consumers or observers. This is very important for OQueues used
for observation as it dramatically reduces the cost of OQueues which are not being observed.

> Note: Consumers and strong observers are extremely similar abstractly. However, consumers should
> be viewed as taking ownership of the message instead of just observing it.

### Ad-hoc OQueues

The `trace_structured_data!` macro creates an OQueue for observing information inline without any
external management code.

First, declare a type:

```rust
#[derive(Element, DefaultProjection)]
pub struct RssDeltaChangeEvent {
    increment: isize,
    current: usize,
}
```

Then create an ad-hoc OQueue using it:
```rust
trace_structured_data!(
    { mm.rss_delta.anon },
    RssDeltaChangeEvent { increment, current },
    RssDeltaChangeEvent
);
```

This will create an OQueue with path `mm.rss_delta.anon` and type `RssDeltaChangeEvent`.

Ad-hoc data collection creates a public API, so you should not expose internal data blindly. Doing
so encourages coupling between observers and observeree and creates maintainability problems.

### Projection

OQueues can abstractly carry any value, but many values cannot or should not be copied to pass the
value to observers. You can't just pass a reference to a value, because the value could be freed or
concurrently modified making referencing it in the observer unsafe.

A projection solves this problem by computing a safely sharable value for observers to receive
during the produce operation itself. This means that the projection function can have access to the
entire produced value regardless of it's ownership and lifetime. The projection produces a value
that can be copied as raw bytes. This allows messages to be stored and observed safely.

As an example, a page fault observer can project VM space ID. The value `event.vm_space` is a
`&Arc<VmSpace>` and so does not require copying the reference.

```rust
page_fault_oqueue.attach_weak_observer(
    Projection::new(|event: &PageFaultMessage| event.vm_space.id())
)
```

Elements may contain borrowed values. This lets an event refer to values available at the point
where it is produced without needing to copy them; importantly, a method's receiver and arguments.
The referenced values only need to be available for the length of the `produce` call.

Doing this correctly requires lifetime parametric types. The page fault element in the OQueues
chapter borrows a VM space:

```rust
#[derive(Element, DefaultProjection)]
pub struct PageFaultMessage<'a> {
    pub vm_space: &'a Arc<VmSpace>,
    pub fault_info: PageFaultInfo,
}
```

The `Element` trait, which is usually be generated with `#[derive(Element)]`, allows lifetime
parametric types to be the elements of OQueues. This also produces another time called a
"descriptor" (named `PageFaultMessageDescriptor` in this case). This type is needed due to the
specific of the Rust typesystem, and you should only need to think about it in specific cases where
you need to pass it as an explicit type parameter to some functions.

#### Filtering

Projections can also filter the selected values. For example, we can weakly observe only the
`fault_info` from a single VM space.

```rust
let selected_id = ...;

page_fault_oqueue.attach_weak_observer(
    Projection::new_filter(|event: &PageFaultMessage| {
        if event.vm_space.id() == selected_id {
            Some(m.fault_info)
        } else {
            None
        }
    })
)
```

*Future work:* Filtering from user-space will be important, but will require a way for user-space
observers to provide some form of projection, or at least parameters for projections.

#### Default projections

Many element types have a default projection that converts the type into some generally useful form.
This is used as the default form to sent to userspace for `OQFS` readers. The default projection can
be derived for your types using `#[derive(DefaultProjection)]` (or implemented yourself in special
cases). See [Default Projections](oqueues.md#default-projections) for more details.

### User-space OQueues via OQFS

OQueues are made available in the guest filesystem in `/oqfs`. This provides access to OQueues via a
set of files which can be read normally to get CBOR formatted records. OQueues can be exported to
userspace if they have `DefaultProjection`, because DefaultProjections are required to be
serializable to CBOR.

> Note: Because of this requirements OQueues with elements without a `DefaultProjection` need
> special handling and will need additional arguments to the tracing macros.

To access the OQueue `mm.rss_delta.anon`, you can access the directory `/oqfs/mm/rss_delta/anon`.
Depending on the exact OQueue this directory will contain files:

* `metadata.yaml`: Meta-information about the OQueue such as its time and potentially, documentation
  and in-memory layout.
* `produce`: Opening this attaches to the OQueue as a producer. Writing CBOR records with the
  correct format to it will produce that value on the OQueue.
* `consume` and `strong_observe`: Opening this will attaches to the OQueue as a consumer, or strong
  observer, resp.

**Future Work:** `weak_observer` and faster non-CBOR access are planned.

## Mariposa Objects

(This is a high-level overview. See the [objects page](objects.md) for details.)

Mariposa builds on the roughly object-oriented design of Asterinas, in which information and
behavior are combined into objects with interfaces defined by Rust traits. On top of this, "Mariposa
objects" are objects that have an identity (a unique ID) and expose their methods for observation:
each call and return is an event in an OQueue. These objects allow their interactions to be
observed, allows introspection into the system, and allows the system to be dynamically
reconfigured.

There is one core object-orient design principle that you should keep in mind:

Two objects which implement the same trait should be "**substitutable**", meaning that you should be
able to replace one with the other and have the system still work (potentially, with different
behavior). Substitutability is *critical* in the context of [slots](slots.md) and is a core
assumption for policy replacement. This requires good documentation describing what an
implementation of a trait must do.

### Method OQueues

We can add OQueues for the call and return of the `StreamSocket::connect` method as follows:

```rust
#[trace_methods]
struct StreamSocket {
    #[trace]
    fn connect(&self,
               #[trace(only(call))] socket_addr: SocketAddr
    ) -> Result<()> {
      ...
    }
}
```

This creates two types `ConnectCall` and `ConnectReturn`.

```rust
struct ConnectCall<'a> {
    self_: &'a StreamSocket,
    socket_addr: &'a SocketAddr,
}
struct ConnectReturn<'a> {
    self_: &'a StreamSocket,
    value: &'a Result<()>,
}
```

These hold the arguments and return values of the call. `socket_addr` would be included in the
return type unless it was disabled via the `only(call)` annotation. This is needed, because
`socket_addr` is no longer available when the function returns (ownership is passed to another part
of the system).

The OQueues are named based on the module path of the type and the method name. In this case,
`aster_core.net.socket.ip.stream.StreamSocket.connect.call` and `[...].return`. They can be accessed
using the same machinery used for any other OQueue. They will have `DefaultProjection`s in many
cases and will be available from user-space.

### Slots

Slots are a simple form of dynamic dependency injection that we use for installing policies and
connecting mechanisms. A slot has two parts:

1. A trait which defines the interface of objects installed into the slot.
2. A setter method used to install an object into the slot.

Any object which implements the trait can be installed in the slot. Substitutability is very
important to allow installation to work correctly.

The setter of a slot should have the `#[slot]` attribute. This serves to mark the intent of the
method and will eventually be used to generate information about all slots and their traits in the
system.

One example of a slot is a synchronous eviction policy used for out-of-memory situations.

```rust
// Part 1
/// An eviction policy which provides a set of items to be evicted when requested.
trait EvictionPolicy {
    /// Return a set of pages to evict. This is expected to use information from
    /// OQueues to make it's decision.
    fn select_range(&self, n: usize) -> Vec<usize>;
}
```

```rust
impl PageCachePolicy {
    // Part 2
    /// Install the eviction policy.
    #[slot]
    fn set_eviction_policy(&self, policy: Box<dyn EvictionPolicy>);
}
```

```rust
// Install an eviction policy
page_cache.set_eviction_policy(Box::new(LruEvictionPolicy::new()));
```

Slots are intended for direct, synchronous interactions, such as installing an eviction policy that
must run during an out-of-memory operation. They complement OQueues rather than replace them: slots
connect objects that must call one another immediately, while OQueues support asynchronous
observation and commands.

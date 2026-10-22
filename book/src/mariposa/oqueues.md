# OQueues

OQueues are the primary observation primitive in Mariposa. They provide a way to efficiently expose
state and events to other components. This information can be observed and used as features for
learned policies or any kind of policy inputs. OQueues can also be used for communication between
components when a command queue or channel is needed.

OQueues provide 4 operations, each of which requires the producer, consumer, or observer to
explicitly attach (i.e., register).

**Strong observers** receive every value in the OQueue exactly once as they are produced. This
provides a way to process the exact event stream. All strong observers receive all values, so strong
observation has "broadcast" semantics. This is useful for tracking precise information, for example,
maintaining a list of all in flight I/O requests. To allow this, strong observers are notified
whenever a new value is available.

**Weak observers** can observe a history of values in the OQueue on a best effort basis. The length
of the history must be specified when the observer registers, but is not guaranteed. This is useful
for accessing approximate recent information, for example, the last 5 I/O requests. Weak observers
are also notified when new information is available, but are not guaranteed to be notified of every
new value and the value may have been discarded before the observer runs.

**Producers** push values into the OQueue. This is the same as a producer on a typical queue. Once a
value is produced, it will be received by one consumer (if one exists) and all strong observers.

**Consumers** receive values from the OQueue as they are produced, with every message going to a
single consumer. This is the same semantics as receivers on typical multi-consumer queues. This is
similar to network "anycast". This is used when messages are requests and there are multiple clients
that can handle requests.

|                 | Message access semantics                   | Message ownership                                                             | Use                          |
|-----------------|--------------------------------------------|-------------------------------------------------------------------------------|------------------------------|
| Consumer        | One consumer receives each message         | Takes ownership of any objects in the message. Can receive non-copyable data. | Request handling             |
| Strong Observer | All strong observers receive each message  | Can only observe copyable data from the message.                              | Event observation            |
| Weak Observer   | Weak observers can view available messages | Can only observe copyable data from the message.                              | History or state observation |

All consumers and observers must *attach* before they can perform their operations. Consumers and
observers can never see or receive values that were produced before they attached. This means that
produce does not need to actually store values if there are no attached consumers or observers. This
is very important for OQueues used for observation as it dramatically reduces the cost of OQueues
which are not being observed. In fact, if there are neither strong observers nor consumers, the
producer can immediately overwrite any data, since producers never need to wait for weak observers.
Further, if there are no consumers not observers, produce is a no-op.

OQueues have paths to identify them, for example `mm.tlb.miss`. Paths are dot-separated and may
include optional numeric indices, for example `mm.tlb[0].miss`.

## Strong Observers vs Consumers

Strong observers and consumers are closely related, but different in an important way: Every strong
observer receives every message, whereas only one consumer receives any given message. The flow of a
given message is: (These operations are not actually ordered and can be concurrent.)

1. The message is produced into the queue.
2. All strong observers receive the message. Each strong observer will process the message
   separately and concurrently.
3. One consumer receives the message. If multiple consumers are waiting to receive a message,
   which one receives the message is arbitrary.

Strong observers are used to receive all messages on a queue without affecting other observers and
consumers, whereas consumers are used to accept and handle messages, treating them as requests that
need processing. For example, in a RAID1 configuration two disks may be able to handle the same
reads, both could be consumers on the same OQueue, to provide a simple form of fault tolerance,
while still having only one disk handle each request during normal execution. However, strong
observers on the read request OQueue would monitor all the requests without handling them.

The difference between strong observers and consumers is also useful when understanding systems
using OQueues. A consumer is assumed to be handling the messages and acting on them, potentially
replying to them.

> **Note:** If there is only a single consumer it will in fact behave the same as a strong observer,
> since it will not compete with any other consumers for messages and will simply receive all the
> messages itself. Consumers always transfer ownership, while observers never to, however.

## Buffering, Blocking, and Revocation

OQueues only keep messages for and wait for attached strong observers or consumers. If there are no
attached consumers, then messages can be discarded as soon as all the strong observers have seen
them. This is important, because real OQueues obviously have limited lengths, so producers may need
to wait if there is no space in the buffer. However, this only occurs if there are observers or
consumers which need to see the data. So, if there is no attached consumer, the OQueue can overwrite
data as soon as all of the strong observers have seen it.

Who can block the producer?

* **Weak observers** never block the producer, since they are not guaranteed to see every value.
* **Consumers** block the producer if they do not process values fast enough.
* **Strong observers** come in two flavors:
  * A strong observer attached with `attach_strong_observer` applies **backpressure**: if it falls
    behind, the producer blocks until it catches up.
  * A strong observer attached with `attach_revocable_strong_observer` is **automatically revoked**
    (instead of blocking the producer) when it falls behind. The observer is notified of the
    revocation when it next tries to observe, and it must re-attach explicitly to continue
    observing. Revocation is per-attachment: only the lagging observer is revoked, not all
    attachments.

Which flavor is appropriate depends on the observer. Revocable attachment is used for
userspace-facing observers, where kernel liveness must not depend on the reader keeping up.

Some OQueues do not allow certain attachments at all, to entirely eliminate the risk of produce
blocking. Attachment operations return `OQueueError::Unsupported` when the operation is not
supported (for example, a consumer on an observation-only OQueue) or is not allowed (for example,
a strong observer on an OQueue filled from a context which cannot block).

The buffer length of an OQueue is provided at creation time. If an observer repeatedly falls behind,
the buffer length may need to be increased.

*Future work:* Buffer size will be set automatically by default.

## Other OQueue Operations

OQueues provide several other operations and variants of the produce, consume, and observe
operations.

For each main operation there are:

* `try_*` operations which attempt the operation, but do not block.
* `can_*` operations which check if the operation *might* succeed. (This operation in inherently
  racy.)

For a full set of operations, see the Rust docs for the `OQueueBase`, `ConsumableOQueue`, and
`OQueue` traits.

## Elements

An OQueue element is the message type that producers place in a queue. Any type used this way must
implement `Element`. Deriving `Element` makes a type usable as an OQueue element; you do not need to
write the generated implementation yourself.

Lifetime-parametric elements are useful when an event should borrow values that are already
available at the call site. For example, the arguments to an instrumented method can be recorded
in an element that borrows `self` and other arguments, without copying them or requiring them to be
`'static`. The borrow is used while the event is produced; a projection can read it and return the
usual owned `Copy + Send` observation value. As a result, projection functions are mostly
transparent to the element's lifetime: they can work with a reference to the element as usual.

Method OQueues generate lifetime-parametric elements implicitly for method arguments and results
that carry borrows. For ad-hoc OQueues, you can declare a lifetime-parametric element yourself and
derive `Element` on it, as in the [page fault example](#page-fault-example).

## Projections

Abstractly any value can be placed in an OQueue. However, not all values can actually be stored for
observation. The observed value is extracted from the message by a *projection*. The result of the
projection must be `Copy + Send` regardless of the message type. This allows the observed value to
be efficiently observed by multiple strong and weak observers.

A projection is an `Projection`, a function which extracts the observable value from a message. The
query is run in the producing context, so it **must** be fast and side-effect free. There are three
ways to construct one:

* `Projection::new(f)` — `f` extracts a value from every message.
* `Projection::new_filter(f)` — `f` returns an `Option` and may discard some values, so they
  are not observed at all.
* `Projection::identity()` — for OQueues with a `Copy` element type, observes the whole
  message. Equivalent to `Projection::new(|x| *x)`.

**Future:** Today, a query is an arbitrary Rust closure. A more advanced query language — field
chains, filtering, aggregation, and even joins — is planned. By using such a language, the system
can perform query optimization across all the queries in the system. This would not be possible
with arbitrary functions.

### Default Projections

A *projection* selects a portion of an element or computes a value from it. The `DefaultProjection`
trait provides the default projection for a type:

```rust
pub trait DefaultProjection {
    type Projection: Copy + Send + Serialize + 'static;
    fn default_projection(&self) -> Self::Projection;
}
```

If the value will be exported to userspace, it
should also implement serde `Serialize` (the CBOR export uses it). The default projection is used
in cases where no other projection is selected. It is how the OQueue filesystem exposes data in an
OQueue, so all userspace accessible OQueues are required to have one.

Some references to [observable objects](objects.md) are projected as their ID. In particular,
`Arc<T: OObject>` implements `DefaultProjection` with `type Projected = ObjectId`. This widens
the set of object types with a derivable default projection, and provides enough information to
reconstruct a range of useful information about the object by joining information from multiple
OQueues.

Projections can use information provided by methods on the element, even if the element does not
actually store that information. The default projections of kernel events also include the current
time and the current task ID (an `EventContext`), which provides context information about the
event. This information does not need to be stored in the actual OQueue element type for this to
work, so this has no overhead on communication OQueues.

### Element descriptors

OQueue APIs use descriptors to identify element types independently of any particular borrow
lifetime. A descriptor represents the element type for any lifetime, which lets one OQueue accept
elements whose references have different lifetimes while still having one stable type identity.
For types without lifetime parameters, `Element` uses a `LifetimelessElementDescriptor`. For a
lifetime-parametric type, deriving `Element` also declares a descriptor named by appending
`Descriptor` to the element type's name. For example, `PageFaultMessage<'a>` uses
`PageFaultMessageDescriptor`; any type parameters other than lifetimes are carried by the
descriptor.

When referring to an OQueue by type, use its descriptor. For example, the page fault lookup uses
`PageFaultMessageDescriptor`:

```rust
let page_faults = registry::lookup_by_path::<PageFaultMessageDescriptor>(
    &path!(mm.page_fault),
);
```

In most code the descriptor is inferred from the element type or OQueue handle, but it may appear
in type annotations and registry lookups.

## Ad-hoc Data Collection

The simplest and most general way to make information observable is by creating an ad-hoc OQueue. In
the real system, this is used when you need an OQueue which is not associated with a method call.
Common cases for ad-hoc OQueues are:

* Subsystems exposing changes in their internal data structure. This is useful if the component does
  not make calls based on that change, or those calls do not effectively encode the information. For
  example, a page cache may not actually call any message when a hit occurs, but still wants to make
  it observable.
* Subsystems collecting information from hardware or otherwise non-observable software. For example,
  a performance counter.

Ad-hoc data collection creates a public API, so you should not expose internal data blindly and
instead provide an abstract interface for the data using traits to hide the implementation details
of the component. This reduces coupling between observers and observeree and improves
maintainability and flexibility of the system. For example, a scheduler which needs to expose
information about the running threads should expose a value with a trait type like
`dyn SchedulerState`, instead of exposing the concrete structure of the tables or trees used
internally.

## OQueues for Object Methods

Observable objects and their traits have public APIs and, as such, should be observable. This means
that a call OQueue and a return OQueue are created for each trait method. There is an OQueue for
each method, and the elements include the object ID. There is *not* an OQueue for each method on
each object.

> **Note:** You can view the OQueue here like a table in a relational database. Instead of having a
> separate table for each object, you have one table, and an ID column which connects each row to
> the appropriate object.

The call OQueue's element type is a struct generated from the arguments to the method (including a
reference to `self`). The return OQueue's element type is a struct generated from the return value
of the method. Both implement `DefaultProjection`, so they can be observed.

The OQueues are named with a dot-separated path, for example
`aster_core.net.socket.ip.stream.StreamSocket.connect.call` and `[...].connect.return`.

These OQueues can be created automatically with the `#[trace_methods]` attribute macro.
The attribute generates the element types and emits events when an instrumented method is called
and when it returns.

### Example

A currently existing example is the `connect` method of the IP stream socket.
The `Socket` trait declares the traced method and the generated element types:

```rust
#[trace_methods]
pub trait Socket: private::SocketPrivate + Send + Sync {
    #[trace]
    fn connect(&self, #[trace(only(call))] _socket_addr: SocketAddr) -> Result<()>;
}
```

The `StreamSocket` implementation reuses those types by naming their module:

```rust
#[trace_methods(types_module(crate::net::socket::socket))]
impl Socket for StreamSocket {
    #[trace]
    fn connect(&self, #[trace(only(call))] socket_addr: SocketAddr) -> Result<()> {
        // ...
    }
}
```

The `#[trace]` attribute selects `connect` for instrumentation. The `#[trace(only(call))]` attribute
includes `socket_addr` in the call event but not the return event. This is needed for cases where
the lifetime of an argument ends before the function returns, such as when a non-`Copy` value is
passed by value to a function.

### Page fault example

The virtual memory subsystem publishes successful page faults to `mm.page_fault`.
Its event type contains a reference to the VM space and the fault information:

```rust
#[derive(Element)]
pub struct PageFaultMessage<'a> {
    pub vm_space: &'a Arc<VmSpace>,
    pub fault_info: PageFaultInfo,
}
```

The page fault handler publishes the event with the default queue length:

```rust
trace_structured_data!(
    { mm.page_fault },
    PageFaultMessage {
        vm_space: &self.vm_space,
        fault_info: *page_fault_info,
    },
    PageFaultMessage
);
```

This creates an OQueue named `mm.page_fault` and publishes a `PageFaultMessage` after the fault is
handled successfully.

You can look up and use this OQueue as shown in [Page fault example, continued](#page-fault-example-continued).

## Finding OQueues

Policies can find OQueues by path (e.g., `mm.page_fault`) and type (e.g., `PageFaultMessage`) in an
OQueue registry. Once a policy has a reference to the OQueue is can attach with any projection it
wants and access any information available.

For userspace, the OQueue filesystem (`/oqfs`) exposes OQueues as files. To be exposed in OQFS, an
OQueue needs to have a `DefaultProjection`.

*Future Work:* We plan to add the ability to register an OQueue with multiple projections which can
be selected from userspace.

To access the OQueue `mm.rss_delta.anon`, you can access the directory `/oqfs/mm/rss_delta/anon`.
Depending on the exact OQueue this directory will contain files:

* `metadata.yaml`: Meta-information about the OQueue such as its time and potentially, documentation
  and in-memory layout.
* `produce`: Opening this attaches to the OQueue as a producer. Writing CBOR records with the
  correct format to it will produce that value on the OQueue.
* `consume` and `strong_observe`: Opening this will attaches to the OQueue as a consumer, or strong
  observer, resp. Strong observers attached this way are revocable and reads return an error once
  they are revoked, the user-program needs to close and reopen the file to reset it.

**Future Work:** `weak_observer` and faster non-CBOR access are planned.

### Page fault example, continued

You can use this to access the page fault information in the OQueue defined in [Page fault example](#page-fault-example),
above. Page It could retain a recent history containing only the VM space ID and fault address:

```rust
#[derive(Clone, Copy)]
struct PageFaultRecord {
    timestamp: Instant,
    vm_space_id: u64,
    address: Vaddr,
}

let page_faults = registry::lookup_by_path::<PageFaultMessageDescriptor>(
    &path!(mm.page_fault),
)
.expect("expected the page-fault OQueue to exist");

let observer = page_faults.attach_weak_observer(
    64,
    Projection::new(|event: &PageFaultMessage| PageFaultRecord {
        timestamp: aster_time::read_monotonic_time().into(),
        vm_space_id: event.vm_space.id(),
        address: event.fault_info.address,
    }),
)?;

let recent_faults = observer.weak_observe_recent(64)?;
```

Then, you can compute whatever statistics you want from the faults  including their timestamps.

## Inline Observers and Consumers

You can attach observers and consumers as functions which are called *during* the produce call (at
the same time projections are run). This has significant limitations and risks, because of when the
function is called. The function is limited to doing non-blocking operations. Notably, this prevents
publishing into another OQueue.

*Future work:* We plan to lift the limitation on publishing within an inline observer in the future.

Inline observers and consumers are useful when having a separate thread has too much overhead or
latency. Common examples are simple summarization, like running averages. They should be considered
a "trap-door" and only used as a last resort. This is because they have significant risks to system
performance an even liveness.

## Implementation and Performance

OQueues need only expose the expected interface, so they can be implemented multiple ways, however
the primary implementations are as follows.

The current OQueue implementation is a set of ring buffers protected by a spin lock. This isn't
ideal for a heavily contended OQueue, but still has low overhead for most cases. This makes strong
and weak observers very simple to implement because all state is protected by the same lock. In the
future, some OQueues will use a lock-free ringbuffer with additional capabilities.

For details see
[OQueue: Observable Communication in Learning Directed Operating Systems](https://doi.org/10.1145/3766882.3767174).

## FUTURE WORK

This page describes a future state of the code in some places. The following items are
**unimplemented**:

1. **Query language.** A richer query language — field chains, filtering, aggregation, and even
   joins — is planned. Today, queries are arbitrary Rust closures (`Projection`).

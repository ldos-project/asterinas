<<<<<<< conflict 1 of 1
+++++++ otmstxwy a057c34a "Merged documentation"
# OQueues

OQueues are the primary observation primitive in Mariposa. They provide a way to efficiently expose
state and events to other components. This information can be observed and used as features for
learned policies or any kind of policy inputs. OQueues can also be used for communication between
components when a command queue or channel is needed.

OQueues provide 4 operations, each of which requires the producer, consumer, or observer to
explicitly attach (e.i., register).

**Strong observers** receive every value in the OQueue exactly once as they are produced. This
provides a way to process the exact event stream. All strong observers receive all values, so strong
observation is “broadcast” semantics. This is useful for tracking precise information, for example,
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
similar to network “anycast”. This is used when messages are requests and there are multiple clients
that can handle requests.

OQueues only keep messages for and wait for attached strong observers or consumers. If there are no
attached consumers, then messages can be discarded as soon as all the strong observers have seen
them. This is important, because real OQueues obviously have limited lengths, so producers may need
to wait if there is no space in the buffer. However, this only occurs if there are observers or
consumers which need to see the data. So, if there is no attached consumer, the OQueue can overwrite
data as soon as all of the strong observers have seen it. In fact, if there are neither strong
observers nor consumers, the producer can immediately overwrite any data, since producers never need
to wait for weak observers.

User code will attach to OQueues as an observer (strong or weak) to collect the data they need for
training (for example, a data logger) or inference in a policy. The attachment does not just request
that a given OQueue capture data, but also specifies a subset of the data it wants. The subset is
specified as a set of OQueue expressions, as described in the previous section.

OQueues have paths to identify them, for example `mm.tlb.miss`.

|                 | Message access semantics                   | Message ownership                                                             | Use                          |
|-----------------|--------------------------------------------|-------------------------------------------------------------------------------|------------------------------|
| Consumer        | One consumer receives each message         | Takes ownership of any objects in the message. Can receive non-copyable data. | Request handling             |
| Strong Observer | All strong observers receive each message  | Can only observe copyable data from the message.                              | Event observation            |
| Weak Observer   | Weak observers can view available messages | Can only observe copyable data from the message.                              | History or state observation |


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

> **Note:** If there is only a single consumer it will in fact behave the same as a strong
> observer, since it will not compete with any other consumers for messages and will simply
> receive all the messages itself.

## Example

In the running example, there are two OQueues of interest: The read requests to the cache, and the
thread information from the scheduler. The read OQueue (`sda.cache.read_call`) contains the
arguments to each read call to the cache. The argument is a `Range` which is storable
(`Copy + Send`), so it can be observed directly (`self` means the OQueues value).

```rust
let reads = read_oqueue.attach_weak_observer(10, query!(self));
```

The scheduler information OQueue (`scheduler/scheduler_state`) contains a set of tasks with
information about each. This is not storable. The user can still observe useful information:

```rust
let load = scheduler_state_oqueue.attach_weak_observer(1, query!(self.ready_tasks.len));
```

(We only request a history of length 1 because we only care about the most recent value.)

## Observable Values

Abstractly any value can be placed in an OQueue. However, not all values can actually be stored for
later use (by a policy, for instance). Storable types are those that can be copied bit-for-bit and
remain safe (in Rust terms, `Copy + Send` types). To allow observing complex data that cannot be
stored, observers can provide a query to extract the information they want. 

Initially, this query will simply be a series of field/method invocations on the value in the
OQueue. However, this can be expanded to a more advanced language to allow for filtering.
Eventually, this may be part of a larger language that supports aggregations and even joins. By
using a query language, the system can perform query optimization across all the queries in the
system. This would not be possible with arbitrary functions.

Non-storable values placed in OQueues must implement traits which provide abstract access to useful
information. These trait methods are what the query is allowed to use. For example, scheduler
information is very complex and varies between scheduler implementation. However, most scheduler
information could provide implementations for a traits such as: 

```rust
#[observation_methods]
trait SchedulerState {
   fn ready_tasks(&self) -> &dyn TaskSet;
}
#[observation_methods]
trait TaskSet {
   fn len(&self) -> usize;
}
```

For a simple FIFO scheduler, this might be implemented just as it would be in Rust:  

```rust
struct FIFOQueue {
   tasks: Vec<Task>
}
impl SchedulerState for FIFOQueue {
   fn ready_tasks(&self) -> &dyn TaskSet { &self.tasks }
}
impl TaskSet for Vec<Task> {
   fn len(&self) -> usize { self.len() }
}
```

A policy can then access the data using an expression like `self.ready_tasks.len`, where self refers
to the value in the OQueue, and the rest of the elements are a chain of methods to call.  The
expression is not a Rust expression. It should be viewed as a simple query language. It is missing a
number of notable features:

Iteration. Instead, collections will need to provide summary methods directly.
???
In the future, LDOS and OQueues will be expanded to support a more sophisticated query language which addresses these and other issues.

## Integration with [Observable Objects](objects.md)

Observable objects and their traits have public APIs and, as such, should be observable. This means
that OQueues are implicitly created for calls and returns from all trait methods. There is a OQueue
for each method and the elements include the object ID. There is *not* an OQueue for each method on
each object.

> **Note:** You can view the OQueue here like a table in a relational database. Instead of having a
> separate table for each object, you have one table, and an ID column which connects each row to
> the appropriate object. (This is the isomorphism of relational and object-oriented data modeling,
> if you are feeling fancy)

The OQueues are named `module.path.trait.method_name_{call,return}`. The call OQueue’s type is a
struct generated from the arguments to the method. The return OQueue’s type is the return type of
the method.

## Ad-hoc Data Collection

Sometimes data needs to be collected that is not associated with a method call. Common cases are:

- Subsystems exposing changes in their internal data structure. This is useful if the component does
  not make calls based on that change, or those calls do not effectively encode the information. For
  example, a scheduler which maintains an internal priority queue, but only provides values from it
  on request.
- Subsystems collecting information from hardware or otherwise non-observable software. For example,
  a performance counter.

Importantly, ad-hoc data collection still creates a public API, so you should not expose internal
data blindly and instead provide an abstract interface for the data using traits to hide the
implementation details of the server. This reduces coupling between servers and improves
maintainability and flexibility of the system. For example, a scheduler which needs to expose
information about the running threads should expose a trait like `SchedulerState`, instead of
exposing the concrete structure of the tables or trees used internally.

To allow optimizations, ad-hoc collection is done with a macro which avoids computing the captured
values at all, unless they are required by an observer. In addition, servers are required to create
their OQueues when they start up. For example,

```rust
// During server setup.
let scheduler_state_oqueue = self.new_oqueue("scheduler_state");
// When data is available.
collect!(scheduler_state_oqueue, tasks);
```

## Finding OQueues

Policies need a way to find and access OQueues. OQueues can be found by path (like
`sda.cache/read`)
which is appropriate in cases where the interest is in the OQueue the observer has no interest in
the server itself. This requires that OQueues have well known names or at least names that match a
known structure. This will be a matter of convention, not defined by the framework. Server
references also provide a way to access their OQueues without having to manipulate paths.

Expanding the example of observation above:

```rust
let reads = find_oqueue("sda.cache/read_call").attach_weak_observer(10, query!(self));
let load = find_oqueue("scheduler/scheduler_state").attach_weak_observer(1, query!(self.ready_tasks.len));
```


## Implementation

### OQueues

OQueues need only expose the expected interface, so they can be implemented multiple ways,
however the primary implementation is as follows.

OQueues are generally implemented using a lock-free ringbuffer with a couple of additional
features over a typical multi-producer multi-consumer queue:

* To support strong observers, OQueues maintain additional head pointers representing the read
  position of the strong observers. This prevents producers (in our terms, senders) from overwriting
  data that has not been strongly observed. They also include the information required to wake
  strong observers when new data is available.
* To support weak observers, OQueues maintain enough information on each element for weak observers
  to safely read values in the entire ring buffer, even those which have already been
  consumed/observed, but have not yet been overwritten.

Especially with longer queue lengths the overhead of observers is quite small. For details see
the OQueue paper: OQueue: Observable Communication in Learning Directed Operating Systems
(PDF).

OQueues only store data which has been requested, often some subset of the information that they are
provided by the sender. OQueues also support structure-of-array storage for data types that
implement some specific traits. This is important to allow for only capturing part of the messages
without wasting space. How the data is selected was discussed previously.

### Performance

ORPC calls theoretically include the overhead of capturing the arguments and return values for
observability. However, in practice, this overhead is seldom actually present. We have two ways to
eliminate it when the information is not being actively observed. Dynamically calls and returns
include a check which skips any data capture which is not required by the set of registered
observers. This check is designed to be predicted correctly by the CPU in the no-observer case. It
will occupy a small amount of cache space and memory bandwidth, but this can be optimized by storing
the flags along with other server information.

When the set of observers is statically known, the checks and observation code can be omitted
entirely at compile time making the overhead effectively zero. This will be the case when the policy
configuration, and hence observer configuration, is fixed at build time. For example, when building
a kernel for deployment in a specific environment. This does not prevent reconfiguring policies or
retraining as long as those requirements are included when statically configuring the system.


## Projections

Elements in OQueues are complex objects and are not always cloneable or copyable. In addition, many
observers only need a portion of the data available in the OQueue (for example, only the timestamp
of I/O operation). As such, many observers will need to select part of the element or compute a
value from it. We call this a **projection** (a la relational algebra).

Projections are functions from the element type of the OQueue to some value that can be stored for
the observer to use later. To allow efficient implementations of OQueues, they are required to be
trivially copyable (i.e., [implement the `Copy`
trait](https://doc.rust-lang.org/std/marker/trait.Copy.html)). To allow the user of CBOR for
self-describing data capture, the projected type should also implement [the serde `Serialize`
trait](https://docs.rs/serde/latest/serde/trait.Serialize.html) if possible.

Many objects had a default projection which is used in cases where no other projection is selected.
These are the default projection when the OQueue is accessed from userspace (via OQFS) and all
user-space accessible OQueues are required to have one. All `Copy` types are projected as themselves
by default.

All references to [observable objects](objects.md) are projected as their ID. This includes, `&T`,
`&mut T`, `Box<T>`, and `Arc<T>`, where `T: OObject`. This applies to the `self` argument in
generated OQueue element types TODO: REFERENCE. This default projection provides enough information
to reconstruct a range of useful information about the object by joining information from multiple
OQueues.

Projections can use information provided by methods on the element, even of the element does not
actually store that information. The default projection of all elements also includes the current
time and the current task ID. This provides context information about the event. This information
does not need to be stored in the actual OQueue element type for this to work, so this has no
overhead on communication OQueues.





<hr/>





# Observable Queues (OLDER VERSION)

**TODO(arthurp)**: This should be a detailed, but accessible description of what OQueues. It should
    link to the in-source rustdocs for details of the practical API, usage, and implement of
    OQueues.

## Queries and projections

Queries allows selecting the information a user wants from the available OQueues. In it's full
generality this is a very sophisticated system. This is a matter of future work.

Due to the Rust safety model and the practical limitations of building a system, some level of query
is *required* even in a preliminary system. This is because all observed values must implement the
Rust `Copy + Send` traits and for performance we need to limit the amount of data we capture as much
as possible. 

To do this we support simple queries which we call "projections". Projections take a *reference* to
the value in the OQueue and must return a value which is `Copy + Send` which is actually observed.
Projections may also decide to discard some values, so they are not observed at all.

**TODO(arthurp)**: Complete

## Avoiding blocking while producing

Observation should not affect the behavior of the system. As such, observers are not allowed block
producers. If producing would block, all lagging observer attachments are revoked. After an observer
is detached, it will need to explicitly reattach.

If a producer produces values two quickly it may fill the buffer before the observer gets a chance
to catch up. This is addressed by setting and adjusting the buffer size. The initial buffer size is
provided at OQueue creation. Producers and consumers can also provide size requests when they
attach. The OQueue implementation will combine these to select an appropriate buffer size.

TODO(arthurp): This needs to be decided and written specifically.


Buffer sizes:

* Initial creation time buffer size: based on the knowledge of the component developer. It is a
  first guess, but probably not that accurate.
* Observer attach buffer size: based on the execution fr 

Buffer size 


, producing into an OQueue can wait for consumer and observers if the storage of the
OQueue is full. In contexts that cannot block (e.g., when preemption is disabled), this is
impossible. In other cases, it may be possible to give the observer a chance to process the value
(by scheduling it), but we still want to limit the time the observer takes to preserve liveness. 



There are various ways to handle this, but we implement two. The mode is chosen when the OQueue is
created.

If the producer would block, all lagging observer attachments are revoked. OQueues configured
   this way can be used in so-called "atomic-mode" (see
   [`ostd::task::atomic_mode`](../../../ostd/src/task/atomic_mode.rs)).

We also provide OQueues which do not allow strong observers at all, to entirely eliminate this
problem.

**TODO(arthurp)**: The below is more notes than real documentation.

2. If the producer would block, the system gives the observer a chance to run (by scheduling it) and
   if it is still lagging, revokes it's attachment. OQueues configured this way will *always* panic
   when used in atomic-mode, even if they did not block.


There are 3 general ways to handle this:

1. Discard the value being produced. This is only acceptable, when individual values are not
   important and the next value will provide the information, for instance, notifying of a value
   changing.
3. Delay producing the value to a point where publication is possible. This means values in the
   OQueues can be out of causal order, even in the same OQueue.
2. Revoke the attachment of any observers that have fallen behind and blocked the queue. This means
   that observers need to handle when they are revoked in some reasonable way, such as by
   reattaching after taking some action to reduce the chance of future overruns.

#2 requires a way to buffer publications, which can in theory overflow as well. However, the buffer
can be CPU local and only needs to hold the productions generated during the period while blocking
is disallowed. This means that the buffer is much much less likely to overflow because it does not
depend on things happening on other threads or CPUs.

<hr/>


# Observable Queues

**TODO(arthurp)**: This should be a detailed, but accessible description of what OQueues. It should
    link to the in-source rustdocs for details of the practical API, usage, and implement of
    OQueues.


## Queries and projections

Queries allows selecting the information a user wants from the available OQueues. In it's full
generality this is a very suffisticated system. This is a matter of future work.

Due to the Rust safety model and the practical limitations of building a system, some level of query
is *required* even in a preliminary system. This is because all observed values must implement the
Rust `Copy + Send` traits and for performance we need to limit the amount of data we capture as much
as possible. 

To do this we support simple queries which we call "projections". Projections take a *reference* to
the value in the OQueue and must return a value which is `Copy + Send` which is actually observed.
Projections may also decide to discard some values, so they are not observed at all.

**TODO(arthurp)**: Complete

### Default projections

```rust
trait OQueueMessage {
   type DefaultProjection: Copy + Sync;
   fn default_project(&self) -> Self::DefaultProjection;
}
```

The default projection of references to [ORPC Objects](./orpc.md) is their ID. This widens the set
of objects with derivable default projections by projecting references as a value that can be used
to corelate references to the same object.

```rust
impl<T: ORPCObject> OQueueMessage for Arc<T> {
   type DefaultProjection = Id;
   fn default_project(&self) -> Self::DefaultProjection {
      self.id()
   }
}
```

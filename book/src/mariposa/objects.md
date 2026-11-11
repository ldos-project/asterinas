# Objects in Mariposa

*(We assume you have a basic understanding of Rust. If not, you may want to look at parts of the
Rust book, such as [Defining Shared Behavior with Traits](https://doc.rust-lang.org/book/ch10-02-traits.html))*

Objects are data with attached behavior. In Rust, an object type is a `struct` with some methods
(defined using `impl`). This provides a nice way to encapsulate concepts like an array, or a lock.
However, we often want to express that we can accept any object which supports a specific interface.
In Rust, we use `trait`s which allow us to say that we accept any type which supports a specific set
of methods. This serves the same purpose as interfaces in Java or concepts in modern C++. Traits in
Rust are not types, they are a constraint on a type parameter. The code will say "I accept any type
A which implements trait T"; in Rust this is written `A: T`. Traits do not *require* dynamic dispatch,
so they can be used without performance overhead. However, traits do *support* dynamic dispatch
because Rust's `dyn` types can hold any value that implements a given trait.

> NOTE: There are many other ideas that are often associated with objects, such as inheritance and
> subtyping. However, they are not fundamental to what makes an object and Mariposa does not use
> them in the vast majority of cases.

These ideas may seem "high-level" for a kernel, but they are actually present to some extent even in
Linux. Most values in Linux have a struct (or other type) and an associated set of functions; these
are the data and methods of an object.

Asterinas, and by extension Mariposa, is a generally object-oriented system. Information and
behavior is combined into objects. Static and dynamic dispatch are handled via traits, to provide
modularity and abstraction. This provides separation of concerns which makes it easier to modify and
reconfigure.

For example, the scheduler's CPU-selection policy can be represented as an object implementing this
trait: (This is not the exact definition in the real system.)
```rust
trait SelectCpuPolicy {
    fn select_cpu(&self, task: &Arc<Task>, input: &SelectCpuInput) -> CpuId;
}
```
Then, an implementation of this can be passed to the scheduler using a policy injection function:
```rust
fn inject_select_cpu_policy(policy: Arc<dyn SelectCpuPolicy>);
```
In most cases, the injection will be via a method on an object instead of a global function. One of
the things we are likely to modify in Asterinas is to add these traits and injection points. For
more discussion of this pattern see, [slots].

> NOTE: Historically, Mariposa had the concept of ["Observable RPC" ("ORPC")](history.md#observable-rpc-orpc)
> as part of the micro-kernel style design. With the move away from a micro-kernel design, "RPC"
> became a misnomer, so we moved to just calling them objects. However, `orpc` still appears in some
> parts of the API.

## Mariposa objects

> Note: As of writing, Mariposa objects exist as a set of conventions. Once their usage has become
> clear we will introduce utilities, such as macros, to make the patterns easier to implement
> correctly. This section has comments in the form *Currently: ...* to describe the current way
> things are implemented.

Mariposa objects are objects with three additional properties:

* It has "identity," meaning that you can get a unique ID for the object to identify it within the
  system. *Currently: In at least some cases the pointer to an object is used as it's ID.*
* Its methods are observable via OQueues. Meaning that each call and return is exposed as an event
  on an OQueue. See [OQueues for Object Methods](oqueues.md#oqueues-for-object-methods) for details.
* If an object depends on another object it uses [slots] to allow other code to inject or change
  that object. All policies should be injected this way.

### Identity and metadata

**Currently:** This information is not available. Some objects may provide an `id()` method, but there
is no trait to create a generalized interface.

Objects have an ID to enable differentiating and correlating observations from different objects.
For example, the block device read OQueue is a mix of reads from many devices. The IDs allow for
separating and filtering reads as well as correlating reads with writes to the same device.

The proposed interface is shown here as `OObject`:
```rust
pub trait OObject: Any + 'static {
    /// Return the global ID of `self`. This value will be unique among all objects in the system.
    fn id(&self) -> ObjectId;

    /// Get metadata about `self`.
    fn metadata(&self) -> &dyn Metadata;
}
```

`ObjectId` is an identifier. The `Metadata` trait provides the object's `type_name()` and `TypeId`.

### Observing object methods

A call OQueue and a return OQueue are created for each trait method. This makes interactions between
objects observable. Other code can use these OQueues to observe interactions between components via
this method. The OQueues are per method, not per object; as such, they contain information to
identify the callee object. See [OQueues for Object Methods](oqueues.md#oqueues-for-object-methods)
on the OQueues page for details and an example.

### Documentation

The signatures of methods in a trait and the implementations of objects do not provide enough
information to really understand how traits and objects should be implemented and used. We need
English documentation to augment the Rust code with that information.

Mariposa will expose this documentation as part of the system reflection interface. This will allow
people and agents which are reconfiguring the system to understand what specific traits do. This
enables selecting and injecting implementations, or implementing traits correctly.

Traits should have documentation explaining what they represent and any requirements for an
implementation to follow. Method documentation should specify that the method does abstractly, in
the terms of the trait, not any specific implementation. Together the documentation should describe
the contract between the user and the implementation of the trait.

For example, `SelectCpuPolicy::select_cpu` specifies what is expected of the implementation:
```rust
/// A CPU selection policy for use by the scheduler.
trait SelectCpuPolicy {
    /// It is called with local IRQs disabled, possibly in interrupt context, and with no run-queue
    /// lock held. It must not sleep, block or allocate. Any state the policy needs lives in the
    /// policy object. The returned CPU should be in the thread's affinity mask.
    fn select_cpu(&self, task: &Arc<Task>, input: &SelectCpuInput) -> CpuId;
}
```

Types which implement traits (usually `struct`s) should be documented to describe how they will
behave. This will be used when selecting the appropriate implementation while configuring the
system.

### Object-oriented design

The general principles of Object-oriented design should be followed for OObjects. One is
specifically important:

**Substitutability**: Two objects which implement the same trait should be "substitutability",
meaning that you should be able to replace one with the other and have the system still work
(potentially, with different behavior). Substitutability is *critical* in the context of [slots] and
is a core assumption for policy replacement.

### Pragmatism

These rules will not always be applied strictly, and various "escape hatches" will be provided.
These will be used in a few cases:

* When upstream Asterinas has is poorly designed and improving that design is not worth it compared
  to the cost of refactoring.
* Then strict usage of these principles would create an unacceptable performance overhead. (This
  will generally need to be justified by benchmarks.)

[slots]: slots.md

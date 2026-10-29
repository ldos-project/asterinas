# Objects in Mariposa

*(We assume you have a basic understanding of Rust. If not, you may want to look at parts of the
Rust book, such as [Defining Shared Behavior with Traits](https://doc.rust-lang.org/book/ch10-02-traits.html))*

Objects are data with attached behavior. In Rust, an object type is a `struct` with some methods
(defined using `impl`). This provides a nice way to encapsulate concepts like an array, or a lock.
However, we often want to express that we can accept any object which supports a specific interface.
In Rust, we use `trait`s which allow us to say that we accept any type which supports a specific set
of methods. This serves the same purpose as interfaces in Java or concepts in modern C++. Traits in
Rust are not types, they are a constraint on a type parameter. The code will say "I accept any type
A which implements trait T"; in Rust this is written `A: T`. Traits do not imply dynamic dispatch,
so they can be used without performance overhead in many cases. In these ways, traits are much the
same as concepts in C++. However, traits do *support* dynamic dispatch because Rust's `dyn` types
can hold any value that implements a given trait without needing to statically know its concrete
type.

> NOTE: There are many other ideas that are often associated with objects, such as inheritance and
> subtyping. However, they are not fundamental to what makes an object and Mariposa does not use
> them in the vast majority of cases.

These ideas may seem "high-level" for a kernel, but they are actually present to some extent even in
Linux. Most values in Linux have a struct (or other type) and an associated set of functions; these
are the data and methods of an object. 

Linux uses tables of functions to dynamically select behavior (e.g., the VFS layer which supports
multiple filesystems); this is dynamic dispatch. Linux lacks static dispatch (because it does not
have type parameters).

Asterinas, and by extension Mariposa, is a generally object-oriented system. Information and
behavior is combined into objects. Static and dynamic dispatch are handled via traits, to provide
modularity and abstraction. This provides separation of concerns which makes it easier to modify and
reconfigure.

One of the things we are likely to modify in Asterinas is to inject new traits and abstractions to
allow changing components that were not configurable in the upstream code. For example, if upstream
introduced a fixed LRU cache eviction policy to a page cache, we may need to add a new abstraction
(via a trait) which allows replacing the LRU policy with some other policy. This is the
motivating example for [slots].

> NOTE: Historically, Mariposa had the concept of "Observable RPC" ("ORPC") as part of the
> micro-kernel style design. An ORPC object was a "server" that isolated faults and could have an
> associated thread. With the move away from a micro-kernel design, "RPC" became a misnomer, so we
> moved to just calling them objects.

## Mariposa objects

> Note: As of writing, Mariposa objects exist as a set of conventions. Once their usage has become
> clear we will introduce utilities, such as macros, to make the patterns easier to implement
> correctly. This section has comments in the form *Currently: ...* to describe the current way
> things are implemented.

Mariposa objects are objects with three additional properties:

* It has "identity," meaning that you can get a unique ID for the object to identify it within the
  system. "Objects" like a number or a lock which carry information and have methods, but do not
  have persistent identity. *Currently: In at least some cases the pointer to an object is used as
  it's ID.*
* Its methods are observable via OQueues. Meaning that each call and return is exposed as an event
  on an OQueue. See [OQueues for Object Methods](oqueues.md#oqueues-for-object-methods) for details.
* If an object depends on another object it uses [slots] to allow other code to inject or change
  that object. All policies should be injected this way.

Mariposa objects serve an important role: They are the thing that "act" or is "acted on" in the
system. This means that most events will have one or more object IDs in them.

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

`ObjectId` is a `Copy` identifier which can be assigned from a global allocator (`ObjectId::new()`),
or derived from component information (`ObjectId::new_tagged(tag)`, for example, an inode number).
The `Metadata` trait provides introspection: the object's `type_name()` and `TypeId`.

References to Mariposa objects are projected as their ID: `Arc<T: OObject>` implements
`DefaultProjection` with `type Projected = ObjectId`. This provides enough information to
reconstruct a range of useful information about the object by joining information from multiple
OQueues. See [Projections](oqueues.md#projections) on the OQueues page.

### Observing object methods

*Currently: Object methods are treated like any function and need to be manually marked as
observable*

A call OQueue and a return OQueue are created for each trait method. This makes interactions between
objects observable. Other code can use these OQueues to observe interactions between components via
this method. The OQueues are per static method, not per object; as such, they contain information to
identify the callee object. See [OQueues for Object Methods](oqueues.md#oqueues-for-object-methods)
on the OQueues page for details and an example.

### Documentation

The signatures of methods in a trait and the implementations of objects do not provide enough
information to really understand how traits and objects should be implemented and used. We need
English documentation to augment the Rust code with that information.

Mariposa will expose this documentation as part of the system reflection interface. This will allow
people and agents which are reconfiguring the system to understand what specific traits and
implementing objects do. This enables selecting and injecting implementations, or implementing
traits correctly.

Traits should have documentation explaining what they represent and any invariants an implementation
is required to follow. Method documentation should specify that the method does abstractly, in the
terms of the trait, not any specific implementation. Together the documentation should describe the
contract between the user and the implementation of the trait. This is a standard object oriented
software engineering practice.

Types which implement traits (usually `struct`s) should be documented to describe how they will
behave. This will be a refinement of the trait behavior because of the requirements of
substitutability (see below).

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

# Objects in Mariposa

*(We assume the you have a basic understanding of Rust. It not you may want to look at parts of the
Rust book, such as [Defining Shared Behavior with
Traits](https://doc.rust-lang.org/book/ch10-02-traits.html))*

Objects are data with attached behavior. In Rust, an object type is a `struct` with some methods
(defined using `impl`). This provides a nice way to encapsulate concepts like an array, or a lock.
However, we often want to express that we can accept any object which supports a specific interface.
In Rust, we use `trait`s which allow us to say that we accept any type which supports a specific set
of methods. This serves the same purpose as inferfaces in Java or concepts in modern C++. Traits in
Rust are not types, they are a constraint on a type parameter. The code will say "I accept any type
A which implements trait T"; in Rust this is written `A: T`. Traits do not imply dynamic dispatch,
so they can be used without performance overhead in many cases. In these ways, traits are much the
same as concepts in C++. However, traits do *support* dynamic dispatch because Rust's `dyn` types
which can hold any value that implements a given trait without needing to statically know it's
concrete type.

To summarize: In Rust, object types are `struct`s with methods (via `impl`) and interfaces are
`trait`s. Static dispatch is handled by placing bounds on type parameters: `A: T`. Dynamic dispatch
is handled using `dyn` types: `Box<dyn T>`. This is ignoring a number of details, so as to provide a
rapid overview of how to think about objects in the context of a Rust kernel.

There are many other ideas that are often associated with objects, such as classes and subtyping.
However, they are not fundimental to what makes an object. Rust does not support inheritance or
subtyping, though both can be emulated when needed.

These ideas may seem "high-level" for a kernel, but they are actually present to some extent even in
Linux. Most values in Linux have struct (or other type) and an associate set of functions; these are
the data and methods of an object. Also, Linux uses tables of functions to dynamically select
behavior (such as for the VFS layer which support multiple filesystems); this is dynamic dispatch.
Linux lacks static dispatch (because it does not have type parameters).

Asterinas, and by extension Mariposa, is a generally object-oriented system. Information and
behavior is combined into objects. Static and dynamic dispatch are handled via traits, to provide
modularity and abstraction. This provides separation of concerns which makes it easier to modify and
reconfigure.

One of the things we are likely to modify in Asterinas is to inject new traits and abstractions to
allow changing components that were not configurable in the upstream code. For example, if upstream
introduced a fixed LRU cache eviction policy to a page cache, we may need to add a new abstraction
(via a trait) which allows replacing the LRU policy with some other policy.

## Observable Objects (OObjects)

Observable Objects are objects with a couple of additional properties:

* It has "identity," meaning that you can get a unique ID for the object to identify it within the
  system. (Having identity like this is different from "objects" like a number or a lock which carry
  information and have methods, but do not have persistant identity.)
* Its methods are observable via OQueues. Meaning that each call and return is exposed as an events
  on an OQueue.

**TODO(amp):** Are observable object *by definition* not Copy? It would make sense if we say that
    have identity. Can they be Clone? If so, does that imply a way "fork" object identity?

Observable Objects serve an important role: They are the thing that "act" or is "acted on" in the
system. This means that most events will have one or more object IDs in them.

**TODO(amp):** ID relationships via separate OQueues.


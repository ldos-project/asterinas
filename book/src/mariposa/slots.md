# Slots

Slots provide modularity for objects which call one another directly. This can be mechanisms
interacting, or policies which are called from the mechanism. For example, an eviction policy that
needs to be run directly during out of memory situations.

## Design

Slots form a dependency injection framework with three properties:

1. **Both sides are known to the system.** The API of the object to be injected, and the point where
   it is injected, are both part of the structure of the system, not private to the types that use
   them.
2. **Both sides are discoverable and documented.** Each side is declared with an annotation, and the
   documentation is taken from the declaration itself.
3. **Injection is dynamic.** The dependency is installed with a setter after the object is
   constructed, not with a construction argument.

> **Slots are orthogonal to [OQueues].** OQueues are an asynchronous channel for observation and
> commands; slots are dependency injection.

## Concepts

**Provider Interface**: The interface of the object to be injected. This is a Rust `trait`, following the
object-oriented approach described in [Objects in Mariposa](objects.md).

**Provider**: The object to be injected. Being a provider is defined purely by implementing the
correct trait. A provider can be annotated with an attribute to make it discoverable; the
documentation is taken from the `impl` itself. In the initial model this is a `dyn` type (a trait
object). The injected value does not always need to be a `dyn` type: it can be a concrete type that
is passed and stored by value.

**Slot**: The point where a provider is injected. A slot is a setter method in the trait, annotated
to mark it as an injection point. All of the meta information about a slot can be taken from the
definition of the setter (the argument type is the API that a provider must implement), and the
documentation from the documentation of the method.

## Defining a Slot

A slot is defined by adding an annotated setter to a trait.

```rust
pub trait PageCachePolicy {
    /// Install the eviction policy used by the page cache.
    #[slot]
    fn set_eviction_policy(&self, policy: Box<dyn EvictionPolicy>);
}
```

The attribute marks the method as an injection point. The API of the object to be injected is the
argument type: anything that implements `EvictionPolicy` can be injected. No registry or separate
metadata declaration is needed; the trait is the definition that the system knows.

**Currently:** `#[slot]` does not do anything except to document the intent of the setter. It will
do more in the future.

## Implementing a Slot

A type which implements a trait with a slot and implements the slot manually: a field to hold the
provider, the setter, and a getter. The getter is often internal, used by the object itself, and is
not part of the trait.

```rust
struct PageCache {
    eviction_policy: Arc<dyn EvictionPolicy>,
}

impl PageCachePolicy for PageCache {
    fn set_eviction_policy(&self, policy: Box<dyn EvictionPolicy>) {
        // Replace `self.eviction_policy` with `policy`.
    }
}
```

The setter can also be an inherent method on a type. This reduces flexibility somewhat, but is
generally reasonable.

## Example

The page cache eviction policy is the motivating example, also used in [Objects in
Mariposa](objects.md). We want to be able to inject eviction policies. An eviction policy is an
object that implements the `EvictionPolicy` trait, with the methods that the page cache invokes
synchronously during out of memory situations. An LRU policy would be a type that implements this
appropriately.

The page cache has a method `set_eviction_policy(&self, Box<dyn EvictionPolicy>)` which installs a
new policy. The policy can be swapped by calling the method again.

## Related work

* The function pointer tables of Linux are a precedent for a known API with an injection point: the
  type of the table (such as `struct file_operations`) fixes the API, a given table is the provider,
  and the kernel installs the table in an object which then dispatches through it. However, unlike
  object-oriented dependency injection, "struct-ops" in Linux, do not include self in the function
  arguments, so the injected code cannot use any custom state or even configuration.

[OQueues]: oqueues.md

# Observable Remote Procedure Calls (ORPC)

**TODO(arthurp)**: Overall, needs work.

The ORPC framework supports defining interfaces between objects in Mariposa and then observing the
interactions of those objects. It follows the object-oriented design of Asterinas with additional
design requirements and patterns.

*NOTE: The term "RPC" here is a bit of a misnomer. The calls are simple method calls. However, the
concept of separate communicating modules is applicable and useful to guide us toward meaningful
observable calls. Also, remote access stubs to move objects into userspace are likely to exist.*

ORPC has three main goals:

1. Allow interactions between objects to be observed.
2. Allow introspection into the system.
3. Allow component implementations to be swapped; dynamically when possible.

Realization of these goals will sometimes be limited by practical constraints, but these provide
guidance.

## Observability

ORPC provide observability by implicitly creating a call and a return OQueue for each method. Other
code can use these OQueues to observe interactions between components via this method. 

**TODO(arthurp)**: Complete

The OQueues are per static method, not per object. As such, they contain information to identify the
callee object.

```rust
mod module {
    trait Trait {
        fn method(&self, Args...) -> R;
    }
}
```

```rust
struct Args<'a> {
    self_: &self,
    arg1: &'a T1,
    ...
}

struct ArgsDefaultProjected {
    self_: SelfType::DefaultProjection,
    arg1: T1::DefaultProjection,
    ...
}

impl<'a> OQueueMessage for Args<'a> {
   type DefaultProjection = ArgsDefaultProjected;
   fn default_project(&self) -> Self::DefaultProjection {
      ArgsDefaultProjected {
        self_: self.self_.default_project(),
        ...
      }
   }
}
```

```rust
impl module::Trait for S {
    fn method(&self, Args...) -> R {
        structured_data_trace!(module.trait.method.call, ???)
        body...
        structured_data_trace!(module.trait.method.return, ???)
    }
}
```

## Identity and metadata

ORPC objects have ID to enable differentiating and corolating observations from different objects.
For example, the block device read OQueue is a mix of reads from many devices. The IDs allow for
separating and filtering reads as well as corolating reads with writes to the same device.

**TODO(arthurp)**: Complete

All objects have an ID which can either be assigned from a global allocator or based on some
component information. For instance, a filesystem would be allocated an ID from the allocator, and
then it's inodes would have IDs based on their inode number.

All objects also have metadata associated with them to allow for introspection:

```rust
trait ORPCObject {
    fn id(&self) -> Id;
    fn metadata(&self) -> &dyn ORPCObjectMetadata;
}
```

## Substitutability

Mariposa needs to be modular to support our research goals. Most importably, allowing policies to be
exchanged. This requires policies to provide a consistent interface to the mechanism that use them
and for mechanism to call them in useful and consistent ways.

**TODO(arthurp)**: Complete

The pattern is for objects which need a policy or other attached object to provide a method:

```rust
fn set_object(&self, obj: Box<dyn ObjectType>) -> Result<()>
```

This method provide an object to use in the future. For example, `set_eviction_policy` would take an
eviction policy and use it for future evictions.



*TODO**: Object will often have fallback policies, either provided in some way or built-in. This is
critical for liveness if a policy fails and for reliable execution during policy reconfiguration.

Many objects within components in Asterinas and therefore Mariposa are tightly coupled.
Specifically, they refer to one another by their concrete type. These objects are not substitutable.

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

## Identity and metadata

ORPC objects have ID to enable differentiating and corolating observations from different objects.
For example, the block device read OQueue is a mix of reads from many devices. The IDs allow for
separating and filtering reads as well as corolating reads with writes to the same device.

**TODO(arthurp)**: Complete

## Substitutability

Mariposa needs to be modular to support our research goals. Most importably, allowing policies to be
exchanged. This requires policies to provide a consistent interface to the mechanism that use them
and for mechanism to call them in useful and consistent ways.

**TODO(arthurp)**: Complete
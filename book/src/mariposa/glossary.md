# Glossary

***Client***: A component which holds a reference to another object (or an OQueue) so it can call
its methods.

***Consume***: To take a message out of an OQueue or "receive" it taking ownership of it. Only one
consumer can consume a given message. This is "exactly once to exactly one" semantics. Consuming a
message does not prevent strong observers from observing it.

***Message***: A value placed in an OQueue. These are often events for observation or commands
between components. Also called an "element".

***Metadata plane***: A transport mechanism for metadata (information about the actual data and
execution in the system) throughout the system. This is distinct from the control plane in that it
carries information about events and state, not commands or other control signals.

***Method***: A method on a trait. This is identical to the usage in both Rust and object-oriented
uses of the term.

***Observability***: Something is observable if the actions it performs, its internal events, or its
state can be viewed from outside. This can include anything from viewing the allocation events
within the kernel to application request processing.

***Mariposa Object***: An object with an identity (a unique ID within the system) whose methods
are observable. See the [objects page](objects.md).

***Observe***: To strongly or weakly observe something on an OQueue.

***OQueue***: An Observable Queue. An abstract type (it may be implemented in multiple
ways) which allows both message-based communication, by sending and receiving, and observation of
those messages, both strongly and weakly.

***Produce***: To put a value into an OQueue or "send" it. Once a message is produced, at most one
consumer and all strong observers will see it.

***Provider***: A component that implements some interface that can be used by some client. This is
an object which implements some trait which it is said to "provide".

***Receive***: See Consume. "Receive" is sometimes used in the context of OQueues used for
message-passing communication.

***Reflection***: Accessing information about or controlling of the running system dynamically. For
example, finding OQueues by path pattern, or enumerating the objects in the system. This term is
used to mean the same thing in Java. Also called "introspection".

***Send***: See Produce. "Send" is sometimes used in the context of OQueues used for message-passing
communication.

***Strong Observation***: Observing messages in an OQueue such that each message is observed exactly
once and the observer is notified and executes for each message. This is "exactly once to all" or
broadcast semantics.

***Trait***: A set of methods which can be implemented by a type. See Rust traits.

***Weak Observation***: Observing messages in an OQueue without interrupting the operation of the
OQueue in any way. The observer can look at whatever messages are available (including looking back
in the history of messages), but is not guaranteed to observe every message. The observer can be
notified when new messages are available.

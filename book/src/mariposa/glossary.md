# Glossary

***Consume***: To take a message out of an OQueue or “receive” it. Only one consumer can consume a
given message. This is “exactly once to exactly one” semantics. This is similar to “anycast” in
networking. It provides message channel semantics. Consuming a message does not prevent strong
observers from observing it. We use the term “consume” instead of receive, because it distinguishes
consumers from observers more clearly and avoids the implication that all messages flow exclusively
from a sender to a receiver.

***Client***: A server (or other component) which accesses another server. A client will have a
reference to the server so it can call its methods or send it messages.

***Message***: A value placed in an OQueue. These are often events for observation or commands
between servers.

***Metadata plane***: A transport mechanism for metadata (information about the actual data being
processed by the system) throughout the system. This is distinct from the control plane in that it
carries information about events and state, not commands or other control signals.

***Method***: A method on a trait. This is identical to the usage in both Rust and object-oriented
uses of the term. ORPC methods have additional limitations, however.

***Observability***: Something is observable if the actions it performs, its internal events, or its
state can be viewed from outside. This can include anything from viewing the allocation events
within the kernel to application request processing metrics to failure recovery events within a
distributed system. This is similar to logging in large-scale systems and tracing within
applications. Our use of “observability” matches the usage in those contexts. In Mariposa, we will
mostly be focused on observing kernel level events, but OQueues are flexible enough to handle any of
the above.

***Observe***: To strongly or weakly observe something on an OQueue.

***OQueue***: An Observable Queue. An abstract data structure (it may be implemented in multiple
ways) which allows both message-based communication, by sending and receiving, and observation of
those messages, both strongly and weakly.

***OQueue Handle***: A handle is a reference to something that allows performing actions on it. On
OQueues, there are kinds of handles for each capability of an OQueue: sender, receiver, strong
observer, and weak observer. A user of an OQueue will have one or more handles to allow it to
perform the operations it needs.

***Produce***: To put a value into an OQueues or “send” it. Once a message is produced, at most one
consumer and all strong observers will see it. We use the term “produce”, because it makes it clear
that messages are being distributed to multiple clients, and it avoids the implication that all
messages flow exclusively from a sender to a receiver.

***Receive***: See Consume. “Receive” is sometimes used in the context of OQueues used for
message-passing communication.

***Reflection***: Accessing the system structure of the currently running system dynamically. For
example, finding OQueues by trait or traversing the graph of connected servers. This term is used to
mean the same thing in Java. In Python, this is called “introspection” instead.

***Send***: See Produce. “Send” is sometimes used in the context of OQueues used for message-passing
communication.

***Server***: A component within Mariposa exposing a well-defined interface (a set of traits) and
hiding its implementation. Servers only communicate by method calls and messages and never share
state directly. A server contains state and threads. Servers are analogous to OO objects and follow
similar design principles.

***Strong Observation***: Observing messages in an OQueue such that each message is observed exactly
once and the observer is notified and executes for each message. This is “exactly once to all” or
broadcast semantics.

***Trait***: A set of methods and OQueues which can be implemented by a server. These are very
similar to Rust traits.

***Weak Observation***: Observing messages in an OQueue without interrupting the operation of the
OQueues in any way. The observer can look at whatever messages are available (including looking back
in the history of messages), but is not guaranteed to observe every message. The observer is
notified when new messages are available.

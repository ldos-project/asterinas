## Observable objects: what gets observed

An **observable object** is just an object — data plus behavior — with three
additions:

1. **Identity** — a unique ID, so its events can be told apart and correlated.
2. **Observable methods** — each method call and return is exposed as an event
   on an OQueue.
3. **Dependency injection** — if it depends on another object, that dependency
   is injectable (via slots).

Observable objects are the things that *act* or are *acted on*. Because their
IDs appear in the events, you can follow what one object did and correlate it
with what another did.

There is **one OQueue per method, not one per object** — like a table in a
relational database where the object ID is a column connecting each row to the
right object. For details, see [Objects](objects.md).

## Relational/joins: getting a useful view

Since events from many objects are interleaved on shared OQueues, observers
**filter and join by object ID**, much like SQL over a database. "All reads for
files opened by process X" is a join of the read events with the file-open
events on the process ID. This is what makes observation tractable: one shared
OQueue per method, and each observer selects the view it wants by filtering.

## Slots: reconfiguration

**Slots** provide modularity for *tightly coupled* components — objects whose
methods must be invoked synchronously and quickly, like an eviction policy that
runs during an out-of-memory situation. A slot is a form of **dynamic
dependency injection**:

- **API** — a Rust trait.
- **Provider** — any object implementing that trait.
- **Slot** — the injection point (a setter method).

Injection happens through the setter, after the object is constructed, so a
policy can be **swapped at runtime** without reinitializing the object. Slots are
orthogonal to OQueues: OQueues are the asynchronous observation/command channel;
slots are the synchronous "which implementation am I using" knob.

Because a setter is just a method, the change is automatically observable — each
setter call becomes an event. For details, see [Slots](slots.md).

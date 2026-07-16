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

## Producing in non-blocking contexts

In general, producing into an OQueue can wait for consumer and observers if the storage of the
OQueue is full. In contexts that cannot block (e.g., when preemption is disabled), this is
unacceptable. 

**TODO(arthurp)**: The below is more notes than real documentation.

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

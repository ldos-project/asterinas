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

# Mariposa

## Overview

Mariposa is a modified version of [OSTD](../ostd/README.md) and [Asterinas](../kernel/README.md). It
has features and architectural changes specifically designed to support observability and policy
development. Mariposa is part of the [The Learning-Directed OS project](https://ldos.utexas.edu/)
with the goal of developing the next-generation Machine Learning-based Operating System to drive
computing infrastructure toward high efficiency and performance. 

## OQueues

(This is a high-level overview. See the [OQueues page](oqueues.md) for details.)

A core component of the Mariposa system are Observable Queues (OQueues). OQueues are a form of
concurrent queue which provides additional operations to enable observation of the values in the
queue. The operations supported on an OQueue are:

* **Produce**: Enqueue a value. Multiple threads can produce.
* **Consume**: Dequeue a value. Multiple threads can consume, and each value will be received by
  exactly one consumer.
* **Strong observe**: Observe the value in the OQueues as a stream. The values are not *consumed* in
  the above sense. Each strong observer receives *all* values.
* **Weak observe**: Get specific values from the history of values still available in the OQueue.
  When a weak observer tried to get a value, the value may already have been discarded.

Strong and weak observation are not available on standard queues. Strong observers and consumers can
cause the producer to block if they do not process values fast enough. However, weak observers can
never block the queue because they are not guaranteed to see every value.

All consumers and observers must *attach* before they can perform their operations. Consumers and
observers can never see or receive values that were produced before they attached. This means that
produce does not need to actually store values if there are no attached consumers or observers. This
is very important for OQueues used for observation as it dramatically reduces the cost of OQueues
which are not being observered.

**TODO(arthurp)**: This should be cleaned up to basically give a high level idea of what OQueues are
    and why we should care. It should not be more than a couple of paragraphs.

## ORPC

(This is a high-level overview. See the [ORPC page](orpc.md) for details.)

Observable RPC (ORPC) is a framework supporting defining interfaces between objects in Mariposa and
then observing the interactions of those objects. It follows the object-oriented design of Asterinas
with additional design requirements and patterns.

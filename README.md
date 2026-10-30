# NeXosim

NeXosim is a developer-friendly, highly optimized discrete-event simulation
framework written in Rust. It is meant to scale from small, simple simulations
to very large simulation benches with complex time-driven state machines.

[![Cargo](https://img.shields.io/crates/v/nexosim.svg)](https://crates.io/crates/nexosim)
[![Documentation](https://docs.rs/nexosim/badge.svg)](https://docs.rs/nexosim)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/asynchronics/nexosim#license)

> **NeXosim 1.0 is out** 🎊🎉🛸!!!
>
> Now with **simulation save/restore** support, **event injection**, improved support for
> **real-time applications**, countless quality-of-life improvements ...and much
> more!
>
> 👉 Head to the [CHANGELOG](CHANGELOG.md).

## Overview

NeXosim leverages asynchronous programming to transparently and efficiently
auto-parallelize simulations by means of a custom multi-threaded executor.

It promotes a component-oriented architecture that is familiar to system
engineers and closely resembles [flow-based programming][FBP]: a model is
essentially an isolated entity with a fixed set of typed inputs and outputs,
communicating with other models through message passing via connections defined
during bench assembly.

Although the main impetus for its development was the need for simulators able
to handle large cyberphysical systems, NeXosim is a general-purpose
discrete-event simulator expected to be suitable for a wide range of simulation
activities. It draws from experience on spacecraft real-time simulators but
differs from existing tools in the space industry in a number of respects,
including:

1. _performance_: by taking advantage of Rust's excellent support for
   multithreading and asynchronous programming, simulation models can run
   efficiently in parallel with all required synchronization being transparently
   handled by the simulator,
2. _developer-friendliness_: an ergonomic API and Rust's support for algebraic
   types make it ideal for the "cyber" part in cyberphysical, i.e. for modelling
   digital devices with even very complex state machines,
3. _open-source_: last but not least, NeXosim is distributed under the very
   permissive MIT and Apache 2 licenses, with the explicit intent to foster an
   ecosystem where models can be easily exchanged without reliance on
   proprietary APIs.

[FBP]: https://en.wikipedia.org/wiki/Flow-based_programming

## Documentation

The [API] documentation is relatively exhaustive and includes a practical
overview which should provide all necessary information to get started.

More fleshed out examples can also be found in the dedicated
[simulator](nexosim/examples) and [utilities](nexosim-util/examples)
directories.

[API]: https://docs.rs/nexosim

## Usage

To use the latest version, add to your `Cargo.toml`:

```toml
[dependencies]
nexosim = "1"
serde = "1"
```

## Example

```rust
// A system made of 2 identical models.
// Each model is a 2× multiplier with an output delayed by 1s.
//
//              ┌──────────────┐      ┌──────────────┐
//              │              │      │              │
// Input ●─────►│ multiplier 1 ├─────►│ multiplier 2 ├─────► Output
//              │              │      │              │
//              └──────────────┘      └──────────────┘

use std::time::Duration;use nexosim::model::{Context, Model, schedulable};
use serde::{Deserialize, Serialize};
use nexosim::ports::{EventSinkReader, EventSource, Output, SinkState, event_slot};
use nexosim::simulation::{Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;

// A model that doubles its input and forwards it with a 1s delay.
#[derive(Default, Serialize, Deserialize)]
pub struct DelayedMultiplier {
    pub output: Output<f64>,
}
#[Model]
impl DelayedMultiplier {
    pub fn input(&mut self, value: f64, cx: &Context<Self>) {
        cx.schedule_event(
            Duration::from_secs(1),
            schedulable!(Self::send),
            2.0 * value,
        )
        .unwrap();
    }

    #[nexosim(schedulable)]
    async fn send(&mut self, value: f64) {
        self.output.send(value).await;
    }
}

fn main() -> Result<(), SimulationError> {
    // Instantiate models and their mailboxes.
    let mut multiplier1 = DelayedMultiplier::default();
    let mut multiplier2 = DelayedMultiplier::default();
    let multiplier1_mbox = Mailbox::new();
    let multiplier2_mbox = Mailbox::new();

    // Connect the output of `multiplier1` to the input of `multiplier2`.
    multiplier1
        .output
        .connect(DelayedMultiplier::input, &multiplier2_mbox);

    // Instantiate the simulation bench.
    let mut bench = SimInit::new();

    // Keep handles to the bench endpoints.
    let (sink, mut output) = event_slot(SinkState::Enabled);
    multiplier2.output.connect_sink(sink);

    let input = EventSource::new()
        .connect(DelayedMultiplier::input, &multiplier1_mbox)
        .register(&mut bench);

    // Instantiate the simulator.
    let t0 = MonotonicTime::EPOCH; // arbitrary start time
    let mut simu = bench
        .add_model(multiplier1, multiplier1_mbox, "multiplier 1")
        .add_model(multiplier2, multiplier2_mbox, "multiplier 2")
        .init(t0)?;

    // Send a value to the first multiplier.
    simu.process_event(&input, 3.5)?;

    // Advance time to the next event.
    simu.step()?;
    assert_eq!(simu.time(), t0 + Duration::from_secs(1));
    assert_eq!(output.try_read(), None);

    // Advance time to the next event.
    simu.step()?;
    assert_eq!(simu.time(), t0 + Duration::from_secs(2));
    assert_eq!(output.try_read(), Some(14.0));

    Ok(())
}
```

# Implementation notes

Under the hood, NeXosim is based on an asynchronous implementation of the
[actor model][actor_model], where each simulation model is an actor. The
messages actually exchanged between models are `async` closures which capture
the event's or request's value and take the model as `&mut self` argument. The
mailbox associated to a model and to which closures are forwarded is the
receiver of an async, bounded MPSC channel.

Computations proceed at discrete times. When executed, models can request the
scheduler to send an event (or rather, a closure capturing such event) at a
certain simulation time. Whenever computations for the current time complete,
the scheduler selects the nearest future time at which one or several events are
scheduled (_next event increment_), thus triggering another set of computations.

This computational process makes it difficult to use general-purposes
asynchronous runtimes such as [Tokio][tokio], because the end of a set of
computations is technically a deadlock: the computation completes when all model
have nothing left to do and are blocked on an empty mailbox. Also, instead of
managing a conventional reactor, the runtime manages a priority queue containing
the posted events. For these reasons, NeXosim relies on a fully custom
runtime.

Even though the runtime was largely influenced by Tokio, it features additional
optimizations that make its faster than any other multi-threaded Rust executor
on the typically message-passing-heavy workloads seen in discrete-event
simulation (see [benchmark]). NeXosim also improves over the state of the
art with a very fast custom MPSC channel, which performance has been
demonstrated through [Tachyonix][tachyonix], a general-purpose offshoot of this
channel.

[actor_model]: https://en.wikipedia.org/wiki/Actor_model
[tokio]: https://github.com/tokio-rs/tokio
[tachyonix]: https://github.com/asynchronics/tachyonix
[benchmark]: https://github.com/asynchronics/tachyobench

## Formal verification

NeXosim can optionally be verified with [Verus](https://github.com/verus-lang/verus).
The verified properties are opt-in via the `verus` Cargo feature and do not affect
normal builds.

<details>
<summary>Setting up the Verus toolchain</summary>

### Nix (recommended)

A Nix flake is provided at the repository root. It pins the exact Verus release,
Z3 version, and Rust toolchain required for verification.

```bash
nix develop
cd nexosim
cargo-verus verify --features verus
```

### Manual setup

Install the following components and make sure `verus`, `cargo-verus`, and `z3`
are available in your `PATH`:

- Verus `0.2026.09.20.aef82ed`
- Z3 `4.16.0`
- Rust toolchain `1.98.1`

Then run:

```bash
cd nexosim
cargo-verus verify --features verus
```

### Verified properties

The `verus` feature enables mechanically checked proofs for selected pure
helper functions, including:

- the range bound of the fast multiply-shift reduction used by `Rng::rand_bounded`,
- the validity of `Path::subpath` range normalization,
- round-trip properties of `InsertKey`,
- absence of overflow and monotonicity of the `PeriodicTicker` step computation.

### Asynchronous code

Verus can verify `async fn` bodies and `.await` sequencing, which is sufficient
to reason about individual model methods written with `async`/`.await`. It does
not verify the custom executor, wakers, `Pin`-based polling, or other runtime
machinery; those are exercised by the existing loom and test suites.

</details>

## License

This software is licensed under the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

# 0.3.4 (2025-09-12)

- Run CI on ARM ([#108])
- Enable time in tokio runtime builder to support gRPC front-ends with a timeout ([#109])
- Add diagnostic messages to InputFn and ReplierFn ([#115])
- Add missing source name to "source not found" error messages ([#118])
- Make single-threaded execution fully deterministic (v0.3 backport of [#122])
- Fix `NoRecipient` Error when all existing model addresses are dropped and then new addresses are created ([#125])

[#108]: https://github.com/asynchronics/nexosim/pull/108
[#109]: https://github.com/asynchronics/nexosim/pull/109
[#115]: https://github.com/asynchronics/nexosim/pull/115
[#118]: https://github.com/asynchronics/nexosim/pull/118
[#122]: https://github.com/asynchronics/nexosim/pull/122
[#125]: https://github.com/asynchronics/nexosim/pull/125

# 0.3.3 (2025-05-16)

- Fix spurious `ExecutionError::MessageLoss` error ([#106])

[#106]: https://github.com/asynchronics/nexosim/pull/106

# 0.3.2 (2025-04-23)

- Enable gRPC server to handle concurrent requests ([#87])
- Add an `EventSinkReader` trait with the ability to set a blocking mode on sinks ([#88])
- Add an `EventQueue` sink implementing `EventSinkReader` ([#88])
- Implement `EventSinkReader` trait for `EventSlot` to enable blocking reads ([#88])
- Add `step_unbounded` and `await_event` methods to the gRPC API ([#88])
- Add a `terminate` simulation method to the gRPC API ([#90], [#96])
- Add server shutdown functionality ([#90])
- Make it possible to resume a simulation after it is halted ([#91], [#101])

### Deprecated

- Deprecate the `EventSinkStream` trait ([#88])
- Deprecate the `BlockingEventQueue` and `EventBuffer` sinks ([#88])

### API-breaking change!

- Sinks added to the `EndPointRegistry` must now implement `EventSinkReader`
  instead of `EventSinkStream` (#[88])

[#87]: https://github.com/asynchronics/nexosim/pull/87
[#88]: https://github.com/asynchronics/nexosim/pull/88
[#90]: https://github.com/asynchronics/nexosim/pull/90
[#91]: https://github.com/asynchronics/nexosim/pull/91
[#96]: https://github.com/asynchronics/nexosim/pull/96
[#101]: https://github.com/asynchronics/nexosim/pull/101

# 0.3.1 (2025-01-28)

- Add a blocking event queue ([#82])

[#82]: https://github.com/asynchronics/nexosim/pull/82

# 0.3.0 (2025-01-20)

The final 0.3.0 release features a very large number of improvements and API
changes, including all those in the beta release and a couple more.

This release is not compatible with the 0.2.\* releases, but porting models and benches should be relatively straightforward.

### Added (mostly API-breaking changes)

- Add a gRPC server for local (Unix Domain Sockets) and remote (http/2)
  execution ([#12], [#24], [#25], [#26], [#29], [#43], [#78], [#79])
- Single-threaded executor supporting compilation to WebAssembly ([#24])
- Add support for the `tracing` crate ([#47])
- Make `Output`s and `Requestor`s `Clone`-able ([#30], [#48])
- Make the global `Scheduler` an owned `Clone`-able type that can be sent to
  other threads ([#30])
- Add an automatically managed action key for scheduled actions/events ([#27])
- Enable connection of different input/output pairs with `map_connect()` methods
  on `Output` and `Requestor` ([#32])
- Streamline the creation of data buses (SPI, CAN, MIL-STD-1553, SpaceWire etc.)
  with `filter_map_connect()` methods on `Output` and `Requestor` ([#32])
- Implement deadlock detection ([#51])
- Streamline the builder pattern for models with a `ProtoModel` trait ([#54])
- Implement execution timeout ([#57])
- Return an error when a real-time simulation clock looses synchronization
  ([#58])
- Catch model panics and report them as errors ([#60])
- Provide additional ordering guaranties when using the global scheduler ([#62])
- Remove `LineId` line disconnection API ([#63])
- Implement detection of lost and undelivered messages ([#68], [#70])
- Provide a `UniRequestor` type for unary requestors ([#69])
- Add support for intentionally halting an ongoing simulation and add a
  `Simulation::step_unbounded` method ([#74], [#77])

[#68]: https://github.com/asynchronics/nexosim/pull/68
[#69]: https://github.com/asynchronics/nexosim/pull/69
[#70]: https://github.com/asynchronics/nexosim/pull/70
[#74]: https://github.com/asynchronics/nexosim/pull/74
[#77]: https://github.com/asynchronics/nexosim/pull/77
[#78]: https://github.com/asynchronics/nexosim/pull/78
[#79]: https://github.com/asynchronics/nexosim/pull/79

# 0.3.0-beta.0 (2024-11-16)

This beta release features a very large number of improvements and API changes,
including:

- Add a gRPC server for remote execution ([#12], [#24], [#25], [#26], [#29],
  [#43])
- Single-threaded executor supporting compilation to WebAssembly ([#24])
- Add support for the `tracing` crate ([#47])
- Make `Output`s and `Requestor`s `Clone`-able ([#30], [#48])
- Make the global `Scheduler` an owned `Clone`-able type ([#30])
- Add an automatically managed action key for scheduled actions/events ([#27])
- Enable connection of different input/output pairs with `map_connect()` methods
  on `Output` and `Requestor` ([#32])
- Streamline the creation of data buses (SPI, CAN, MIL-STD-1553, SpaceWire etc.)
  with `filter_map_connect()` methods on `Output` and `Requestor` ([#32])
- Implement deadlock detection ([#51])
- Streamline the builder pattern for models with a `ProtoModel` trait ([#54])
- Implement execution timeout ([#57])
- Return an error when a real-time simulation clock looses synchronization
  ([#58])
- Catch model panics and report them as errors ([#60])
- Provide additional ordering guaranties when using the global scheduler ([#62])
- Remove `LineId` line disconnection API ([#63])

[#12]: https://github.com/asynchronics/nexosim/pull/12
[#24]: https://github.com/asynchronics/nexosim/pull/24
[#25]: https://github.com/asynchronics/nexosim/pull/25
[#26]: https://github.com/asynchronics/nexosim/pull/26
[#27]: https://github.com/asynchronics/nexosim/pull/27
[#29]: https://github.com/asynchronics/nexosim/pull/29
[#30]: https://github.com/asynchronics/nexosim/pull/30
[#32]: https://github.com/asynchronics/nexosim/pull/32
[#43]: https://github.com/asynchronics/nexosim/pull/43
[#47]: https://github.com/asynchronics/nexosim/pull/47
[#48]: https://github.com/asynchronics/nexosim/pull/48
[#51]: https://github.com/asynchronics/nexosim/pull/51
[#54]: https://github.com/asynchronics/nexosim/pull/54
[#57]: https://github.com/asynchronics/nexosim/pull/57
[#58]: https://github.com/asynchronics/nexosim/pull/58
[#60]: https://github.com/asynchronics/nexosim/pull/60
[#62]: https://github.com/asynchronics/nexosim/pull/62
[#63]: https://github.com/asynchronics/nexosim/pull/63

# 0.2.4 (2024-11-16)

- Add crate rename notice

# 0.2.3 (2024-08-24)

- Force the waker VTable to be uniquely instantiated to re-enable the
  `will_wake` optimisation after its implementation was changed in `std` ([#38])
- Ignore broadcast error when sending to a closed `EventStream` ([#37])

[#37]: https://github.com/asynchronics/nexosim/pull/37
[#38]: https://github.com/asynchronics/nexosim/pull/38

# 0.2.2 (2024-04-04)

- Add `serde` feature and serialization support for `MonotonicTime` ([#19])
- Update `multishot` dependency due to soundness issue in older version ([#23])

[#19]: https://github.com/asynchronics/nexosim/pull/19
[#23]: https://github.com/asynchronics/nexosim/pull/23

# 0.2.1 (2024-03-06)

### Added

- Add support for custom clocks and provide an optional real-time clock
  ([#9], [#15]).

[#9]: https://github.com/asynchronics/nexosim/pull/9
[#15]: https://github.com/asynchronics/nexosim/pull/15

### Misc

- Update copyright in MIT license to include contributors

# 0.2.0 (2023-08-15)

### Added (API-breaking changes)

- Enable cancellation of events up to the very last moment, even if the event is
  scheduled for the current time ([#5])
- Makes it possible to schedule periodic events from a `Simulation` or a model's
  `Scheduler` ([#6])
- Mitigate the increase in API surface by merging each pair of
  `schedule_*event_in`/`schedule_*event_at` methods into one overloaded
  `schedule_*event` method that accept either a `Duration` or a `MonotonicTime`
  ([#7])

[#5]: https://github.com/asynchronics/nexosim/pull/5
[#6]: https://github.com/asynchronics/nexosim/pull/6
[#7]: https://github.com/asynchronics/nexosim/pull/7

# 0.1.0 (2023-01-16)

Initial release

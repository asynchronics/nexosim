//! Event scheduling from a `Simulation` instance.

use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[cfg(not(miri))]
use nexosim::model::Context;
use nexosim::model::Model;
use nexosim::ports::{EventQueue, EventQueueReader, Output};
use nexosim::simulation::{Mailbox, Scheduler, SimInit, Simulation, SourceId};
use nexosim::time::MonotonicTime;

const MT_NUM_THREADS: usize = 4;

// Input-to-output pass-through model.
#[derive(Serialize, Deserialize)]
struct PassThroughModel<T>
where
    T: Clone + Send + 'static,
{
    pub output: Output<T>,
}
impl<T> PassThroughModel<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    pub fn new() -> Self {
        Self {
            output: Output::default(),
        }
    }
    pub async fn input(&mut self, arg: T) {
        self.output.send(arg).await;
    }
}
impl<T> Model for PassThroughModel<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    type Env = ();
}

/// A simple bench containing a single pass-through model (input forwarded to
/// output) running as fast as possible.
fn passthrough_bench<T>(
    num_threads: usize,
    t0: MonotonicTime,
) -> (Simulation, Scheduler, SourceId<T>, EventQueueReader<T>)
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    // Bench assembly.
    let mut model = PassThroughModel::new();
    let mbox = Mailbox::new();

    let out_stream = EventQueue::new();
    model.output.connect_sink(&out_stream);
    let addr = mbox.address();

    let mut bench = SimInit::with_num_threads(num_threads).add_model(model, mbox, "");

    let source_id = bench.register_input(PassThroughModel::input, &addr);

    let simu = bench.init(t0).unwrap();
    let scheduler = simu.scheduler();

    (simu, scheduler, source_id, out_stream.into_reader())
}

fn schedule_events(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let (mut simu, scheduler, source_id, mut output) = passthrough_bench(num_threads, t0);

    // Queue 2 events at t0+3s and t0+2s, in reverse order.
    scheduler
        .schedule_event(Duration::from_secs(3), &source_id, ())
        .unwrap();
    scheduler
        .schedule_event(t0 + Duration::from_secs(2), &source_id, ())
        .unwrap();

    // Move to the 1st event at t0+2s.
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(2));
    assert!(output.next().is_some());

    // Schedule another event in 4s (at t0+6s).
    scheduler
        .schedule_event(Duration::from_secs(4), &source_id, ())
        .unwrap();

    // Move to the 2nd event at t0+3s.
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(3));
    assert!(output.next().is_some());

    // Move to the 3rd event at t0+6s.
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(6));
    assert!(output.next().is_some());
    assert!(output.next().is_none());
}

fn schedule_keyed_events(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let (mut simu, scheduler, source_id, mut output) = passthrough_bench(num_threads, t0);

    let event_t1 = scheduler
        .schedule_keyed_event(t0 + Duration::from_secs(1), &source_id, 1)
        .unwrap();

    let event_t2_1 = scheduler
        .schedule_keyed_event(Duration::from_secs(2), &source_id, 21)
        .unwrap();

    scheduler
        .schedule_event(Duration::from_secs(2), &source_id, 22)
        .unwrap();

    // Move to the 1st event at t0+1.
    simu.step().unwrap();

    // Try to cancel the 1st event after it has already taken place and check
    // that the cancellation had no effect.
    event_t1.cancel();
    assert_eq!(simu.time(), t0 + Duration::from_secs(1));
    assert_eq!(output.next(), Some(1));

    // Cancel the second event (t0+2) before it is meant to takes place and
    // check that we move directly to the 3rd event.
    event_t2_1.cancel();
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(2));
    assert_eq!(output.next(), Some(22));
    assert!(output.next().is_none());
}

fn schedule_periodic_events(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let (mut simu, scheduler, source_id, mut output) = passthrough_bench(num_threads, t0);

    // Queue 2 periodic events at t0 + 3s + k*2s.
    scheduler
        .schedule_periodic_event(
            Duration::from_secs(3),
            Duration::from_secs(2),
            &source_id,
            1,
        )
        .unwrap();
    scheduler
        .schedule_periodic_event(
            t0 + Duration::from_secs(3),
            Duration::from_secs(2),
            &source_id,
            2,
        )
        .unwrap();

    // Move to the next events at t0 + 3s + k*2s.
    for k in 0..10 {
        simu.step().unwrap();
        assert_eq!(
            simu.time(),
            t0 + Duration::from_secs(3) + k * Duration::from_secs(2)
        );
        assert_eq!(output.next(), Some(1));
        assert_eq!(output.next(), Some(2));
        assert!(output.next().is_none());
    }
}

fn schedule_periodic_keyed_events(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let (mut simu, scheduler, source_id, mut output) = passthrough_bench(num_threads, t0);

    // Queue 2 periodic events at t0 + 3s + k*2s.
    scheduler
        .schedule_periodic_event(
            Duration::from_secs(3),
            Duration::from_secs(2),
            &source_id,
            1,
        )
        .unwrap();
    let event2_key = scheduler
        .schedule_keyed_periodic_event(
            t0 + Duration::from_secs(3),
            Duration::from_secs(2),
            &source_id,
            2,
        )
        .unwrap();

    // Move to the next event at t0+3s.
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(3));
    assert_eq!(output.next(), Some(1));
    assert_eq!(output.next(), Some(2));
    assert!(output.next().is_none());

    // Cancel the second event.
    event2_key.cancel();

    // Move to the next events at t0 + 3s + k*2s.
    for k in 1..10 {
        simu.step().unwrap();
        assert_eq!(
            simu.time(),
            t0 + Duration::from_secs(3) + k * Duration::from_secs(2)
        );
        assert_eq!(output.next(), Some(1));
        assert!(output.next().is_none());
    }
}

#[test]
fn schedule_events_st() {
    schedule_events(1);
}

#[test]
fn schedule_events_mt() {
    schedule_events(MT_NUM_THREADS);
}

#[test]
fn schedule_keyed_events_st() {
    schedule_keyed_events(1);
}

#[test]
fn schedule_keyed_events_mt() {
    schedule_keyed_events(MT_NUM_THREADS);
}

#[test]
fn schedule_periodic_events_st() {
    schedule_periodic_events(1);
}

#[test]
fn schedule_periodic_events_mt() {
    schedule_periodic_events(MT_NUM_THREADS);
}

#[test]
fn schedule_periodic_keyed_events_st() {
    schedule_periodic_keyed_events(1);
}

#[test]
fn schedule_periodic_keyed_events_mt() {
    schedule_periodic_keyed_events(MT_NUM_THREADS);
}

#[cfg(not(miri))]
use std::time::{Instant, SystemTime};

#[cfg(not(miri))]
use nexosim::time::{AutoSystemClock, Clock, SystemClock};

// Model that outputs timestamps at init and each time its input is triggered.
#[cfg(not(miri))]
#[derive(Default, Serialize, Deserialize)]
struct TimestampModel {
    pub stamp: Output<(Instant, SystemTime)>,
}
#[cfg(not(miri))]
impl TimestampModel {
    pub async fn trigger(&mut self) {
        self.stamp.send((Instant::now(), SystemTime::now())).await;
    }
}
#[cfg(not(miri))]
impl Model for TimestampModel {
    type Env = ();

    async fn init(
        mut self,
        _: &Context<Self>,
        _: &mut (),
    ) -> nexosim::model::InitializedModel<Self> {
        self.stamp.send((Instant::now(), SystemTime::now())).await;
        self.into()
    }
}

/// A simple bench containing a single timestamping model with a custom clock.
#[cfg(not(miri))]
fn timestamp_bench(
    num_threads: usize,
    t0: MonotonicTime,
    clock: impl Clock + 'static,
) -> (
    Simulation,
    Scheduler,
    SourceId<()>,
    EventQueueReader<(Instant, SystemTime)>,
) {
    // Bench assembly.
    let mut model = TimestampModel::default();
    let mbox = Mailbox::new();

    let stamp_stream = EventQueue::new();
    model.stamp.connect_sink(&stamp_stream);
    let addr = mbox.address();

    let mut bench = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "")
        .set_clock(clock);

    let source_id = bench.register_input(TimestampModel::trigger, &addr);

    let simu = bench.init(t0).unwrap();
    let scheduler = simu.scheduler();

    (simu, scheduler, source_id, stamp_stream.into_reader())
}

#[cfg(not(miri))]
fn system_clock_from_instant(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    const TOLERANCE: f64 = 0.005; // [s]

    // The reference simulation time is set in the past of t0 so that the
    // simulation starts in the future when the reference wall clock time is
    // close to the wall clock time when the simulation in initialized.
    let simulation_ref_offset = 0.3; // [s] must be greater than any `instant_offset`.
    let simulation_ref = t0 - Duration::from_secs_f64(simulation_ref_offset);

    // Test reference wall clock times in the near past and near future.
    for wall_clock_offset in [-0.1, 0.1] {
        // The clock reference is the current time offset by `instant_offset`.
        let wall_clock_init = Instant::now();
        let wall_clock_ref = if wall_clock_offset >= 0.0 {
            wall_clock_init + Duration::from_secs_f64(wall_clock_offset)
        } else {
            wall_clock_init - Duration::from_secs_f64(-wall_clock_offset)
        };

        let clock = SystemClock::from_instant(simulation_ref, wall_clock_ref);

        let (mut simu, scheduler, source_id, mut stamp) = timestamp_bench(num_threads, t0, clock);

        // Queue a single event at t0 + 0.1s.
        scheduler
            .schedule_event(Duration::from_secs_f64(0.1), &source_id, ())
            .unwrap();

        // Check the stamps.
        for expected_time in [
            simulation_ref_offset + wall_clock_offset,
            simulation_ref_offset + wall_clock_offset + 0.1,
        ] {
            let measured_time = (stamp.next().unwrap().0 - wall_clock_init).as_secs_f64();
            assert!(
                (expected_time - measured_time).abs() <= TOLERANCE,
                "Expected t = {expected_time:.6}s +/- {TOLERANCE:.6}s, measured t = {measured_time:.6}s",
            );

            simu.step().unwrap();
        }
    }
}

#[cfg(not(miri))]
fn system_clock_from_system_time(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    const TOLERANCE: f64 = 0.005; // [s]

    // The reference simulation time is set in the past of t0 so that the
    // simulation starts in the future when the reference wall clock time is
    // close to the wall clock time when the simulation in initialized.
    let simulation_ref_offset = 0.3; // [s] must be greater than any `instant_offset`.
    let simulation_ref = t0 - Duration::from_secs_f64(simulation_ref_offset);

    // Test reference wall clock times in the near past and near future.
    for wall_clock_offset in [-0.1, 0.1] {
        // The clock reference is the current time offset by `instant_offset`.
        let wall_clock_init = SystemTime::now();
        let wall_clock_ref = if wall_clock_offset >= 0.0 {
            wall_clock_init + Duration::from_secs_f64(wall_clock_offset)
        } else {
            wall_clock_init - Duration::from_secs_f64(-wall_clock_offset)
        };

        let clock = SystemClock::from_system_time(simulation_ref, wall_clock_ref);

        let (mut simu, scheduler, source_id, mut stamp) = timestamp_bench(num_threads, t0, clock);

        // Queue a single event at t0 + 0.1s.
        scheduler
            .schedule_event(Duration::from_secs_f64(0.1), &source_id, ())
            .unwrap();

        // Check the stamps.
        for expected_time in [
            simulation_ref_offset + wall_clock_offset,
            simulation_ref_offset + wall_clock_offset + 0.1,
        ] {
            let measured_time = stamp
                .next()
                .unwrap()
                .1
                .duration_since(wall_clock_init)
                .unwrap()
                .as_secs_f64();
            assert!(
                (expected_time - measured_time).abs() <= TOLERANCE,
                "Expected t = {expected_time:.6}s +/- {TOLERANCE:.6}s, measured t = {measured_time:.6}s",
            );

            simu.step().unwrap();
        }
    }
}

#[cfg(not(miri))]
fn auto_system_clock(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    const TOLERANCE: f64 = 0.005; // [s]

    let (mut simu, scheduler, source_id, mut stamp) =
        timestamp_bench(num_threads, t0, AutoSystemClock::new());
    let instant_t0 = Instant::now();

    // Queue a periodic event at t0 + 0.2s + k*0.2s.
    scheduler
        .schedule_periodic_event(
            Duration::from_secs_f64(0.2),
            Duration::from_secs_f64(0.2),
            &source_id,
            (),
        )
        .unwrap();

    // Queue a single event at t0 + 0.3s.
    scheduler
        .schedule_event(Duration::from_secs_f64(0.3), &source_id, ())
        .unwrap();

    // Check the stamps.
    for expected_time in [0.0, 0.2, 0.3, 0.4, 0.6] {
        let measured_time = (stamp.next().unwrap().0 - instant_t0).as_secs_f64();
        assert!(
            (expected_time - measured_time).abs() <= TOLERANCE,
            "Expected t = {expected_time:.6}s +/- {TOLERANCE:.6}s, measured t = {measured_time:.6}s",
        );

        simu.step().unwrap();
    }
}

#[cfg(not(miri))]
#[test]
fn system_clock_from_instant_st() {
    system_clock_from_instant(1);
}

#[cfg(not(miri))]
#[test]
fn system_clock_from_instant_mt() {
    system_clock_from_instant(MT_NUM_THREADS);
}

#[cfg(not(miri))]
#[test]
fn system_clock_from_system_time_st() {
    system_clock_from_system_time(1);
}

#[cfg(not(miri))]
#[test]
fn system_clock_from_system_time_mt() {
    system_clock_from_system_time(MT_NUM_THREADS);
}

#[cfg(not(miri))]
#[test]
fn auto_system_clock_st() {
    auto_system_clock(1);
}

#[cfg(not(miri))]
#[test]
fn auto_system_clock_mt() {
    auto_system_clock(MT_NUM_THREADS);
}

//! Simulation with clock ticks.

use std::thread;
use std::time::Duration;

use nexosim::model::Context;
use serde::{Deserialize, Serialize};

#[cfg(not(miri))]
use nexosim::model::Model;
use nexosim::ports::{
    EventQueueReader, EventSinkReader, EventSource, Output, SinkState, event_queue,
};
use nexosim::simulation::{EventId, ExecutionError, Mailbox, SimInit, Simulation};
use nexosim::time::{AutoSystemClock, MonotonicTime, PeriodicTicker};

const MT_NUM_THREADS: usize = 4;

// A time-stamping model.
#[derive(Serialize, Deserialize)]
struct TimestampingModel {
    pub output: Output<MonotonicTime>,
}
#[Model]
impl TimestampingModel {
    pub fn new() -> Self {
        Self {
            output: Output::default(),
        }
    }
    pub async fn timestamp(&mut self, _: (), ctx: &Context<Self>) {
        self.output.send(ctx.time()).await;
    }
}

/// A simple real-time bench containing a single time-stamping model (timestamp
/// forwarded to output upon trigger).
fn timestamping_bench(
    num_threads: usize,
    t0: MonotonicTime,
    tick_period: Duration,
) -> (Simulation, EventId<()>, EventQueueReader<MonotonicTime>) {
    // Bench assembly.
    let mut model = TimestampingModel::new();
    let mbox = Mailbox::new();

    let (sink, out_stream) = event_queue(SinkState::Enabled);
    model.output.connect_sink(sink);
    let addr = mbox.address();

    let mut bench = SimInit::with_num_threads(num_threads)
        .with_clock(AutoSystemClock::new(), PeriodicTicker::new(tick_period));

    let input = EventSource::new()
        .connect(TimestampingModel::timestamp, &addr)
        .register(&mut bench);

    let simu = bench.add_model(model, mbox, "").init(t0).unwrap();
    (simu, input, out_stream)
}

fn ticked_mode_scheduling(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let t_end = t0 + Duration::from_secs(1);
    let tick_period = Duration::from_millis(100);

    let (mut simu, source, mut output) = timestamping_bench(num_threads, t0, tick_period);
    let scheduler = simu.scheduler();

    // Queue an event at t0+1s.
    scheduler.schedule_event(t_end, &source, ()).unwrap();

    let th = thread::spawn(move || {
        // Schedule an event at t0+250ms while between the ticks at t0+100ms and
        // t0+200ms.
        thread::sleep(Duration::from_millis(150));
        assert!(
            scheduler
                .schedule_event(t0 + Duration::from_millis(250), &source, ())
                .is_ok()
        );
    });

    simu.step_until(t_end).unwrap();
    assert_eq!(
        output.try_read(),
        Some(MonotonicTime::EPOCH + Duration::from_millis(250))
    );

    th.join().unwrap();
}

fn ticked_mode_halt(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let t_end = t0 + Duration::from_secs(1);
    let tick_period = Duration::from_millis(100);

    let (mut simu, _source, _output) = timestamping_bench(num_threads, t0, tick_period);
    let scheduler = simu.scheduler();

    let th = thread::spawn(move || {
        // Request the simulation to halt after approximately 450ms, expecting
        // the command to be acknowledged at the next tick at t0+500ms.
        thread::sleep(Duration::from_millis(450));
        scheduler.halt();
    });

    assert!(matches!(
        simu.step_until(t_end),
        Err(ExecutionError::Halted)
    ));
    assert_eq!(
        simu.time(),
        MonotonicTime::EPOCH + Duration::from_millis(500)
    );

    th.join().unwrap();
}

#[test]
fn ticked_mode_scheduling_st() {
    ticked_mode_scheduling(1);
}

#[test]
fn ticked_mode_scheduling_mt() {
    ticked_mode_scheduling(MT_NUM_THREADS);
}

#[test]
fn ticked_mode_halt_st() {
    ticked_mode_halt(1);
}

#[test]
fn ticked_mode_halt_mt() {
    ticked_mode_halt(MT_NUM_THREADS);
}

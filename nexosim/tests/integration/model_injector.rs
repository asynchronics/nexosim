//! Ticked simulation with event injection.

use std::thread;
use std::time::Duration;

use nexosim::model::Context;
use serde::{Deserialize, Serialize};

#[cfg(not(miri))]
use nexosim::model::{BuildContext, Model, ProtoModel, schedulable};
use nexosim::ports::{EventSinkReader, EventSource, Output, SinkState, event_queue};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::{AutoSystemClock, MonotonicTime, PeriodicTicker};

const MT_NUM_THREADS: usize = 4;

// A time-stamping model.
struct TestProtoModel {
    output: Output<(usize, MonotonicTime)>,
    delay1: Duration,
    delay2: Duration,
}
impl TestProtoModel {
    fn new(delay1: Duration, delay2: Duration) -> Self {
        Self {
            output: Output::default(),
            delay1,
            delay2,
        }
    }
}
impl ProtoModel for TestProtoModel {
    type Model = TestModel;

    fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, ()) {
        let injector = cx.injector();
        let manual_schedulable_trigger =
            cx.register_schedulable(TestModel::manual_schedulable_trigger);

        thread::spawn(move || {
            thread::sleep(self.delay1);
            // Use the `schedulable` macro.
            injector.inject_event(schedulable!(TestModel::auto_schedulable_trigger), 1);

            // Use a manually registered schedulable.
            thread::sleep(self.delay2);
            injector.inject_event(&manual_schedulable_trigger, 2);
        });

        (TestModel::new(self.output), ())
    }
}
#[derive(Serialize, Deserialize)]
struct TestModel {
    output: Output<(usize, MonotonicTime)>,
}
#[Model]
impl TestModel {
    fn new(output: Output<(usize, MonotonicTime)>) -> Self {
        Self { output }
    }
    #[nexosim(schedulable)]
    async fn auto_schedulable_trigger(&mut self, payload: usize, cx: &Context<Self>) {
        self.output.send((payload, cx.time())).await;
    }
    async fn manual_schedulable_trigger(&mut self, payload: usize, cx: &Context<Self>) {
        self.output.send((payload, cx.time())).await;
    }
    fn dummy(&mut self) {}
}

fn injector_basic(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let t_end = t0 + Duration::from_secs(1);
    let tick_period = Duration::from_millis(250);

    let mut model = TestProtoModel::new(Duration::from_millis(100), Duration::from_millis(500));
    let mbox = Mailbox::new();

    let (sink, mut output) = event_queue(SinkState::Enabled);
    model.output.connect_sink(sink);

    let mut simu = SimInit::with_num_threads(num_threads)
        .with_clock(AutoSystemClock::new(), PeriodicTicker::new(tick_period))
        .add_model(model, mbox, "")
        .init(t0)
        .unwrap();

    simu.step_until(t_end).unwrap();
    assert_eq!(
        output.try_read(),
        Some((1, MonotonicTime::EPOCH + Duration::from_millis(500)))
    );
    assert_eq!(
        output.try_read(),
        Some((2, MonotonicTime::EPOCH + Duration::from_millis(1000)))
    );
}

fn injector_tickless(num_threads: usize) {
    let t0 = MonotonicTime::EPOCH;
    let t_end = t0 + Duration::from_secs(1);
    let pseudo_tick_period = Duration::from_millis(250);

    let mut model = TestProtoModel::new(Duration::from_millis(100), Duration::from_millis(500));
    let mbox = Mailbox::new();

    let (sink, mut output) = event_queue(SinkState::Enabled);
    model.output.connect_sink(sink);

    let mut bench =
        SimInit::with_num_threads(num_threads).with_tickless_clock(AutoSystemClock::new());

    let dummy = EventSource::new()
        .connect(TestModel::dummy, &mbox)
        .register(&mut bench);

    let mut simu = bench.add_model(model, mbox, "").init(t0).unwrap();
    simu.scheduler()
        .schedule_periodic_event(t0 + pseudo_tick_period, pseudo_tick_period, &dummy, ())
        .unwrap();

    simu.step_until(t_end).unwrap();
    assert_eq!(
        output.try_read(),
        Some((1, MonotonicTime::EPOCH + Duration::from_millis(500)))
    );
    assert_eq!(
        output.try_read(),
        Some((2, MonotonicTime::EPOCH + Duration::from_millis(1000)))
    );
}

#[cfg(not(miri))]
#[test]
fn injector_ordering_mt() {
    const NUM_THREAD: usize = 16;
    let t0 = MonotonicTime::EPOCH;
    let tick_period = Duration::from_millis(500);
    let t_end = t0 + 2 * tick_period;

    let mut bench = SimInit::with_num_threads(NUM_THREAD)
        .with_clock(AutoSystemClock::new(), PeriodicTicker::new(tick_period));

    // Connect each model to its own sink.
    let mut outputs = Vec::new();
    for _ in 0..NUM_THREAD {
        let mut model = TestProtoModel::new(Duration::from_millis(150), Duration::from_millis(100));
        let mbox = Mailbox::new();

        let (sink, output) = event_queue(SinkState::Enabled);
        model.output.connect_sink(sink);
        bench = bench.add_model(model, mbox, "");
        outputs.push(output);
    }

    let mut simu = bench.init(t0).unwrap();
    simu.step_until(t_end).unwrap();

    for mut output in outputs {
        // Make sure the events arrived in the same time slice but in order.
        assert_eq!(output.try_read(), Some((1, t_end)));
        assert_eq!(output.try_read(), Some((2, t_end)));
    }
}

#[cfg(not(miri))]
#[test]
fn injector_basic_st() {
    injector_basic(1);
}

#[cfg(not(miri))]
#[test]
fn injector_basic_mt() {
    injector_basic(MT_NUM_THREADS);
}

#[cfg(not(miri))]
#[test]
fn injector_tickless_st() {
    injector_tickless(1);
}

#[cfg(not(miri))]
#[test]
fn injector_tickless_mt() {
    injector_tickless(MT_NUM_THREADS);
}

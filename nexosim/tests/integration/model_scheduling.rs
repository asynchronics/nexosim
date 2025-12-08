//! Event scheduling within `Model` input methods.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{schedulable, Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{EventKey, Mailbox, SimInit};
use nexosim::time::MonotonicTime;

const MT_NUM_THREADS: usize = 4;

fn model_schedule_event(num_threads: usize) {
    #[derive(Default, Serialize, Deserialize)]
    struct TestModel {
        output: Output<()>,
    }
    #[Model]
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &Context<Self>) {
            cx.schedule_event(Duration::from_secs(2), schedulable!(Self::action), ())
                .unwrap();
        }
        #[nexosim(schedulable)]
        async fn action(&mut self) {
            self.output.send(()).await;
        }
    }

    let mut model = TestModel::default();
    let mbox = Mailbox::new();

    let output = EventQueue::new();
    model.output.connect_sink(&output);
    let mut output = output.into_reader();
    let addr = mbox.address();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "")
        .init(t0)
        .unwrap();

    simu.process_event(TestModel::trigger, (), addr).unwrap();
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(2));
    assert!(output.next().is_some());
    simu.step().unwrap();
    assert!(output.next().is_none());
}

// This test mainly checks that ModelRegistry and SchedulableId with bitmasking
// works properly when multiple models are involved (the won't override their
// inputs).
fn multiple_models_scheduling(num_threads: usize) {
    const MODEL_COUNT: usize = 3;

    #[derive(Serialize, Deserialize)]
    struct TestModel {
        delay: u64,
        val: usize,
        output: Output<usize>,
    }
    #[Model]
    impl TestModel {
        #[nexosim(init)]
        async fn init(self, cx: &Context<Self>, _: &mut ()) -> InitializedModel<Self> {
            cx.schedule_event(
                Duration::from_secs(self.delay),
                schedulable!(Self::action),
                (),
            )
            .unwrap();
            self.into()
        }

        // Does nothing, but alters the ModelRegistry
        #[nexosim(schedulable)]
        async fn dead_input(&mut self) {}

        #[nexosim(schedulable)]
        async fn action(&mut self) {
            self.output.send(self.val).await;
        }
    }

    let t0 = MonotonicTime::EPOCH;
    let mut bench = SimInit::with_num_threads(num_threads);

    let output = EventQueue::new();

    // Iterate in reverse, to avoid happy accidents.
    // Last added model will be scheduled first, etc.
    for idx in (0..MODEL_COUNT).rev() {
        let mut model = TestModel {
            delay: idx as u64 + 1,
            val: 13 * (idx + 1),
            output: Output::default(),
        };
        let mbox = Mailbox::new();
        model.output.connect_sink(&output);
        bench = bench.add_model(model, mbox, format!("Model_{idx}"));
    }

    let mut simu = bench.init(t0).unwrap();
    let mut output = output.into_reader();

    for idx in 0..MODEL_COUNT {
        simu.step().unwrap();
        // This should assert that the order of the SourceIds is correct.
        // If bitmasking is removed from `SchedulableId::__from_decorated` this would
        // fail.
        assert_eq!(output.next(), Some(13 * (idx + 1)));
    }
}

fn model_cancel_future_keyed_event(num_threads: usize) {
    #[derive(Default, Serialize, Deserialize)]
    struct TestModel {
        output: Output<i32>,
        key: Option<EventKey>,
    }
    #[Model]
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &Context<Self>) {
            cx.schedule_event(Duration::from_secs(1), schedulable!(Self::action1), ())
                .unwrap();
            self.key = cx
                .schedule_keyed_event(Duration::from_secs(2), schedulable!(Self::action2), ())
                .ok();
        }
        #[nexosim(schedulable)]
        async fn action1(&mut self) {
            self.output.send(1).await;
            // Cancel the call to `action2`.
            self.key.take().unwrap().cancel();
        }
        #[nexosim(schedulable)]
        async fn action2(&mut self) {
            self.output.send(2).await;
        }
    }

    let mut model = TestModel::default();
    let mbox = Mailbox::new();

    let output = EventQueue::new();
    model.output.connect_sink(&output);
    let mut output = output.into_reader();
    let addr = mbox.address();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "")
        .init(t0)
        .unwrap();

    simu.process_event(TestModel::trigger, (), addr).unwrap();
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(1));
    assert_eq!(output.next(), Some(1));
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(1));
    assert!(output.next().is_none());
}

fn model_cancel_same_time_keyed_event(num_threads: usize) {
    #[derive(Default, Serialize, Deserialize)]
    struct TestModel {
        output: Output<i32>,
        key: Option<EventKey>,
    }
    #[Model]
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &Context<Self>) {
            cx.schedule_event(Duration::from_secs(2), schedulable!(Self::action1), ())
                .unwrap();
            self.key = cx
                .schedule_keyed_event(Duration::from_secs(2), schedulable!(Self::action2), ())
                .ok();
        }
        #[nexosim(schedulable)]
        async fn action1(&mut self) {
            self.output.send(1).await;
            // Cancel the call to `action2`.
            self.key.take().unwrap().cancel();
        }
        #[nexosim(schedulable)]
        async fn action2(&mut self) {
            self.output.send(2).await;
        }
    }

    let mut model = TestModel::default();
    let mbox = Mailbox::new();

    let output = EventQueue::new();
    model.output.connect_sink(&output);
    let mut output = output.into_reader();
    let addr = mbox.address();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "")
        .init(t0)
        .unwrap();

    simu.process_event(TestModel::trigger, (), addr).unwrap();
    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(2));
    assert_eq!(output.next(), Some(1));
    assert!(output.next().is_none());
    simu.step().unwrap();
    assert!(output.next().is_none());
}

fn model_schedule_periodic_event(num_threads: usize) {
    #[derive(Default, Serialize, Deserialize)]
    struct TestModel {
        output: Output<i32>,
    }
    #[Model]
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &Context<Self>) {
            cx.schedule_periodic_event(
                Duration::from_secs(2),
                Duration::from_secs(3),
                schedulable!(Self::action),
                42,
            )
            .unwrap();
        }
        #[nexosim(schedulable)]
        async fn action(&mut self, payload: i32) {
            self.output.send(payload).await;
        }
    }

    let mut model = TestModel::default();
    let mbox = Mailbox::new();

    let output = EventQueue::new();
    model.output.connect_sink(&output);
    let mut output = output.into_reader();
    let addr = mbox.address();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "")
        .init(t0)
        .unwrap();

    simu.process_event(TestModel::trigger, (), addr).unwrap();

    // Move to the next events at t0 + 2s + k*3s.
    for k in 0..10 {
        simu.step().unwrap();
        assert_eq!(
            simu.time(),
            t0 + Duration::from_secs(2) + k * Duration::from_secs(3)
        );
        assert_eq!(output.next(), Some(42));
        assert!(output.next().is_none());
    }
}

fn model_cancel_periodic_event(num_threads: usize) {
    #[derive(Default, Serialize, Deserialize)]
    struct TestModel {
        output: Output<()>,
        key: Option<EventKey>,
    }
    #[Model]
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &Context<Self>) {
            self.key = cx
                .schedule_keyed_periodic_event(
                    Duration::from_secs(2),
                    Duration::from_secs(3),
                    schedulable!(Self::action),
                    (),
                )
                .ok();
        }
        #[nexosim(schedulable)]
        async fn action(&mut self) {
            self.output.send(()).await;
            // Cancel the next events.
            self.key.take().unwrap().cancel();
        }
    }

    let mut model = TestModel::default();
    let mbox = Mailbox::new();

    let output = EventQueue::new();
    model.output.connect_sink(&output);
    let mut output = output.into_reader();
    let addr = mbox.address();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "")
        .init(t0)
        .unwrap();

    simu.process_event(TestModel::trigger, (), addr).unwrap();

    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(2));
    assert!(output.next().is_some());
    assert!(output.next().is_none());

    simu.step().unwrap();
    assert_eq!(simu.time(), t0 + Duration::from_secs(2));
    assert!(output.next().is_none());
}

#[test]
fn model_schedule_event_st() {
    model_schedule_event(1);
}

#[test]
fn model_schedule_event_mt() {
    model_schedule_event(MT_NUM_THREADS);
}

#[test]
fn multiple_models_scheduling_st() {
    multiple_models_scheduling(1);
}

#[test]
fn multiple_models_scheduling_mt() {
    multiple_models_scheduling(MT_NUM_THREADS);
}

#[test]
fn model_cancel_future_keyed_event_st() {
    model_cancel_future_keyed_event(1);
}

#[test]
fn model_cancel_future_keyed_event_mt() {
    model_cancel_future_keyed_event(MT_NUM_THREADS);
}

#[test]
fn model_cancel_same_time_keyed_event_st() {
    model_cancel_same_time_keyed_event(1);
}

#[test]
fn model_cancel_same_time_keyed_event_mt() {
    model_cancel_same_time_keyed_event(MT_NUM_THREADS);
}

#[test]
fn model_schedule_periodic_event_st() {
    model_schedule_periodic_event(1);
}

#[test]
fn model_schedule_periodic_event_mt() {
    model_schedule_periodic_event(MT_NUM_THREADS);
}

#[test]
fn model_cancel_periodic_event_st() {
    model_cancel_periodic_event(1);
}

#[test]
fn model_cancel_periodic_event_mt() {
    model_cancel_periodic_event(MT_NUM_THREADS);
}

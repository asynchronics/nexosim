//! Event scheduling within `Model` input methods.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::Context;
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{EventKey, Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::{schedulable, Model};

const MT_NUM_THREADS: usize = 4;

fn model_schedule_event(num_threads: usize) {
    #[derive(Default, Serialize, Deserialize)]
    struct TestModel {
        output: Output<()>,
    }
    #[Model]
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
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

fn model_cancel_future_keyed_event(num_threads: usize) {
    #[derive(Default, Serialize, Deserialize)]
    struct TestModel {
        output: Output<i32>,
        key: Option<EventKey>,
    }
    #[Model]
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
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
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
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
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
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
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
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

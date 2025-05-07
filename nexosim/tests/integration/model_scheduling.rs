//! Event scheduling within `Model` input methods.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{BuildContext, Context, InitializedModel, InputId, Model, ProtoModel};
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{EventKey, Mailbox, SimInit};
use nexosim::time::MonotonicTime;

const MT_NUM_THREADS: usize = 4;

fn model_schedule_event(num_threads: usize) {
    #[derive(Serialize, Deserialize)]
    struct TestModel {
        output: Output<()>,
        input: InputId<Self, ()>,
    }
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
            cx.schedule_event(Duration::from_secs(2), &self.input, ())
                .unwrap();
        }
        async fn action(&mut self) {
            self.output.send(()).await;
        }
    }
    impl Model for TestModel {
        type Env = ();
    }

    #[derive(Default)]
    struct ProtoTest {
        output: Output<()>,
    }
    impl ProtoModel for ProtoTest {
        type Model = TestModel;

        fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, <Self::Model as Model>::Env) {
            let input = cx.register_input(TestModel::action);
            (
                TestModel {
                    input,
                    output: self.output,
                },
                (),
            )
        }
    }

    let mut model = ProtoTest::default();
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
    #[derive(Serialize, Deserialize)]
    struct TestModel {
        output: Output<i32>,
        key: Option<EventKey>,
        input_1: InputId<Self, ()>,
        input_2: InputId<Self, ()>,
    }
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
            cx.schedule_event(Duration::from_secs(1), &self.input_1, ())
                .unwrap();
            self.key = cx
                .schedule_keyed_event(Duration::from_secs(2), &self.input_1, ())
                .ok();
        }
        async fn action1(&mut self) {
            self.output.send(1).await;
            // Cancel the call to `action2`.
            self.key.take().unwrap().cancel();
        }
        async fn action2(&mut self) {
            self.output.send(2).await;
        }
    }
    impl Model for TestModel {
        type Env = ();
    }

    #[derive(Default)]
    struct ProtoTest {
        output: Output<i32>,
    }
    impl ProtoModel for ProtoTest {
        type Model = TestModel;

        fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, <Self::Model as Model>::Env) {
            let input_1 = cx.register_input(TestModel::action1);
            let input_2 = cx.register_input(TestModel::action2);
            (
                TestModel {
                    input_1,
                    input_2,
                    output: self.output,
                    key: None,
                },
                (),
            )
        }
    }

    let mut model = ProtoTest::default();
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
    #[derive(Serialize, Deserialize)]
    struct TestModel {
        output: Output<i32>,
        key: Option<EventKey>,
        input_1: InputId<Self, ()>,
        input_2: InputId<Self, ()>,
    }
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
            cx.schedule_event(Duration::from_secs(2), &self.input_1, ())
                .unwrap();
            self.key = cx
                .schedule_keyed_event(Duration::from_secs(2), &self.input_2, ())
                .ok();
        }
        async fn action1(&mut self) {
            println!("Action 1");
            self.output.send(1).await;
            // Cancel the call to `action2`.
            self.key.take().unwrap().cancel();
            println!("Cancelled");
        }
        async fn action2(&mut self) {
            println!("Action 2");
            self.output.send(2).await;
        }
    }
    impl Model for TestModel {
        type Env = ();
    }

    #[derive(Default)]
    struct ProtoTest {
        output: Output<i32>,
    }
    impl ProtoModel for ProtoTest {
        type Model = TestModel;

        fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, <Self::Model as Model>::Env) {
            let input_1 = cx.register_input(TestModel::action1);
            let input_2 = cx.register_input(TestModel::action2);
            (
                TestModel {
                    input_1,
                    input_2,
                    output: self.output,
                    key: None,
                },
                (),
            )
        }
    }

    let mut model = ProtoTest::default();
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
    #[derive(Serialize, Deserialize)]
    struct TestModel {
        input: InputId<Self, i32>,
        output: Output<i32>,
    }
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
            cx.schedule_periodic_event(
                Duration::from_secs(2),
                Duration::from_secs(3),
                &self.input,
                42,
            )
            .unwrap();
        }
        async fn action(&mut self, payload: i32) {
            self.output.send(payload).await;
        }
    }
    impl Model for TestModel {
        type Env = ();
    }

    #[derive(Default)]
    struct ProtoTest {
        output: Output<i32>,
    }
    impl ProtoModel for ProtoTest {
        type Model = TestModel;

        fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, <Self::Model as Model>::Env) {
            let input = cx.register_input(TestModel::action);
            (
                TestModel {
                    input,
                    output: self.output,
                },
                (),
            )
        }
    }

    let mut model = ProtoTest::default();
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
    #[derive(Serialize, Deserialize)]
    struct TestModel {
        input: InputId<Self, ()>,
        output: Output<()>,
        key: Option<EventKey>,
    }
    impl TestModel {
        fn trigger(&mut self, _: (), cx: &mut Context<Self>) {
            self.key = cx
                .schedule_keyed_periodic_event(
                    Duration::from_secs(2),
                    Duration::from_secs(3),
                    &self.input,
                    (),
                )
                .ok();
        }
        async fn action(&mut self) {
            self.output.send(()).await;
            // Cancel the next events.
            self.key.take().unwrap().cancel();
        }
    }
    impl Model for TestModel {
        type Env = ();
    }

    #[derive(Default)]
    struct ProtoTest {
        output: Output<()>,
    }
    impl ProtoModel for ProtoTest {
        type Model = TestModel;

        fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, <Self::Model as Model>::Env) {
            let input = cx.register_input(TestModel::action);
            (
                TestModel {
                    input,
                    output: self.output,
                    key: None,
                },
                (),
            )
        }
    }

    let mut model = ProtoTest::default();
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

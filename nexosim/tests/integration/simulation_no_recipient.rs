//! Missing recipient detection.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::ports::{Output, Requestor};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::Model;

const MT_NUM_THREADS: usize = 4;

#[derive(Default, Serialize, Deserialize)]
struct TestModel {
    output: Output<()>,
    requestor: Requestor<(), ()>,
}
#[Model]
impl TestModel {
    async fn activate_output(&mut self) {
        self.output.send(()).await;
    }
    async fn activate_requestor(&mut self) {
        let _ = self.requestor.send(()).await;
    }
}

/// Send an event from a model to a dead input.
fn no_input_from_model(num_threads: usize) {
    const MODEL_NAME: &str = "testmodel";

    let mut model = TestModel::default();
    let mbox = Mailbox::new();
    let addr = mbox.address();
    let bad_mbox = Mailbox::new();

    model.output.connect(TestModel::activate_output, &bad_mbox);

    drop(bad_mbox);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, MODEL_NAME)
        .init(t0)
        .unwrap();

    match simu.process_event(TestModel::activate_output, (), addr) {
        Err(ExecutionError::NoRecipient { model }) => {
            assert_eq!(model, Some(String::from(MODEL_NAME)));
        }
        _ => panic!("missing recipient not detected"),
    }
}

/// Send an event from a model to a dead replier.
fn no_replier_from_model(num_threads: usize) {
    const MODEL_NAME: &str = "testmodel";

    let mut model = TestModel::default();
    let mbox = Mailbox::new();
    let addr = mbox.address();
    let bad_mbox = Mailbox::new();

    model
        .requestor
        .connect(TestModel::activate_requestor, &bad_mbox);

    drop(bad_mbox);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, MODEL_NAME)
        .init(t0)
        .unwrap();

    match simu.process_event(TestModel::activate_requestor, (), addr) {
        Err(ExecutionError::NoRecipient { model }) => {
            assert_eq!(model, Some(String::from(MODEL_NAME)));
        }
        _ => panic!("missing recipient not detected"),
    }
}

/// Send an event from the scheduler to a dead input.
fn no_input_from_scheduler(num_threads: usize) {
    let bad_mbox = Mailbox::new();

    let t0 = MonotonicTime::EPOCH;
    let mut bench = SimInit::with_num_threads(num_threads);

    let source_id = bench.register_input(TestModel::activate_output, &bad_mbox);
    drop(bad_mbox);

    let mut simu = bench.init(t0).unwrap();
    let scheduler = simu.scheduler();

    scheduler
        .schedule_event(Duration::from_secs(1), &source_id, ())
        .unwrap();

    match simu.step() {
        Err(ExecutionError::NoRecipient { model }) => {
            assert_eq!(model, None);
        }
        _ => panic!("missing recipient not detected"),
    }
}

/// Process a query on a dead input.
fn no_replier_from_query(num_threads: usize) {
    let bad_mbox = Mailbox::new();
    let addr = bad_mbox.address();

    drop(bad_mbox);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads).init(t0).unwrap();

    let result = simu.process_query(TestModel::activate_requestor, (), addr);

    match result {
        Err(ExecutionError::BadQuery) => (),
        _ => panic!("missing recipient not detected"),
    }
}

#[test]
fn no_input_from_model_st() {
    no_input_from_model(1);
}

#[test]
fn no_input_from_model_mt() {
    no_input_from_model(MT_NUM_THREADS);
}

#[test]
fn no_replier_from_model_st() {
    no_replier_from_model(1);
}

#[test]
fn no_replier_from_model_mt() {
    no_replier_from_model(MT_NUM_THREADS);
}

#[test]
fn no_input_from_scheduler_st() {
    no_input_from_scheduler(1);
}

#[test]
fn no_input_from_scheduler_mt() {
    no_input_from_scheduler(MT_NUM_THREADS);
}

#[test]
fn no_replier_from_query_st() {
    no_replier_from_query(1);
}

#[test]
fn no_replier_from_query_mt() {
    no_replier_from_query(MT_NUM_THREADS);
}

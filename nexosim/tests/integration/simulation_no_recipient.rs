//! Missing recipient detection.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::path::Path;
use nexosim::ports::{EventSource, Output, QuerySource, Requestor};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit};
use nexosim::time::MonotonicTime;

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

    let mut bench = SimInit::with_num_threads(num_threads);

    let activate_output = EventSource::new()
        .connect(TestModel::activate_output, &addr)
        .register(&mut bench);

    let mut simu = bench
        .add_model(model, mbox, MODEL_NAME)
        .init(MonotonicTime::EPOCH)
        .unwrap();

    match simu.process_event(&activate_output, ()) {
        Err(ExecutionError::NoRecipient { model }) => {
            assert_eq!(model, Some(Path::from(MODEL_NAME)));
        }
        _ => panic!("missing recipient not detected"),
    }
}

/// Send an event from a model to a dead replier.
fn no_replier_from_model(num_threads: usize) {
    const MODEL_NAME: &str = "testmodel";

    let mut model = TestModel::default();
    let mbox = Mailbox::new();
    let bad_mbox = Mailbox::new();

    model
        .requestor
        .connect(TestModel::activate_requestor, &bad_mbox);

    drop(bad_mbox);

    let mut bench = SimInit::with_num_threads(num_threads);

    let activate_requestor = EventSource::new()
        .connect(TestModel::activate_requestor, &mbox)
        .register(&mut bench);

    let mut simu = bench
        .add_model(model, mbox, MODEL_NAME)
        .init(MonotonicTime::EPOCH)
        .unwrap();

    match simu.process_event(&activate_requestor, ()) {
        Err(ExecutionError::NoRecipient { model }) => {
            assert_eq!(model, Some(Path::from(MODEL_NAME)));
        }
        _ => panic!("missing recipient not detected"),
    }
}

/// Send an event from the scheduler to a dead input.
fn no_input_from_scheduler(num_threads: usize) {
    let bad_mbox = Mailbox::new();

    let mut bench = SimInit::with_num_threads(num_threads);

    let activate_output = EventSource::new()
        .connect(TestModel::activate_output, &bad_mbox)
        .register(&mut bench);

    drop(bad_mbox);

    let mut simu = bench.init(MonotonicTime::EPOCH).unwrap();
    let scheduler = simu.scheduler();

    scheduler
        .schedule_event(Duration::from_secs(1), &activate_output, ())
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

    let mut bench = SimInit::with_num_threads(num_threads);

    let query = QuerySource::new()
        .connect(TestModel::activate_requestor, &addr)
        .register(&mut bench);

    let mut simu = bench.init(MonotonicTime::EPOCH).unwrap();

    let result = simu.process_query(&query, ());

    match result {
        Err(ExecutionError::NoRecipient { .. }) => (),
        _ => panic!("missing recipient not detected"),
    }
}

/// Drop an address and recreate one, making sure it does not trigger a missing
/// recipient.
///
/// See: https://github.com/asynchronics/nexosim/issues/125
fn dropped_address(num_threads: usize) {
    const MODEL_A_NAME: &str = "testmodel_A";
    const MODEL_B_NAME: &str = "testmodel_B";

    let mut model_a = TestModel::default();
    let model_b = TestModel::default();
    let mbox_a = Mailbox::new();
    let mbox_b = Mailbox::new();
    let addr_a = mbox_a.address();
    let _ = mbox_b.address(); // Create and drop immediately an address before any other exists.

    let mut bench = SimInit::with_num_threads(num_threads);

    model_a
        .output
        .connect(TestModel::activate_requestor, &mbox_b);

    let activate_output = EventSource::new()
        .connect(TestModel::activate_output, addr_a)
        .register(&mut bench);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench
        .add_model(model_a, mbox_a, MODEL_A_NAME)
        .add_model(model_b, mbox_b, MODEL_B_NAME)
        .init(t0)
        .unwrap();

    assert!(simu.process_event(&activate_output, ()).is_ok());
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

#[test]
fn dropped_address_st() {
    dropped_address(1);
}

#[test]
fn dropped_address_mt() {
    dropped_address(MT_NUM_THREADS);
}

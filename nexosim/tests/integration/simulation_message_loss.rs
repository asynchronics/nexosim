//! Message loss detection.

use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::ports::{EventSource, Output, Requestor};
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
    async fn activate_output_twice(&mut self) {
        self.output.send(()).await;
        self.output.send(()).await;
    }
    async fn activate_requestor_twice(&mut self) {
        let _ = self.requestor.send(()).await;
        let _ = self.requestor.send(()).await;
    }
}

/// Loose an event.
fn event_loss(num_threads: usize) {
    let mut model = TestModel::default();
    let mbox = Mailbox::new();
    let bad_mbox = Mailbox::new();

    // Make two self-connections so that each outgoing message generates two
    // incoming messages.
    model
        .output
        .connect(TestModel::activate_output_twice, &bad_mbox);

    let mut bench = SimInit::with_num_threads(num_threads);

    let activate_output_twice = EventSource::new()
        .connect(TestModel::activate_output_twice, &mbox)
        .register(&mut bench);

    let mut simu = bench
        .add_model(model, mbox, "")
        .init(MonotonicTime::EPOCH)
        .unwrap();

    match simu.process_event(&activate_output_twice, ()) {
        Err(ExecutionError::MessageLoss(msg_count)) => {
            assert_eq!(msg_count, 2);
        }
        _ => panic!("message loss not detected"),
    }
}

/// Loose an event.
fn request_loss(num_threads: usize) {
    let mut model = TestModel::default();
    let mbox = Mailbox::new();
    let addr = mbox.address();
    let bad_mbox = Mailbox::new();

    model
        .requestor
        .connect(TestModel::activate_requestor_twice, &bad_mbox);

    let mut bench = SimInit::with_num_threads(num_threads);

    let activate_requestor_twice = EventSource::new()
        .connect(TestModel::activate_requestor_twice, &addr)
        .register(&mut bench);

    let mut simu = bench
        .add_model(model, mbox, "")
        .init(MonotonicTime::EPOCH)
        .unwrap();

    match simu.process_event(&activate_requestor_twice, ()) {
        Err(ExecutionError::MessageLoss(msg_count)) => {
            assert_eq!(msg_count, 1);
        }
        _ => panic!("message loss not detected"),
    }
}

#[test]
fn event_loss_st() {
    event_loss(1);
}

#[test]
fn event_loss_mt() {
    event_loss(MT_NUM_THREADS);
}

#[test]
fn request_loss_st() {
    request_loss(1);
}

#[test]
fn request_loss_mt() {
    request_loss(MT_NUM_THREADS);
}

//! Deadlock-detection for model loops.

use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::ports::{EventSource, Output, QuerySource, Requestor};
use nexosim::simulation::{DeadlockInfo, ExecutionError, Mailbox, SimInit};
use nexosim::time::MonotonicTime;

const MT_NUM_THREADS: usize = 4;

#[derive(Default, Deserialize, Serialize)]
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

/// Overflows a mailbox by sending 2 messages in loopback for each incoming
/// message.
fn deadlock_on_mailbox_overflow(num_threads: usize) {
    const MODEL_NAME: &str = "testmodel";
    const MAILBOX_SIZE: usize = 5;

    let mut model = TestModel::default();
    let mbox = Mailbox::with_capacity(MAILBOX_SIZE);

    let mut bench = SimInit::with_num_threads(num_threads);

    let activate_output = EventSource::new()
        .connect(TestModel::activate_output, &mbox)
        .register(&mut bench);

    // Make two self-connections so that each outgoing message generates two
    // incoming messages.
    model.output.connect(TestModel::activate_output, &mbox);
    model.output.connect(TestModel::activate_output, &mbox);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench.add_model(model, mbox, MODEL_NAME).init(t0).unwrap();

    match simu.process_event(&activate_output, ()) {
        Err(ExecutionError::Deadlock(deadlock_info)) => {
            // We expect only 1 deadlocked model.
            assert_eq!(deadlock_info.len(), 1);
            // We expect the mailbox to be full.
            assert_eq!(
                deadlock_info[0],
                DeadlockInfo {
                    model: MODEL_NAME.into(),
                    mailbox_size: MAILBOX_SIZE
                }
            )
        }
        _ => panic!("deadlock not detected"),
    }
}

/// Generates a deadlock with a query loopback.
fn deadlock_on_query_loopback(num_threads: usize) {
    const MODEL_NAME: &str = "testmodel";

    let mut model = TestModel::default();
    let mbox = Mailbox::new();

    model
        .requestor
        .connect(TestModel::activate_requestor, &mbox);

    let mut bench = SimInit::with_num_threads(num_threads);

    let query = QuerySource::new()
        .connect(TestModel::activate_requestor, &mbox)
        .register(&mut bench);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench.add_model(model, mbox, MODEL_NAME).init(t0).unwrap();

    match simu.process_query(&query, ()) {
        Err(ExecutionError::Deadlock(deadlock_info)) => {
            // We expect only 1 deadlocked model.
            assert_eq!(deadlock_info.len(), 1);
            // We expect the mailbox to have a single query.
            assert_eq!(
                deadlock_info[0],
                DeadlockInfo {
                    model: MODEL_NAME.into(),
                    mailbox_size: 1,
                }
            );
        }
        _ => panic!("deadlock not detected"),
    }
}

/// Generates a deadlock with a query loopback involving several models.
fn deadlock_on_transitive_query_loopback(num_threads: usize) {
    const MODEL1_NAME: &str = "testmodel1";
    const MODEL2_NAME: &str = "testmodel2";

    let mut model1 = TestModel::default();
    let mut model2 = TestModel::default();
    let mbox1 = Mailbox::new();
    let mbox2 = Mailbox::new();

    model1
        .requestor
        .connect(TestModel::activate_requestor, &mbox2);

    model2
        .requestor
        .connect(TestModel::activate_requestor, &mbox1);

    let mut bench = SimInit::with_num_threads(num_threads);

    let query = QuerySource::new()
        .connect(TestModel::activate_requestor, &mbox1)
        .register(&mut bench);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench
        .add_model(model1, mbox1, MODEL1_NAME)
        .add_model(model2, mbox2, MODEL2_NAME)
        .init(t0)
        .unwrap();

    match simu.process_query(&query, ()) {
        Err(ExecutionError::Deadlock(deadlock_info)) => {
            // We expect only 1 deadlocked model.
            assert_eq!(deadlock_info.len(), 1);
            // We expect the mailbox of this model to have a single query.
            assert_eq!(
                deadlock_info[0],
                DeadlockInfo {
                    model: MODEL1_NAME.into(),
                    mailbox_size: 1,
                }
            );
        }
        _ => panic!("deadlock not detected"),
    }
}

/// Generates deadlocks with query loopbacks on several models at the same time.
fn deadlock_on_multiple_query_loopback(num_threads: usize) {
    const MODEL0_NAME: &str = "testmodel0";
    const MODEL1_NAME: &str = "testmodel1";
    const MODEL2_NAME: &str = "testmodel2";

    let mut model0 = TestModel::default();
    let mut model1 = TestModel::default();
    let mut model2 = TestModel::default();
    let mbox0 = Mailbox::new();
    let mbox1 = Mailbox::new();
    let mbox2 = Mailbox::new();

    model0
        .requestor
        .connect(TestModel::activate_requestor, &mbox1);

    model0
        .requestor
        .connect(TestModel::activate_requestor, &mbox2);

    model1
        .requestor
        .connect(TestModel::activate_requestor, &mbox1);

    model2
        .requestor
        .connect(TestModel::activate_requestor, &mbox2);

    let mut bench = SimInit::with_num_threads(num_threads);

    let query = QuerySource::new()
        .connect(TestModel::activate_requestor, &mbox0)
        .register(&mut bench);

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench
        .add_model(model0, mbox0, MODEL0_NAME)
        .add_model(model1, mbox1, MODEL1_NAME)
        .add_model(model2, mbox2, MODEL2_NAME)
        .init(t0)
        .unwrap();

    match simu.process_query(&query, ()) {
        Err(ExecutionError::Deadlock(deadlock_info)) => {
            // We expect 2 deadlocked models.
            assert_eq!(deadlock_info.len(), 2);
            // We expect the mailbox of each deadlocked model to have a single
            // query.
            assert_eq!(
                deadlock_info[0],
                DeadlockInfo {
                    model: MODEL1_NAME.into(),
                    mailbox_size: 1,
                }
            );
            assert_eq!(
                deadlock_info[1],
                DeadlockInfo {
                    model: MODEL2_NAME.into(),
                    mailbox_size: 1,
                }
            );
        }
        _ => panic!("deadlock not detected"),
    }
}

#[test]
fn deadlock_on_mailbox_overflow_st() {
    deadlock_on_mailbox_overflow(1);
}

#[test]
fn deadlock_on_mailbox_overflow_mt() {
    deadlock_on_mailbox_overflow(MT_NUM_THREADS);
}

#[test]
fn deadlock_on_query_loopback_st() {
    deadlock_on_query_loopback(1);
}

#[test]
fn deadlock_on_query_loopback_mt() {
    deadlock_on_query_loopback(MT_NUM_THREADS);
}

#[test]
fn deadlock_on_transitive_query_loopback_st() {
    deadlock_on_transitive_query_loopback(1);
}

#[test]
fn deadlock_on_transitive_query_loopback_mt() {
    deadlock_on_transitive_query_loopback(MT_NUM_THREADS);
}

#[test]
fn deadlock_on_multiple_query_loopback_st() {
    deadlock_on_multiple_query_loopback(1);
}

#[test]
fn deadlock_on_multiple_query_loopback_mt() {
    deadlock_on_multiple_query_loopback(MT_NUM_THREADS);
}

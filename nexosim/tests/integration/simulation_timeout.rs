//! Timeout during simulation step execution.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{BuildContext, Model, ProtoModel};
use nexosim::ports::Output;
use nexosim::simulation::{ExecutionError, Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::Model;

const MT_NUM_THREADS: usize = 4;

#[derive(Serialize, Deserialize)]
struct TestModel {
    output: Output<()>,
}
#[Model(type Env=TestEnv)]
impl TestModel {
    async fn input(&mut self) {
        self.output.send(()).await;
    }
}

struct TestEnv {
    // A liveliness flag that is cleared when the model is dropped.
    is_alive: Arc<AtomicBool>,
}
impl Drop for TestEnv {
    fn drop(&mut self) {
        self.is_alive.store(false, Ordering::Relaxed);
    }
}

struct TestProto {
    output: Output<()>,
    is_alive: Arc<AtomicBool>,
}
impl ProtoModel for TestProto {
    type Model = TestModel;

    fn build(self, _: &mut BuildContext<Self>) -> (Self::Model, <Self::Model as Model>::Env) {
        (
            TestModel {
                output: self.output,
            },
            TestEnv {
                is_alive: self.is_alive,
            },
        )
    }
}

fn timeout_untriggered(num_threads: usize) {
    let model = TestProto {
        output: Output::default(),
        is_alive: Arc::new(AtomicBool::new(true)),
    };
    let mbox = Mailbox::new();
    let addr = mbox.address();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "test")
        .set_timeout(Duration::from_secs(1))
        .init(t0)
        .unwrap()
        .0;

    assert!(simu.process_event(TestModel::input, (), addr).is_ok());
}

fn timeout_triggered(num_threads: usize) {
    let model_is_alive = Arc::new(AtomicBool::new(true));
    let mut model = TestProto {
        output: Output::default(),
        is_alive: model_is_alive.clone(),
    };
    let mbox = Mailbox::new();
    let addr = mbox.address();

    // Make a loopback connection.
    model.output.connect(TestModel::input, addr.clone());

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::with_num_threads(num_threads)
        .add_model(model, mbox, "test")
        .set_timeout(Duration::from_secs(1))
        .init(t0)
        .unwrap()
        .0;

    assert!(matches!(
        simu.process_event(TestModel::input, (), addr),
        Err(ExecutionError::Timeout)
    ));

    // Make sure the request to stop the simulation has succeeded.
    thread::sleep(Duration::from_millis(10));
    assert!(!model_is_alive.load(Ordering::Relaxed));
}

#[test]
fn timeout_untriggered_st() {
    timeout_untriggered(1);
}

#[test]
fn timeout_untriggered_mt() {
    timeout_untriggered(MT_NUM_THREADS);
}

#[test]
fn timeout_triggered_st() {
    timeout_triggered(1);
}

#[test]
fn timeout_triggered_mt() {
    timeout_triggered(MT_NUM_THREADS);
}

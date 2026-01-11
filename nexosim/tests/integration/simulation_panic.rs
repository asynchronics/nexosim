//! Model panic reporting.

use serde::{Deserialize, Serialize};

use nexosim::model::Model;
use nexosim::path::Path;
use nexosim::ports::{EventSource, Output};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit};
use nexosim::time::MonotonicTime;

const MT_NUM_THREADS: usize = 4;

#[derive(Default, Deserialize, Serialize)]
struct TestModel {
    countdown_out: Output<usize>,
}
#[Model]
impl TestModel {
    async fn countdown_in(&mut self, count: usize) {
        if count == 0 {
            panic!("test message");
        }
        self.countdown_out.send(count - 1).await;
    }
}

/// Pass a counter around several models and decrement it each time, panicking
/// when it becomes zero.
fn model_panic(num_threads: usize) {
    const MODEL_COUNT: usize = 5;
    const INIT_COUNTDOWN: usize = 9;

    // Connect all models in a cycle graph.
    let mut model0 = TestModel::default();
    let mbox0 = Mailbox::new();

    let mut bench = SimInit::with_num_threads(num_threads);

    let mut addr = mbox0.address();
    for model_id in (1..MODEL_COUNT).rev() {
        let mut model = TestModel::default();
        let mbox = Mailbox::new();
        model.countdown_out.connect(TestModel::countdown_in, addr);
        addr = mbox.address();
        bench = bench.add_model(model, mbox, &model_id.to_string());
    }
    model0.countdown_out.connect(TestModel::countdown_in, addr);

    let countdown_in = EventSource::new()
        .connect(TestModel::countdown_in, &mbox0)
        .register(&mut bench);

    bench = bench.add_model(model0, mbox0, &0.to_string());

    // Run the simulation.
    let mut simu = bench.init(MonotonicTime::EPOCH).unwrap();

    match simu.process_event(&countdown_in, INIT_COUNTDOWN) {
        Err(ExecutionError::Panic { model, payload }) => {
            let msg = payload.downcast_ref::<&str>().unwrap();
            let panicking_model_id = INIT_COUNTDOWN % MODEL_COUNT;

            assert_eq!(model, Path::from(panicking_model_id.to_string()));
            assert_eq!(*msg, "test message");
        }
        _ => panic!("panic not detected"),
    }
}

#[test]
fn model_panic_st() {
    model_panic(1);
}

#[test]
fn model_panic_mt() {
    model_panic(MT_NUM_THREADS);
}

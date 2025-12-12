use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{schedulable, Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, EventSource, Output, QuerySource};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;

/// Stepper motor.
#[derive(Serialize, Deserialize)]
pub struct MyModel {
    value: u64,
}
#[Model]
impl MyModel {
    #[nexosim(schedulable)]
    pub fn update_value(&mut self) {
        self.value += 1;
    }
    pub async fn get_value(&mut self) -> u64 {
        self.value
    }
}

#[allow(dead_code)]
fn main() -> Result<(), nexosim::simulation::SimulationError> {
    let model = MyModel { value: 0 };
    let mbox = Mailbox::new();
    let model_addr = mbox.address();

    let t0 = MonotonicTime::EPOCH;

    let mut bench = SimInit::new().add_model(model, mbox, "my_model");

    let event_id = EventSource::new()
        .connect(MyModel::update_value, &model_addr)
        .register(&mut bench);

    let query_id = QuerySource::new()
        .connect(MyModel::get_value, &model_addr)
        .register(&mut bench);

    let mut simu = bench.init(t0)?;
    let scheduler = simu.scheduler();

    scheduler
        .schedule_event(Duration::from_secs(2), &event_id, ())
        .unwrap();

    let mut reader = scheduler
        .schedule_query(Duration::from_secs(3), &query_id, ())
        .unwrap();

    simu.step()?;
    assert!(reader.try_read().is_none());

    simu.step()?;
    assert_eq!(reader.read().unwrap().next(), Some(1));

    Ok(())
}

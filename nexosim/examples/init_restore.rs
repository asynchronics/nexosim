use std::iter;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, InitializedModel, Model, schedulable};
use nexosim::ports::{EventSinkReader, EventSource, Output, QuerySource, SinkState, event_queue};
use nexosim::simulation::{EventId, Mailbox, QueryId, SimInit};
use nexosim::time::MonotonicTime;

#[derive(Serialize, Deserialize)]
pub struct Motor {
    pub value: u32,
}
#[Model]
impl Motor {
    #[nexosim(init)]
    async fn init(&mut self, cx: &Context<Self>) {
        println!("@@@@@@ INIT");
        // cx.schedule_event(Duration::from_secs(1), schedulable!(Self::add),
        // 3);
    }

    #[nexosim(restore)]
    async fn restore(&mut self) {
        println!("@@@@@@ RESTORE");
    }

    #[nexosim(schedulable)]
    fn add(&mut self, arg: u32) {
        println!("Adding");
        self.value += arg;
    }

    async fn query(&mut self) -> u32 {
        self.value
    }
}

fn get_bench() -> (SimInit, EventId<u32>, QueryId<(), u32>) {
    let motor = Motor { value: 3 };

    // Mailboxes.
    let motor_mbox = Mailbox::new();
    let mut bench = SimInit::new();

    let add = EventSource::new()
        .connect(Motor::add, &motor_mbox)
        .register(&mut bench);
    let query = QuerySource::new()
        .connect(Motor::query, &motor_mbox)
        .register(&mut bench);
    bench = bench.add_model(motor, motor_mbox, "motor");

    (bench, add, query)
}

#[allow(dead_code)]
fn main() -> Result<(), nexosim::simulation::SimulationError> {
    let (mut bench, add, _) = get_bench();

    let t0 = MonotonicTime::EPOCH; // arbitrary since models do not depend on absolute time
    let mut simu = bench.init(t0)?;
    simu.scheduler()
        .schedule_event(Duration::from_secs(1), &add, 4);

    simu.step_unbounded()?;

    println!("PRE SAVE");
    let mut state = Vec::new();
    simu.save(&mut state)?;

    let (bench, _, query) = get_bench();
    let (mut simu, _) = bench.restore(&state[..])?;

    println!("{:?}", simu.process_query(&query, ()));

    Ok(())
}

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nexosim::model::{Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{MonotonicTime, SystemClock};

#[derive(Default)]
struct DelayedModel {
    pub message: Output<()>,
    delay: Duration,
}
impl DelayedModel {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            ..Default::default()
        }
    }
    async fn process(&mut self) {
        self.message.send(()).await;
    }
}
impl Model for DelayedModel {
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_periodic_event(self.delay, self.delay, DelayedModel::process, ())
            .unwrap();
        self.into()
    }
}

fn main() -> Result<(), SimulationError> {
    let mut model = DelayedModel::new(Duration::from_secs(2));
    let mailbox = Mailbox::new();

    let message = EventQueue::new();
    model.message.connect_sink(&message);
    let mut message = message.into_reader();

    let t0 = MonotonicTime::EPOCH;
    let now = Instant::now();

    let (simu, mut scheduler) = SimInit::new()
        .add_model(model, mailbox, "timed_model")
        .set_clock(SystemClock::from_instant(t0, now))
        .init(t0)?;

    let simulation = Arc::new(Mutex::new(simu));
    let spawned_simulation = simulation.clone();

    thread::spawn(move || spawned_simulation.lock().unwrap().step_unbounded());

    thread::sleep(Duration::from_secs(1));

    scheduler.pause();
    // assert that the step has completed, even though `pause` was called before it's scheduled time
    assert!(message.next().is_some());

    // step can't be performed while paused
    assert!(simulation.lock().unwrap().step().is_err());
    thread::sleep(Duration::from_secs(3));

    // paused - no new messages
    assert!(message.next().is_none());
    scheduler.unpause();

    let spawned_simulation = simulation.clone();
    let simulation_handle =
        thread::spawn(move || spawned_simulation.lock().unwrap().step_unbounded());

    thread::sleep(Duration::from_secs(1));
    // now new messages yet, as the paused time should be invisible to the scheduler
    assert!(message.next().is_none());

    thread::sleep(Duration::from_secs(1));
    assert!(message.next().is_some());

    scheduler.pause();

    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Paused) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}

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

    let (mut simu, mut scheduler) = SimInit::new()
        .add_model(model, mailbox, "timed_model")
        .set_clock(SystemClock::from_instant(t0, now))
        .init(t0)?;

    let simulation_handle = thread::spawn(move || simu.step_unbounded());

    thread::sleep(Duration::from_secs(1));
    // pause at simulation time = ca. 1s
    scheduler.pause();
    thread::sleep(Duration::from_secs(3));
    scheduler.unpause();
    // the simulation time is still < 2s
    assert!(message.next().is_none());

    // now we wait for the simulation time of about 2s
    thread::sleep(Duration::from_secs(1));
    assert!(message.next().is_some());

    // re-check another cycle if still in sync
    thread::sleep(Duration::from_secs(1));
    assert!(message.next().is_none());
    thread::sleep(Duration::from_secs(1));
    assert!(message.next().is_some());
    scheduler.halt();

    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}

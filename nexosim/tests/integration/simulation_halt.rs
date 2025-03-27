//! Simulation halt and resume.
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use nexosim::model::{Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{MonotonicTime, SystemClock};

struct RecurringModel {
    pub message: Output<()>,
    delay: Duration,
}
impl RecurringModel {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            message: Output::new(),
        }
    }
    async fn process(&mut self) {
        self.message.send(()).await;
    }
}
impl Model for RecurringModel {
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_periodic_event(self.delay, self.delay, RecurringModel::process, ())
            .unwrap();
        self.into()
    }
}

#[test]
fn halt_and_resume() -> Result<(), SimulationError> {
    let mut model = RecurringModel::new(Duration::from_secs(2));
    let mailbox = Mailbox::new();

    let message = EventQueue::new();
    model.message.connect_sink(&message);
    let mut message = message.into_reader();

    let t0 = MonotonicTime::EPOCH;

    let (simu, mut scheduler) = SimInit::new()
        .add_model(model, mailbox, "timed_model")
        .set_clock(SystemClock::from_instant(t0, Instant::now()))
        .init(t0)?;

    let simulation = Arc::new(Mutex::new(simu));
    let spawned_simulation = simulation.clone();

    let simulation_handle =
        thread::spawn(move || spawned_simulation.lock().unwrap().step_unbounded());

    thread::sleep(Duration::from_secs(1));

    scheduler.halt();
    thread::sleep(Duration::from_secs(1));

    // Assert that the step has completed, even though `halt` was called before
    // its scheduled time.
    assert!(message.next().is_some());

    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => (),
        Err(e) => return Err(e.into()),
        _ => (),
    };

    thread::sleep(Duration::from_secs(3));

    // Halted - no new messages.
    assert!(message.next().is_none());

    {
        let mut s = simulation.lock().unwrap();
        let t = s.time();
        s.reset_clock(SystemClock::from_instant(t, Instant::now()));
    }
    let spawned_simulation = simulation.clone();
    let simulation_handle =
        thread::spawn(move || spawned_simulation.lock().unwrap().step_unbounded());

    thread::sleep(Duration::from_secs(1));
    // No new messages yet, as the halted time should be invisible to the
    // scheduler.
    assert!(message.next().is_none());

    thread::sleep(Duration::from_secs(2));
    assert!(message.next().is_some());

    scheduler.halt();

    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}

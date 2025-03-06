//! Example: a simulation that runs infinitely, receiving data from
//! outside. This setup is typical for hardware-in-the-loop use case.
//!
//! This example demonstrates in particular:
//!
//! * infinite simulation (useful in hardware-in-the-loop),
//! * simulation halting,
//! * processing of external data (useful in co-simulation),
//! * system clock,
//! * periodic scheduling.
//!
//! ```text
//!                              ┏━━━━━━━━━━━━━━━━━━━━━━━━┓
//!                              ┃ Simulation             ┃
//!┌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┐           ┃   ┌──────────┐         ┃
//!┆                 ┆  message  ┃   │          │ message ┃
//!┆ External thread ├╌╌╌╌╌╌╌╌╌╌╌╂╌╌►│ Listener ├─────────╂─►
//!┆                 ┆ [channel] ┃   │          │         ┃
//!└╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┘           ┃   └──────────┘         ┃
//!                              ┗━━━━━━━━━━━━━━━━━━━━━━━━┛
//! ```

use std::sync::mpsc::{channel, Receiver};
use std::thread::{self, sleep};
use std::time::Duration;

use nexosim::model::{Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};

const DELTA: Duration = Duration::from_millis(2);
const PERIOD: Duration = Duration::from_millis(20);
const N: usize = 10;

/// The `Listener` Model.
pub struct Listener {
    /// Received message.
    pub message: Output<String>,

    /// Source of external messages.
    external: Receiver<String>,
}

impl Listener {
    /// Creates new `Listener` model.
    fn new(external: Receiver<String>) -> Self {
        Self {
            message: Output::default(),
            external,
        }
    }

    /// Periodically scheduled function that processes external events.
    async fn process(&mut self) {
        while let Ok(message) = self.external.try_recv() {
            self.message.send(message).await;
        }
    }
}

impl Model for Listener {
    /// Initialize model.
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        // Schedule periodic function that processes external events.
        cx.schedule_periodic_event(DELTA, PERIOD, Listener::process, ())
            .unwrap();

        self.into()
    }
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Channel for communication with simulation from outside.
    let (tx, rx) = channel();

    // Models.

    // The listener model.
    let mut listener = Listener::new(rx);

    // Mailboxes.
    let listener_mbox = Mailbox::new();

    // Model handles for simulation.
    let message = EventQueue::new();
    listener.message.connect_sink(&message);
    let mut message = message.into_reader();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let (mut simu, mut scheduler) = SimInit::new()
        .add_model(listener, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    // Simulation thread.
    let simulation_handle = thread::spawn(move || {
        // ----------
        // Simulation.
        // ----------
        simu.step_unbounded()
    });

    // Send data to simulation from outside.
    for i in 0..N {
        tx.send(i.to_string()).unwrap();
        if i % 3 == 0 {
            sleep(PERIOD * i as u32)
        }
    }

    // Check collected external messages.
    for i in 0..N {
        assert_eq!(message.next().unwrap(), i.to_string());
    }
    assert_eq!(message.next(), None);

    // Stop the simulation.
    scheduler.halt();
    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}

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
//!                               ┏━━━━━━━━━━━━━━━━━━━━━━━━┓
//!                               ┃ Simulation             ┃
//! ┌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┐           ┃   ┌──────────┐         ┃
//! ┆                 ┆  message  ┃   │          │ message ┃
//! ┆ External thread ├╌╌╌╌╌╌╌╌╌╌╌╂╌╌►│ Listener ├─────────╂─►
//! ┆                 ┆ [channel] ┃   │          │         ┃
//! └╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┘           ┃   └──────────┘         ┃
//!                               ┗━━━━━━━━━━━━━━━━━━━━━━━━┛
//! ```

use std::sync::mpsc::{Receiver, channel};
use std::thread::{self, sleep};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, InitializedModel, Model, ProtoModel, schedulable};
use nexosim::ports::{EventSinkReader, Output, SinkState, event_queue};
use nexosim::simulation::{ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};

const DELTA: Duration = Duration::from_millis(2);
const PERIOD: Duration = Duration::from_millis(20);
const N: usize = 10;

/// The `Listener` Model.
#[derive(Serialize, Deserialize)]
pub struct Listener {
    /// Received message.
    pub message: Output<String>,
}

#[Model(type Env=ListenerEnv)]
impl Listener {
    /// Creates new `Listener` model.
    fn new(message: Output<String>) -> Self {
        Self { message }
    }

    /// Initialize model.
    #[nexosim(init)]
    async fn init(self, cx: &Context<Self>) -> InitializedModel<Self> {
        // Schedule periodic function that processes external events.
        cx.schedule_periodic_event(DELTA, PERIOD, schedulable!(Self::process), ())
            .unwrap();

        self.into()
    }

    /// Periodically scheduled function that processes external events.
    #[nexosim(schedulable)]
    async fn process(&mut self, _: (), _: &Context<Self>, env: &mut ListenerEnv) {
        while let Ok(message) = env.external.try_recv() {
            self.message.send(message).await;
        }
    }
}

pub struct ListenerEnv {
    /// Source of external messages.
    external: Receiver<String>,
}

impl ListenerEnv {
    /// Creates new `Listener` model.
    fn new(external: Receiver<String>) -> Self {
        Self { external }
    }
}

struct ProtoListener {
    external: Receiver<String>,
    pub message: Output<String>,
}
impl ProtoListener {
    pub fn new(external: Receiver<String>) -> Self {
        Self {
            external,
            message: Output::default(),
        }
    }
}
impl ProtoModel for ProtoListener {
    type Model = Listener;

    fn build(
        self,
        _: &mut nexosim::model::BuildContext<Self>,
    ) -> (Self::Model, <Self::Model as Model>::Env) {
        (Listener::new(self.message), ListenerEnv::new(self.external))
    }
}

fn main() -> Result<(), SimulationError> {
    // Channel for communication with simulation from outside.
    let (tx, rx) = channel();

    // Models.
    let mut listener = ProtoListener::new(rx);

    let listener_mbox = Mailbox::new();

    // Endpoints.
    let (sink, mut message) = event_queue(SinkState::Enabled);
    listener.message.connect_sink(sink);

    // Assembly and initialization.
    let t0 = MonotonicTime::EPOCH; // arbitrary since models do not depend on absolute time
    let mut simu = SimInit::new()
        .add_model(listener, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    let scheduler = simu.scheduler();

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
        assert_eq!(message.try_read().unwrap(), i.to_string());
    }
    assert_eq!(message.try_read(), None);

    // Stop the simulation.
    scheduler.halt();
    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}

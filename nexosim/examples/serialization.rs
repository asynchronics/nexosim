use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use nexosim::model::{Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, EventSource, Output};
use nexosim::simulation::{Address, ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};

/// The `Listener` Model.
pub struct Listener {
    pub message: Output<u32>,
}

impl Listener {
    /// Creates new `Listener` model.
    fn new() -> Self {
        Self {
            message: Output::default(),
        }
    }
    pub async fn process(&mut self, msg: u32) {
        println!("Running!");
        self.message.send(msg).await;
    }
}

impl Model for Listener {
    /// Initialize model.
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        // Schedule periodic function that processes external events.
        cx.schedule_event_from_source(Duration::from_secs(2), "input", 13u32)
            .unwrap();

        self.into()
    }
}

fn main() -> Result<(), SimulationError> {
    let mut listener = Listener::new();
    let listener_mbox = Mailbox::new();

    let mut input = EventSource::new();
    input.connect(Listener::process, listener_mbox.address());

    let message = EventQueue::new();
    listener.message.connect_sink(&message);
    let mut message = message.into_reader();

    let t0 = MonotonicTime::EPOCH;

    let (mut simu, mut scheduler) = SimInit::new()
        .register_source(input, "input")
        .add_model(listener, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    println!(
        "{}",
        serde_json::to_string(&simu.serializable_queue()).unwrap()
    );

    simu.step().unwrap();

    Ok(())
}

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
    pub message_str: Output<String>,
}

impl Listener {
    /// Creates new `Listener` model.
    fn new() -> Self {
        Self {
            message: Output::default(),
            message_str: Output::default(),
        }
    }
    pub async fn process(&mut self, msg: u32) {
        println!("Running! {}", msg);
        self.message.send(msg).await;
    }
    pub async fn process_str(&mut self, msg: String) {
        println!("Running! {}", msg);
        self.message_str.send(msg).await;
    }
}

impl Model for Listener {
    /// Initialize model.
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_event_from_source(Duration::from_secs(2), "input", 13u32)
            .unwrap();
        cx.schedule_event_from_source(Duration::from_secs(5), "input", 15u32)
            .unwrap();
        cx.schedule_event_from_source(Duration::from_secs(7), "input", 20u32)
            .unwrap();
        cx.schedule_event_from_source(Duration::from_secs(3), "input_str", "Three".to_string())
            .unwrap();

        self.into()
    }
}

fn main() -> Result<(), SimulationError> {
    let mut listener = Listener::new();
    let listener_mbox = Mailbox::new();

    let mut input = EventSource::new();
    input.connect(Listener::process, listener_mbox.address());

    let mut input_str = EventSource::new();
    input_str.connect(Listener::process_str, listener_mbox.address());

    let message = EventQueue::new();
    listener.message.connect_sink(&message);
    let mut message = message.into_reader();

    let t0 = MonotonicTime::EPOCH;

    let (mut simu, mut scheduler) = SimInit::new()
        .register_source(input, "input")
        .register_source(input_str, "input_str")
        .add_model(listener, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    let s = serde_json::to_string(&simu.serializable_queue()).unwrap();
    println!("{}", s);
    simu.restore_deserialized_queue(serde_json::from_str(&s).unwrap());

    simu.step_unbounded().unwrap();

    Ok(())
}

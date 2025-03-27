use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, EventSource, Output};
use nexosim::simulation::{Address, ExecutionError, Mailbox, SimInit, SimulationError, SourceId};
use nexosim::time::{AutoSystemClock, MonotonicTime};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CustomArg {
    pub key: String,
    pub val: usize,
}

/// The `Listener` Model.
pub struct Listener {
    pub message: Output<u32>,
    pub message_str: Output<CustomArg>,
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
    pub async fn process_str(&mut self, msg: CustomArg) {
        println!("Running! {:?}", msg);
        self.message_str.send(msg).await;
    }
}

impl Model for Listener {
    /// Initialize model.
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        let input_id = cx.register_input(Listener::process);
        cx.schedule_event(Duration::from_secs(2), input_id, 13u32)
            .unwrap();

        let input_str_id = cx.register_input(Listener::process_str);
        cx.schedule_event(
            Duration::from_secs(2),
            input_str_id,
            CustomArg {
                key: "SomeKey".to_string(),
                val: 54,
            },
        )
        .unwrap();

        cx.schedule_periodic_event(
            Duration::from_secs(1),
            input_id,
            Duration::from_secs(1),
            17u32,
        )
        .unwrap();

        self.into()
    }
}

fn main() -> Result<(), SimulationError> {
    let mut listener = Listener::new();
    let listener_mbox = Mailbox::new();

    // input.connect(Listener::process, listener_mbox.address());
    // let mut input_str = EventSource::new();
    // input_str.connect(Listener::process_str, listener_mbox.address());

    let message = EventQueue::new();
    listener.message.connect_sink(&message);
    let mut message = message.into_reader();

    let t0 = MonotonicTime::EPOCH;

    let (mut simu, mut scheduler) = SimInit::new()
        .add_model(listener, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    let s = serde_json::to_string(&scheduler.dump_queue()).unwrap();
    println!("{}", s);
    scheduler.load_queue(serde_json::from_str(&s).unwrap());

    simu.step_unbounded().unwrap();

    Ok(())
}

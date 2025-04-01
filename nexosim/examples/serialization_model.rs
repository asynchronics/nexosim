use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{BuildContext, Context, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventQueue, EventSource, Output};
use nexosim::simulation::{Address, ExecutionError, Mailbox, SimInit, SimulationError, SourceId};
use nexosim::time::{AutoSystemClock, MonotonicTime};

pub struct ListenerEnvironment {
    pub message: Output<String>,
}
impl ListenerEnvironment {
    pub fn new() -> Self {
        Self {
            message: Output::new(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Listener {
    input_id: SourceId,
    pub value: u32,
}

impl Listener {
    pub async fn process(&mut self, msg: u32, cx: &mut Context<Self>) {
        cx.environment
            .message
            .send(format!("{} @{}", msg, cx.time()))
            .await;
    }
}

impl Model for Listener {
    type Environment = ListenerEnvironment;

    /// Initialize model.
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_event(Duration::from_secs(2), self.input_id, 13u32)
            .unwrap();
        self.into()
    }
}

struct ProtoListener;
impl ProtoModel for ProtoListener {
    type Model = Listener;
    fn build(self, cx: &mut BuildContext<Self>) -> Self::Model {
        let input_id = cx.register_input(Listener::process);
        Listener { input_id, value: 0 }
    }
}

fn main() -> Result<(), SimulationError> {
    let mut listener = ProtoListener;
    let mut listener_env = ListenerEnvironment::new();
    let listener_mbox = Mailbox::new();

    let message = EventQueue::new();
    listener_env.message.connect_sink(&message);
    let mut message = message.into_reader();

    let t0 = MonotonicTime::EPOCH;

    let (mut simu, mut scheduler) = SimInit::new()
        .add_model(listener, listener_env, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    simu.step().unwrap();
    // assert!(message.next().is_some());
    println!("{:?}", message.next());

    Ok(())
}

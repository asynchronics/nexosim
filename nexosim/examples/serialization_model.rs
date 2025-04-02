use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{BuildContext, Context, Environment, InitializedModel, Model, ProtoModel};
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
impl Environment for ListenerEnvironment {}

#[derive(Serialize, Deserialize)]
pub struct Listener {
    input_id: SourceId,
    pub value: u32,
}

impl Listener {
    pub async fn process(&mut self, msg: u32, cx: &mut Context<Self>) {
        println!("Process {}", msg);
        cx.environment
            .message
            // .send(format!("{} @{}", msg, cx.environment.time()))
            .send(format!("{}", msg))
            .await;
    }
}

impl Model for Listener {
    type Environment = ListenerEnvironment;

    /// Initialize model.
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_event(Duration::from_secs(3), self.input_id, 17u32)
            .unwrap();
        cx.environment
            .schedule_event_from(Duration::from_secs(2), self.input_id, 13u32, 1)
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

fn dump(listener: &mut Listener) {
    println!("dump")
}

fn main() -> Result<(), SimulationError> {
    let listener = ProtoListener;
    let mut listener_env = ListenerEnvironment::new();
    let listener_mbox = Mailbox::new();

    let message = EventQueue::new();
    listener_env.message.connect_sink(&message);
    let mut message = message.into_reader();

    let mut dump_source = EventSource::new();
    dump_source.connect(dump, listener_mbox.address().clone());

    let t0 = MonotonicTime::EPOCH;

    let (mut simu, mut scheduler) = SimInit::new()
        .add_model(listener, listener_env, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    simu.step().unwrap();

    // let dump_id = scheduler.register_event_source(dump_source);
    // scheduler
    //     .schedule_event(Duration::from_secs(5), dump_id, ())
    //     .unwrap();

    // simu.step().unwrap();

    println!("{:?}", message.next());

    Ok(())
}

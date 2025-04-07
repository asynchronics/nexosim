use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{BuildContext, Environment, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventQueue, EventSource, Output};
use nexosim::simulation::{
    ActionKey, Address, ExecutionError, Mailbox, SimInit, SimulationError, SourceId,
};
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
    pub key: Option<ActionKey>,
}

impl Listener {
    pub async fn process(&mut self, msg: u32, env: &mut ListenerEnvironment) {
        self.value += 1;
        env.message
            .send(format!("{}/{} @{}", msg, self.value, env.time()))
            .await;
        if self.value > 6 && self.key.is_some() {
            println!("Cancelling");
            self.key.take().unwrap().cancel();
        }
        if msg == 17 {
            panic!("This event should have been cancelled!");
        }
    }
}

impl Model for Listener {
    type Environment = ListenerEnvironment;

    /// Initialize model.
    async fn init(mut self, env: &mut ListenerEnvironment) -> InitializedModel<Self> {
        self.value = 2;
        env.schedule_periodic_event(
            Duration::from_secs(2),
            self.input_id,
            Duration::from_secs(2),
            13u32,
        )
        .unwrap();

        self.key = Some(
            env.schedule_keyed_event(Duration::from_secs(15), self.input_id, 17u32)
                .unwrap(),
        );

        self.into()
    }
}

struct ProtoListener;
impl ProtoModel for ProtoListener {
    type Model = Listener;
    fn build(self, cx: &mut BuildContext<Self>) -> Self::Model {
        let input_id = cx.register_input(Listener::process);
        Listener {
            input_id,
            value: 0,
            key: None,
        }
    }
}

fn ping(listener: &mut Listener) {
    println!("pong")
}

fn main() -> Result<(), SimulationError> {
    let listener = ProtoListener;
    let mut listener_env = ListenerEnvironment::new();
    let listener_mbox = Mailbox::new();

    let message = EventQueue::new();
    listener_env.message.connect_sink(&message);
    let mut message = message.into_reader();

    let mut ping_source = EventSource::new();
    ping_source.connect(ping, listener_mbox.address().clone());

    let t0 = MonotonicTime::EPOCH;
    let addr = listener_mbox.address();

    let bench = SimInit::new()
        .add_model(listener, listener_env, listener_mbox, "listener")
        .set_clock(AutoSystemClock::new());

    let (mut simu, mut scheduler) = bench.init(t0)?;

    let state_0 = simu.serialize_state();

    simu.step().unwrap();
    println!("{:?}", message.next());

    let state_1 = simu.serialize_state();

    simu.step().unwrap();
    println!("{:?}", message.next());

    println!("Restore 0");
    simu.restore_state(state_0);
    simu.step().unwrap();
    println!("{:?}", message.next());
    simu.step().unwrap();
    println!("{:?}", message.next());

    println!("Restore 1");
    simu.restore_state(state_1);

    for _ in 0..20 {
        simu.step().unwrap();
        println!("{:?}", message.next());
    }

    Ok(())
}

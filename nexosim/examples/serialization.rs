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
    input_id: SourceId<u32>,
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
            println!("Cancelling single event");
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
            13,
        )
        .unwrap();

        self.key = Some(
            env.schedule_keyed_event(Duration::from_secs(15), self.input_id, 17)
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

    // save state at the beginning
    let state_0 = simu.serialize_state();

    simu.step().unwrap();
    assert_eq!(
        message.next(),
        Some("13/3 @1970-01-01 00:00:02".to_string())
    );

    // save state after one step
    let state_1 = simu.serialize_state();

    simu.step().unwrap();
    assert_eq!(
        message.next(),
        Some("13/4 @1970-01-01 00:00:04".to_string())
    );

    println!("Restore 0");
    simu.restore_state(state_0);
    simu.step().unwrap();
    // back to value == 3 and time == :02
    assert_eq!(
        message.next(),
        Some("13/3 @1970-01-01 00:00:02".to_string())
    );
    simu.step().unwrap();
    assert_eq!(
        message.next(),
        Some("13/4 @1970-01-01 00:00:04".to_string())
    );

    println!("Restore 1");
    simu.restore_state(state_1);
    simu.step().unwrap();
    // now back to value == 4 and time == :04
    assert_eq!(
        message.next(),
        Some("13/4 @1970-01-01 00:00:04".to_string())
    );

    // run in a loop to make sure the cancelled event won't fire
    for _ in 0..10 {
        simu.step().unwrap();
    }

    Ok(())
}

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{BuildContext, Environment, InitializedModel, Model, ProtoModel};
use nexosim::ports::{EventQueue, EventQueueReader, EventSource, Output};
use nexosim::simulation::{Mailbox, SimInit, SimulationError, SourceId};
use nexosim::time::{MonotonicTime, NoClock};

#[derive(Default)]
pub struct ListenerEnvironment {
    pub message: Output<String>,
}
impl Environment for ListenerEnvironment {}

#[derive(Serialize, Deserialize)]
pub struct Listener {
    input_id: SourceId<u32>,
    pub value: u32,
}

impl Listener {
    pub async fn process(&mut self, msg: u32, env: &mut ListenerEnvironment) {
        self.value += 1;
        env.message
            .send(format!("{}/{} @{}", msg, self.value, env.time()))
            .await;
    }
    pub async fn other(&mut self, msg: u32, env: &mut ListenerEnvironment) {
        println!("Other: {} @{}", msg, env.time());
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

        self.into()
    }
}

struct ProtoListener;
impl ProtoModel for ProtoListener {
    type Model = Listener;
    fn build(self, cx: &mut BuildContext<Self>, _: &mut ListenerEnvironment) -> Self::Model {
        let input_id = cx.register_input(Listener::process);
        Listener { input_id, value: 0 }
    }
}

fn get_bench() -> (SimInit, EventQueueReader<String>, SourceId<u32>) {
    let listener = ProtoListener;
    let mut listener_env = ListenerEnvironment::default();
    let listener_mbox = Mailbox::new();
    let address = listener_mbox.address();

    let message = EventQueue::new();
    listener_env.message.connect_sink(&message);

    let bench = SimInit::new()
        .add_model(listener, listener_env, listener_mbox, "listener")
        .set_clock(NoClock::new());

    let mut source = EventSource::new();
    source.connect(Listener::other, address.clone());
    let other_source = bench.register_event_source(source);

    (bench, message.into_reader(), other_source)
}

fn main() -> Result<(), SimulationError> {
    let (bench, mut message, other_source) = get_bench();

    let t0 = MonotonicTime::EPOCH;
    let (mut simu, mut scheduler) = bench.init(t0)?;

    scheduler.schedule_event(Duration::from_secs(13), other_source, 214);

    // save state at the beginning
    let state = simu.serialize_state();

    let (bench, mut message, _) = get_bench();
    let (mut simu, _) = bench.restore(state).unwrap();

    simu.step_until(Duration::from_secs(14));
    while let Some(msg) = message.next() {
        println!("{}", msg);
    }

    Ok(())
}

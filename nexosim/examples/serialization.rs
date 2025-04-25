use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim_util::observables::ObservableValue;

use nexosim::model::{BuildContext, Context, InitializedModel, InputId, Model, ProtoModel};
use nexosim::ports::{EventQueue, EventQueueReader, EventSource, Output, UniRequestor};
use nexosim::simulation::{ActionKey, Mailbox, SimInit, SimulationError, SourceId};
use nexosim::time::{MonotonicTime, NoClock};

static INIT_COUNT: AtomicU32 = AtomicU32::new(0);

#[derive(Serialize, Deserialize)]
pub struct Listener {
    input_id: InputId<Self, u32>,
    pub value: u32,
    pub key: Option<ActionKey>,
    pub message: Output<String>,
    pub temp: UniRequestor<(), f64>,
    observable: ObservableValue<f64>,
}

impl Listener {
    pub async fn process(&mut self, msg: u32, cx: &mut Context<Self>) {
        self.value += 1;
        self.message
            .send(format!("{}/{} @{}", msg, self.value, cx.time()))
            .await;
        if self.value > 6 && self.key.is_some() {
            println!("Cancelling single event");
            self.key.take().unwrap().cancel();
        }
        if msg == 17 {
            panic!("This event should have been cancelled!");
        }

        let temp = self.temp.send(()).await.unwrap();
        self.observable.set(temp).await;
        println!("Temp: {} @ {}", temp, cx.time());
    }
}

impl Model for Listener {
    type Environment = ();

    /// Initialize model.
    async fn init(mut self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        INIT_COUNT.fetch_add(1, Ordering::Relaxed);
        self.value = 2;
        cx.schedule_periodic_event(
            Duration::from_secs(2),
            self.input_id,
            Duration::from_secs(2),
            13,
        )
        .unwrap();

        self.observable.set(1.31).await;

        self.key = Some(
            cx.schedule_keyed_event(Duration::from_secs(15), self.input_id, 17)
                .unwrap(),
        );

        self.into()
    }
}

struct ProtoListener {
    pub observable_output: Output<f64>,
    pub message: Output<String>,
    pub temp: UniRequestor<(), f64>,
}
impl ProtoModel for ProtoListener {
    type Model = Listener;
    fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, ()) {
        let input_id = cx.register_input(Listener::process);
        (
            Listener {
                input_id,
                value: 0,
                key: None,
                message: self.message,
                temp: self.temp,
                observable: ObservableValue::new(self.observable_output.clone()),
            },
            (),
        )
    }
}

#[derive(Serialize, Deserialize)]
struct Env {
    pub temp: f64,
}
impl Env {
    pub async fn set_temp(&mut self, temp: f64) {
        self.temp = temp;
    }

    pub async fn get_temp(&mut self, _: ()) -> f64 {
        self.temp
    }
}
impl Model for Env {
    type Environment = ();
}

fn get_bench() -> (
    SimInit,
    EventQueueReader<String>,
    EventQueueReader<f64>,
    SourceId<f64>,
) {
    let listener_mbox = Mailbox::new();
    let env_mbox = Mailbox::new();

    let mut listener = ProtoListener {
        observable_output: Output::new(),
        message: Output::default(),
        temp: UniRequestor::new(Env::get_temp, &env_mbox),
    };

    let message = EventQueue::new();
    listener.message.connect_sink(&message);

    let env = Env { temp: 13.2 };

    let mut temp_source = EventSource::new();
    temp_source.connect(Env::set_temp, &env_mbox);

    let obs = EventQueue::new();
    listener.observable_output.connect_sink(&obs);

    let bench = SimInit::new()
        .add_model(listener, listener_mbox, "listener")
        .add_model(env, env_mbox, "env")
        .set_clock(NoClock::new());
    let temp_input = bench.register_event_source(temp_source);
    (bench, message.into_reader(), obs.into_reader(), temp_input)
}

fn main() -> Result<(), SimulationError> {
    let (bench, mut message, mut obs, temp_source) = get_bench();

    let t0 = MonotonicTime::EPOCH;
    let (mut simu, scheduler) = bench.init(t0)?;

    scheduler
        .schedule_event(Duration::from_secs(5), temp_source, 25.2)
        .unwrap();

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
    let (bench, mut message, mut obs, temp_source) = get_bench();
    let (mut simu, _) = bench.restore(state_0)?;

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
    let (bench, mut message, mut obs, temp_source) = get_bench();
    let (mut simu, _) = bench.restore(state_1)?;

    assert_eq!(1, INIT_COUNT.load(Ordering::Relaxed));

    simu.step().unwrap();
    // now back to value == 4 and time == :04
    assert_eq!(
        message.next(),
        Some("13/4 @1970-01-01 00:00:04".to_string())
    );

    assert_eq!(obs.next(), Some(13.2));
    // drain observable
    while let Some(_) = obs.next() {}

    // observable value change at T=5s
    simu.step().unwrap();
    // extra step to propagate?
    simu.step().unwrap();
    assert_eq!(obs.next(), Some(25.2));

    // run in a loop to make sure the cancelled event won't fire
    for _ in 0..10 {
        simu.step().unwrap();
    }
    Ok(())
}

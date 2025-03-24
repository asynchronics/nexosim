use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
// use typetag::Serialize;

use nexosim::executor::Executor;
use nexosim::model::{Context, InitializedModel, Model};
use nexosim::ports::{EventQueue, Output};
use nexosim::simulation::{
    process_event, Address, ExecutionError, Mailbox, SimInit, SimulationError,
};
use nexosim::time::{AutoSystemClock, MonotonicTime};

#[derive(Debug, Default)]
struct HandlerRegistry {
    inner: HashMap<String, Box<dyn EventHandler>>,
}
impl HandlerRegistry {
    pub fn get_handler(&self, tag: &str) -> Option<&Box<dyn EventHandler>> {
        self.inner.get(tag)
    }
    pub fn register(&mut self, tag: &str, handler: Box<dyn EventHandler>) {
        self.inner.insert(tag.to_string(), handler);
    }
}

#[typetag::serde(tag = "input_params")]
trait InputParams {}

trait EventHandler: std::fmt::Debug {
    fn into_future(&self, arg: &dyn Any) -> Box<dyn Future<Output = ()>>;
}

#[derive(Debug)]
struct ListenerHandler {
    address: Address<Listener>,
}
impl EventHandler for ListenerHandler {
    fn into_future(&self, arg: &dyn Any) -> Box<dyn Future<Output = ()>> {
        Box::new(process_event(
            Listener::process,
            arg.downcast_ref::<&String>().unwrap().to_string(),
            self.address.0.clone(),
        ))
    }
}

#[typetag::serde]
impl InputParams for String {}

#[derive(Serialize)]
struct ScheduledEvent {
    pub tag: String,
    pub params: Box<dyn InputParams>,
}

/// The `Listener` Model.
pub struct Listener {
    pub message: Output<String>,
}

impl Listener {
    /// Creates new `Listener` model.
    fn new() -> Self {
        Self {
            message: Output::default(),
        }
    }
    pub async fn process(&mut self, msg: String) {
        self.message.send(msg).await;
    }
}

impl Model for Listener {
    /// Initialize model.
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        // Schedule periodic function that processes external events.
        // cx.schedule_periodic_event(DELTA, PERIOD, Listener::process, ())
        //     .unwrap();

        self.into()
    }
}

fn main() -> Result<(), SimulationError> {
    let mut registry = HandlerRegistry::default();

    let mut listener_0 = Listener::new();
    let listener_0_mbox = Mailbox::new();
    let mut listener_1 = Listener::new();
    let listener_1_mbox = Mailbox::new();

    registry.register(
        "listener_0",
        Box::new(ListenerHandler {
            address: listener_0_mbox.address(),
        }),
    );
    registry.register(
        "listener_1",
        Box::new(ListenerHandler {
            address: listener_1_mbox.address(),
        }),
    );
    println!("{:?}", registry);

    let event = ScheduledEvent {
        tag: "listener_0".to_string(),
        params: Box::new("My Message".to_string()),
    };

    println!("{:?}", serde_json::to_string(&event));

    let handler = registry
        .get_handler(&event.tag)
        .unwrap()
        .into_future(&event.params);
    // let queue: Vec<ScheduledEvent> = Vec::new();
    // registry["listener_0"].execute(params, executor);

    // let message = EventQueue::new();
    // listener.message.connect_sink(&message);
    // let mut message = message.into_reader();

    // let t0 = MonotonicTime::EPOCH;

    // let (mut simu, mut scheduler) = SimInit::new()
    //     .add_model(listener, listener_mbox, "listener")
    //     .set_clock(AutoSystemClock::new())
    //     .init(t0)?;

    Ok(())
}

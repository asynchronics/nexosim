use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
// use typetag::Serialize;

use nexosim::executor::Executor;
use nexosim::model::{Context, InitializedModel, Model};
use nexosim::ports::{markers::AsyncWithContext, EventQueue, InputFn, Output};
use nexosim::simulation::{
    process_event, Address, ExecutionError, Mailbox, SimInit, SimulationError,
};
use nexosim::time::{AutoSystemClock, MonotonicTime};

const DELTA: Duration = Duration::from_millis(2);
const PERIOD: Duration = Duration::from_millis(20);
const N: usize = 10;

// struct H {
//     pub tag: String,
//     address: Address<Listener>,
// }
// impl H {
//     pub fn foo(&self) -> impl FnOnce(&Executor) {
//         let sender = self.address.0.clone();
//         let f = |executor: &Executor| {
//             executor.spawn_and_forget(process_event(Listener::process, "a".to_string(), sender));
//         };
//         f
//     }
// }

trait EventHandler {
    fn execute(&self, params: &dyn Any, executor: &Executor);
    // fn serialize_input(&self, params: &dyn InputParams, serializer: &mut dyn Serializer);
}

#[typetag::serde(tag = "input_params")]
trait InputParams {}
// trait InputParams {
//     fn as_any(&self) -> &dyn Any;
// }

struct ListenerHandler {
    address: Address<Listener>,
}
impl EventHandler for ListenerHandler {
    // fn execute(&self, params: &dyn InputParams, executor: &Executor) {
    //     let input = params.as_any().downcast_ref::<ListenerParams>().unwrap();
    //     let f = process_event(Listener::process, input.msg.clone(), self.address.0.clone());
    //     executor.spawn_and_forget(f);
    // }
    fn execute(&self, params: &dyn Any, executor: &Executor) {
        let input = params.downcast_ref::<ListenerParams>().unwrap();
        let f = process_event(Listener::process, input.msg.clone(), self.address.0.clone());
        executor.spawn_and_forget(f);
    }
    // fn serialize_input(&self, params: &dyn InputParams, serializer: &mut dyn Serializer) {
    //     let input = params.as_any().downcast_ref::<ListenerParams>().unwrap();
    //     serializer.erased_serialize_struct("ListenerParams", size_of::<ListenerParams>());
    // }
}

#[derive(Deserialize, Serialize)]
struct ListenerParams {
    pub msg: String,
}
#[typetag::serde]
impl InputParams for ListenerParams {
    // fn as_any(&self) -> &dyn Any {
    //     self
    // }
}

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
    let mut registry: HashMap<String, Box<dyn EventHandler>> = HashMap::new();

    let mut listener_0 = Listener::new();
    let listener_0_mbox = Mailbox::new();
    let mut listener_1 = Listener::new();
    let listener_1_mbox = Mailbox::new();

    registry.insert(
        "listener_0".to_string(),
        Box::new(ListenerHandler {
            address: listener_0_mbox.address(),
        }),
    );
    registry.insert(
        "listener_1".to_string(),
        Box::new(ListenerHandler {
            address: listener_1_mbox.address(),
        }),
    );

    let event = ScheduledEvent {
        tag: "listener_0".to_string(),
        params: Box::new(ListenerParams {
            msg: "My Message".to_string(),
        }),
    };

    println!("{:?}", serde_json::to_string(&event));

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

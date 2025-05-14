use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{BuildContext, Context, InitializedModel, InputId, Model, ProtoModel};
use nexosim::ports::{EventQueue, EventQueueReader, Output};
use nexosim::simulation::{AutoEventKey, EventKey, Mailbox, SimInit};
use nexosim::time::{MonotonicTime, NoClock};

#[derive(Serialize, Deserialize)]
pub struct MyModel {
    input_id: InputId<Self, u32>,
    key: Option<EventKey>,
    auto_key: Option<AutoEventKey>,
    msg: Output<u32>,
    value: u32,
}
impl MyModel {
    pub async fn process(&mut self, input: u32) {
        // Auto key event.
        if input == 27 {
            self.msg.send(27).await;
            return;
        }

        self.value += 1;
        self.msg.send(self.value).await;

        if self.value > 6 && self.key.is_some() {
            self.key.take().unwrap().cancel();
        }

        if input == 17 {
            panic!("This event should have been cancelled!");
        }
    }
}

impl Model for MyModel {
    type Env = ();

    async fn init(mut self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        self.value = 2;
        cx.schedule_periodic_event(
            Duration::from_secs(2),
            Duration::from_secs(2),
            &self.input_id,
            13,
        );

        // This event is meant to be cancelled after deserialization.
        self.key = Some(
            cx.schedule_keyed_event(Duration::from_secs(15), &self.input_id, 17)
                .unwrap(),
        );

        self.auto_key = Some(
            cx.schedule_keyed_event(Duration::from_secs(5), &self.input_id, 27)
                .unwrap()
                .into_auto(),
        );

        self.into()
    }
}

struct MyProto {
    pub msg: Output<u32>,
}
impl ProtoModel for MyProto {
    type Model = MyModel;

    fn build(self, cx: &mut BuildContext<Self>) -> (Self::Model, <Self::Model as Model>::Env) {
        let input_id = cx.register_input(MyModel::process);
        (
            MyModel {
                input_id,
                value: 0,
                key: None,
                auto_key: None,
                msg: self.msg,
            },
            (),
        )
    }
}

fn get_bench() -> (SimInit, EventQueueReader<u32>) {
    let mbox = Mailbox::new();

    let mut model = MyProto {
        msg: Output::default(),
    };
    let message = EventQueue::new();
    model.msg.connect_sink(&message);

    let bench = SimInit::new()
        .add_model(model, mbox, "myModel")
        .with_post_init(|simu| Ok(()))
        .set_clock(NoClock::new());
    (bench, message.into_reader())
}

fn main() {
    let (bench, mut message) = get_bench();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench.init(t0).unwrap();

    simu.step().unwrap();
    // Initial 2 + 1 from `process`.
    assert_eq!(message.next(), Some(3));

    // Store state after one step.
    let state = simu.save().unwrap();

    // Execute two more steps.
    simu.step().unwrap();
    assert_eq!(message.next(), Some(4));

    // Assert auto key didn't drop when serializing.
    simu.step().unwrap();
    assert_eq!(message.next(), Some(27));

    // Restore state from the first step.
    let (bench, mut message) = get_bench();
    let mut simu = bench.restore(&state).unwrap();

    simu.step().unwrap();
    // Back to `4` as this is the second step again.
    assert_eq!(message.next(), Some(4));

    // Assert auto key didn't drop after deserializing either.
    simu.step().unwrap();
    assert_eq!(message.next(), Some(27));

    // Run in the loop for a while to verify that the cancelled event won't fire.
    for _ in 0..20 {
        simu.step().unwrap();
    }
}

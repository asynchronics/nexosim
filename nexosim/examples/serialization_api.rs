use nexosim::model::{Context, InitializedModel, ProtoModel, SchedulableId};
use nexosim::ports::Output;
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::{schedulable, Argument, Model};

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, schemars::JsonSchema)]
struct MyOutput {
    value: usize,
    args: Vec<u8>,
}

#[Argument]
#[derive(Clone)]
struct CompoundValue {
    value: u32,
    coeff: f32,
}

#[derive(Serialize, Debug, Deserialize)]
struct MyModel {
    state: u32,
    voltage: Output<u16>,
    output_map: HashMap<String, Output<MyOutput>>,
    print_id: SchedulableId<Self, ()>,
}

#[Model]
impl MyModel {
    #[nexosim(schedulable)]
    pub async fn input(&mut self, arg: u8, cx: &mut Context<Self>) {
        self.state += arg as u32;
        cx.schedule_event(Duration::from_secs(1), &self.print_id, ())
            .unwrap();
        cx.schedule_event(Duration::from_secs(2), schedulable!(Self::input), 2)
            .unwrap();
    }

    pub async fn query(&mut self, arg: u32) -> u32 {
        self.state * arg
    }

    pub async fn apply_value(&mut self, arg: CompoundValue) {
        self.state += (arg.value as f32 * arg.coeff) as u32;
    }

    #[nexosim(init)]
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_event(Duration::from_secs(2), schedulable!(Self::input), 3)
            .unwrap();
        self.into()
    }
}

struct MyProto;
impl ProtoModel for MyProto {
    type Model = MyModel;

    fn build(
        self,
        cx: &mut nexosim::model::BuildContext<Self>,
    ) -> (Self::Model, <Self::Model as nexosim::model::Model>::Env) {
        let print_id = cx.register_schedulable(|m: &mut Self::Model| {
            println!("Current state: {}", m.state);
        });

        let output_map = HashMap::from_iter([
            ("port_a".to_string(), Output::tagged("PortA".to_string())),
            ("port_b".to_string(), Output::tagged("PortB".to_string())),
            ("port_c".to_string(), Output::default()),
        ]);
        (
            MyModel {
                state: 0,
                voltage: Output::tagged("Voltage".to_string()),
                output_map,
                print_id,
            },
            (),
        )
    }
}

#[derive(Serialize, Deserialize, Default)]
struct SimpleModel {
    value: u32,
}
#[Model]
impl SimpleModel {
    #[nexosim(schedulable)]
    pub async fn input(&mut self, arg: u32) {
        println!("Simple Model called with: {}", arg);
        self.value = arg;
    }
    #[nexosim(init)]
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        cx.schedule_event(Duration::from_secs(5), schedulable!(Self::input), 3)
            .unwrap();
        self.into()
    }
}

fn main() {
    let model = MyProto;
    let mbox = Mailbox::new();
    let simple_model = SimpleModel::default();
    let smbox = Mailbox::new();

    let t0 = MonotonicTime::EPOCH;
    let bench = SimInit::new().add_model(model, mbox, "my_model").add_model(
        simple_model,
        smbox,
        "simple_model",
    );

    let mut simu = bench.init(t0).unwrap();

    println!("\n\n---START---");

    for _ in 0..10 {
        simu.step().unwrap();
    }
}

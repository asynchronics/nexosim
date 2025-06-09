use nexosim::model::{Context, InitializedModel, ProtoModel, SchedulableId};
use nexosim::ports::Output;
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::{schedulable, Model};

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, schemars::JsonSchema)]
struct MyOutput {
    value: usize,
    args: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MyModel {
    state: u32,
    manual_id: SchedulableId<Self, i32>,
    output_16: Output<u16>,
    output_u: Output<usize>,
    output_map: HashMap<String, Output<MyOutput>>,
}

#[Model]
impl MyModel {
    #[nexosim(schedulable)]
    pub async fn input(&mut self, arg: u8) {
        println!("{}", arg);
    }

    pub fn input_sync(&mut self, arg: u8) {
        println!("{}", arg);
    }

    #[nexosim(schedulable)]
    pub async fn tick(&mut self) {
        println!("Tick");
    }

    async fn _tick(&mut self) {
        println!("Tick");
    }

    pub async fn with_cx(&mut self, a: u8, cx: &mut Context<Self>) {
        println!("Tick");
    }

    pub async fn with_cx_pathed(&mut self, a: u8, cx: &mut nexosim::model::Context<Self>) {
        println!("Tick");
    }

    pub async fn with_not_cx(&mut self, a: u8, other: &mut Self) {
        println!("Tick");
    }

    pub async fn not_mut(&self, a: u8) {
        println!("Tick");
    }

    #[nexosim(schedulable)]
    pub async fn path_type(&mut self, _: std::primitive::usize) {
        //
    }

    pub async fn manual(&mut self, arg: i32) {
        println!("Manual: {:+}", -arg);
    }

    pub async fn query(&mut self, arg: u8) -> u16 {
        12
    }

    pub async fn query_simple(&mut self) -> u16 {
        12
    }

    pub async fn query_cx(&mut self, arg: u8, cx: &mut Context<Self>) -> u16 {
        12
    }

    pub fn query_sync(&mut self, arg: u8) -> u16 {
        12
    }

    #[nexosim(init)]
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        println!("Custom init");
        cx.schedule_event(Duration::from_secs(2), schedulable!(Self::input), 12)
            .unwrap();
        // cx.schedule_event(Duration::from_secs(3), schedulable!(Self::manual), ())
        //     .unwrap();
        cx.schedule_event(Duration::from_secs(2), schedulable!(Self::tick), ())
            .unwrap();
        cx.schedule_event(Duration::from_secs(1), &self.manual_id, -5)
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
        let output_map = HashMap::from_iter([
            ("One".to_string(), Output::tagged("One".to_string())),
            ("Two".to_string(), Output::tagged("Two".to_string())),
        ]);
        let manual_id = cx.register_schedulable(MyModel::manual);
        (
            MyModel {
                state: 0,
                manual_id,
                output_16: Output::tagged("Current 16".to_string()),
                output_u: Output::default(),
                output_map,
            },
            (),
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OtherModel;
#[Model(Env=u8)]
impl OtherModel {
    #[nexosim(schedulable)]
    pub fn non_input_method(&mut self, _: (), cx: &mut Context<Self>) {
        *cx.env() + 12;
    }
}

fn main() {
    let m = MyProto;

    let mbox = Mailbox::new();
    let addr = mbox.address();
    let t0 = MonotonicTime::EPOCH;
    let mut bench = SimInit::new().add_model(m, mbox, "my_model");

    let source_id = bench.register_input(MyModel::input, &addr);

    let mut simu = bench.init(t0).unwrap();
    let scheduler = simu.scheduler();

    scheduler.schedule_event(Duration::from_secs(1), &source_id, 37);

    simu.step().unwrap();
    simu.step().unwrap();
    simu.step().unwrap();
}

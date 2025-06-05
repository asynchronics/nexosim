use nexosim::model::{Context, InitializedModel, ProtoModel, SchedulableId};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::{schedulable, Model};

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct MyModel {
    state: u32,
    manual_id: SchedulableId<Self, i32>,
}
#[Model]
impl MyModel {
    #[nexosim(schedulable)]
    pub async fn input(&mut self, arg: u8) {
        println!("{}", arg);
    }

    #[nexosim(schedulable)]
    pub async fn tick(&mut self) {
        println!("Tick");
    }

    #[nexosim(schedulable)]
    pub async fn path_type(&mut self, _: std::primitive::usize) {
        //
    }

    pub async fn manual(&mut self, arg: i32) {
        println!("Manual: {:+}", -arg);
    }

    const fn b() -> bool {
        false
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
        let manual_id = cx.register_schedulable(MyModel::manual);
        (
            MyModel {
                state: 0,
                manual_id,
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
    std::any::type_name_of_val(&MyModel::input);

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

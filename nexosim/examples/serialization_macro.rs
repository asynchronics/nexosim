use nexosim::model::{Context, InitializedModel, Model};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::Model;

use std::time::Duration;

use paste::paste;
use serde::{Deserialize, Serialize};

macro_rules! schedulable {
    ($func:ident) => {
        paste! { Self::[<__ $func>]() }
    };
}

#[derive(Debug, Serialize, Deserialize)]
struct MyModel {
    state: u32,
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
    pub async fn path_type(&mut self, arg: std::primitive::usize) {
        //
    }

    #[nexosim(init)]
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        println!("Custom init");
        cx.schedule_event(Duration::from_secs(2), schedulable!(input), 12);
        cx.schedule_event(Duration::from_secs(2), Self::__tick(), ());
        self.into()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OtherModel;
#[Model(Env=u8)]
impl OtherModel {
    pub fn non_input_method(&mut self, cx: &mut Context<Self>) {
        *cx.env() + 12;
    }
}

fn main() {
    let m = MyModel { state: 0 };
    MyModel::__tick();

    let mbox = Mailbox::new();
    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::new()
        .add_model(m, mbox, "my_model")
        .init(t0)
        .unwrap();

    simu.step().unwrap();
    simu.step().unwrap();
}

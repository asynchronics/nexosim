use nexosim::model::{Context, InitializedModel, Model};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::{methods, schedulable};

use std::future::Future;
use std::time::Duration;

use paste::paste;
use serde::{Deserialize, Serialize};

macro_rules! schedulable {
    ($func:ident) => {
        paste! { (Self::[<__ $func>]() , Self::$func) }
    };
}

#[derive(Debug, Serialize, Deserialize)]
struct BasicModel;
#[methods]
impl BasicModel {
    pub async fn non_schedulable_fn(&self) {}
}
impl Model for BasicModel {
    type Env = ();
}

#[derive(Debug, Serialize, Deserialize)]
struct MyModel {
    state: u32,
}
#[methods]
impl MyModel {
    #[schedulable]
    pub async fn input(&mut self, arg: u8) {
        println!("{}", arg);
    }

    #[schedulable]
    pub async fn tick(&mut self) {
        println!("Tick");
    }
}
impl Model for MyModel {
    type Env = ();

    fn init(self, cx: &mut Context<Self>) -> impl Future<Output = InitializedModel<Self>> + Send {
        cx.schedule_test(Duration::from_secs(2), schedulable!(input), 13);
        async { self.into() }
    }
}

fn main() {
    let m = MyModel { state: 0 };
    let bm = BasicModel;

    let mbox = Mailbox::new();
    let bmbox = Mailbox::new();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::new()
        .add_model(m, mbox, "my_model")
        .add_model(bm, bmbox, "basic")
        .init(t0)
        .unwrap();

    simu.step().unwrap();
}

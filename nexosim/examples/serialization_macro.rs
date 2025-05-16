use nexosim::model::{Context, InitializedModel, Model};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim::{init, model, schedulable};

use std::time::Duration;

use paste::paste;
use serde::{Deserialize, Serialize};

// macro_rules! schedulable {
//     ($func:ident) => {
//         paste! { Self::[<__ $func>](), Self::$ident }
//     };
// }
macro_rules! schedulable {
    ($func:ident) => {
        paste! { (Self::[<__ $func>]() , Self::$func) }
    };
}

#[derive(Debug, Serialize, Deserialize)]
struct MyModel {
    state: u32,
}
#[model(Env=())]
impl MyModel {
    #[schedulable]
    pub async fn input(&mut self, arg: u8) {
        println!("{}", arg);
    }

    #[schedulable]
    pub async fn tick(&mut self) {
        println!("Tick");
    }

    #[init]
    async fn init(self, cx: &mut Context<Self>) -> InitializedModel<Self> {
        println!("Custom init");
        cx.schedule_test(Duration::from_secs(2), schedulable!(input), 12);
        self.into()
    }
}

fn main() {
    let mut m = MyModel { state: 0 };

    let mbox = Mailbox::new();
    let t0 = MonotonicTime::EPOCH;
    let mut simu = SimInit::new()
        .add_model(m, mbox, "my_model")
        .init(t0)
        .unwrap();

    simu.step().unwrap();
}

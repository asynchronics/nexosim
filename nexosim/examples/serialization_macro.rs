use nexosim::model::Model;
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::{model, schedulable};

use serde::{Deserialize, Serialize};

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
}

fn main() {
    let mut m = MyModel { state: 0 };
    m.__input();
    m.__tick();

    let mbox = Mailbox::new();
    let sim_init = SimInit::new().add_model(m, mbox, "my_model");
}

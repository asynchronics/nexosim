use nexosim::model::Model;
use nexosim::Model;

use serde::{Deserialize, Serialize};

#[derive(Model, Serialize, Deserialize)]
#[Env(())]
struct MyModel {
    state: u32,
}
impl MyModel {
    pub async fn input(&mut self, arg: u8) {
        #![schedulable]
        println!("{}", arg);
    }
}

fn main() {
    //
}

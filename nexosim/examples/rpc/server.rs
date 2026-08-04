use nexosim::{
    model::Model,
    ports::{EventSource, QuerySource},
    server::run_with_shutdown,
    simulation::{Mailbox, SimInit},
};
use serde::{Deserialize, Serialize};

const HOST: &str = "127.0.0.1";
const PORT: u16 = 8888;

#[derive(Serialize, Deserialize)]
struct MyModel(u32);
#[Model]
impl MyModel {
    async fn store(&mut self, value: u32) {
        self.0 = value;
    }
    async fn load(&mut self) -> u32 {
        self.0
    }
}

fn sim_gen(initial: u32) -> Result<SimInit, Box<dyn std::error::Error>> {
    let model = MyModel(initial);
    let mbox = Mailbox::new();

    let mut bench = SimInit::new();

    EventSource::new()
        .connect(MyModel::store, &mbox)
        .bind_endpoint(&mut bench, "store")?;

    QuerySource::new()
        .connect(MyModel::load, &mbox)
        .bind_endpoint(&mut bench, "load")?;

    let bench = bench.add_model(model, mbox, "my-model");
    Ok(bench)
}

fn main() {
    tracing_subscriber::fmt::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let signal = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    run_with_shutdown(sim_gen, format!("{HOST}:{PORT}").parse().unwrap(), signal).unwrap();
}

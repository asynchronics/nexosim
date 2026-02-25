use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, Model, ProtoModel};
use nexosim::simulation::{EventInjector, Mailbox, SimInit};
use nexosim::time::{MonotonicTime, NoClock, PeriodicTicker};

pub struct ProtoInjector;
impl ProtoModel for ProtoInjector {
    type Model = InjectingModel;

    fn build(
        self,
        cx: &mut nexosim::model::BuildContext<Self>,
    ) -> (Self::Model, <Self::Model as Model>::Env) {
        let injector = cx.event_injector(InjectingModel::input);
        let mapped_injector =
            cx.mapped_event_injector(InjectingModel::input, |&a: &i8| a.abs() as u8);
        (
            InjectingModel,
            InjectingEnv {
                injector,
                mapped_injector,
            },
        )
    }
}

pub struct InjectingEnv {
    injector: EventInjector<u8>,
    mapped_injector: EventInjector<i8>,
}

#[derive(Serialize, Deserialize)]
pub struct InjectingModel;
#[Model(type Env=InjectingEnv)]
impl InjectingModel {
    #[nexosim(init)]
    async fn init(&mut self, _: &Context<Self>, env: &mut InjectingEnv) {
        env.injector.inject(16);
        env.mapped_injector.inject(-24);
    }

    async fn input(&mut self, arg: u8) {
        println!("{arg}");
    }
}

#[allow(dead_code)]
fn main() -> Result<(), nexosim::simulation::SimulationError> {
    let model = ProtoInjector;
    let mbox = Mailbox::new();

    let bench = SimInit::new();

    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench
        .add_model(model, mbox, "model")
        .with_clock(NoClock {}, PeriodicTicker::new(Duration::from_millis(100)))
        .init(t0)?;

    simu.step()?;

    Ok(())
}

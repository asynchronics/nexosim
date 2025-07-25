use std::future::Future;
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use nexosim::model::{Context, InitializedModel};
use nexosim::ports::{EventQueue, EventQueueReader, Output};
use nexosim::simulation::{Address, EventKey, Mailbox, SimInit, SourceId};
use nexosim::time::MonotonicTime;
use nexosim::{schedulable, Model};

#[derive(Default, Serialize, Deserialize)]
struct ModelWithOutput {
    output: Output<String>,
}
#[Model]
impl ModelWithOutput {
    #[nexosim(schedulable)]
    pub async fn send(&mut self, input: u32) {
        self.output.send(format!("{input}")).await;
    }
}

#[derive(Default, Serialize, Deserialize)]
struct ModelWithKey {
    key: Option<EventKey>,
}
#[Model]
impl ModelWithKey {
    #[nexosim(schedulable)]
    pub async fn process(&mut self) {
        if let Some(key) = self.key.take() {
            key.cancel();
        }
    }
    pub fn set_key(&mut self, key: EventKey) {
        self.key = Some(key);
    }
}

#[derive(Serialize, Deserialize)]
struct ModelWithState {
    state: u32,
}
#[Model]
impl ModelWithState {
    pub fn new(state: u32) -> Self {
        Self { state }
    }
    #[nexosim(init)]
    fn init(
        mut self,
        _: &mut Context<Self>,
    ) -> impl Future<Output = InitializedModel<Self>> + Send {
        self.state *= 11;
        async { self.into() }
    }
    #[nexosim(restore)]
    fn restore(
        mut self,
        _: &mut Context<Self>,
    ) -> impl Future<Output = InitializedModel<Self>> + Send {
        self.state *= 13;
        async { self.into() }
    }
    pub async fn query(&mut self) -> u32 {
        self.state
    }
    pub async fn add(&mut self, arg: u32) {
        self.state += arg;
    }
    pub async fn mul(&mut self, arg: u32) {
        self.state *= arg;
    }
}

#[derive(Serialize, Deserialize)]
struct ModelWithSchedule {
    output: Output<u32>,
}
#[Model]
impl ModelWithSchedule {
    pub fn new() -> Self {
        Self {
            output: Output::default(),
        }
    }
    #[nexosim(init)]
    fn init(self, cx: &mut Context<Self>) -> impl Future<Output = InitializedModel<Self>> + Send {
        cx.schedule_periodic_event(
            Duration::from_secs(1),
            Duration::from_secs(2),
            schedulable!(Self::send),
            7,
        )
        .unwrap();
        async { self.into() }
    }
    #[nexosim(schedulable)]
    async fn send(&mut self, arg: u32, cx: &mut Context<Self>) {
        self.output.send(cx.time().as_secs() as u32 * arg).await;
    }
}

#[derive(Serialize, Deserialize)]
pub struct ModelWithGenerics<T, U, const N: usize>
where
    U: Clone + Send + Sync + 'static,
{
    value: T,
    output: Output<U>,
}

#[Model]
impl<T, U, const N: usize> ModelWithGenerics<T, U, N>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    U: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
{
    #[nexosim(schedulable)]
    async fn input(&mut self, arg: (T, U)) {
        self.value = arg.0;
        self.output.send(arg.1).await;
    }

    #[nexosim(init)]
    async fn init(self, _: &mut Context<Self>) -> InitializedModel<Self> {
        self.into()
    }
}

#[test]
fn model_with_output() {
    fn get_bench() -> (SimInit, SourceId<u32>, EventQueueReader<String>) {
        let mbox = Mailbox::new();
        let mut model = ModelWithOutput::default();

        let msg = EventQueue::new();
        model.output.connect_sink(&msg);

        let mut bench = SimInit::new();
        let source_id = bench.register_input(ModelWithOutput::send, &mbox);
        bench = bench.add_model(model, mbox, "modelWithOutput");

        (bench, source_id, msg.into_reader())
    }

    let (bench, source_id, _) = get_bench();
    let t0 = MonotonicTime::EPOCH;

    let mut simu = bench.init(t0).unwrap();
    // Schedule event on an initialized sim.
    let _ = simu
        .scheduler()
        .schedule_event(Duration::from_secs(5), &source_id, 5);

    // Store state with an event scheduled.
    let mut state = Vec::new();
    simu.save(&mut state).unwrap();

    // Recreate the bench with the state restored.
    let (bench, _, mut msg) = get_bench();
    let mut simu = bench.restore(&state[..]).unwrap();

    // Verify that the scheduled event gets fired.
    simu.step().unwrap();
    assert_eq!(msg.next(), Some("5".to_string()));
}

#[test]
fn model_with_key() {
    fn get_bench() -> (
        SimInit,
        SourceId<u32>,
        Address<ModelWithKey>,
        EventQueueReader<String>,
    ) {
        let output_mbox = Mailbox::new();
        let mut output_model = ModelWithOutput::default();

        let key_mbox = Mailbox::new();
        let key_model = ModelWithKey { key: None };

        let msg = EventQueue::new();
        output_model.output.connect_sink(&msg);

        let key_addr = key_mbox.address();

        let mut bench = SimInit::new();
        let output_id = bench.register_input(ModelWithOutput::send, &output_mbox);
        bench = bench
            .add_model(output_model, output_mbox, "modelWithOutput")
            .add_model(key_model, key_mbox, "modelWithKey");

        (bench, output_id, key_addr, msg.into_reader())
    }

    let (bench, output_id, key_addr, _) = get_bench();
    let t0 = MonotonicTime::EPOCH;

    let mut simu = bench.init(t0).unwrap();
    // Schedule event on an initialized sim.
    let key = simu
        .scheduler()
        .schedule_keyed_event(Duration::from_secs(5), &output_id, 5)
        .unwrap();

    let _ = simu.process_event(ModelWithKey::set_key, key, key_addr);

    // Store state with an event scheduled and key set.
    let mut state = Vec::new();
    simu.save(&mut state).unwrap();

    // Recreate the bench with the state restored.
    let (bench, _, key_addr, mut msg) = get_bench();
    let mut simu = bench.restore(&state[..]).unwrap();

    // Cancel the serialized key.
    let _ = simu.process_event(ModelWithKey::process, (), key_addr);

    // Verify that the scheduled event does not fire.
    simu.step().unwrap();
    assert_eq!(msg.next(), None);
}

#[test]
fn model_init_restore() {
    fn get_bench() -> (SimInit, Address<ModelWithState>) {
        let mbox = Mailbox::new();
        let model = ModelWithState::new(1);

        let addr = mbox.address();

        let bench = SimInit::new().add_model(model, mbox, "modelWithKey");

        (bench, addr)
    }

    let (bench, addr) = get_bench();
    let t0 = MonotonicTime::EPOCH;

    let mut simu = bench.init(t0).unwrap();
    // Verify `init` called.
    let model_state = simu.process_query(ModelWithState::query, (), addr).unwrap();
    assert_eq!(model_state, 11);

    // // Store state with an initialized model.
    let mut state = Vec::new();
    simu.save(&mut state).unwrap();

    // // Recreate the bench with the state restored.
    let (bench, addr) = get_bench();
    let mut simu = bench.restore(&state[..]).unwrap();

    // Verify that `restore` has been called instead of `init` this time
    let model_state = simu.process_query(ModelWithState::query, (), addr).unwrap();
    assert_eq!(model_state, 11 * 13);
}

#[test]
fn model_with_schedule() {
    fn get_bench() -> (SimInit, EventQueueReader<u32>) {
        let mbox = Mailbox::new();
        let mut model = ModelWithSchedule::new();

        let msg = EventQueue::new();
        model.output.connect_sink(&msg);

        let bench = SimInit::new().add_model(model, mbox, "modelWithSchedule");

        (bench, msg.into_reader())
    }

    let (bench, mut msg) = get_bench();
    let t0 = MonotonicTime::EPOCH;

    let mut simu = bench.init(t0).unwrap();
    simu.step().unwrap();
    assert_eq!(msg.next(), Some(7));

    // Store state with a scheduled model after one step.
    let mut state = Vec::new();
    simu.save(&mut state).unwrap();

    // Recreate the bench with the state restored.
    let (bench, mut msg) = get_bench();
    let mut simu = bench.restore(&state[..]).unwrap();

    // Verify that the scheduled event gets fired as step two.
    simu.step().unwrap();
    assert_eq!(msg.next(), Some(21));
}

#[test]
fn model_with_generics() {
    fn get_bench() -> (SimInit, EventQueueReader<f64>, SourceId<(i32, f64)>) {
        let mbox = Mailbox::new();
        let mut model = ModelWithGenerics::<i32, f64, 13> {
            value: 8,
            output: Output::<f64>::default(),
        };

        let msg = EventQueue::new();
        model.output.connect_sink(&msg);
        let address = mbox.address();

        let mut bench = SimInit::new().add_model(model, mbox, "modelWithGenerics");
        let input_id = bench.register_input(ModelWithGenerics::input, address);

        (bench, msg.into_reader(), input_id)
    }

    let (bench, mut msg, input_id) = get_bench();
    let t0 = MonotonicTime::EPOCH;

    let mut simu = bench.init(t0).unwrap();
    let scheduler = simu.scheduler();
    scheduler
        .schedule_event(Duration::from_secs(2), &input_id, (-5, 5.14))
        .unwrap();
    scheduler
        .schedule_event(Duration::from_secs(5), &input_id, (-5, 7.14))
        .unwrap();
    simu.step().unwrap();

    assert_eq!(msg.next(), Some(5.14));

    // Store state with a scheduled model after one step.
    let mut state = Vec::new();
    simu.save(&mut state).unwrap();

    // Recreate the bench with the state restored.
    let (bench, mut msg, _) = get_bench();
    let mut simu = bench.restore(&state[..]).unwrap();

    // Verify that the scheduled event gets fired as step two.
    simu.step().unwrap();
    assert_eq!(msg.next(), Some(7.14));
}

#[test]
fn model_relative_order() {
    fn get_bench() -> (
        SimInit,
        SourceId<u32>,
        SourceId<u32>,
        Address<ModelWithState>,
    ) {
        let mbox = Mailbox::new();
        let model = ModelWithState::new(1);

        let addr = mbox.address();

        let mut bench = SimInit::new();
        let add_id = bench.register_input(ModelWithState::add, &addr);
        let mul_id = bench.register_input(ModelWithState::mul, &addr);

        bench = bench.add_model(model, mbox, "modelWithKey");

        (bench, add_id, mul_id, addr)
    }

    // Test mul -> add order.

    let (bench, add_id, mul_id, _) = get_bench();
    let t0 = MonotonicTime::EPOCH;

    let mut simu = bench.init(t0).unwrap();

    // Schedule two events at the same time
    simu.scheduler()
        .schedule_event(Duration::from_secs(1), &mul_id, 7)
        .unwrap();
    simu.scheduler()
        .schedule_event(Duration::from_secs(1), &add_id, 19)
        .unwrap();

    // Store state with an initialized model and events scheduled.
    let mut state = Vec::new();
    simu.save(&mut state).unwrap();

    // Recreate the bench with the state restored.
    let (bench, _, _, addr) = get_bench();
    let mut simu = bench.restore(&state[..]).unwrap();

    // Verify events have been called in the right order.
    simu.step().unwrap();
    assert_eq!(
        11 * 13 * 7 + 19,
        simu.process_query(ModelWithState::query, (), &addr)
            .unwrap()
    );

    // Test add -> mul order.

    let (bench, add_id, mul_id, _) = get_bench();
    let t0 = MonotonicTime::EPOCH;

    let mut simu = bench.init(t0).unwrap();

    // Schedule two events at the same time
    simu.scheduler()
        .schedule_event(Duration::from_secs(1), &add_id, 19)
        .unwrap();
    simu.scheduler()
        .schedule_event(Duration::from_secs(1), &mul_id, 7)
        .unwrap();

    // Store state with an initialized model and events scheduled.
    let mut state = Vec::new();
    simu.save(&mut state).unwrap();

    // Recreate the bench with the state restored.
    let (bench, _, _, addr) = get_bench();
    let mut simu = bench.restore(&state[..]).unwrap();

    // Verify events have been called in the right order.
    simu.step().unwrap();
    assert_eq!(
        (11 * 13 + 19) * 7,
        simu.process_query(ModelWithState::query, (), &addr)
            .unwrap()
    );
}

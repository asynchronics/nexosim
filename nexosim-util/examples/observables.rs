//! Example: processor with observable states.
//!
//! This example demonstrates in particular:
//!
//! * the use of observable states,
//! * state machine with delays.
//!
//! ```text
//!                     ┌───────────┐
//! Switch ON/OFF ●────►│           ├────► Mode
//!                     │ Processor │
//! Process data  ●────►│           ├────► Value
//!                     │           │
//!                     │           ├────► House Keeping
//!                     └───────────┘
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

use nexosim::model::{Context, InitializedModel, Model, schedulable};
use nexosim::ports::{EventQueue, EventQueueReader, EventSinkReader, EventSource, Output};
use nexosim::simulation::{AutoEventKey, Mailbox, SimInit, SimulationError};
use nexosim::time::MonotonicTime;
use nexosim_util::observable::{Observable, Observe};

/// House keeping TM.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Hk {
    pub voltage: f64,
    pub current: f64,
}

impl Default for Hk {
    fn default() -> Self {
        Self {
            voltage: 0.0,
            current: 0.0,
        }
    }
}

/// Processor mode ID.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ModeId {
    #[default]
    Off,
    Idle,
    Processing,
}

/// Processor state.
#[derive(Default, Deserialize, Serialize)]
pub enum State {
    #[default]
    Off,
    Idle,
    Processing(AutoEventKey),
}

impl Observe<ModeId> for State {
    fn observe(&self) -> ModeId {
        match *self {
            State::Off => ModeId::Off,
            State::Idle => ModeId::Idle,
            State::Processing(_) => ModeId::Processing,
        }
    }
}

/// Processor model.
#[derive(Serialize, Deserialize)]
pub struct Processor {
    /// Mode output.
    pub mode: Output<ModeId>,

    /// Calculated value output.
    pub value: Output<u16>,

    /// HK output.
    pub hk: Output<Hk>,

    /// Internal state.
    state: Observable<State, ModeId>,

    /// Accumulator.
    acc: Observable<u16>,

    /// Electrical data.
    elc: Observable<Hk>,
}

#[Model]
impl Processor {
    /// Create a new processor.
    pub fn new() -> Self {
        let mode = Output::new();
        let value = Output::new();
        let hk = Output::new();
        Self {
            mode: mode.clone(),
            value: value.clone(),
            hk: hk.clone(),
            state: Observable::with_default(mode),
            acc: Observable::with_default(value),
            elc: Observable::with_default(hk),
        }
    }

    /// Switch processor ON/OFF.
    pub async fn switch_power(&mut self, on: bool) {
        if on {
            self.state.set(State::Idle).await;
            self.elc
                .set(Hk {
                    voltage: 5.5,
                    current: 0.1,
                })
                .await;
            self.acc.set(0).await;
        } else {
            self.state.set(State::Off).await;
            self.elc.set(Hk::default()).await;
            self.acc.set(0).await;
        }
    }

    /// Process data for dt milliseconds.
    pub async fn process(&mut self, dt: u64, cx: &Context<Self>) {
        if matches!(self.state.observe(), ModeId::Idle | ModeId::Processing) {
            self.state
                .set(State::Processing(
                    cx.schedule_keyed_event(
                        Duration::from_millis(dt),
                        schedulable!(Self::finish_processing),
                        (),
                    )
                    .unwrap()
                    .into_auto(),
                ))
                .await;
            self.elc.modify(|hk| hk.current = 1.0).await;
        }
    }

    /// Finish processing.
    #[nexosim(schedulable)]
    async fn finish_processing(&mut self) {
        self.state.set(State::Idle).await;
        self.acc.modify(|a| *a += 1).await;
        self.elc.modify(|hk| hk.current = 0.1).await;
    }

    /// Propagate all internal states.
    #[nexosim(init)]
    async fn init(mut self) -> InitializedModel<Self> {
        self.state.propagate().await;
        self.acc.propagate().await;
        self.elc.propagate().await;
        self.into()
    }
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.
    let mut proc = Processor::new();

    // Mailboxes.
    let proc_mbox = Mailbox::new();

    // Model handles for simulation.
    let mode = EventQueue::new_open();
    let value = EventQueue::new_open();
    let hk = EventQueue::new_open();

    proc.mode.connect_sink(&mode);
    proc.value.connect_sink(&value);
    proc.hk.connect_sink(&hk);
    let mut mode = mode.into_reader();
    let mut value = value.into_reader();
    let mut hk = hk.into_reader();
    let proc_addr = proc_mbox.address();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let mut simu = SimInit::new().add_model(proc, proc_mbox, "proc");

    let switch_power_event_id = EventSource::new()
        .connect(Processor::switch_power, &proc_addr)
        .register(&mut simu);
    let process_event_id = EventSource::new()
        .connect(Processor::process, &proc_addr)
        .register(&mut simu);

    let mut simu = simu.init(t0)?;

    // ----------
    // Simulation.
    // ----------

    // Initial state.
    expect(
        &mut mode,
        Some(ModeId::Off),
        &mut value,
        Some(0),
        &mut hk,
        0.0,
        0.0,
    );

    // Switch processor on.
    simu.process_event(&switch_power_event_id, true)?;
    expect(
        &mut mode,
        Some(ModeId::Idle),
        &mut value,
        Some(0),
        &mut hk,
        5.5,
        0.1,
    );

    // Trigger processing.
    simu.process_event(&process_event_id, 100)?;
    expect(
        &mut mode,
        Some(ModeId::Processing),
        &mut value,
        None,
        &mut hk,
        5.5,
        1.0,
    );

    // All data processed.
    simu.step_until(Duration::from_millis(101))?;
    expect(
        &mut mode,
        Some(ModeId::Idle),
        &mut value,
        Some(1),
        &mut hk,
        5.5,
        0.1,
    );

    // Trigger long processing.
    simu.process_event(&process_event_id, 100)?;
    expect(
        &mut mode,
        Some(ModeId::Processing),
        &mut value,
        None,
        &mut hk,
        5.5,
        1.0,
    );

    // Trigger short processing, it cancels the previous one.
    simu.process_event(&process_event_id, 10)?;
    expect(
        &mut mode,
        Some(ModeId::Processing),
        &mut value,
        None,
        &mut hk,
        5.5,
        1.0,
    );

    // Wait for short processing to finish, check results.
    simu.step_until(Duration::from_millis(11))?;
    expect(
        &mut mode,
        Some(ModeId::Idle),
        &mut value,
        Some(2),
        &mut hk,
        5.5,
        0.1,
    );

    // Wait long enough, no state change as the long processing has been
    // cancelled.
    simu.step_until(Duration::from_millis(100))?;
    assert_eq!(mode.try_read(), None);
    assert_eq!(value.try_read(), None);
    assert_eq!(hk.try_read(), None);

    Ok(())
}

// Check observable state.
fn expect(
    mode: &mut EventQueueReader<ModeId>,
    mode_ex: Option<ModeId>,
    value: &mut EventQueueReader<u16>,
    value_ex: Option<u16>,
    hk: &mut EventQueueReader<Hk>,
    voltage_ex: f64,
    current_ex: f64,
) {
    assert_eq!(mode.try_read(), mode_ex);
    assert_eq!(value.try_read(), value_ex);
    let hk_value = hk.try_read().unwrap();
    assert!(same(hk_value.voltage, voltage_ex));
    assert!(same(hk_value.current, current_ex));
}

// Compare two voltages or currents.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

//! Example: a simulation that runs infinitely until stopped. This setup is
//! typical for hardware-in-the-loop use case. The test scenario is driven by
//! simulation events.
//!
//! This example demonstrates in particular:
//!
//! * infinite simulation,
//! * blocking event queue,
//! * simulation halting,
//! * system clock,
//! * periodic scheduling,
//! * observable state.
//!
//! ```text
//! ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
//! ┃ Simulation                             ┃
//! ┃   ┌──────────┐       ┌──────────┐mode  ┃
//! ┃   │          │pulses │          ├──────╂┐BlockingEventQueue
//! ┃   │ Detector ├──────►│ Counter  │count ┃├───────────────────►
//! ┃   │          │       │          ├──────╂┘
//! ┃   └──────────┘       └──────────┘      ┃
//! ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
//! ```

use std::future::Future;
use std::thread;
use std::time::Duration;

use rand::Rng;

use nexosim::model::{Context, Model};
use nexosim::ports::{BlockingEventQueue, Output};
use nexosim::simulation::{ActionKey, ExecutionError, Mailbox, SimInit, SimulationError};
use nexosim::time::{AutoSystemClock, MonotonicTime};
use nexosim_util::helper_models::Ticker;
use nexosim_util::joiners::SimulationJoiner;
use nexosim_util::observables::ObservableValue;

const SWITCH_ON_DELAY: Duration = Duration::from_secs(1);
const MAX_PULSE_PERIOD: u64 = 100;
const TICK: Duration = Duration::from_millis(100);
const N: u64 = 10;

/// Counter mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    #[default]
    Off,
    On,
}

/// Simulation event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Mode(Mode),
    Count(u64),
}

/// The `Counter` Model.
pub struct Counter {
    /// Operation mode.
    pub mode: Output<Mode>,

    /// Pulses count.
    pub count: Output<u64>,

    /// Internal state.
    state: ObservableValue<Mode>,

    /// Counter.
    acc: ObservableValue<u64>,
}

impl Counter {
    /// Creates a new `Counter` model.
    fn new() -> Self {
        let mode = Output::default();
        let count = Output::default();
        Self {
            mode: mode.clone(),
            count: count.clone(),
            state: ObservableValue::new(mode),
            acc: ObservableValue::new(count),
        }
    }

    /// Power -- input port.
    pub async fn power_in(&mut self, on: bool, cx: &mut Context<Self>) {
        match *self.state {
            Mode::Off if on => cx
                .schedule_event(SWITCH_ON_DELAY, Self::switch_on, ())
                .unwrap(),
            Mode::On if !on => self.switch_off().await,
            _ => (),
        };
    }

    /// Pulse -- input port.
    pub async fn pulse(&mut self) {
        self.acc.modify(|x| *x += 1).await;
    }

    /// Switches `Counter` on.
    async fn switch_on(&mut self) {
        self.state.set(Mode::On).await;
    }

    /// Switches `Counter` off.
    async fn switch_off(&mut self) {
        self.state.set(Mode::Off).await;
    }
}

impl Model for Counter {}

/// Detector model that produces pulses.
pub struct Detector {
    /// Output pulse.
    pub pulse: Output<()>,

    /// `ActionKey` of the next scheduled detection.
    next: Option<ActionKey>,
}

impl Detector {
    /// Creates a new `Detector` model.
    pub fn new() -> Self {
        Self {
            pulse: Output::default(),
            next: None,
        }
    }

    /// Switches `Detector` on -- input port.
    pub async fn switch_on(&mut self, _: (), cx: &mut Context<Self>) {
        self.schedule_next(cx).await;
    }

    /// Switches `Detector` off -- input port.
    pub async fn switch_off(&mut self) {
        self.next = None;
    }

    /// Generates a pulse.
    ///
    /// Note: self-scheduling async methods must be for now defined with an
    /// explicit signature instead of `async fn` due to a rustc issue.
    fn pulse<'a>(
        &'a mut self,
        _: (),
        cx: &'a mut Context<Self>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            self.pulse.send(()).await;
            self.schedule_next(cx).await;
        }
    }

    /// Schedules the next detection.
    async fn schedule_next(&mut self, cx: &mut Context<Self>) {
        let next = {
            let mut rng = rand::thread_rng();
            rng.gen_range(1..MAX_PULSE_PERIOD)
        };
        self.next = Some(
            cx.schedule_keyed_event(Duration::from_millis(next), Self::pulse, ())
                .unwrap(),
        );
    }
}

impl Model for Detector {}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.

    // The detector model that produces pulses.
    let mut detector = Detector::new();

    // The counter model.
    let mut counter = Counter::new();

    // The ticker model that keeps simulation alive.
    let ticker = Ticker::new(TICK);

    // Mailboxes.
    let detector_mbox = Mailbox::new();
    let counter_mbox = Mailbox::new();
    let ticker_mbox = Mailbox::new();

    // Connections.
    detector.pulse.connect(Counter::pulse, &counter_mbox);

    // Model handles for simulation.
    let detector_addr = detector_mbox.address();
    let counter_addr = counter_mbox.address();
    let observer = BlockingEventQueue::new();
    counter
        .mode
        .map_connect_sink(|m| Event::Mode(*m), &observer);
    counter
        .count
        .map_connect_sink(|c| Event::Count(*c), &observer);
    let mut observer = observer.into_reader();

    // Start time (arbitrary since models do not depend on absolute time).
    let t0 = MonotonicTime::EPOCH;

    // Assembly and initialization.
    let (mut simu, scheduler) = SimInit::new()
        .add_model(detector, detector_mbox, "detector")
        .add_model(counter, counter_mbox, "counter")
        .add_model(ticker, ticker_mbox, "ticker")
        .set_clock(AutoSystemClock::new())
        .init(t0)?;

    // Simulation thread.
    let simulation_handle = SimulationJoiner::new_with_drop_action(
        scheduler.clone(),
        thread::spawn(move || {
            // ---------- Simulation.  ----------
            // Infinitely kept alive by the ticker model until halted.
            simu.step_unbounded()
        }),
        |res| {
            println!("Simulation thread result: {:?}.", res);
        },
    );

    // Switch the counter on.
    scheduler.schedule_event(
        Duration::from_millis(1),
        Counter::power_in,
        true,
        counter_addr,
    )?;

    // Wait until counter mode is `On`.
    loop {
        let event = observer.next();
        match event {
            Some(Event::Mode(Mode::On)) => {
                break;
            }
            None => panic!("Simulation exited unexpectedly"),
            _ => (),
        }
    }

    // Switch the detector on.
    scheduler.schedule_event(
        Duration::from_millis(100),
        Detector::switch_on,
        (),
        detector_addr,
    )?;

    // Wait until `N` detections.
    loop {
        let event = observer.next();
        match event {
            Some(Event::Count(c)) if c >= N => {
                break;
            }
            None => panic!("Simulation exited unexpectedly"),
            _ => (),
        }
    }

    // Stop the simulation.
    match simulation_handle.halt().unwrap() {
        Err(ExecutionError::Halted) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}

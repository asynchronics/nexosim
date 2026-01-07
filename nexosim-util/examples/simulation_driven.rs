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
//! ┃   │          │pulses │          ├──────╂┐ EventQueue
//! ┃   │ Detector ├──────►│ Counter  │count ┃├───────────────────►
//! ┃   │          │       │          ├──────╂┘
//! ┃   └──────────┘       └──────────┘      ┃
//! ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛
//! ```

use std::future::Future;
use std::thread;
use std::time::Duration;

use rand::Rng;
use serde::{Deserialize, Serialize};
use thread_guard::ThreadGuard;

use nexosim::model::{Context, Model, schedulable};
use nexosim::ports::{EventQueue, EventSinkReader, EventSource, Output};
use nexosim::simulation::{
    AutoEventKey, EventKey, ExecutionError, Mailbox, SimInit, SimulationError,
};
use nexosim::time::{AutoSystemClock, MonotonicTime};
use nexosim_util::models::Ticker;
use nexosim_util::observable::Observable;

/// Switch ON delay.
const SWITCH_ON_DELAY: Duration = Duration::from_secs(1);

/// Maximal period between detection pulses.
const MAX_PULSE_PERIOD: u64 = 100;

/// Tick for the `Ticker` model.
const TICK: Duration = Duration::from_millis(100);

/// Initial detections count.
const INITIAL: u64 = 5;

/// Number of detections to wait for.
const N: u64 = 10 + INITIAL;

/// Counter mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Off,
    On,
}

/// Simulation event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Mode(Mode),
    Count(u64),
}

/// The `Counter` Model.
#[derive(Serialize, Deserialize)]
pub struct Counter {
    /// Operation mode.
    pub mode: Output<Mode>,

    /// Pulses count.
    pub count: Output<u64>,

    /// Internal state.
    state: Observable<Mode>,

    /// Counter.
    acc: Observable<u64>,

    /// Switch ON key.
    switch_on: Option<AutoEventKey>,
}

#[Model]
impl Counter {
    /// Creates a new `Counter` model.
    fn new(initial_count: u64) -> Self {
        let mode = Output::default();
        let count = Output::default();
        Self {
            mode: mode.clone(),
            count: count.clone(),
            state: Observable::with_default(mode),
            acc: Observable::new(count, initial_count),
            switch_on: None,
        }
    }

    /// Power -- input port.
    pub async fn power_in(&mut self, on: bool, cx: &Context<Self>) {
        match *self.state {
            Mode::Off if on && self.switch_on.is_none() => {
                self.switch_on = Some(
                    cx.schedule_keyed_event(SWITCH_ON_DELAY, schedulable!(Self::switch_on), ())
                        .unwrap()
                        .into_auto(),
                )
            }
            _ if !on => self.switch_off().await,
            _ => (),
        };
    }

    /// Pulse -- input port.
    pub async fn pulse(&mut self) {
        if *self.state == Mode::On {
            self.acc.modify(|x| *x += 1).await;
        }
    }

    /// Switches `Counter` on.
    #[nexosim(schedulable)]
    async fn switch_on(&mut self) {
        self.switch_on = None;
        self.state.set(Mode::On).await;
    }

    /// Switches `Counter` off.
    async fn switch_off(&mut self) {
        self.switch_on = None;
        self.state.set(Mode::Off).await;
    }
}

/// Detector model that produces pulses.
#[derive(Serialize, Deserialize)]
pub struct Detector {
    /// Output pulse.
    pub pulse: Output<()>,

    /// `ActionKey` of the next scheduled detection.
    next: Option<EventKey>,
}

#[Model]
impl Detector {
    /// Creates a new `Detector` model.
    pub fn new() -> Self {
        Self {
            pulse: Output::default(),
            next: None,
        }
    }

    /// Switches `Detector` on -- input port.
    pub async fn switch_on(&mut self, _: (), cx: &Context<Self>) {
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
    #[nexosim(schedulable)]
    fn pulse<'a>(
        &'a mut self,
        _: (),
        cx: &'a Context<Self>,
    ) -> impl Future<Output = ()> + Send + 'a {
        async move {
            self.pulse.send(()).await;
            self.schedule_next(cx).await;
        }
    }

    /// Schedules the next detection.
    async fn schedule_next(&mut self, cx: &Context<Self>) {
        let next = {
            let mut rng = rand::thread_rng();
            rng.gen_range(1..MAX_PULSE_PERIOD)
        };
        self.next = Some(
            cx.schedule_keyed_event(Duration::from_millis(next), schedulable!(Self::pulse), ())
                .unwrap(),
        );
    }
}

fn main() -> Result<(), SimulationError> {
    // ---------------
    // Bench assembly.
    // ---------------

    // Models.

    // The detector model that produces pulses.
    let mut detector = Detector::new();

    // The counter model.
    let mut counter = Counter::new(INITIAL);

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
    let observer = EventQueue::new_open();
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
    let mut bench = SimInit::new()
        .add_model(detector, detector_mbox, "detector")
        .add_model(counter, counter_mbox, "counter")
        .add_model(ticker, ticker_mbox, "ticker")
        .set_clock(AutoSystemClock::new());

    let power_in_id = EventSource::new()
        .connect(Counter::power_in, counter_addr)
        .register(&mut bench);
    let switch_on_id = EventSource::new()
        .connect(Detector::switch_on, detector_addr)
        .register(&mut bench);

    let mut simu = bench.init(t0)?;

    let scheduler = simu.scheduler();

    // Simulation thread.
    let sim_scheduler = scheduler.clone();
    let simulation_handle = ThreadGuard::with_actions(
        thread::spawn(move || {
            // ---------- Simulation.  ----------
            // Infinitely kept alive by the ticker model until halted.
            simu.step_unbounded()
        }),
        move |_| {
            sim_scheduler.halt();
        },
        |_, res| {
            println!("Simulation thread result: {res:?}.");
        },
    );

    // Switch the counter on.
    scheduler.schedule_event(Duration::from_millis(1), &power_in_id, true)?;

    // Wait until counter mode is `On`.
    loop {
        let event = observer.read();
        match event {
            Some(Event::Mode(Mode::On)) => {
                break;
            }
            None => panic!("Simulation exited unexpectedly"),
            _ => (),
        }
    }

    // Switch the detector on.
    scheduler.schedule_event(Duration::from_millis(100), &switch_on_id, ())?;

    // Wait until `N` detections.
    loop {
        let event = observer.read();
        match event {
            Some(Event::Count(c)) if c >= N => {
                break;
            }
            None => panic!("Simulation exited unexpectedly"),
            _ => (),
        }
    }

    // Stop the simulation.
    match simulation_handle.join().unwrap() {
        Err(ExecutionError::Halted) => Ok(()),
        Err(e) => Err(e.into()),
        _ => Ok(()),
    }
}

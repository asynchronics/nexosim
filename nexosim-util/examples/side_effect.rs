//! Example: counter with observable state with a side effect.
//!
//! This example demonstrates in particular:
//!
//! * the use of observable states,
//! * side effects of observable states.
//!
//! On every change of counter state data is send to the outside thread that
//! forwards it as a UDP packet.
//!
//! NOTE: it is not recommended to do any communication with outside world from
//! the simulation (as it is typically blocking), so the data is first sent to
//! the thread residing outside of the simulation.
//!
//! ```text
//!                     ┌───────────┐
//!                     │           │
//!                     │ Processor │
//! Pulse        ●────►│           ├────► Count
//!                     │           │
//!                     │           │
//!                     └───────────┘
//! ```

use std::net::UdpSocket;
use std::sync::mpsc::channel;
use std::thread;

use serde::{Deserialize, Serialize};
use thread_guard::ThreadGuard;

use nexosim::model::{BuildContext, Model, ProtoModel};
use nexosim::ports::{EventSinkReader, EventSource, Output, SinkState, event_queue};
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::MonotonicTime;
use nexosim_util::observable::Observable;

/// SIMU address.
const SIMU_THREAD_ADDR: &str = "127.0.0.1:34254";

/// HOUSTON server address.
const HOUSTON_THREAD_ADDR: &str = "127.0.0.1:34255";

/// Counter model environment.
pub struct CounterEnv {
    _udp_sender: ThreadGuard<()>,
}

/// Prototype for Counter model.
pub struct ProtoCounter {
    pub count: Output<usize>,
}

impl ProtoCounter {
    /// Creates new prototype for Counter model.
    pub fn new() -> Self {
        Self {
            count: Output::default(),
        }
    }
}

impl ProtoModel for ProtoCounter {
    type Model = Counter;

    fn build(self, _: &mut BuildContext<Self>) -> (Counter, CounterEnv) {
        // Communication with the outside world, especially the blocking one,
        // shall be delegated to an external thread.
        let (tx, rx) = channel();

        (
            Counter {
                count: self.count.clone(),
                count_: Observable::with_default_and_side_effect(self.count, move |_, state| {
                    // Send data to the external thread.
                    let _ = tx.send(*state);
                }),
            },
            CounterEnv {
                _udp_sender: ThreadGuard::new(thread::spawn(move || {
                    let socket = UdpSocket::bind(SIMU_THREAD_ADDR).unwrap();
                    // This thread will exit on simulation exit as `recv` will
                    // fail in this case.
                    while let Ok(count) = rx.recv() {
                        socket
                            .send_to(&count.to_be_bytes(), HOUSTON_THREAD_ADDR)
                            .unwrap();
                    }
                })),
            },
        )
    }
}

/// Counter model.
#[derive(Debug, Serialize, Deserialize)]
pub struct Counter {
    /// Counts output.
    pub count: Output<usize>,

    /// Counter internal state.
    count_: Observable<usize>,
}

#[Model(type Env = CounterEnv)]
impl Counter {
    /// Initializes model.
    #[nexosim(init)]
    async fn init(&mut self) {
        self.count_.propagate().await
    }

    /// Pulses input.
    pub async fn pulse(&mut self, _: ()) {
        self.count_.set(*self.count_ + 1).await
    }
}

/// Main example function.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Server thread (could be an external binary residing on the other host).
    let server = ThreadGuard::new(thread::spawn(|| {
        let socket = UdpSocket::bind(HOUSTON_THREAD_ADDR).unwrap();
        let mut buf = vec![0; size_of::<usize>()];

        // Receive 10 pulses and exit.
        while socket.recv(&mut buf).is_ok() {
            let count = usize::from_be_bytes(buf[..size_of::<usize>()].try_into().unwrap());
            println!("Server received count: {count}");
            if count > 9 {
                break;
            }
        }
    }));

    // ---------------
    // Bench assembly.
    // ---------------

    // Models.
    let mut counter = ProtoCounter::new();

    // Mailboxes.
    let counter_mbox = Mailbox::new();

    // Bench.
    let mut bench = SimInit::new();

    // Sources.
    let pulse = EventSource::new()
        .connect(Counter::pulse, &counter_mbox)
        .register(&mut bench);

    // Sinks.
    let (sink, mut count) = event_queue(SinkState::Enabled);
    counter.count.connect_sink(sink);

    // Assembly and initialization.
    let t0 = MonotonicTime::EPOCH;
    let mut simu = bench.add_model(counter, counter_mbox, "counter").init(t0)?;

    // ----------
    // Simulation.
    // ----------

    // Initial state.
    assert_eq!(count.read(), Some(0));

    // Send 10 pulses to the counter.
    for i in 1..11 {
        simu.process_event(&pulse, ())?;
        assert_eq!(count.read(), Some(i));
    }

    // Join the server thread.
    server.join().unwrap();

    Ok(())
}

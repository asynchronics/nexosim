use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

use diatomic_waker::{WakeSink, WakeSource};
use futures_core::Stream;
use futures_util::stream::StreamExt;
use serde::Serialize;

use crate::model::Message;
use crate::path::Path;
use crate::simulation::{DuplicateEventSinkError, SimInit};
use crate::util::shared_cell::SharedCell;

use super::{EventSinkReader, EventSinkWriter, SinkState};

/// Creates an event slot.
///
/// This function returns an [`EventSlotReader`] and an [`EventSlotWriter`]
/// which implement [`EventSinkReader`] and [`EventSinkWriter`], respectively.
pub fn event_slot<T: Send>(state: SinkState) -> (EventSlotWriter<T>, EventSlotReader<T>) {
    // Create an event slot with one writer.
    let event_slot = Arc::new(EventSlot {
        is_enabled: AtomicBool::new(state == SinkState::Enabled),
        writer_count: AtomicUsize::new(1),
        cell: SharedCell::new(),
    });

    let wake_sink = WakeSink::new();
    let wake_source = wake_sink.source();

    let reader = EventSlotReader {
        inner: event_slot.clone(),
        wake_sink,
    };
    let writer = EventSlotWriter {
        inner: event_slot,
        wake_source,
    };

    (writer, reader)
}

/// Creates an event slot, adding it as a simulation endpoint sink under the
/// provided path.
///
/// This is a convenience function and is equivalent to calling [`event_slot`]
/// and immediately registering the reader as an endpoint with
/// [`SimInit::bind_event_sink`].
pub fn event_slot_endpoint<T: Message + Serialize + Send + 'static>(
    sim_init: &mut SimInit,
    state: SinkState,
    path: impl Into<Path>,
) -> Result<EventSlotWriter<T>, DuplicateEventSinkError> {
    let (writer, reader) = event_slot(state);

    sim_init.bind_event_sink(reader, path).map(|()| writer)
}

/// Creates an event slot, adding it as a simulation endpoint sink under the
/// provided path without requiring a [`Message`] implementation for its item
/// type.
///
/// This is a convenience function and is equivalent to calling [`event_slot`]
/// and immediately registering the reader as an endpoint with
/// [`SimInit::bind_event_sink_raw`].
pub fn event_slot_endpoint_raw<T: Serialize + Send + 'static>(
    sim_init: &mut SimInit,
    state: SinkState,
    path: impl Into<Path>,
) -> Result<EventSlotWriter<T>, DuplicateEventSinkError> {
    let (writer, reader) = event_slot(state);

    sim_init.bind_event_sink_raw(reader, path).map(|()| writer)
}

/// Internal data of an event slot.
#[derive(Default)]
struct EventSlot<T: Send> {
    is_enabled: AtomicBool,
    writer_count: AtomicUsize,
    cell: SharedCell<T>,
}

impl<T: Send> fmt::Debug for EventSlot<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventSlotReader")
            .field("is_enabled", &self.is_enabled.load(Ordering::Relaxed))
            .field("writer_count", &self.writer_count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// The unique consumer handle of an event slot.
///
/// Implements [`EventSinkReader`].
pub struct EventSlotReader<T: Send> {
    inner: Arc<EventSlot<T>>,
    wake_sink: WakeSink,
}

impl<T: Send> Stream for EventSlotReader<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Avoid waker registration if an event is readily available.
        if let Some(event) = self.inner.cell.try_read() {
            return Poll::Ready(Some(event));
        }

        // Register the waker to be polled again once a value is available.
        self.wake_sink.register(cx.waker());

        // Check again after registering the waker to prevent a race condition.
        if let Some(event) = self.inner.cell.try_read() {
            self.wake_sink.unregister();

            return Poll::Ready(Some(event));
        } else if self.inner.writer_count.load(Ordering::Relaxed) == 0 {
            // If there is no event available and no writer left, the stream is
            // exhausted.
            self.wake_sink.unregister();

            return Poll::Ready(None);
        }

        Poll::Pending
    }
}

impl<T: Send + 'static> EventSinkReader<T> for EventSlotReader<T> {
    fn enable(&mut self) {
        self.inner.is_enabled.store(true, Ordering::Relaxed);
    }

    fn disable(&mut self) {
        self.inner.is_enabled.store(false, Ordering::Relaxed);
    }

    fn try_read(&mut self) -> Option<T> {
        self.inner.cell.try_read()
    }

    fn read(&mut self) -> Option<T> {
        pollster::block_on(self.next())
    }
}

impl<T: Send> fmt::Debug for EventSlotReader<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventSlotReader")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

/// A cloneable producer handle of an event slot.
pub struct EventSlotWriter<T: Send> {
    inner: Arc<EventSlot<T>>,
    wake_source: WakeSource,
}

impl<T: Send + 'static> EventSinkWriter<T> for EventSlotWriter<T> {
    fn write(&self, event: T) {
        if !self.inner.is_enabled.load(Ordering::Relaxed) {
            return;
        }

        // Notify the reader if the write is successful. Otherwise, bail out
        // since it means another concurrent writer succeeded.
        if self.inner.cell.try_write(event).is_ok() {
            self.wake_source.notify();
        }
    }
}

impl<T: Send> Drop for EventSlotWriter<T> {
    fn drop(&mut self) {
        if self.inner.writer_count.fetch_sub(1, Ordering::Relaxed) == 1 {
            // This was the last writer: we need to notify the sink that no new
            // events are expected.
            self.wake_source.notify();
        }
    }
}

impl<T: Send> Clone for EventSlotWriter<T> {
    fn clone(&self) -> Self {
        self.inner.writer_count.fetch_add(1, Ordering::Relaxed);

        Self {
            inner: self.inner.clone(),
            wake_source: self.wake_source.clone(),
        }
    }
}

impl<T: Send> fmt::Debug for EventSlotWriter<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("EventSlotWriter")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

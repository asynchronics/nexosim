use std::any::{self, Any, TypeId};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::marker::PhantomData;
#[cfg(feature = "server")]
use std::pin::Pin;
#[cfg(feature = "server")]
use std::task::{Context, Poll};

#[cfg(feature = "server")]
use ciborium;

#[cfg(feature = "server")]
use futures_core::Stream;
use serde::Serialize;

use crate::ports::EventSinkReader;

use super::EndpointError;

#[cfg(feature = "server")]
type SerializationError = ciborium::ser::Error<std::io::Error>;

/// A registry that holds all sinks meant to be accessed through remote
/// procedure calls.
#[derive(Default)]
pub(crate) struct EventSinkRegistry(HashMap<String, Option<Box<dyn EventSinkReaderEntryAny>>>);

impl EventSinkRegistry {
    /// Adds a sink to the registry.
    ///
    /// If the specified name is already used by another sink, the name of the
    /// sink and the event sink are returned in the error.
    pub(crate) fn add<S, T>(&mut self, sink: S, name: String) -> Result<(), (String, S)>
    where
        S: EventSinkReader<T> + Send + Sync + 'static,
        T: Serialize + 'static,
    {
        match self.0.entry(name) {
            Entry::Vacant(s) => {
                s.insert(Some(Box::new(EventSinkReaderEntry {
                    sink,
                    _phantom: PhantomData,
                })));

                Ok(())
            }
            Entry::Occupied(e) => Err((e.key().clone(), sink)),
        }
    }

    /// Removes and returns an event sink reader.
    pub(crate) fn take<T>(
        &mut self,
        name: &str,
    ) -> Result<Box<dyn EventSinkReader<T>>, EndpointError>
    where
        T: Clone + Send + 'static,
    {
        if let Entry::Occupied(entry) = self.0.entry(name.to_string())
            && let Some(inner) = entry.get()
        {
            if inner.event_type_id() == TypeId::of::<T>() {
                // We now know that the downcast will succeed and can safely unwrap.
                let sink = entry
                    .remove_entry()
                    .1
                    .unwrap()
                    .into_event_sink_reader()
                    .downcast::<Box<dyn EventSinkReader<T>>>()
                    .unwrap();

                return Ok(*sink);
            }

            return Err(EndpointError::InvalidEventSinkType {
                name: name.to_string(),
                event_type: any::type_name::<T>(),
            });
        }

        Err(EndpointError::EventSinkNotFound {
            name: name.to_string(),
        })
    }

    /// Returns `true` if a sink under this name is registered, whether or not
    /// it is currently rented.
    #[cfg(feature = "server")]
    pub(crate) fn has_sink(&mut self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// Returns a mutable handle to an entry.
    #[cfg(feature = "server")]
    pub(crate) fn get_entry_mut(
        &mut self,
        name: &str,
    ) -> Result<&mut Box<dyn EventSinkReaderEntryAny>, EndpointError> {
        self.0
            .get_mut(name)
            .and_then(|s| s.as_mut())
            .ok_or_else(|| EndpointError::EventSinkNotFound {
                name: name.to_string(),
            })
    }

    /// Extracts an entry, leaving its name in the registry.
    ///
    /// The entry is expected to be reinserted later with `insert_entry`.
    #[cfg(feature = "server")]
    pub(crate) fn rent_entry(
        &mut self,
        name: &str,
    ) -> Result<Box<dyn EventSinkReaderEntryAny>, EndpointError> {
        self.0.get_mut(name).and_then(|s| s.take()).ok_or_else(|| {
            EndpointError::EventSinkNotFound {
                name: name.to_string(),
            }
        })
    }

    /// Re-inserts an entry under an already registered name, typically after
    /// the entry was rented with `rent_entry`.
    ///
    /// If the name exists in the registry, the entry is always inserted,
    /// whether or not the entry slot is already populated.
    ///
    /// An [`EndpointError::EventSinkNotFound`] is returned if no sink was
    /// registered under this name.
    #[cfg(feature = "server")]
    pub(crate) fn return_entry(
        &mut self,
        name: &str,
        entry: Box<dyn EventSinkReaderEntryAny>,
    ) -> Result<(), EndpointError> {
        self.0
            .get_mut(name)
            .map(|s| {
                *s = Some(entry);
            })
            .ok_or_else(|| EndpointError::EventSinkNotFound {
                name: name.to_string(),
            })
    }
}

impl fmt::Debug for EventSinkRegistry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "EventSinkRegistry ({} sinks)", self.0.len())
    }
}

/// A type-erased `EventSinkReaderEntry`.
#[cfg(feature = "server")]
pub(crate) trait EventSinkReaderEntryAny:
    Stream<Item = Result<Vec<u8>, SerializationError>> + Send + Unpin + 'static
{
    /// Starts or resumes the collection of new events.
    fn open(&mut self);

    /// Pauses the collection of new events.
    fn close(&mut self);

    /// Returns the next event, if any.
    fn try_read(&mut self) -> Option<Result<Vec<u8>, SerializationError>>;

    /// The `TypeId` of the event.
    fn event_type_id(&self) -> TypeId;

    /// Human-readable name of the event type, as returned by `any::type_name`.
    fn event_type_name(&self) -> &'static str;

    /// Consumes this item and returns a `Box<Box<dyn EventSinkReader<T>>>`
    /// (yes, the double-box is needed) cast to a `Box<dyn Any>`.
    fn into_event_sink_reader(self: Box<Self>) -> Box<dyn Any>;
}

/// A type-erased `EventSinkReaderEntry`.
#[cfg(not(feature = "server"))]
pub(crate) trait EventSinkReaderEntryAny {
    /// The `TypeId` of the event.
    fn event_type_id(&self) -> TypeId;

    /// Consumes this item and returns a `Box<Box<dyn EventSinkReader<T>>>`
    /// (yes, the double-box is needed) cast to a `Box<dyn Any>`.
    fn into_event_sink_reader(self: Box<Self>) -> Box<dyn Any>;
}

struct EventSinkReaderEntry<S, T>
where
    S: EventSinkReader<T> + Send + Sync + 'static,
    T: 'static,
{
    sink: S,
    _phantom: PhantomData<fn(T)>,
}

impl<S, T> EventSinkReaderEntryAny for EventSinkReaderEntry<S, T>
where
    S: EventSinkReader<T> + Send + Sync + 'static,
    T: Serialize + 'static,
{
    #[cfg(feature = "server")]
    fn open(&mut self) {
        self.sink.open();
    }
    #[cfg(feature = "server")]
    fn close(&mut self) {
        self.sink.close();
    }
    #[cfg(feature = "server")]
    fn try_read(&mut self) -> Option<Result<Vec<u8>, SerializationError>> {
        self.sink.try_read().map(|event| {
            let mut buffer = Vec::new();
            ciborium::into_writer(&event, &mut buffer).map(|_| buffer)
        })
    }
    fn event_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    #[cfg(feature = "server")]
    fn event_type_name(&self) -> &'static str {
        any::type_name::<T>()
    }
    fn into_event_sink_reader(self: Box<Self>) -> Box<dyn Any> {
        // Make sure we box the trait object and not the concrete sink reader.
        let event_sink_reader: Box<dyn EventSinkReader<T>> = Box::new(self.sink);

        Box::new(event_sink_reader)
    }
}

#[cfg(feature = "server")]
impl<S, T> Stream for EventSinkReaderEntry<S, T>
where
    S: EventSinkReader<T> + Send + Sync + 'static,
    T: Serialize + 'static,
{
    type Item = Result<Vec<u8>, SerializationError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let sink = &mut self.get_mut().sink;
        Pin::new(sink).poll_next(cx).map(|e| {
            e.map(|event| {
                let mut buffer = Vec::new();
                ciborium::into_writer(&event, &mut buffer).map(|_| buffer)
            })
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.sink.size_hint()
    }
}

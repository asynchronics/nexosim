use crate::simulation::EventKey;
use crate::time::MonotonicTime;
use crate::util::indexed_priority_queue::{IndexedPriorityQueue, InsertKey};

pub(crate) type KeyRegistryId = InsertKey;

/// A collection of `EventKey`s indexed by a unique identifier.
#[derive(Default)]
pub(crate) struct KeyRegistry {
    keys: IndexedPriorityQueue<MonotonicTime, EventKey>,
}

impl KeyRegistry {
    /// Inserts an `EventKey` into the registry.
    ///
    /// The provided expiration deadline is the latest time at which the key is
    /// guaranteed to be extractable.
    pub(crate) fn insert_key(
        &mut self,
        event_key: EventKey,
        expiration: MonotonicTime,
    ) -> KeyRegistryId {
        self.keys.insert(expiration, event_key)
    }

    /// Inserts a non-expiring `EventKey` into the registry.
    pub(crate) fn insert_eternal_key(&mut self, event_key: EventKey) -> KeyRegistryId {
        self.keys.insert(MonotonicTime::MAX, event_key)
    }

    /// Removes an `EventKey` from the registry and returns it.
    ///
    /// Returns `None` if the key was not found in the registry.
    pub(crate) fn extract_key(&mut self, key_id: KeyRegistryId) -> Option<EventKey> {
        self.keys.extract(key_id).map(|(_, key)| key)
    }

    /// Remove keys with an expiration deadline strictly predating the argument.
    pub(crate) fn remove_expired_keys(&mut self, now: MonotonicTime) {
        while let Some(expiration) = self.keys.peek_key() {
            if *expiration >= now {
                return;
            }

            self.keys.pull();
        }
    }
}

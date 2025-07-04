use crate::Message;
use alloc::collections::{BTreeMap, BTreeSet};
use indexmap2::{IndexMap, IndexSet};

forward_impl!((<K: Message, V: Message, H> Message for IndexMap<K, V, H>) => BTreeMap<K, V>);
forward_impl!((<T: Message, H> Message for IndexSet<T, H>) => BTreeSet<T>);

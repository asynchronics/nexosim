use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::macros::scoped_thread_local::scoped_thread_local;

scoped_thread_local!(pub(crate) static EVENT_KEY_REG: EventKeyReg);
pub(crate) type EventKeyReg = Arc<Mutex<HashMap<usize, Arc<AtomicBool>>>>;

use serde::{de::Visitor, ser::SerializeTuple, Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, LazyLock, Mutex,
};

static REG: LazyLock<Mutex<HashMap<usize, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Debug)]
struct ActionKey(Arc<AtomicBool>);
impl Serialize for ActionKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut tup = serializer.serialize_tuple(2)?;
        tup.serialize_element(&self.0.load(Ordering::Relaxed))?;
        tup.serialize_element(&Arc::as_ptr(&self.0).addr())?;
        tup.end()
    }
}
impl<'de> Deserialize<'de> for ActionKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ActionKeyVisitor;
        impl<'de> Visitor<'de> for ActionKeyVisitor {
            type Value = ActionKey;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("tuple")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let value = seq.next_element()?.unwrap();
                let addr: usize = seq.next_element()?.unwrap();
                let mut reg = REG.lock().unwrap();
                let target = reg.entry(addr).or_insert(Arc::new(value));
                Ok(ActionKey(target.clone()))
            }
        }

        deserializer.deserialize_tuple_struct("ActionKey", 1, ActionKeyVisitor)
    }
}

fn main() {
    let a = ActionKey(Arc::new(AtomicBool::new(false)));
    let b = a.clone();

    let c = ActionKey(Arc::new(AtomicBool::new(false)));

    let s = serde_json::to_string(&vec![a, b, c]).unwrap();
    println!("{}", s);

    let v: Vec<ActionKey> = serde_json::from_str(&s).unwrap();
    println!("{:?}", v);
    REG.lock().unwrap().clear();

    v[0].0.store(true, Ordering::Relaxed);
    assert!(v[1].0.load(Ordering::Relaxed));
    assert_eq!(false, v[2].0.load(Ordering::Relaxed));
    println!("{:?}", v);
}

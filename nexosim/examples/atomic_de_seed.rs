use serde::{
    de::{DeserializeSeed, Visitor},
    ser::SerializeTuple,
    Deserialize, Deserializer, Serialize,
};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, LazyLock, Mutex,
};

type AtomicReg = HashMap<usize, Arc<AtomicBool>>;

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

struct ActionKeyDeSeed<'a>(&'a mut AtomicReg);
impl<'de, 'a> DeserializeSeed<'de> for ActionKeyDeSeed<'a> {
    type Value = ActionKey;

    fn deserialize<D>(mut self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ActionKeyVisitor<'a>(&'a mut AtomicReg);
        impl<'de, 'a> Visitor<'de> for ActionKeyVisitor<'a> {
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
                let target = self.0.entry(addr).or_insert(Arc::new(value));
                Ok(ActionKey(target.clone()))
            }
        }

        deserializer.deserialize_seq(ActionKeyVisitor(&mut self.0))
    }
}

#[derive(Debug, Serialize)]
struct KeyList(Vec<ActionKey>);
impl<'de> Deserialize<'de> for KeyList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ListVisitor<'a>(&'a mut AtomicReg);
        impl<'de, 'a> Visitor<'de> for ListVisitor<'a> {
            type Value = KeyList;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("sequence")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut output = Vec::new();
                while let Some(element) = seq.next_element_seed(ActionKeyDeSeed(self.0)).unwrap() {
                    output.push(element);
                }
                Ok(KeyList(output))
            }
        }
        let mut reg = HashMap::new();
        deserializer.deserialize_seq(ListVisitor(&mut reg))
    }
}

#[derive(Debug, Serialize)] // --> can't derive Deserialize
struct UserModel {
    pub value: usize,
    pub key: ActionKey,
}

fn main() {
    let a = ActionKey(Arc::new(AtomicBool::new(false)));
    let b = a.clone();

    let c = ActionKey(Arc::new(AtomicBool::new(false)));
    let model = UserModel {
        value: 13,
        key: a.clone(),
    };
    let list = KeyList(vec![a, b, c]);

    let s_list = serde_json::to_string(&list).unwrap();
    println!("{}", s_list);
    let s_model = serde_json::to_string(&model).unwrap();
    println!("{}", s_model);

    let d_list: KeyList = serde_json::from_str(&s_list).unwrap();
    println!("{:?}", d_list);
    d_list.0[0].0.store(true, Ordering::Relaxed);
    println!("{:?}", d_list);
    assert!(d_list.0[1].0.load(Ordering::Relaxed));
    assert_eq!(false, d_list.0[2].0.load(Ordering::Relaxed));
}

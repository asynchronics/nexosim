use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use nexosim::model::{Context, InitializedModel};
use nexosim::Model;

#[derive(Serialize, Deserialize)]
pub struct ModelA<T> {
    value: T,
}
#[Model]
impl<T: Serialize + DeserializeOwned + Send + 'static> ModelA<T> {}

#[derive(Serialize, Deserialize)]
pub struct ModelB<T> {
    value: T,
}
#[Model]
impl<T> ModelB<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
{
    #[nexosim(schedulable)]
    fn input(&mut self, _: T) {}

    #[nexosim(init)]
    async fn init(self, _: &Context<Self>, _: &mut ()) -> InitializedModel<Self> {
        self.into()
    }
}

#[derive(Serialize, Deserialize)]
pub struct ModelC<T, U, W> {
    value: (T, U, W),
}
#[Model]
impl<T, U, W> ModelC<T, U, W>
where
    T: Serialize + DeserializeOwned + Send + 'static,
    U: Serialize + DeserializeOwned + Send + 'static,
    W: Serialize + DeserializeOwned + Send + 'static,
{
}

#[derive(Serialize, Deserialize)]
pub struct ModelD<const N: usize> {}
#[Model]
impl<const N: usize> ModelD<N> {
    fn calc(a: usize) -> usize {
        a * N
    }
}

fn main() {}

use nexosim::time::MonotonicTime;

mod serialization_macro;

fn main() {
    let t0 = MonotonicTime::EPOCH;
    let mut simu = serialization_macro::get_bench().init(t0).unwrap();
}

//! Integration test that runs the gear transmission example.

#[path = "../examples/gear_transmission.rs"]
mod example;

#[test]
fn example_runs_to_completion() {
    example::main().expect("the gear transmission example should complete successfully");
}

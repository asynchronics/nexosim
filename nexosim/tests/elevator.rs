//! Integration test that runs the elevator example.

#[path = "../examples/elevator.rs"]
mod example;

#[test]
fn example_runs_to_completion() {
    example::main().expect("the elevator example should complete successfully");
}

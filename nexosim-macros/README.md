# NeXosim macros

This crate contains proc-macros utilized by the `nexosim` crate.

Currently it provides:

- `Message`: derive macro which enables schema generation for the server endpoint data
- `Model`: proc-macro which implements the `Model` trait and enables custom attributes:
    - `#[nexosim(schedulable)]`: - registers model's input method for self-scheduling
    - `#[nexosim(init)]`: - marks implementation of the `Model::init` method
    - `#[nexosim(restore)]`: - marks implementation of the `Model::restore` method
- `schedulable!`: function-like proc-macro that allows for easy model self-scheduling

Check the [`examples`][ex] directory to learn more about common use-cases.

[ex]: https://github.com/asynchronics/nexosim/tree/main/nexosim/examples

# NeXosim macros

This crate contains the procedural macros used by the `nexosim` crate:

- `Message`: a derive macro which enables schema generation for server endpoints,
- `Model`: an attribute macro which automatically implements the `Model` trait
  and recognizes the following attributes:
  - `#[nexosim(schedulable)]`: registers a model input method as schedulable
    with the `schedulable!` macro,
  - `#[nexosim(init)]`: annotates a method implementating the `Model::init`
    method,
- `schedulable!`: a function-like macro that enables models to schedule their
  own methods without need to register such method beforehand.

// edition:2024
// A module-level constant living in `constants.rs`: this is exactly where the
// policy wants it, so the lint must NOT fire.
#![allow(dead_code)]

const MAX_INSPECTED_BODY: usize = 1024 * 1024;

fn main() {}

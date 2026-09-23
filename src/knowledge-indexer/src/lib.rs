pub use common_models::knowledge::*;

mod extract;
mod project;

pub use extract::{IndexedSource, extract};
pub use project::{load_sources, native_target};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod project_tests;

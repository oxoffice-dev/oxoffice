//! Behaviour of the pptx facade, one module per area, in one test binary:
//! `cargo test -p oxoffice-pptx --test pptx`.

mod charts;
mod corpus;
mod embedded;
// Lint runs this binary without `render` too: the writings run once.
#[cfg(feature = "render")]
mod equivalent_writings;
mod fonts;
mod notes_and_comments;
mod pictures;
#[cfg(feature = "render")]
mod render;
mod save;
mod shapes_and_text;
mod shapes_walk;
mod slides_and_layouts;
mod stack;
mod support;
mod tables;

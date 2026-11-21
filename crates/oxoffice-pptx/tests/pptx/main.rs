//! Behaviour of the pptx facade, one module per area, in one test binary:
//! `cargo test -p oxoffice-pptx --test pptx`.

mod corpus;
mod save;
mod shapes_walk;
mod stack;
mod support;

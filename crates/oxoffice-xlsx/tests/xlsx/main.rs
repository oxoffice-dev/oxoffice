//! Behaviour of the xlsx facade, one module per area, in one test binary:
//! `cargo test -p oxoffice-xlsx --test xlsx`.

mod cells;
mod comments;
mod corpus;
mod edit_cells;
mod equivalent_writings;
mod grid;
mod libreoffice;
mod new_workbook;
mod page_setup;
mod refusals;
mod saving;
mod signatures;
mod styles;
mod support;

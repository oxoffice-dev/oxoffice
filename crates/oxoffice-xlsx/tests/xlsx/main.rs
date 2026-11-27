//! Behaviour of the xlsx facade, one module per area, in one test binary:
//! `cargo test -p oxoffice-xlsx --test xlsx`.

mod corpus;
mod equivalent_writings;
mod grid;
mod libreoffice;
mod new_workbook;
mod page_setup;
mod refusals;
mod saving;
mod signatures;
mod support;

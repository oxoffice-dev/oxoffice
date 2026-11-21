//! Behaviour of the docx facade, one module per area, in one test binary:
//! `cargo test -p oxoffice-docx --test docx`.

mod body_savability;
mod checked_replacement;
mod comment_dates;
mod comments;
mod corpus;
mod fields_and_toc;
mod lists;
mod notes;
mod paragraphs_and_styles;
mod revisions_and_compare;
mod save;
mod sections_and_headers;
mod story_paragraphs;
mod support;
mod tables;
mod text_and_runs;

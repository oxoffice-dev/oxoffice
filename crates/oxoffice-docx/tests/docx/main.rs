//! Behaviour of the docx facade, one module per area, in one test binary:
//! `cargo test -p oxoffice-docx --test docx`.

mod body_savability;
mod checked_replacement;
mod comment_dates;
mod comment_on_text;
mod comments;
mod content_controls;
mod corpus;
mod fields_and_toc;
mod fonts;
mod fragment_import;
mod html_and_markdown;
mod inspection;
mod lists;
mod mail_merge_and_templates;
mod notes;
mod paragraphs_and_styles;
mod pdf;
mod pdf_forms;
mod pictures;
mod properties_and_settings;
mod revisions_and_compare;
mod save;
mod sections_and_headers;
mod settings_strings;
mod story_paragraphs;
mod support;
mod tables;
mod text_and_runs;

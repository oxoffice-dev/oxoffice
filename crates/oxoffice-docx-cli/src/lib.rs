//! The `oxoffice docx` command tree: inspect, convert, diff, and manipulate
//! DOCX files from the command line.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

use clap::{Args, Parser, Subcommand};

mod bibliography;
mod commands;

#[derive(Parser)]
#[command(version, about = "CLI tool for DOCX files")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum FontCommand {
    /// List native font records and faces
    List {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Embed one caller-owned font face with explicit license authorization
    Embed {
        file: PathBuf,
        /// Exact font record/typeface name
        #[arg(long, required_unless_present = "index", conflicts_with = "index")]
        name: Option<String>,
        /// The record at this index of `font list`: one of several records
        /// with the same name
        #[arg(long)]
        index: Option<usize>,
        /// Local TrueType or OpenType font file
        #[arg(long)]
        font: PathBuf,
        #[arg(long, default_value = "regular", value_parser = ["regular", "bold", "italic", "bold-italic"])]
        kind: String,
        /// Explicitly authorize embedding under the identified license
        #[arg(long, required = true)]
        license_authorized: bool,
        #[arg(long)]
        license_identity: String,
        /// Explicit OOXML font key, otherwise derived from checked font bytes
        #[arg(long)]
        font_key: Option<String>,
        /// Mark this Word face as subsetted
        #[arg(long)]
        subsetted: bool,
        #[arg(long, short = 'o')]
        output: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Remove a font record, by name or by index, or only the selected embedded face
    Remove {
        file: PathBuf,
        #[arg(long, required_unless_present = "index", conflicts_with = "index")]
        name: Option<String>,
        /// The record at this index of `font list`: one of several records
        /// with the same name
        #[arg(long)]
        index: Option<usize>,
        #[arg(long, value_parser = ["regular", "bold", "italic", "bold-italic"])]
        kind: Option<String>,
        #[arg(long, short = 'o')]
        output: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Read flags, or set explicitly named tri-state flags into a new output
    Settings {
        file: PathBuf,
        #[arg(long, value_parser = ["true", "false", "none"])]
        embed_true_type_fonts: Option<String>,
        /// Include system fonts: true, false or none (remove setting)
        #[arg(long, value_parser = ["true", "false", "none"])]
        embed_system_fonts: Option<String>,
        #[arg(long, value_parser = ["true", "false", "none"])]
        save_subset_fonts: Option<String>,
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum Command {
    /// Sign current package content with a local PKCS#8 key and X.509 certificate
    Sign {
        file: PathBuf,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        cert: PathBuf,
        #[arg(short = 'o', long)]
        output: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify signatures and complete coverage, without asserting chain trust
    VerifySignatures {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// List, embed, remove and configure explicitly licensed fonts
    Font {
        #[command(subcommand)]
        command: FontCommand,
    },
    /// Print document structure: paragraph/table count, styles, images, metadata
    Inspect {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Extract plain text from a DOCX file
    Text {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output accepted-view rich text as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Count a literal text in every story, by story kind
    ///
    /// Counts what `text --json` reads, paragraph by paragraph, in the
    /// accepted view: the body with its table cells, text boxes and content
    /// controls, each header and footer part, the footnotes, the endnotes and
    /// the comments. Occurrences do not overlap and never cross the end of a
    /// paragraph, so a count above what `replace` replaces names text that
    /// replacement cannot reach, such as a match across the edge of a
    /// content control.
    Count {
        /// Path to the DOCX file
        file: PathBuf,
        /// Literal, case-sensitive text to count
        text: String,
        /// Output the counts as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Inspect top-level body layout geometry
    Layout {
        /// Path to the DOCX file
        file: PathBuf,
        /// Directory of caller fonts used before bundled fallback
        #[arg(long)]
        font_dir: Option<PathBuf>,
        /// Resolve font families in the system fonts first, the bundled
        /// fonts as fallback
        #[arg(long)]
        system_fonts: bool,
        /// Use the bundled fonts only, never a system font: the same output
        /// on every machine (by default a system font draws only what the
        /// bundled fonts cannot)
        #[arg(long, conflicts_with = "system_fonts")]
        bundled_fonts_only: bool,
        /// Output point-space body fragments as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Convert DOCX to another format (pdf, html, md, png, jpeg, tiff)
    Convert {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output format: pdf, html, md, png, jpeg, tiff
        #[arg(long, short = 't')]
        to: Option<String>,
        /// Output file path (defaults to input with new extension)
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
        /// DOCX template for HTML or Markdown input
        #[arg(long)]
        template: Option<PathBuf>,
        /// Replace existing output files, but never the input file
        #[arg(long)]
        force: bool,
        /// DPI for image rendering (default: 150)
        #[arg(long)]
        dpi: Option<u32>,
        /// Directory containing caller font files (.ttf/.otf/.ttc)
        #[arg(long)]
        font_dir: Option<PathBuf>,
        /// Resolve font families in the system fonts first, the bundled
        /// fonts as fallback
        #[arg(long)]
        system_fonts: bool,
        /// Use the bundled fonts only, never a system font: the same output
        /// on every machine (by default a system font draws only what the
        /// bundled fonts cannot)
        #[arg(long, conflicts_with = "system_fonts")]
        bundled_fonts_only: bool,
        /// Revision view for PDF and image output
        #[arg(long, value_parser = commands::parse_revision_view)]
        revision_view: Option<oxoffice_docx::RevisionView>,
        /// Paint tracked changes using plain or legal styling
        #[arg(long, value_parser = commands::parse_revision_style)]
        revision_style: Option<oxoffice_docx::RevisionStyle>,
        /// One-based page range for image output, such as 1,3-5
        #[arg(long)]
        pages: Option<String>,
        /// JPEG quality from 1 through 100
        #[arg(long)]
        quality: Option<u8>,
        /// Preserve unpainted PNG pixels as transparent
        #[arg(long)]
        transparent: bool,
        /// Export the content controls and legacy form fields of the body, its
        /// text boxes, the headers and footers and the notes as fillable PDF form fields (PDF only, accepted view). A control tagged `signature` or
        /// `signature:<name>` becomes an empty signature field.
        #[arg(long)]
        fillable_forms: bool,
        /// Output the files written as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Compare the paragraph text of every story of two DOCX files
    ///
    /// Compares the body paragraphs, the table cells of the body, the text
    /// boxes, the headers and footers of each section, the footnotes, the
    /// endnotes, and the comments. A changed paragraph prints a `-` and a `+`
    /// line, each located between brackets: `[2]` for the second body
    /// paragraph, or a story location such as `[table 1, row 1, cell 2,
    /// paragraph 1]` or `[header default, section 1, paragraph 1]`. A story
    /// that cannot be read is named as not compared. Comments pair by author
    /// and text, then by author and anchor text, not by their order in the
    /// file, so comments a producer only reordered show no difference.
    Diff {
        /// First DOCX file
        file_a: PathBuf,
        /// Second DOCX file
        file_b: PathBuf,
        /// Output the differences as schema-1 JSON
        #[arg(long)]
        json: bool,
        /// Exit with 1 when the files differ and 2 on an error, as `diff` does
        #[arg(long)]
        exit_code: bool,
    },
    /// Replace placeholders in a DOCX file
    ///
    /// Give one pair with -p and -v, or many with --map. Nothing is written
    /// unless every pair with an expected count replaced exactly that count,
    /// and, without --regex or --allow-unreached, unless no placeholder is
    /// left after its replacement, out of its reach. A tab or a positional tab
    /// reads as a tab character, a non-breaking hyphen as "-" and a break as a
    /// line feed, as `text` prints them (in --map JSON, write them "\t" and
    /// "\n"): a placeholder holding one matches it, and a value holding one
    /// writes it. A carriage return or a Windows line ending in a value writes
    /// one line break; a page or column break matched and written back becomes
    /// a line break, a positional tab an ordinary tab and a non-breaking hyphen
    /// a plain one. With --track-author, the replacements are written as
    /// tracked changes by that author, as Word's tracked Replace All writes
    /// them: each whole match deleted and its whole value inserted; a value
    /// equal to its match writes nothing. A later --map pair does not match
    /// across the edge of an earlier pair's insertion, so a pair that
    /// searches an earlier value finds fewer. A match in a text box, or an
    /// edit the tracked changes cannot record, is refused and nothing is
    /// written.
    Replace {
        /// Path to the DOCX file
        file: PathBuf,
        /// Placeholder string, a regular expression with --regex
        #[arg(
            long,
            short = 'p',
            required_unless_present = "map",
            requires = "value",
            conflicts_with = "map"
        )]
        placeholder: Option<String>,
        /// Replacement value, where $1 names a capture group with --regex
        #[arg(long, short = 'v', requires = "placeholder")]
        value: Option<String>,
        /// JSON file holding an array of pairs, applied in order, such as
        /// [{"placeholder": "{{name}}", "value": "Ada", "expect": 2}]
        #[arg(long, value_name = "PAIRS_JSON")]
        map: Option<PathBuf>,
        /// Read each placeholder as a regular expression
        #[arg(long)]
        regex: bool,
        /// Output file path
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Require exactly this many replacements before publishing output
        #[arg(long, conflicts_with = "map")]
        expect: Option<usize>,
        /// Publish even when a placeholder is still in the document after its
        /// replacement, out of its reach
        #[arg(long, conflicts_with = "regex")]
        allow_unreached: bool,
        /// Write the replacements as tracked changes by this author
        #[arg(long, value_name = "NAME")]
        track_author: Option<String>,
        /// RFC 3339 date of the tracked changes, the current time when absent
        #[arg(long, value_name = "RFC3339", requires = "track_author")]
        track_timestamp: Option<String>,
        /// Write the tracked changes without a date
        #[arg(long, requires = "track_author", conflicts_with = "track_timestamp")]
        track_undated: bool,
        /// Output the count of each pair as JSON
        #[arg(long)]
        json: bool,
    },
    /// Validate the package and a documented WordprocessingML subset
    ///
    /// Checks package relationships, well-formed XML 1.0, style ids, body/p/r
    /// children and order, text placement, jc and color, tracked-change id and
    /// author, extents, m:rad required children, relationship ids, note references
    /// and deleted text. Repeated bookmark ids are warnings, and so is each
    /// paragraph numbered by a numbering instance or level the document does
    /// not define, which Word shows without a number. This is not a full schema
    /// validator.
    Validate {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output the verdict, the errors and the warnings as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Render pages to image files
    Render {
        /// Path to the DOCX file
        file: PathBuf,
        /// Directory of caller fonts used before bundled fallback
        #[arg(long)]
        font_dir: Option<PathBuf>,
        /// Resolve font families in the system fonts first, the bundled
        /// fonts as fallback
        #[arg(long)]
        system_fonts: bool,
        /// Use the bundled fonts only, never a system font: the same output
        /// on every machine (by default a system font draws only what the
        /// bundled fonts cannot)
        #[arg(long, conflicts_with = "system_fonts")]
        bundled_fonts_only: bool,
        /// Output directory (defaults to current directory)
        #[arg(long, short = 'o')]
        output_dir: Option<PathBuf>,
        /// Replace existing page images, but never the input file
        #[arg(long)]
        force: bool,
        /// DPI resolution (default: 150)
        #[arg(long, default_value = "150")]
        dpi: f64,
        /// Render only a specific page (0-based index)
        #[arg(long, conflicts_with = "pages")]
        page: Option<usize>,
        /// One-based page range, such as 1,3-5
        #[arg(long)]
        pages: Option<String>,
        /// Output format: png, jpeg, tiff
        #[arg(long, default_value = "png")]
        format: String,
        /// Revision view to render
        #[arg(long, default_value = "accepted", value_parser = commands::parse_revision_view)]
        revision_view: oxoffice_docx::RevisionView,
        /// How the tracked view marks revisions: plain, or legal for blue
        /// insertions, red deletions and green moves
        #[arg(long, default_value = "plain", value_parser = commands::parse_revision_style)]
        revision_style: oxoffice_docx::RevisionStyle,
        /// JPEG quality from 1 through 100
        #[arg(long, default_value = "90")]
        quality: u8,
        /// Preserve unpainted PNG pixels as transparent
        #[arg(long)]
        transparent: bool,
        /// Output the files written as schema-1 JSON
        #[arg(long)]
        json: bool,
    },
    /// Inspect and mutate Word comment threads
    Comment {
        #[command(subcommand)]
        command: CommentCommand,
    },
    /// Inspect and resolve tracked Word revisions
    Revision {
        #[command(subcommand)]
        command: RevisionCommand,
    },
    /// Create a tracked-changes document from an original and edited file
    ///
    /// The original may already hold tracked changes, such as the redline of
    /// an earlier comparison. A paragraph, table or content control that
    /// holds one is kept byte for byte when the edited file holds it
    /// unchanged. Any other change to it fails with its location and creates
    /// no output. Section properties holding revisions must also remain
    /// unchanged in both inputs. Changing them fails without creating output.
    ///
    /// Bookmark ids, which Word renumbers on every save, and the `_Toc` names
    /// a rebuilt table of contents gives its heading bookmarks are no change:
    /// a body bookmark both files hold keeps the original's id, a heading
    /// paired with one of the original's takes its `_Toc` name, a new heading
    /// takes a fresh one where the original uses its own, and the links and
    /// page references of the edited file follow them. Footnote and endnote
    /// ids, which Word also renumbers, take the original's, the notes of each
    /// kind paired in reference order. A footnote or an endnote added or
    /// removed is inserted or deleted whole with its reference, and one whose
    /// reference moved is deleted with its old reference and inserted again,
    /// under a new id, with the new one, as Word shows it.
    ///
    /// No tracked change records a bookmark: a paragraph whose bookmarks
    /// differ is deleted and inserted whole, the insertion first, and a
    /// bookmark both files hold is written once, outside the revision marks,
    /// its start in the insertion and its end in the deletion, as Word
    /// resolves them. Accepting leaves such an end just after the inserted
    /// paragraph, and rejecting such a start at the restored paragraph's
    /// start, over the same text. Such a paragraph is still refused when a
    /// bookmark both files hold starts or ends inside its text, or sits under
    /// another id or inside a hyperlink or control, or when it holds a complex
    /// field or a comment range both files hold.
    Compare {
        /// Original DOCX file
        original: PathBuf,
        /// Edited DOCX file
        edited: PathBuf,
        /// Revision author recorded in the comparison
        #[arg(long)]
        author: String,
        /// RFC 3339 revision timestamp
        #[arg(long)]
        timestamp: String,
        /// Unit of a text change: run, word, or character
        #[arg(
            long,
            value_name = "UNIT",
            default_value = "run",
            value_parser = commands::parse_comparison_granularity
        )]
        granularity: oxoffice_docx::ComparisonGranularity,
        /// Keep the original formatting and record no formatting change
        #[arg(long)]
        ignore_formatting: bool,
        /// Keep the original whitespace and record no whitespace-only change
        #[arg(long)]
        ignore_whitespace: bool,
        /// Keep the original field results and record no field change
        #[arg(long)]
        ignore_fields: bool,
        /// Keep the original comments and anchors, dropping the edited ones
        #[arg(long)]
        ignore_comments: bool,
        /// Keep one story of the original, repeatable: body, header, footer,
        /// comment, text_box, footnote, or endnote
        #[arg(
            long = "ignore-story",
            value_name = "KIND",
            value_parser = commands::parse_comparison_story
        )]
        ignore_stories: Vec<oxoffice_docx::ComparisonStoryKind>,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Rebuild supported fields
    Toc {
        #[command(subcommand)]
        command: TocCommand,
    },
    /// Bibliography sources, style, citations and bibliographies
    Bibliography {
        #[command(subcommand)]
        command: bibliography::BibliographyCommand,
    },
    /// Update field results
    Fields {
        #[command(subcommand)]
        command: FieldsCommand,
    },
    /// Extract physical pictures from supported story owners
    Images {
        #[command(subcommand)]
        command: ImagesCommand,
    },
    /// Read and write core and custom document properties
    Meta {
        #[command(subcommand)]
        command: MetaCommand,
    },
    /// Set the value of content controls by tag
    ///
    /// Every --tag must name at least one control, in the body, a header, a
    /// footer, a note, a comment or a text box, or nothing is written. A
    /// control bound to custom XML gets the value in its bound part too.
    Fill {
        /// Path to the DOCX file
        file: PathBuf,
        /// Set every control whose tag is NAME, repeatable
        #[arg(long = "tag", value_name = "NAME=VALUE", conflicts_with = "values", value_parser = commands::parse_assignment)]
        tags: Vec<(String, String)>,
        /// Fill exact content-control tags, aliases and legacy names from flat JSON
        #[arg(long, value_name = "VALUES.json", conflicts_with = "tags")]
        values: Option<PathBuf>,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the count of each assignment as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FieldsCommand {
    /// Update PAGE, NUMPAGES, PAGEREF, SECTIONPAGES, DATE, TIME, SEQ, REF and
    /// the other supported field results
    ///
    /// Field results that need a value the document does not hold, such as a
    /// mail-merge field, keep their cached result. Page numbers come from
    /// oxoffice's own pagination.
    Update {
        /// Path to the DOCX file
        file: PathBuf,
        /// Civil date and time for DATE and TIME fields, exactly
        /// YYYY-MM-DDTHH:MM:SS, unresolved when absent
        #[arg(long, value_parser = commands::parse_field_date_time)]
        now: Option<oxoffice_docx::FieldDateTime>,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ImagesCommand {
    /// Write each internal media part of supported physical pictures into a directory and list them
    ///
    /// Story ownership is reported. External links are listed without fetching.
    Extract {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output directory, created when absent
        dir: PathBuf,
        /// Output the listing as JSON
        #[arg(long)]
        json: bool,
    },
}

// Parsed once per run, so the size of the `set` variant costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum MetaCommand {
    /// Print the core and custom properties
    Get {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Set core properties and add, replace or remove custom properties
    Set {
        /// Path to the DOCX file
        file: PathBuf,
        #[command(flatten)]
        core: CoreArgs,
        /// Add or replace a custom property, repeatable. An existing number,
        /// integer or Boolean property keeps its type
        #[arg(long = "custom", value_name = "NAME=TYPE:VALUE", value_parser = commands::parse_assignment)]
        custom: Vec<(String, String)>,
        /// Remove a custom property, repeatable
        #[arg(long = "remove-custom", value_name = "NAME")]
        remove_custom: Vec<String>,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the properties written as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct CoreArgs {
    /// Title
    #[arg(long)]
    title: Option<String>,
    /// Author (dc:creator)
    #[arg(long)]
    author: Option<String>,
    /// Subject
    #[arg(long)]
    subject: Option<String>,
    /// Keywords
    #[arg(long)]
    keywords: Option<String>,
    /// Description (comments)
    #[arg(long)]
    description: Option<String>,
    /// Category
    #[arg(long)]
    category: Option<String>,
}

#[derive(Subcommand)]
enum CommentCommand {
    /// List comments in package order
    List {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Add a comment over a zero-based half-open body run range, or on a
    /// piece of text with --anchor
    Add {
        /// Path to the DOCX file
        file: PathBuf,
        #[command(flatten)]
        range: CommentRangeArgs,
        /// Anchor the comment on this literal, case-sensitive text of the
        /// main story instead of a run range
        #[arg(
            long,
            conflicts_with_all = ["start_paragraph", "start_run", "end_paragraph", "end_run"]
        )]
        anchor: Option<String>,
        /// Zero-based occurrence of the --anchor text in document order,
        /// 0 when absent
        #[arg(
            long,
            requires = "anchor",
            conflicts_with_all = ["start_paragraph", "start_run", "end_paragraph", "end_run"]
        )]
        occurrence: Option<usize>,
        /// Count only the --anchor matches in table cells, nested tables
        /// included
        #[arg(
            long,
            requires = "anchor",
            conflicts_with_all = ["start_paragraph", "start_run", "end_paragraph", "end_run"]
        )]
        tables_only: bool,
        /// Comment author
        #[arg(long)]
        author: String,
        /// Optional comment author initials
        #[arg(long)]
        initials: Option<String>,
        /// Comment text, one paragraph per line
        #[arg(long)]
        text: String,
        /// RFC 3339 comment timestamp, the current time when absent
        #[arg(long)]
        date: Option<String>,
        /// Write the comment without a date, as Word's Document Inspector does
        #[arg(long, conflicts_with = "date")]
        undated: bool,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Move an existing root comment and its replies onto literal main-story text
    Move {
        /// Path to the DOCX file
        file: PathBuf,
        /// Existing root comment id
        id: i32,
        /// Literal case-sensitive destination text
        #[arg(long)]
        text: String,
        /// Zero-based destination occurrence
        #[arg(long, default_value_t = 0)]
        occurrence: usize,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Reply to an existing comment
    Reply {
        /// Path to the DOCX file
        file: PathBuf,
        /// Parent comment id
        #[arg(long)]
        id: i32,
        /// Reply author
        #[arg(long)]
        author: String,
        /// Reply text, one paragraph per line
        #[arg(long)]
        text: String,
        /// Optional reply author initials
        #[arg(long)]
        initials: Option<String>,
        /// RFC 3339 reply timestamp, the current time when absent
        #[arg(long)]
        date: Option<String>,
        /// Write the reply without a date, as Word's Document Inspector does
        #[arg(long, conflicts_with = "date")]
        undated: bool,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Mark one comment thread resolved
    Resolve {
        /// Path to the DOCX file
        file: PathBuf,
        /// Comment id
        #[arg(long)]
        id: i32,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove one comment and its replies
    Remove {
        /// Path to the DOCX file
        file: PathBuf,
        /// Comment id
        #[arg(long)]
        id: i32,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct CommentRangeArgs {
    /// Zero-based body paragraph index at the inclusive start
    #[arg(long, required_unless_present = "anchor")]
    start_paragraph: Option<usize>,
    /// Zero-based run boundary at the inclusive start, counting the runs that
    /// `text --json` lists
    #[arg(long, required_unless_present = "anchor")]
    start_run: Option<usize>,
    /// Zero-based body paragraph index at the exclusive end
    #[arg(long, required_unless_present = "anchor")]
    end_paragraph: Option<usize>,
    /// Zero-based run boundary at the exclusive end, counting the runs that
    /// `text --json` lists
    #[arg(long, required_unless_present = "anchor")]
    end_run: Option<usize>,
}

#[derive(Subcommand)]
enum RevisionCommand {
    /// List modeled revisions from every supported story
    List {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Accept revisions from every supported story
    Accept {
        /// Path to the DOCX file
        file: PathBuf,
        #[command(flatten)]
        selector: RevisionSelectorArgs,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Reject revisions from every supported story
    Reject {
        /// Path to the DOCX file
        file: PathBuf,
        #[command(flatten)]
        selector: RevisionSelectorArgs,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct RevisionSelectorArgs {
    /// Select one shared revision id
    #[arg(long, conflicts_with_all = ["author", "start_date", "end_date"])]
    id: Option<i32>,
    /// Select revisions by exact case-sensitive author
    #[arg(long, conflicts_with_all = ["id", "start_date", "end_date"])]
    author: Option<String>,
    /// Inclusive RFC 3339 lower date bound
    #[arg(long, requires = "end_date", conflicts_with_all = ["id", "author"])]
    start_date: Option<String>,
    /// Inclusive RFC 3339 upper date bound
    #[arg(long, requires = "start_date", conflicts_with_all = ["id", "author"])]
    end_date: Option<String>,
}

#[derive(Subcommand)]
enum TocCommand {
    /// Rebuild supported existing table-of-contents fields
    Rebuild {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Run one parsed command. Errors are printed to standard error and end
/// the process with the documented exit status.
pub fn run(cli: Cli) {
    let signing_command = matches!(&cli.command, Command::Sign { .. });
    if let Command::VerifySignatures { file, json } = &cli.command {
        let result = commands::verify_signatures(file, *json);
        let flushed = io::stdout().flush();
        match (result, flushed) {
            (Ok(true), Ok(())) => return,
            (Ok(false), Ok(())) => process::exit(1),
            (Err(error), _) => {
                eprintln!("Error: {error}");
                process::exit(1);
            }
            (_, Err(error)) => {
                eprintln!("Error: {error}");
                process::exit(1);
            }
        }
    }

    // `validate` always carries a verdict in its exit status, so it is
    // dispatched separately from the commands that only report errors.
    if let Command::Validate { file, json } = &cli.command {
        match commands::validate(file, *json) {
            Ok(true) => return,
            Ok(false) => process::exit(1),
            Err(e) => {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        }
    }

    // `diff --exit-code` carries a verdict too: 1 for files that differ, which
    // survives a closed standard output, and 2 for an error.
    let diff_exit_code = matches!(
        cli.command,
        Command::Diff {
            exit_code: true,
            ..
        }
    );
    let mut diff_status = 0;

    let result = match cli.command {
        Command::VerifySignatures { .. } => unreachable!("verification is dispatched above"),
        Command::Sign {
            file,
            key,
            cert,
            output,
            json,
        } => commands::sign(&file, &key, &cert, &output, json),
        Command::Font { command } => match command {
            FontCommand::List { file, json } => commands::font_list(&file, json),
            FontCommand::Embed {
                file,
                name,
                index,
                font,
                kind,
                license_authorized,
                license_identity,
                font_key,
                subsetted,
                output,
                json,
            } => commands::font_embed(
                &file,
                &commands::FontEmbedOptions {
                    record: commands::font_record(name.as_deref(), index),
                    font: &font,
                    kind: &kind,
                    license_authorized,
                    license_identity: &license_identity,
                    font_key: font_key.as_deref(),
                    subsetted,
                    output: &output,
                    json,
                },
            ),
            FontCommand::Remove {
                file,
                name,
                index,
                kind,
                output,
                json,
            } => commands::font_remove(
                &file,
                commands::font_record(name.as_deref(), index),
                kind.as_deref(),
                &output,
                json,
            ),
            FontCommand::Settings {
                file,
                embed_true_type_fonts,
                embed_system_fonts,
                save_subset_fonts,
                output,
                json,
            } => commands::font_settings(
                &file,
                embed_true_type_fonts.as_deref(),
                embed_system_fonts.as_deref(),
                save_subset_fonts.as_deref(),
                output.as_deref(),
                json,
            ),
        },

        Command::Inspect { file, json } => commands::inspect(&file, json),
        Command::Text { file, json } => commands::text(&file, json),
        Command::Count { file, text, json } => commands::count(&file, &text, json),
        Command::Layout {
            file,
            json,
            font_dir,
            system_fonts,
            bundled_fonts_only,
        } => commands::layout(
            &file,
            json,
            font_dir.as_deref(),
            commands::font_switches(system_fonts, bundled_fonts_only),
        ),
        Command::Convert {
            file,
            to,
            output,
            template,
            force,
            dpi,
            font_dir,
            system_fonts,
            bundled_fonts_only,
            revision_view,
            revision_style,
            pages,
            quality,
            transparent,
            fillable_forms,
            json,
        } => commands::convert_input(
            &file,
            to.as_deref(),
            template.as_deref(),
            dpi.is_some()
                || revision_view.is_some()
                || revision_style.is_some()
                || quality.is_some()
                || pages.is_some()
                || transparent
                || font_dir.is_some()
                || system_fonts
                || bundled_fonts_only
                || fillable_forms,
            output.as_deref(),
            force,
            dpi.unwrap_or(150),
            font_dir.as_deref(),
            commands::font_switches(system_fonts, bundled_fonts_only),
            revision_view.unwrap_or(oxoffice_docx::RevisionView::Accepted),
            revision_style.unwrap_or(oxoffice_docx::RevisionStyle::Plain),
            commands::ImageOptions {
                pages: pages.as_deref(),
                quality: quality.unwrap_or(90),
                transparent,
            },
            fillable_forms,
            json,
        ),
        Command::Diff {
            file_a,
            file_b,
            json,
            exit_code: _,
        } => commands::diff(&file_a, &file_b, json, &mut diff_status),
        Command::Replace {
            file,
            placeholder,
            value,
            map,
            regex,
            output,
            expect,
            allow_unreached,
            track_author,
            track_timestamp,
            track_undated,
            json,
        } => commands::replace(
            &file,
            commands::ReplaceInput {
                pair: placeholder.zip(value),
                map: map.as_deref(),
                expect,
                regex,
                allow_unreached,
                track: track_author.map(|author| (author, track_timestamp, track_undated)),
            },
            &output,
            json,
        ),
        // Handled above so its exit code can reflect the verdict.
        Command::Validate { .. } => unreachable!(),
        Command::Render {
            file,
            font_dir,
            system_fonts,
            bundled_fonts_only,
            output_dir,
            force,
            dpi,
            page,
            pages,
            format,
            revision_view,
            revision_style,
            quality,
            transparent,
            json,
        } => commands::render(
            &file,
            output_dir.as_deref(),
            force,
            dpi,
            commands::RenderOptions {
                font_dir: font_dir.as_deref(),
                fonts: commands::font_switches(system_fonts, bundled_fonts_only),
                page,
                pages: pages.as_deref(),
                format: &format,
                revision_view,
                revision_style,
                quality,
                transparent,
            },
            json,
        ),
        Command::Comment { command } => match command {
            CommentCommand::List { file, json } => commands::comment_list(&file, json),
            CommentCommand::Add {
                file,
                range,
                anchor,
                occurrence,
                tables_only,
                author,
                initials,
                text,
                date,
                undated,
                output,
                json,
            } => commands::comment_add(
                &file,
                match (
                    anchor.as_deref(),
                    range.start_paragraph,
                    range.start_run,
                    range.end_paragraph,
                    range.end_run,
                ) {
                    (Some(anchor), ..) => commands::CommentAnchor::Text {
                        text: anchor,
                        occurrence: occurrence.unwrap_or(0),
                        tables_only,
                    },
                    (
                        None,
                        Some(start_paragraph),
                        Some(start_run),
                        Some(end_paragraph),
                        Some(end_run),
                    ) => commands::CommentAnchor::Range(oxoffice_docx::RunRange {
                        start: oxoffice_docx::RunPosition {
                            body_index: start_paragraph,
                            run_index: start_run,
                        },
                        end: oxoffice_docx::RunPosition {
                            body_index: end_paragraph,
                            run_index: end_run,
                        },
                    }),
                    (None, ..) => unreachable!("clap requires the range without --anchor"),
                },
                &author,
                initials.as_deref(),
                &text,
                comment_date(date.as_deref(), undated),
                &output,
                json,
            ),
            CommentCommand::Move {
                file,
                id,
                text,
                occurrence,
                output,
                json,
            } => commands::comment_move(&file, id, &text, occurrence, &output, json),
            CommentCommand::Reply {
                file,
                id,
                author,
                text,
                date,
                undated,
                initials,
                output,
                json,
            } => commands::comment_reply(
                &file,
                id,
                &author,
                initials.as_deref(),
                &text,
                comment_date(date.as_deref(), undated),
                &output,
                json,
            ),
            CommentCommand::Resolve {
                file,
                id,
                output,
                json,
            } => commands::comment_resolve(&file, id, &output, json),
            CommentCommand::Remove {
                file,
                id,
                output,
                json,
            } => commands::comment_remove(&file, id, &output, json),
        },
        Command::Revision { command } => match command {
            RevisionCommand::List { file, json } => commands::revision_list(&file, json),
            RevisionCommand::Accept {
                file,
                selector,
                output,
                json,
            } => commands::resolve_revisions(
                &file,
                commands::RevisionAction::Accept,
                commands::RevisionSelector {
                    id: selector.id,
                    author: selector.author.as_deref(),
                    start_date: selector.start_date.as_deref(),
                    end_date: selector.end_date.as_deref(),
                },
                &output,
                json,
            ),
            RevisionCommand::Reject {
                file,
                selector,
                output,
                json,
            } => commands::resolve_revisions(
                &file,
                commands::RevisionAction::Reject,
                commands::RevisionSelector {
                    id: selector.id,
                    author: selector.author.as_deref(),
                    start_date: selector.start_date.as_deref(),
                    end_date: selector.end_date.as_deref(),
                },
                &output,
                json,
            ),
        },
        Command::Compare {
            original,
            edited,
            author,
            timestamp,
            granularity,
            ignore_formatting,
            ignore_whitespace,
            ignore_fields,
            ignore_comments,
            ignore_stories,
            output,
            json,
        } => commands::compare(
            &original,
            &edited,
            &author,
            &timestamp,
            &oxoffice_docx::ComparisonOptions {
                granularity,
                ignore_formatting,
                ignore_whitespace,
                ignore_fields,
                ignore_comments,
                ignored_stories: ignore_stories,
            },
            &output,
            json,
        ),
        Command::Bibliography { command } => bibliography::run(command),
        Command::Fields { command } => match command {
            FieldsCommand::Update {
                file,
                now,
                output,
                json,
            } => commands::fields_update(&file, now, &output, json),
        },
        Command::Images { command } => match command {
            ImagesCommand::Extract { file, dir, json } => {
                commands::images_extract(&file, &dir, json)
            }
        },
        Command::Meta { command } => match command {
            MetaCommand::Get { file, json } => commands::meta_get(&file, json),
            MetaCommand::Set {
                file,
                core,
                custom,
                remove_custom,
                output,
                json,
            } => commands::meta_set(
                &file,
                &commands::MetaChanges {
                    title: core.title,
                    author: core.author,
                    subject: core.subject,
                    keywords: core.keywords,
                    description: core.description,
                    category: core.category,
                    custom,
                    remove_custom,
                },
                &output,
                json,
            ),
        },
        Command::Fill {
            file,
            tags,
            values,
            output,
            json,
        } => match values {
            Some(values) => commands::fill_values(&file, &values, &output, json),
            None => commands::fill(&file, &tags, &output, json),
        },
        Command::Toc { command } => match command {
            TocCommand::Rebuild { file, output, json } => {
                commands::toc_rebuild(&file, &output, json)
            }
        },
    };

    // Standard output is line buffered, so a last line without a newline is
    // only written, and can only fail, when it is flushed.
    let result = result.and_then(|()| io::stdout().flush().map_err(Into::into));
    if let Err(e) = result {
        // A reader that closes standard output early, as `| head` does, ends
        // the output. That is not a failure of the command.
        let closed_stdout = e
            .downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe);
        if !closed_stdout || signing_command {
            eprintln!("Error: {e}");
            process::exit(if diff_exit_code { 2 } else { 1 });
        }
    }
    if diff_exit_code && diff_status != 0 {
        process::exit(diff_status);
    }
}

/// The date `--date` and `--undated` give a new comment or reply: the
/// current time when neither is given.
fn comment_date(date: Option<&str>, undated: bool) -> oxoffice_docx::CommentDate<'_> {
    match (date, undated) {
        (Some(date), _) => oxoffice_docx::CommentDate::At(date),
        (None, true) => oxoffice_docx::CommentDate::Undated,
        (None, false) => oxoffice_docx::CommentDate::Now,
    }
}

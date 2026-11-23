//! `oxoffice docx bibliography`: sources, style, citations, bibliographies
//! and the update of their results.
//!
//! Style, kind, role and field names are the facade's stable names
//! (`BibliographyStyle::name` and its siblings). Flags take them in snake
//! or kebab case (`harvard_anglia`, `harvard-anglia`); JSON, in and out,
//! uses snake case.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use oxoffice_docx::{
    BibliographyAuthor, BibliographyContributor, BibliographyContributorRole, BibliographyOptions,
    BibliographyPerson, BibliographyProperty, BibliographySource, BibliographySourceField,
    BibliographySourceInfo, BibliographySourceKind, BibliographyStyle, CitationOptions,
    CitationSourceOptions, ContentLocation, Document, StoryKind, StoryRunPosition,
};
use serde_json::{Map, Value, json};

use crate::commands::{mutation_record, print_json, publish_document};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The largest source or citation JSON file read.
const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Subcommand)]
pub enum BibliographyCommand {
    /// List the sources: tag, GUID, kind, locale, contributors, fields and
    /// what could not be read
    Sources {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Add one source, or each source of a JSON array, and print their GUIDs
    ///
    /// The JSON is the `source` object that `sources --json` prints. A
    /// source with no `guid`, or an empty one, gets one derived from its
    /// content.
    AddSource {
        /// Path to the DOCX file
        file: PathBuf,
        /// JSON file holding a source object or an array of them
        #[arg(long, value_name = "SRC.json")]
        source: PathBuf,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Replace sources by tag, each keeping its GUID
    ReplaceSource {
        /// Path to the DOCX file
        file: PathBuf,
        /// JSON file holding a source object or an array of them; a `guid`
        /// left out or empty is the one the source has
        #[arg(long, value_name = "SRC.json")]
        source: PathBuf,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove a source that no citation names
    RemoveSource {
        /// Path to the DOCX file
        file: PathBuf,
        /// Tag of the source
        tag: String,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the saved bibliography style
    Style {
        /// Path to the DOCX file
        file: PathBuf,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Set the style, and the selectors of every bibliography in the document
    SetStyle {
        /// Path to the DOCX file
        file: PathBuf,
        #[command(flatten)]
        options: BibliographyArgs,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Insert a citation after a piece of text, or at a run boundary of a
    /// body paragraph
    Cite {
        /// Path to the DOCX file
        file: PathBuf,
        /// Cite right after this literal, case-sensitive text of the main story
        #[arg(long, required_unless_present = "paragraph", conflicts_with_all = ["paragraph", "run"])]
        after: Option<String>,
        /// Zero-based occurrence of the --after text in document order, 0
        /// when absent
        #[arg(long, requires = "after")]
        occurrence: Option<usize>,
        /// Zero-based body item index of the paragraph
        #[arg(long, requires = "run")]
        paragraph: Option<usize>,
        /// Zero-based run boundary in that paragraph, counting the runs that
        /// `text --json` lists
        #[arg(long, requires = "paragraph")]
        run: Option<usize>,
        /// A cited source, repeatable, in order: its tag, then switches
        /// after commas (pages=, volume=, prefix=, suffix=, no-author,
        /// no-year, no-title)
        #[arg(
            long = "source",
            value_name = "SPEC",
            required_unless_present = "sources",
            conflicts_with = "sources",
            value_parser = parse_citation_source
        )]
        source: Vec<CitationSourceOptions>,
        /// JSON file holding an array of cited sources, for values holding
        /// a comma: {"tag", "pages", "volume", "prefix", "suffix",
        /// "suppress_author", "suppress_year", "suppress_title"}
        #[arg(long, value_name = "CITE.json")]
        sources: Option<PathBuf>,
        /// Formatting locale of the citation (a Windows LCID, such as 1033)
        #[arg(long)]
        locale: Option<u32>,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Insert a bibliography before a body item, and set the style
    Insert {
        /// Path to the DOCX file
        file: PathBuf,
        /// Zero-based body item index; the item count inserts at the end
        #[arg(long)]
        index: usize,
        #[command(flatten)]
        options: BibliographyArgs,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
    /// Rebuild the results of every citation and bibliography and print
    /// what was done
    ///
    /// A field the formatter does not cover yet fails the whole update, and
    /// nothing is written.
    Update {
        /// Path to the DOCX file
        file: PathBuf,
        /// Locale for the fields and sources that name none (a Windows LCID)
        #[arg(long)]
        default_locale: Option<u32>,
        /// Output DOCX file
        #[arg(long, short = 'o')]
        output: PathBuf,
        /// Output the operation record as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
pub struct BibliographyArgs {
    /// Style name, such as apa6, chicago or harvard-anglia
    #[arg(long, value_parser = parse_style)]
    style: BibliographyStyle,
    /// Formatting locale of the bibliography (a Windows LCID)
    #[arg(long)]
    locale: Option<u32>,
    /// List only the sources in this locale (a Windows LCID)
    #[arg(long)]
    locale_filter: Option<u32>,
    /// List only this source, repeatable
    #[arg(long = "tag", value_name = "TAG")]
    tags: Vec<String>,
}

impl BibliographyArgs {
    fn options(self) -> BibliographyOptions {
        BibliographyOptions {
            style: self.style,
            locale: self.locale,
            locale_filter: self.locale_filter,
            tags: self.tags,
        }
    }
}

pub fn run(command: BibliographyCommand) -> Result<()> {
    match command {
        BibliographyCommand::Sources { file, json } => sources(&file, json),
        BibliographyCommand::AddSource {
            file,
            source,
            output,
            json,
        } => add_sources(&file, &source, &output, json),
        BibliographyCommand::ReplaceSource {
            file,
            source,
            output,
            json,
        } => replace_sources(&file, &source, &output, json),
        BibliographyCommand::RemoveSource {
            file,
            tag,
            output,
            json,
        } => {
            let mut doc = Document::open(&file)?;
            doc.remove_bibliography_source(&tag)?;
            publish_document(&mut doc, &output)?;
            mutation_record(
                json,
                "bibliography",
                "remove-source",
                json!({ "tag": tag }),
                &output,
            )
        }
        BibliographyCommand::Style { file, json } => style(&file, json),
        BibliographyCommand::SetStyle {
            file,
            options,
            output,
            json,
        } => {
            let options = options.options();
            let mut doc = Document::open(&file)?;
            doc.set_bibliography_options(&options)?;
            publish_document(&mut doc, &output)?;
            mutation_record(
                json,
                "bibliography",
                "set-style",
                options_json(&options),
                &output,
            )
        }
        BibliographyCommand::Cite {
            file,
            after,
            occurrence,
            paragraph,
            run,
            source,
            sources,
            locale,
            output,
            json,
        } => {
            let sources = match sources {
                Some(path) => citation_sources_file(&path)?,
                None => source,
            };
            let options = CitationOptions { sources, locale };
            let mut doc = Document::open(&file)?;
            let place = match (after, paragraph, run) {
                (Some(text), ..) => {
                    let occurrence = occurrence.unwrap_or(0);
                    doc.insert_citation_after_text(&text, occurrence, &options)?;
                    json!({ "after": text, "occurrence": occurrence })
                }
                (None, Some(paragraph), Some(run)) => {
                    let location = doc
                        .direct_body_item_location(paragraph)?
                        .ok_or_else(|| format!("body item {paragraph} does not exist"))?;
                    let position = StoryRunPosition {
                        location,
                        run_index: run,
                    };
                    doc.insert_citation(&position, &options)?;
                    json!({ "paragraph": paragraph, "run": run })
                }
                _ => unreachable!("clap requires --after or --paragraph and --run"),
            };
            publish_document(&mut doc, &output)?;
            let mut detail = json!({
                "sources": options.sources.iter().map(citation_source_json).collect::<Vec<_>>(),
                "locale": options.locale,
            });
            detail
                .as_object_mut()
                .expect("literal is an object")
                .extend(place.as_object().expect("literal is an object").clone());
            mutation_record(json, "bibliography", "cite", detail, &output)
        }
        BibliographyCommand::Insert {
            file,
            index,
            options,
            output,
            json,
        } => {
            let options = options.options();
            let mut doc = Document::open(&file)?;
            let location = body_location(&doc, index)?;
            doc.insert_bibliography(&location, &options)?;
            publish_document(&mut doc, &output)?;
            let mut detail = options_json(&options);
            detail
                .as_object_mut()
                .expect("literal is an object")
                .insert("index".to_owned(), json!(index));
            mutation_record(json, "bibliography", "insert", detail, &output)
        }
        BibliographyCommand::Update {
            file,
            default_locale,
            output,
            json,
        } => update(&file, default_locale, &output, json),
    }
}

/// The boundary before body item `index`, or the end of the body when
/// `index` is the item count.
fn body_location(doc: &Document, index: usize) -> Result<ContentLocation> {
    let count = doc.content_count();
    if index > count {
        return Err(
            format!("body item index {index} is past the {count} items of the body").into(),
        );
    }
    if index < count {
        return doc
            .direct_body_item_location(index)?
            .ok_or_else(|| format!("body item {index} has no checked location").into());
    }
    let body = doc
        .stories()?
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .ok_or("the document has no body story")?;
    Ok(ContentLocation::end(body))
}

fn sources(file: &Path, json_output: bool) -> Result<()> {
    let doc = Document::open(file)?;
    let sources = doc.bibliography_sources()?;
    if json_output {
        return print_json(json!({
            "sources": sources.iter().map(source_info_json).collect::<Vec<_>>(),
        }));
    }
    let mut out = String::new();
    for info in &sources {
        let kind = match &info.supported {
            Some(source) => source.kind.name().to_owned(),
            None => info.source_type.clone().unwrap_or_else(|| "?".to_owned()),
        };
        let guid = info.guid.as_deref().unwrap_or("-");
        let title = info
            .supported
            .as_ref()
            .and_then(|source| {
                source
                    .properties
                    .iter()
                    .find(|property| property.field == BibliographySourceField::Title)
            })
            .map(|property| property.value.as_str())
            .unwrap_or("");
        out.push_str(&format!("{}\t{kind}\t{guid}\t{title}\n", info.tag));
        for diagnostic in &info.diagnostics {
            out.push_str(&format!("  {diagnostic}\n"));
        }
    }
    if sources.is_empty() {
        out.push_str("no sources\n");
    }
    print!("{out}");
    Ok(())
}

fn add_sources(file: &Path, path: &Path, output: &Path, json_output: bool) -> Result<()> {
    let sources = sources_file(path)?;
    let mut doc = Document::open(file)?;
    let mut added = Vec::new();
    for source in &sources {
        let guid = doc
            .add_bibliography_source(source)
            .map_err(|error| format!("source {:?}: {error}", source.tag))?;
        added.push((source.tag.clone(), guid));
    }
    publish_document(&mut doc, output)?;
    if !json_output {
        for (tag, guid) in &added {
            println!("{tag}\t{guid}");
        }
    }
    mutation_record(
        json_output,
        "bibliography",
        "add-source",
        json!({
            "sources": added
                .iter()
                .map(|(tag, guid)| json!({ "tag": tag, "guid": guid }))
                .collect::<Vec<_>>(),
        }),
        output,
    )
}

fn replace_sources(file: &Path, path: &Path, output: &Path, json_output: bool) -> Result<()> {
    let sources = sources_file(path)?;
    let mut doc = Document::open(file)?;
    let mut replaced = Vec::new();
    for mut source in sources {
        if source.guid.is_empty() {
            let existing = doc
                .bibliography_sources()?
                .into_iter()
                .find(|info| info.tag == source.tag)
                .ok_or_else(|| format!("source {:?}: no source has this tag", source.tag))?;
            source.guid = existing
                .guid
                .ok_or_else(|| format!("source {:?}: the source has no GUID", source.tag))?;
        }
        doc.replace_bibliography_source(&source)
            .map_err(|error| format!("source {:?}: {error}", source.tag))?;
        replaced.push(json!({ "tag": source.tag, "guid": source.guid }));
    }
    publish_document(&mut doc, output)?;
    mutation_record(
        json_output,
        "bibliography",
        "replace-source",
        json!({ "sources": replaced }),
        output,
    )
}

fn style(file: &Path, json_output: bool) -> Result<()> {
    let doc = Document::open(file)?;
    let info = doc.bibliography_style()?;
    if json_output {
        return print_json(json!({
            "style": info.as_ref().map(|info| json!({
                "name": info.supported_style.map(BibliographyStyle::name),
                "style_key": info.style_key,
                "style_path": info.style_path,
                "locale": info.locale,
            })),
        }));
    }
    match info {
        None => println!("no style"),
        Some(info) => println!(
            "{}\t{}\t{}",
            info.supported_style
                .map_or("unknown", BibliographyStyle::name),
            info.style_key.as_deref().unwrap_or("-"),
            info.style_path.as_deref().unwrap_or("-"),
        ),
    }
    Ok(())
}

fn update(
    file: &Path,
    default_locale: Option<u32>,
    output: &Path,
    json_output: bool,
) -> Result<()> {
    let mut doc = Document::open(file)?;
    let report = match default_locale {
        Some(locale) => doc.update_bibliography_with_default_locale(locale)?,
        None => doc.update_bibliography()?,
    };
    publish_document(&mut doc, output)?;
    if !json_output {
        println!(
            "{} citations updated, {} bibliographies rebuilt",
            report.updated_citations, report.rebuilt_bibliographies
        );
        for diagnostic in &report.diagnostics {
            println!("  {diagnostic}");
        }
    }
    mutation_record(
        json_output,
        "bibliography",
        "update",
        json!({
            "updated_citations": report.updated_citations,
            "rebuilt_bibliographies": report.rebuilt_bibliographies,
            "diagnostics": report.diagnostics,
        }),
        output,
    )
}

fn options_json(options: &BibliographyOptions) -> Value {
    json!({
        "style": options.style.name(),
        "locale": options.locale,
        "locale_filter": options.locale_filter,
        "tags": options.tags,
    })
}

// --- names -------------------------------------------------------------

fn flag_name(value: &str) -> String {
    value.replace('-', "_")
}

pub fn parse_style(value: &str) -> std::result::Result<BibliographyStyle, String> {
    BibliographyStyle::from_name(&flag_name(value)).ok_or_else(|| {
        format!(
            "unknown bibliography style {value:?}, expected one of {}",
            BibliographyStyle::ALL
                .iter()
                .map(|style| style.name().replace('_', "-"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

// --- citation sources --------------------------------------------------

/// `TAG[,pages=P][,volume=V][,prefix=F][,suffix=S][,no-author][,no-year][,no-title]`.
pub fn parse_citation_source(spec: &str) -> std::result::Result<CitationSourceOptions, String> {
    let mut parts = spec.split(',');
    let tag = parts.next().unwrap_or_default();
    if tag.trim().is_empty() {
        return Err(format!("citation source {spec:?} has no tag"));
    }
    let mut source = citation_source(tag);
    for part in parts {
        let (name, value) = match part.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (part, None),
        };
        let slot = match (name, value) {
            ("pages", Some(_)) => &mut source.pages,
            ("volume", Some(_)) => &mut source.volume,
            ("prefix", Some(_)) => &mut source.prefix,
            ("suffix", Some(_)) => &mut source.suffix,
            ("no-author", None) => {
                source.suppress_author = true;
                continue;
            }
            ("no-year", None) => {
                source.suppress_year = true;
                continue;
            }
            ("no-title", None) => {
                source.suppress_title = true;
                continue;
            }
            _ => {
                return Err(format!(
                    "citation source {spec:?}: unknown switch {part:?}; expected pages=, volume=, \
                     prefix=, suffix=, no-author, no-year or no-title (a tag or a value holding \
                     a comma goes in --sources CITE.json)"
                ));
            }
        };
        if slot.is_some() {
            return Err(format!("citation source {spec:?} repeats {name}="));
        }
        *slot = value.map(str::to_owned);
    }
    Ok(source)
}

fn citation_source(tag: &str) -> CitationSourceOptions {
    CitationSourceOptions {
        tag: tag.to_owned(),
        pages: None,
        volume: None,
        prefix: None,
        suffix: None,
        suppress_author: false,
        suppress_year: false,
        suppress_title: false,
    }
}

fn citation_sources_file(path: &Path) -> Result<Vec<CitationSourceOptions>> {
    let value = json_file(path)?;
    let Value::Array(items) = value else {
        return Err("--sources requires a JSON array of cited sources".into());
    };
    if items.is_empty() {
        return Err("--sources requires at least one cited source".into());
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let context = format!("cited source {index}");
            let object = object(item, &context)?;
            known_keys(
                object,
                &[
                    "tag",
                    "pages",
                    "volume",
                    "prefix",
                    "suffix",
                    "suppress_author",
                    "suppress_year",
                    "suppress_title",
                ],
                &context,
            )?;
            let mut source = citation_source(&required_string(object, "tag", &context)?);
            source.pages = optional_string(object, "pages", &context)?;
            source.volume = optional_string(object, "volume", &context)?;
            source.prefix = optional_string(object, "prefix", &context)?;
            source.suffix = optional_string(object, "suffix", &context)?;
            source.suppress_author = optional_bool(object, "suppress_author", &context)?;
            source.suppress_year = optional_bool(object, "suppress_year", &context)?;
            source.suppress_title = optional_bool(object, "suppress_title", &context)?;
            Ok(source)
        })
        .collect()
}

fn citation_source_json(source: &CitationSourceOptions) -> Value {
    json!({
        "tag": source.tag,
        "pages": source.pages,
        "volume": source.volume,
        "prefix": source.prefix,
        "suffix": source.suffix,
        "suppress_author": source.suppress_author,
        "suppress_year": source.suppress_year,
        "suppress_title": source.suppress_title,
    })
}

// --- sources as JSON ---------------------------------------------------

fn source_info_json(info: &BibliographySourceInfo) -> Value {
    json!({
        "tag": info.tag,
        "guid": info.guid,
        "source_type": info.source_type,
        "source": info.supported.as_ref().map(source_json),
        "diagnostics": info.diagnostics,
    })
}

fn source_json(source: &BibliographySource) -> Value {
    json!({
        "tag": source.tag,
        "kind": source.kind.name(),
        "guid": source.guid,
        "locale": source.locale,
        "contributors": source.contributors.iter().map(contributor_json).collect::<Vec<_>>(),
        "fields": source
            .properties
            .iter()
            .map(|property| json!([property.field.name(), property.value]))
            .collect::<Vec<_>>(),
    })
}

fn contributor_json(contributor: &BibliographyContributor) -> Value {
    match &contributor.value {
        BibliographyAuthor::People(people) => json!({
            "role": contributor.role.name(),
            "people": people
                .iter()
                .map(|person| json!({
                    "last": person.last,
                    "first": person.first,
                    "middle": person.middle,
                }))
                .collect::<Vec<_>>(),
        }),
        BibliographyAuthor::Corporate(name) => json!({
            "role": contributor.role.name(),
            "corporate": name,
        }),
    }
}

/// The sources of a JSON file: one source object or an array of them.
fn sources_file(path: &Path) -> Result<Vec<BibliographySource>> {
    let value = json_file(path)?;
    let items = match value {
        Value::Array(items) => items,
        value @ Value::Object(_) => vec![value],
        _ => return Err("--source requires a source object or an array of them".into()),
    };
    if items.is_empty() {
        return Err("--source requires at least one source".into());
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| parse_source(item, &format!("source {index}")))
        .collect()
}

fn parse_source(value: &Value, context: &str) -> Result<BibliographySource> {
    let object = object(value, context)?;
    known_keys(
        object,
        &["tag", "kind", "guid", "locale", "contributors", "fields"],
        context,
    )?;
    let tag = required_string(object, "tag", context)?;
    let context = &format!("source {tag:?}");
    let kind = required_string(object, "kind", context)?;
    let kind = BibliographySourceKind::from_name(&kind)
        .ok_or_else(|| format!("{context}: unknown source kind {kind:?}"))?;
    let locale = match object.get("locale") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|locale| u32::try_from(locale).ok())
                .ok_or_else(|| format!("{context}: locale must be a number such as 1033"))?,
        ),
    };
    let contributors = match object.get("contributors") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| parse_contributor(item, context))
            .collect::<Result<_>>()?,
        Some(_) => return Err(format!("{context}: contributors must be an array").into()),
    };
    let properties = match object.get("fields") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| parse_field(item, context))
            .collect::<Result<_>>()?,
        Some(_) => {
            return Err(
                format!("{context}: fields must be an array of [field, value] pairs").into(),
            );
        }
    };
    Ok(BibliographySource {
        tag,
        guid: optional_string(object, "guid", context)?.unwrap_or_default(),
        kind,
        locale,
        contributors,
        properties,
    })
}

fn parse_contributor(value: &Value, context: &str) -> Result<BibliographyContributor> {
    let object = object(value, &format!("{context}: contributor"))?;
    known_keys(
        object,
        &["role", "people", "corporate"],
        &format!("{context}: contributor"),
    )?;
    let role = required_string(object, "role", context)?;
    let role = BibliographyContributorRole::from_name(&role)
        .ok_or_else(|| format!("{context}: unknown contributor role {role:?}"))?;
    let context = &format!("{context}: {} contributor", role.name());
    let value = match (object.get("people"), object.get("corporate")) {
        (Some(Value::Array(people)), None) => BibliographyAuthor::People(
            people
                .iter()
                .map(|person| parse_person(person, context))
                .collect::<Result<_>>()?,
        ),
        (None, Some(Value::String(name))) => BibliographyAuthor::Corporate(name.clone()),
        _ => {
            return Err(format!(
                "{context} needs either \"people\" (an array) or \"corporate\" (a string)"
            )
            .into());
        }
    };
    Ok(BibliographyContributor { role, value })
}

fn parse_person(value: &Value, context: &str) -> Result<BibliographyPerson> {
    let object = object(value, &format!("{context}: person"))?;
    known_keys(
        object,
        &["last", "first", "middle"],
        &format!("{context}: person"),
    )?;
    let parts = |key| -> Result<Vec<String>> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::String(part)) => Ok(vec![part.clone()]),
            Some(Value::Array(parts)) => parts
                .iter()
                .map(|part| {
                    part.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("{context}: person {key} must hold strings").into())
                })
                .collect(),
            Some(_) => Err(format!("{context}: person {key} must be a string or an array").into()),
        }
    };
    Ok(BibliographyPerson {
        last: parts("last")?,
        first: parts("first")?,
        middle: parts("middle")?,
    })
}

fn parse_field(value: &Value, context: &str) -> Result<BibliographyProperty> {
    let pair = value.as_array().filter(|pair| pair.len() == 2);
    let (Some(name), Some(text)) = (
        pair.and_then(|pair| pair[0].as_str()),
        pair.and_then(|pair| pair[1].as_str()),
    ) else {
        return Err(format!("{context}: a field is a [field, value] pair of strings").into());
    };
    let field = BibliographySourceField::from_name(name)
        .ok_or_else(|| format!("{context}: unknown source field {name:?}"))?;
    Ok(BibliographyProperty {
        field,
        value: text.to_owned(),
    })
}

// --- JSON helpers ------------------------------------------------------

fn json_file(path: &Path) -> Result<Value> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(format!("{} exceeds 16 MiB", path.display()).into());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()).into())
}

fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} must be a JSON object").into())
}

fn known_keys(object: &Map<String, Value>, known: &[&str], context: &str) -> Result<()> {
    match object.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!(
            "{context}: unknown key {key:?}, expected {}",
            known.join(", ")
        )
        .into()),
        None => Ok(()),
    }
}

fn required_string(object: &Map<String, Value>, key: &str, context: &str) -> Result<String> {
    optional_string(object, key, context)?
        .ok_or_else(|| format!("{context}: {key:?} is required").into())
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{context}: {key:?} must be a string").into()),
    }
}

fn optional_bool(object: &Map<String, Value>, key: &str, context: &str) -> Result<bool> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{context}: {key:?} must be true or false").into()),
    }
}

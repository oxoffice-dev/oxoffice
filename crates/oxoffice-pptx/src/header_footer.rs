//! Headers and footers: master, layout, slide, notes and handout settings, the footer placeholders and their text fields, and the field identities a copy renews.

use super::*;

impl Presentation {
    /// Returns the `p:hf` flags of one master, `None` without the element
    /// or for an unknown master. New slides receive the date, footer and
    /// slide-number placeholders these flags enable.
    pub fn master_header_footer(&self, master_index: usize) -> Result<Option<HeaderFooterFlags>> {
        let Some(master_part) = self.master_part(master_index)? else {
            return Ok(None);
        };
        let master = self
            .masters
            .iter()
            .find(|record| record.part_name == master_part)
            .ok_or_else(|| {
                footer_malformed(
                    &master_part,
                    "master is absent from the presentation inventory",
                )
            })?
            .master
            .as_ref()
            .map_err(|error| footer_malformed(&master_part, error))?;
        Ok(master.header_footer.as_ref().map(HeaderFooterFlags::from))
    }

    /// Returns the `p:hf` flags of one layout, which win over its master's,
    /// `None` without the element or for an unknown layout.
    pub fn layout_header_footer(&self, layout_index: usize) -> Option<HeaderFooterFlags> {
        self.layouts
            .get(layout_index)
            .and_then(|record| record.layout.header_footer.as_ref())
            .map(HeaderFooterFlags::from)
    }

    /// Returns legal inherited policy provenance without projecting painted slide fields.
    pub fn slide_header_footer_policy(
        &self,
        slide_index: usize,
    ) -> Result<Option<(&'static str, HeaderFooterFlags)>> {
        if self.slides.get(slide_index).is_none() {
            return Err(invalid_slide_mutation(
                "read header/footer policy",
                "slide index out of range",
            ));
        }
        let layout_index = self.slide_layout_index(slide_index).ok_or_else(|| {
            invalid_slide_mutation(
                "read header/footer policy",
                "slide layout is not reached from a master",
            )
        })?;
        if let Some(flags) = self.layout_header_footer(layout_index) {
            return Ok(Some(("layout", flags)));
        }
        let (_, master) = self.layout_master_part(layout_index)?;
        Ok(master
            .header_footer
            .as_ref()
            .map(|flags| ("master", HeaderFooterFlags::from(flags))))
    }
}

impl Presentation {
    /// Returns mutable notes-master header/footer settings when both exist.
    pub fn notes_header_footer_mut(&mut self) -> Option<&mut CT_HeaderFooter> {
        self.notes_master_dirty = self
            .notes_master
            .as_ref()
            .is_some_and(|master| master.header_footer.is_some());
        self.notes_master
            .as_mut()
            .and_then(|master| Arc::make_mut(master).header_footer.as_mut())
    }

    /// Returns mutable handout-master header/footer settings when both exist.
    pub fn handout_header_footer_mut(&mut self) -> Option<&mut CT_HeaderFooter> {
        self.handout_master_dirty = self
            .handout_master
            .as_ref()
            .is_some_and(|master| master.header_footer.is_some());
        self.handout_master
            .as_mut()
            .and_then(|master| Arc::make_mut(master).header_footer.as_mut())
    }
}

/// The date, footer and slide number of slides, as PowerPoint's Header and
/// Footer dialog sets them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeaderFooter {
    /// Whether the slide shows its number.
    pub slide_number: bool,
    /// The footer text, or `None` for no footer.
    pub footer: Option<String>,
    /// The date shown on the slide.
    pub date: HeaderFooterDate,
}

/// The date a slide shows in its date placeholder.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum HeaderFooterDate {
    /// No date placeholder.
    #[default]
    Off,
    /// Fixed text, written as a run.
    Fixed(String),
    /// A `datetime1` to `datetime13` field that PowerPoint refreshes when it
    /// opens the file. `text` is the cached value other readers show and
    /// oxoffice renders, see [`date_field_text`].
    Automatic {
        field_type: String,
        text: Option<String>,
    },
}

/// Which date, footer and slide-number placeholders a master's or layout's
/// `p:hf` supplies. Optional values retain source attribute presence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeaderFooterFlags {
    pub date: Option<bool>,
    pub footer: Option<bool>,
    pub slide_number: Option<bool>,
}

impl From<&CT_HeaderFooter> for HeaderFooterFlags {
    fn from(header_footer: &CT_HeaderFooter) -> Self {
        Self {
            date: header_footer.date_time,
            footer: header_footer.footer,
            slide_number: header_footer.slide_number,
        }
    }
}

/// The kinds of `a:fld` that [`Presentation::add_text_field`] writes.
const FIELD_TYPES: [&str; 15] = [
    "slidenum",
    "datetime",
    "datetime1",
    "datetime2",
    "datetime3",
    "datetime4",
    "datetime5",
    "datetime6",
    "datetime7",
    "datetime8",
    "datetime9",
    "datetime10",
    "datetime11",
    "datetime12",
    "datetime13",
];

/// The text PowerPoint caches for a slide-number field before it numbers it.
pub const SLIDE_NUMBER_FIELD_TEXT: &str = "\u{2039}#\u{203a}";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Formats a calendar date as PowerPoint's English (United States) date
/// fields `datetime1` to `datetime7` show it, for use as cached field text.
///
/// `datetime` reads as `datetime1`. The time formats `datetime8` to
/// `datetime13` need a time of day and are refused, as is an invalid date.
pub fn date_field_text(field_type: &str, year: i32, month: u32, day: u32) -> Result<String> {
    let operation = "format a date field";
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    };
    if day == 0 || day > days_in_month {
        return Err(invalid_slide_mutation(
            operation,
            format!("{year:04}-{month:02}-{day:02} is not a calendar date"),
        ));
    }
    let month_name = MONTHS[month as usize - 1];
    let short_month = &month_name[..3];
    let short_year = year.rem_euclid(100);
    Ok(match field_type {
        "datetime" | "datetime1" => format!("{month}/{day}/{year}"),
        "datetime2" => format!(
            "{}, {month_name} {day}, {year}",
            WEEKDAYS[weekday_index(year, month, day)]
        ),
        "datetime3" => format!("{day} {month_name} {year}"),
        "datetime4" => format!("{month_name} {day}, {year}"),
        "datetime5" => format!("{day}-{short_month}-{short_year:02}"),
        "datetime6" => format!("{month_name} {short_year:02}"),
        "datetime7" => format!("{short_month}-{short_year:02}"),
        other => {
            return Err(invalid_slide_mutation(
                operation,
                format!(
                    "date field type {other:?} is not a date format, use datetime1 to datetime7"
                ),
            ));
        }
    })
}

/// Monday is 0, by the days-from-civil count of 1970-01-01, a Thursday.
fn weekday_index(year: i32, month: u32, day: u32) -> usize {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    (days + 3).rem_euclid(7) as usize
}

/// Builds one `a:fld` of a validated type with its cached text.
pub(crate) fn text_field(
    field_type: &str,
    text: Option<&str>,
    ids: &mut HashSet<String>,
) -> Result<CT_TextField> {
    if !FIELD_TYPES.contains(&field_type) {
        return Err(invalid_slide_mutation(
            "add a field",
            format!(
                "field type {field_type:?} is not supported, use slidenum or datetime1 to datetime13"
            ),
        ));
    }
    oxoffice_core::xml::reject_non_xml_characters("field text", text.unwrap_or(""))
        .map_err(|error| invalid_slide_mutation("add a field", error.to_string()))?;
    let id = next_field_identity(ids)?;
    let escaped = text
        .unwrap_or("")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut field = CT_TextField::from_xml(
        format!(
            "<a:fld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" id=\"{id}\" type=\"{field_type}\"><a:t>{escaped}</a:t></a:fld>"
        )
        .as_bytes(),
    )
    .map_err(|error| invalid_slide_mutation("add a field", error.to_string()))?;
    if text.is_none() {
        field.text = None;
    }
    Ok(field)
}

fn next_field_identity(ids: &mut HashSet<String>) -> Result<String> {
    let mut next = 1u64;
    loop {
        let id = format!("{{8C1E7A2B-5F3D-4B6E-9A40-{next:012X}}}");
        if ids.insert(id.clone()) {
            return Ok(id);
        }
        next = next
            .checked_add(1)
            .filter(|value| *value <= 0xffffffffffff)
            .ok_or_else(|| invalid_slide_mutation("add a field", "field identity exhausted"))?;
    }
}

fn renew_copied_field_ids(body: &mut CT_TextBody, ids: &mut HashSet<String>) -> Result<()> {
    for index in 0..body.paragraphs().len() {
        for run in &mut body.paragraph_mut(index).expect("owned paragraph").runs {
            if let TextRun::Field(field) = run {
                field.id = next_field_identity(ids)?;
            }
        }
    }
    Ok(())
}

/// Appends a field after the paragraph's existing runs.
pub(crate) fn push_field(paragraph: &mut CT_TextParagraph, field: CT_TextField) {
    // `add_run` keeps preserved paragraph children in place, then the new
    // regular run becomes the field.
    paragraph.add_run("");
    if let Some(last) = paragraph.runs.last_mut() {
        *last = TextRun::Field(Box::new(field));
    }
}

/// The three latent placeholder kinds, in PowerPoint's slide order.
const LATENT_KINDS: [PhType; 3] = [PhType::DateTime, PhType::Footer, PhType::SlideNumber];

fn latent_name(kind: &PhType) -> &'static str {
    match kind {
        PhType::DateTime => "Date Placeholder",
        PhType::Footer => "Footer Placeholder",
        _ => "Slide Number Placeholder",
    }
}

fn latent_label(kind: &PhType) -> &'static str {
    match kind {
        PhType::DateTime => "date",
        PhType::Footer => "footer",
        _ => "slide number",
    }
}

fn is_kind(shape: &CT_Shape, kind: &PhType) -> bool {
    shape
        .placeholder
        .as_ref()
        .is_some_and(|placeholder| placeholder.ph_type.as_ref() == Some(kind))
}

/// Finds the first shape placeholder of one kind, groups included.
fn find_latent<'a>(children: &'a [ShapeTreeChild], kind: &PhType) -> Option<&'a CT_Shape> {
    children.iter().find_map(|child| match child {
        ShapeTreeChild::Shape(shape) if is_kind(shape, kind) => Some(shape),
        ShapeTreeChild::GroupShape(group) => find_latent(&group.children, kind),
        _ => None,
    })
}

fn find_latent_mut<'a>(
    children: &'a mut [ShapeTreeChild],
    kind: &PhType,
) -> Option<&'a mut CT_Shape> {
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape) if is_kind(shape, kind) => return Some(shape),
            ShapeTreeChild::GroupShape(group) => {
                if let Some(found) = find_latent_mut(&mut group.children, kind) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn remove_latent(children: &mut Vec<ShapeTreeChild>, kind: &PhType) {
    children.retain_mut(|child| match child {
        ShapeTreeChild::Shape(shape) => !is_kind(shape, kind),
        ShapeTreeChild::GroupShape(group) => {
            remove_latent(&mut group.children, kind);
            true
        }
        _ => true,
    });
}

fn has_field(body: &CT_TextBody, prefix: &str) -> bool {
    field_type(body).is_some_and(|field_type| field_type.starts_with(prefix))
}

fn field_type(body: &CT_TextBody) -> Option<&str> {
    body.paragraphs().iter().find_map(|paragraph| {
        paragraph.runs.iter().find_map(|run| match run {
            TextRun::Field(field) => field.field_type.as_deref(),
            _ => None,
        })
    })
}

/// One paragraph holding only `field`, formatted as `template`'s first one.
///
/// The field keeps the character properties, `lang` included, of the first
/// run or field it replaces, as PowerPoint does.
fn field_paragraph(template: Option<&CT_TextBody>, mut field: CT_TextField) -> CT_TextParagraph {
    let mut paragraph = template
        .and_then(|body| body.paragraphs().first())
        .cloned()
        .unwrap_or_default();
    let properties = paragraph.runs.iter().find_map(|run| match run {
        TextRun::Run(run) => run.properties.clone(),
        TextRun::Field(field) => field.run_properties.clone(),
        TextRun::Break(_) => None,
    });
    if field.run_properties.is_none() {
        field.run_properties = properties;
    }
    paragraph.set_text("");
    if paragraph.runs.is_empty() {
        paragraph.add_run("");
    }
    if let Some(first) = paragraph.runs.first_mut() {
        *first = TextRun::Field(Box::new(field));
    }
    paragraph
}

/// What one latent placeholder holds on a slide.
enum LatentContent<'a> {
    SlideNumber,
    Text(&'a str),
    DateField {
        field_type: &'a str,
        text: Option<&'a str>,
    },
}

/// Rewrites a latent placeholder's paragraphs to hold `content`, keeping the
/// formatting of its first paragraph. A slide number that already holds a
/// `slidenum` field is kept as it is.
fn fill_latent_body(
    body: &mut CT_TextBody,
    content: &LatentContent<'_>,
    ids: &mut HashSet<String>,
) -> Result<()> {
    let paragraph = match content {
        LatentContent::SlideNumber if has_field(body, "slidenum") => return Ok(()),
        LatentContent::SlideNumber => field_paragraph(
            Some(body),
            text_field("slidenum", Some(SLIDE_NUMBER_FIELD_TEXT), ids)?,
        ),
        LatentContent::Text(text) => {
            if body.plain_text() == *text
                && !body
                    .paragraphs()
                    .iter()
                    .flat_map(|paragraph| &paragraph.runs)
                    .any(|run| matches!(run, TextRun::Field(_)))
            {
                return Ok(());
            }
            body.set_text(text);
            return Ok(());
        }
        LatentContent::DateField { field_type, text }
            if body.paragraphs().len() == 1
                && body.paragraphs()[0].runs.len() == 1
                && matches!(&body.paragraphs()[0].runs[0], TextRun::Field(field)
                if field.field_type.as_deref().map(|value| if value == "datetime" { "datetime1" } else { value }) == Some(if *field_type == "datetime" { "datetime1" } else { *field_type }) && (text.is_none() || field.text.as_ref().map(|value| value.value.as_str()) == *text)) =>
        {
            return Ok(());
        }
        LatentContent::DateField { field_type, text } => {
            field_paragraph(Some(body), text_field(field_type, *text, ids)?)
        }
    };
    body.set_text("");
    if let Some(first) = body.paragraph_mut(0) {
        *first = paragraph;
    }
    Ok(())
}

/// The layout placeholder a new slide copies, else the master one placed by
/// an explicit transform, since a slide placeholder inherits only through
/// its layout.
fn latent_template<'a>(
    layout: &'a CT_SlideLayout,
    master: &'a CT_SlideMaster,
    kind: &PhType,
) -> Option<(&'a CT_Shape, bool)> {
    find_latent(&layout.common_slide_data.shape_tree.children, kind)
        .map(|shape| (shape, false))
        .or_else(|| {
            find_latent(&master.common_slide_data.shape_tree.children, kind)
                .map(|shape| (shape, true))
        })
}

/// Finds the unique inherited placeholder without guessing a kind from its name.
fn footer_geometry_owner<'a>(
    children: &'a [ShapeTreeChild],
    key: &PlaceholderKey,
) -> Result<Option<&'a CT_Shape>> {
    let mut found = None;
    for child in children {
        let candidate = match child {
            ShapeTreeChild::Shape(shape) => shape
                .placeholder
                .as_ref()
                .is_some_and(|placeholder| {
                    if matches!(
                        key.ph_type,
                        PhType::DateTime | PhType::Footer | PhType::SlideNumber
                    ) {
                        key.ph_type == placeholder.key().ph_type
                    } else {
                        key.matches(&placeholder.key())
                    }
                })
                .then_some(shape),
            ShapeTreeChild::GroupShape(group) => footer_geometry_owner(&group.children, key)?,
            _ => None,
        };
        if let Some(candidate) = candidate {
            if found.is_some() {
                return Err(invalid_slide_mutation(
                    "set the header and footer",
                    "ambiguous inherited footer geometry",
                ));
            }
            found = Some(candidate);
        }
    }
    Ok(found)
}

/// Uses the same whole-transform precedence as slide rendering. An incomplete
/// explicit transform cannot borrow an extent from a different owner.
fn validate_footer_geometry(
    shape: &CT_Shape,
    layout: &CT_SlideLayout,
    master: Option<&CT_SlideMaster>,
) -> Result<()> {
    let mut transform = shape.shape_properties.transform.as_ref();
    if transform.is_none()
        && let Some(placeholder) = shape.placeholder.as_ref()
        && let Some(layout_shape) = footer_geometry_owner(
            &layout.common_slide_data.shape_tree.children,
            &placeholder.key(),
        )?
    {
        transform = layout_shape.shape_properties.transform.as_ref();
        if transform.is_none()
            && let (Some(master), Some(layout_placeholder)) =
                (master, layout_shape.placeholder.as_ref())
        {
            transform = footer_geometry_owner(
                &master.common_slide_data.shape_tree.children,
                &layout_placeholder.key(),
            )?
            .and_then(|owner| owner.shape_properties.transform.as_ref());
        }
    }
    if !transform.is_some_and(|value| {
        value
            .extent
            .is_some_and(|extent| extent.cx.0 > 0 && extent.cy.0 > 0)
    }) {
        return Err(invalid_slide_mutation(
            "set the header and footer",
            "missing effective footer geometry",
        ));
    }
    Ok(())
}

/// A slide-owned copy of a layout or master latent placeholder.
fn latent_copy(
    template: &CT_Shape,
    from_master: bool,
    kind: &PhType,
    ids: &mut ShapeIdAllocator,
) -> Result<CT_Shape> {
    let _ = from_master;
    let id = ids.allocate();
    let mut shape = template
        .clone_with_id(id)
        .map_err(|error| invalid_slide_mutation("add a header or footer", error.to_string()))?;
    shape
        .set_name(&format!("{} {}", latent_name(kind), id))
        .map_err(|error| invalid_slide_mutation("add a header or footer", error.to_string()))?;
    Ok(shape)
}

fn copy_footer_template(
    children: &[ShapeTreeChild],
    kind: &PhType,
    ids: &mut ShapeIdAllocator,
    field_ids: &mut HashSet<String>,
) -> Result<Option<ShapeTreeChild>> {
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape) if is_kind(shape, kind) => {
                let mut shape = latent_copy(shape, true, kind, ids)?;
                if let Some(body) = shape.text_body.as_mut() {
                    renew_copied_field_ids(body, field_ids)?;
                }
                return Ok(Some(ShapeTreeChild::Shape(shape)));
            }
            ShapeTreeChild::GroupShape(group) => {
                if let Some(child) = copy_footer_template(&group.children, kind, ids, field_ids)? {
                    let mut cloned = group.clone_with_id(ids.allocate()).map_err(|error| {
                        invalid_slide_mutation("set the header and footer", error.to_string())
                    })?;
                    cloned.children = vec![child];
                    return Ok(Some(ShapeTreeChild::GroupShape(Box::new(cloned))));
                }
            }
            _ => {}
        }
    }
    Ok(None)
}

/// Gives a slide exactly the date, footer and slide-number placeholders
/// `settings` asks for, copying missing ones from its layout or master.
fn apply_to_slide(
    slide: &mut CT_Slide,
    layout: &CT_SlideLayout,
    master: &CT_SlideMaster,
    layout_name: &str,
    settings: &HeaderFooter,
    field_ids: &mut HashSet<String>,
) -> Result<()> {
    let mut ids = ShapeIdAllocator::scan(&slide.common_slide_data.shape_tree);
    for kind in LATENT_KINDS {
        let content = match (&kind, settings) {
            (
                PhType::SlideNumber,
                HeaderFooter {
                    slide_number: true, ..
                },
            ) => Some(LatentContent::SlideNumber),
            (
                PhType::Footer,
                HeaderFooter {
                    footer: Some(text), ..
                },
            ) => Some(LatentContent::Text(text)),
            (PhType::DateTime, HeaderFooter { date, .. }) => match date {
                HeaderFooterDate::Off => None,
                HeaderFooterDate::Fixed(text) => Some(LatentContent::Text(text)),
                HeaderFooterDate::Automatic { field_type, text } => {
                    Some(LatentContent::DateField {
                        field_type,
                        text: text.as_deref(),
                    })
                }
            },
            _ => None,
        };
        let count = footer_placeholder_count(&slide.common_slide_data.shape_tree.children, &kind);
        if count > 1 {
            return Err(invalid_slide_mutation(
                "set the header and footer",
                "duplicate slide footer placeholder",
            ));
        }
        let children = &mut slide.common_slide_data.shape_tree.children;
        let Some(content) = content else {
            remove_latent(children, &kind);
            continue;
        };
        if find_latent(children, &kind).is_none() {
            let (template, from_master) =
                latent_template(layout, master, &kind).ok_or_else(|| {
                    invalid_slide_mutation(
                        "set the header and footer",
                        format!(
                            "layout {layout_name:?} and its master have no {} placeholder to copy, add one to the master or layout first",
                            latent_label(&kind)
                        ),
                    )
                })?;
            if footer_placeholder_count(&layout.common_slide_data.shape_tree.children, &kind) > 1
                || (footer_placeholder_count(&layout.common_slide_data.shape_tree.children, &kind)
                    == 0
                    && footer_placeholder_count(
                        &master.common_slide_data.shape_tree.children,
                        &kind,
                    ) > 1)
            {
                return Err(invalid_slide_mutation(
                    "set the header and footer",
                    "ambiguous template footer placeholder",
                ));
            }
            validate_footer_geometry(template, layout, Some(master))?;
            let source_xml = template.to_xml().map_err(|error| {
                invalid_slide_mutation("set the header and footer", error.to_string())
            })?;
            if !relationship_ids(&source_xml)
                .map_err(|error| {
                    invalid_slide_mutation("set the header and footer", error.to_string())
                })?
                .is_empty()
            {
                return Err(invalid_slide_mutation(
                    "set the header and footer",
                    "relationship-bearing footer template requires owner remapping",
                ));
            }
            let source = if from_master {
                &master.common_slide_data.shape_tree.children
            } else {
                &layout.common_slide_data.shape_tree.children
            };
            let copy =
                copy_footer_template(source, &kind, &mut ids, field_ids)?.ok_or_else(|| {
                    invalid_slide_mutation(
                        "set the header and footer",
                        "missing qualified template",
                    )
                })?;
            let copied_xml = match &copy {
                ShapeTreeChild::Shape(shape) => shape.to_xml(),
                ShapeTreeChild::GroupShape(group) => group.to_xml(),
                _ => unreachable!(),
            }
            .map_err(|e| invalid_slide_mutation("set the header and footer", e.to_string()))?;
            if !relationship_ids(&copied_xml)
                .map_err(|e| invalid_slide_mutation("set the header and footer", e.to_string()))?
                .is_empty()
            {
                return Err(invalid_slide_mutation(
                    "set the header and footer",
                    "relationship-bearing footer group requires owner remapping",
                ));
            }
            children.push(copy);
        }
        let shape = find_latent_mut(children, &kind).expect("the placeholder exists");
        validate_footer_geometry(shape, layout, Some(master))?;
        let body = shape.text_body.get_or_insert_with(CT_TextBody::new);
        fill_latent_body(body, &content, field_ids)?;
    }
    Ok(())
}

/// Reads what a slide's own latent placeholders show.
fn read_slide(slide: &CT_Slide) -> HeaderFooter {
    let children = &slide.common_slide_data.shape_tree.children;
    let text = |kind: &PhType| {
        find_latent(children, kind).map(|shape| {
            (
                shape
                    .text_body
                    .as_ref()
                    .map(CT_TextBody::plain_text)
                    .unwrap_or_default(),
                shape.text_body.as_ref(),
            )
        })
    };
    let date = match text(&PhType::DateTime) {
        None => HeaderFooterDate::Off,
        Some((_, Some(body))) if has_field(body, "datetime") => HeaderFooterDate::Automatic {
            field_type: field_type(body).unwrap_or("datetime1").to_owned(),
            text: body
                .paragraphs()
                .iter()
                .flat_map(|paragraph| &paragraph.runs)
                .find_map(|run| match run {
                    TextRun::Field(field) => {
                        Some(field.text.as_ref().map(|value| value.value.clone()))
                    }
                    _ => None,
                })
                .flatten(),
        },
        Some((text, _)) => HeaderFooterDate::Fixed(text),
    };
    HeaderFooter {
        slide_number: find_latent(children, &PhType::SlideNumber).is_some(),
        footer: text(&PhType::Footer).map(|(text, _)| text),
        date,
    }
}

/// The `p:hf` flags that make new slides receive these placeholders.
fn update_header_footer_flags(flags: &mut Option<CT_HeaderFooter>, settings: &HeaderFooter) {
    let flags = flags.get_or_insert_with(|| CT_HeaderFooter::new(None, None, None, None));
    flags.slide_number = Some(settings.slide_number);
    flags.footer = Some(settings.footer.is_some());
    flags.date_time = Some(settings.date != HeaderFooterDate::Off);
}

/// Updates the text of a template's footer and date placeholders, so a
/// slide added later copies what the deck shows.
fn update_templates(
    children: &mut [ShapeTreeChild],
    settings: &HeaderFooter,
    ids: &mut HashSet<String>,
) -> Result<()> {
    if let (Some(text), Some(shape)) = (
        settings.footer.as_deref(),
        find_latent_mut(children, &PhType::Footer),
    ) {
        fill_latent_body(
            shape.text_body.get_or_insert_with(CT_TextBody::new),
            &LatentContent::Text(text),
            ids,
        )?;
    }
    if let Some(shape) = find_latent_mut(children, &PhType::DateTime) {
        let content = match &settings.date {
            HeaderFooterDate::Off => return Ok(()),
            HeaderFooterDate::Fixed(text) => LatentContent::Text(text),
            HeaderFooterDate::Automatic { field_type, text } => LatentContent::DateField {
                field_type,
                text: text.as_deref(),
            },
        };
        fill_latent_body(
            shape.text_body.get_or_insert_with(CT_TextBody::new),
            &content,
            ids,
        )?;
    }
    Ok(())
}

fn validate_settings(settings: &HeaderFooter) -> Result<()> {
    let date_text = match &settings.date {
        HeaderFooterDate::Off => None,
        HeaderFooterDate::Fixed(text) => Some(text.as_str()),
        HeaderFooterDate::Automatic { text, .. } => text.as_deref(),
    };
    for text in settings.footer.as_deref().into_iter().chain(date_text) {
        oxoffice_core::xml::reject_non_xml_characters("footer text", text).map_err(|error| {
            invalid_slide_mutation("set the header and footer", error.to_string())
        })?;
    }
    if let HeaderFooterDate::Automatic { field_type, .. } = &settings.date
        && !(field_type == "datetime" || (1..=13).any(|i| field_type == &format!("datetime{i}")))
    {
        return Err(invalid_slide_mutation(
            "set the header and footer",
            "use datetime1 to datetime13",
        ));
    }
    Ok(())
}

impl Presentation {
    /// Returns a layout's master part and parsed master.
    pub(crate) fn layout_master_part(
        &self,
        layout_index: usize,
    ) -> Result<(String, CT_SlideMaster)> {
        let record = &self.layouts[layout_index];
        let master_part =
            related_internal_part(&self.package, &record.part_name, rel_types::SLIDE_MASTER)?
                .ok_or_else(|| {
                    footer_malformed(&record.part_name, "layout has no slide master relationship")
                })?;
        let master = self
            .masters
            .iter()
            .find(|record| record.part_name == master_part)
            .ok_or_else(|| {
                footer_malformed(
                    &master_part,
                    "master is absent from the presentation inventory",
                )
            })?
            .master
            .as_ref()
            .map_err(|error| footer_malformed(&master_part, error))?
            .clone();
        Ok((master_part, master))
    }

    /// The latent placeholders a new slide on this layout receives.
    ///
    /// Explicit authoring copies supported layout or master templates enabled
    /// by the layout policy, else the master policy.
    /// Without either container a new slide receives none.
    pub(crate) fn new_slide_latent_shapes(
        &self,
        layout_index: usize,
        ids: &mut ShapeIdAllocator,
    ) -> Result<Vec<ShapeTreeChild>> {
        let layout = &self.layouts[layout_index].layout;
        if layout.layout_type() == Some("title")
            && self
                .presentation
                .show_special_placeholders_on_title_slide()
                .map_err(|e| invalid_slide_mutation("add slide", e.to_string()))?
                == Some(false)
        {
            return Ok(Vec::new());
        }
        // A layout reached without a master relationship has no master flags
        // or placeholders to fall back on.
        let layout_part = &self.layouts[layout_index].part_name;
        let master =
            match related_internal_part(&self.package, layout_part, rel_types::SLIDE_MASTER)? {
                Some(master_part) => Some(
                    self.masters
                        .iter()
                        .find(|record| record.part_name == master_part)
                        .ok_or_else(|| {
                            footer_malformed(
                                &master_part,
                                "master is absent from the presentation inventory",
                            )
                        })?
                        .master
                        .as_ref()
                        .map_err(|error| footer_malformed(&master_part, error))?
                        .clone(),
                ),
                None => None,
            };
        let Some(header_footer) = layout.header_footer.clone().or_else(|| {
            master
                .as_ref()
                .and_then(|master| master.header_footer.clone())
        }) else {
            return Ok(Vec::new());
        };
        let mut field_ids = self.field_identity_inventory()?;
        let mut shapes = Vec::new();
        for kind in LATENT_KINDS {
            let enabled = match kind {
                PhType::DateTime => header_footer.date_time_enabled(),
                PhType::Footer => header_footer.footer_enabled(),
                _ => header_footer.slide_number_enabled(),
            };
            // As set_header_footer does, a layout without the placeholder
            // falls back on the master's.
            let template = match &master {
                Some(master) => latent_template(layout, master, &kind),
                None => find_latent(&layout.common_slide_data.shape_tree.children, &kind)
                    .map(|shape| (shape, false)),
            };
            if enabled && template.is_none() {
                return Err(invalid_slide_mutation(
                    "add slide",
                    "enabled footer policy has no qualified template",
                ));
            }
            let layout_count =
                footer_placeholder_count(&layout.common_slide_data.shape_tree.children, &kind);
            if enabled
                && (layout_count > 1
                    || (layout_count == 0
                        && master.as_ref().is_some_and(|master| {
                            footer_placeholder_count(
                                &master.common_slide_data.shape_tree.children,
                                &kind,
                            ) > 1
                        })))
            {
                return Err(invalid_slide_mutation(
                    "add slide",
                    "ambiguous enabled footer template",
                ));
            }
            if let Some((template, from_master)) = template
                && enabled
            {
                validate_footer_geometry(template, layout, master.as_ref())?;
                let source = if from_master {
                    &master
                        .as_ref()
                        .expect("master source")
                        .common_slide_data
                        .shape_tree
                        .children
                } else {
                    &layout.common_slide_data.shape_tree.children
                };
                let mut copy = copy_footer_template(source, &kind, ids, &mut field_ids)?
                    .ok_or_else(|| {
                        invalid_slide_mutation("add slide", "missing qualified footer template")
                    })?;
                let xml = match &copy {
                    ShapeTreeChild::Shape(shape) => shape.to_xml(),
                    ShapeTreeChild::GroupShape(group) => group.to_xml(),
                    _ => unreachable!(),
                }
                .map_err(|error| invalid_slide_mutation("add slide", error.to_string()))?;
                if !relationship_ids(&xml)
                    .map_err(|error| invalid_slide_mutation("add slide", error.to_string()))?
                    .is_empty()
                {
                    return Err(invalid_slide_mutation(
                        "add slide",
                        "relationship-bearing footer template requires owner remapping",
                    ));
                }
                if kind == PhType::SlideNumber {
                    let body = match &mut copy {
                        ShapeTreeChild::Shape(shape) => {
                            shape.text_body.get_or_insert_with(CT_TextBody::new)
                        }
                        ShapeTreeChild::GroupShape(group) => {
                            find_latent_mut(&mut group.children, &kind)
                                .expect("copied placeholder")
                                .text_body
                                .get_or_insert_with(CT_TextBody::new)
                        }
                        _ => unreachable!(),
                    };
                    fill_latent_body(body, &LatentContent::SlideNumber, &mut field_ids)?;
                }
                shapes.push(copy);
            }
        }
        Ok(shapes)
    }

    /// Returns the date, footer and slide number one slide shows through
    /// the placeholders it owns, or `None` for an unknown slide.
    pub fn slide_header_footer(&self, slide_index: usize) -> Option<HeaderFooter> {
        self.slides
            .get(slide_index)
            .map(|record| read_slide(&record.slide))
    }

    /// Appends a checked field to one current shape or table-cell paragraph.
    /// IDs are allocated against the complete retained presentation inventory.
    pub fn add_text_field(
        &mut self,
        slide_index: usize,
        child_path: &[usize],
        cell: Option<(usize, usize)>,
        paragraph_index: usize,
        field_type: &str,
        text: Option<&str>,
    ) -> Result<String> {
        self.require_slide_index(slide_index)?;
        let mut ids = self.field_identity_inventory()?;
        let mut field = text_field(field_type, text, &mut ids)?;
        let mut candidate = self.clone();
        let child = child_at_mut(
            &mut Arc::make_mut(&mut candidate.slides[slide_index])
                .slide
                .common_slide_data
                .shape_tree
                .children,
            child_path,
        )
        .ok_or_else(|| {
            invalid_slide_mutation("Paragraph.add_field", "text owner path is absent")
        })?;
        let body = match (child, cell) {
            (ShapeTreeChild::Shape(shape), None) => shape.text_body.as_mut(),
            (ShapeTreeChild::GraphicFrame(frame), Some((row, column))) => frame
                .graphic_data
                .table_mut()
                .and_then(|table| table.rows.get_mut(row))
                .and_then(|row| row.cells.get_mut(column))
                .and_then(|cell| cell.text_body.as_mut()),
            _ => None,
        }
        .ok_or_else(|| invalid_slide_mutation("Paragraph.add_field", "owner has no text body"))?;
        let paragraph = body.paragraph_mut(paragraph_index).ok_or_else(|| {
            invalid_slide_mutation("Paragraph.add_field", "paragraph index is absent")
        })?;
        if let Some(TextRun::Run(run)) = paragraph.runs.last() {
            field.run_properties = run.properties.clone();
        }
        let id = field.id.clone();
        push_field(paragraph, field);
        self.commit_candidate(candidate)?;
        Ok(id)
    }

    /// Sets one slide's date, footer and slide number.
    ///
    /// Missing placeholders are copied from the slide's layout, or from its
    /// master with an explicit position when the layout has none, and
    /// switched-off ones are removed. The masters and layouts are unchanged.
    /// A layout and master without the placeholder a setting needs is an
    /// error, and the slide is left as it was.
    pub fn set_slide_header_footer(
        &mut self,
        slide_index: usize,
        settings: &HeaderFooter,
    ) -> Result<bool> {
        validate_settings(settings)?;
        let slide_count = self.slides.len();
        let layout_index =
            self.slide_layout_index(slide_index)
                .ok_or(Error::UnknownSlideIndex {
                    index: slide_index,
                    slide_count,
                })?;
        let (_, master) = self.layout_master_part(layout_index)?;
        let layout = &self.layouts[layout_index].layout;
        let layout_name = layout.common_slide_data.name.clone().unwrap_or_default();
        let mut field_ids = self.field_identity_inventory()?;
        let mut staged = self.clone();
        let mut slide = staged.slides[slide_index].slide.clone();
        apply_to_slide(
            &mut slide,
            layout,
            &master,
            &layout_name,
            settings,
            &mut field_ids,
        )?;
        if slide == self.slides[slide_index].slide {
            return Ok(false);
        }
        Arc::make_mut(&mut staged.slides[slide_index]).slide = slide;
        self.commit_candidate(staged)?;
        Ok(true)
    }

    /// Sets the date, footer and slide number of every slide, as
    /// PowerPoint's Header and Footer dialog does with Apply to All.
    ///
    /// Each slide owns the placeholders it shows, copied from its layout
    /// when missing. With `hide_on_title`, slides on a title layout show
    /// none, as "Don't show on title slide" does. Every master and layout
    /// receives the matching `p:hf` flags and the footer and date text, so a
    /// slide added later gets the same placeholders. A layout and master
    /// without a placeholder a slide needs is an error, and nothing changes.
    pub fn set_header_footer(
        &mut self,
        settings: &HeaderFooter,
        hide_on_title: bool,
    ) -> Result<bool> {
        validate_settings(settings)?;
        let hidden = HeaderFooter::default();
        let title_layouts = self
            .layouts
            .iter()
            .map(|record| hide_on_title && record.layout.layout_type() == Some("title"))
            .collect::<Vec<_>>();
        let mut staged = self.clone();
        let mut field_ids = self.field_identity_inventory()?;
        let mut changed = false;
        let mut masters = Vec::<(String, CT_SlideMaster)>::new();
        for layout_index in 0..staged.layouts.len() {
            let (master_part, master) = staged.layout_master_part(layout_index)?;
            if !masters.iter().any(|(part, _)| part == &master_part) {
                masters.push((master_part, master));
            }
        }
        for slide_index in 0..staged.slides.len() {
            let layout_index = staged.slide_layout_index(slide_index).ok_or_else(|| {
                footer_malformed(
                    &staged.slides[slide_index].part_name,
                    "slide layout is not reached from a slide master",
                )
            })?;
            let (master_part, _) = staged.layout_master_part(layout_index)?;
            let master = &masters
                .iter()
                .find(|(part, _)| part == &master_part)
                .expect("every layout master was collected")
                .1;
            let layout = &staged.layouts[layout_index].layout;
            let layout_name = layout.common_slide_data.name.clone().unwrap_or_default();
            let effective = if title_layouts[layout_index] {
                &hidden
            } else {
                settings
            };
            let mut slide = staged.slides[slide_index].slide.clone();
            apply_to_slide(
                &mut slide,
                layout,
                master,
                &layout_name,
                effective,
                &mut field_ids,
            )?;
            // An unchanged slide keeps its shared record.
            if slide != staged.slides[slide_index].slide {
                changed = true;
                Arc::make_mut(&mut staged.slides[slide_index]).slide = slide;
            }
        }
        for (layout_index, record) in staged.layouts.iter_mut().enumerate() {
            let mut layout = record.layout.clone();
            let effective = if title_layouts[layout_index] {
                &hidden
            } else {
                settings
            };
            update_header_footer_flags(&mut layout.header_footer, effective);
            update_templates(
                &mut layout.common_slide_data.shape_tree.children,
                settings,
                &mut field_ids,
            )?;
            if layout == record.layout {
                continue;
            }
            changed = true;
            let xml = layout
                .to_xml()
                .map_err(|error| footer_malformed(&record.part_name, error))?;
            staged.package.set_part(&record.part_name, xml);
            Arc::make_mut(record).layout = layout;
        }
        staged
            .presentation
            .set_show_special_placeholders_on_title_slide(hide_on_title.then_some(false));
        for (master_part, mut master) in masters {
            let original = master.clone();
            update_header_footer_flags(&mut master.header_footer, settings);
            update_templates(
                &mut master.common_slide_data.shape_tree.children,
                settings,
                &mut field_ids,
            )?;
            if master == original {
                continue;
            }
            changed = true;
            let xml = master
                .to_xml()
                .map_err(|error| footer_malformed(&master_part, error))?;
            staged.package.set_part(&master_part, xml);
        }
        if !changed && staged.presentation == self.presentation {
            return Ok(false);
        }
        self.commit_candidate(staged)?;
        Ok(true)
    }
}

fn footer_placeholder_count(children: &[ShapeTreeChild], kind: &PhType) -> usize {
    children
        .iter()
        .map(|child| match child {
            ShapeTreeChild::Shape(shape) => usize::from(is_kind(shape, kind)),
            ShapeTreeChild::GroupShape(group) => footer_placeholder_count(&group.children, kind),
            _ => 0,
        })
        .sum()
}

impl Presentation {
    /// Scans current qualified field identities including retained owner parts.
    pub fn field_identity_inventory(&self) -> Result<HashSet<String>> {
        let package = self.staged_package(true)?;
        // A part still holding the bytes the last opening read keeps the
        // identities found then, so only the parts an edit changed are
        // scanned again.
        let inventory = field_identities(&package, self.parsed.as_deref());
        // Debug builds, which run the binding tests, scan every part as
        // well and assert the same identities or the same error.
        #[cfg(debug_assertions)]
        if self.parsed.is_some() {
            let scanned = field_identities(&package, None);
            match (&inventory, &scanned) {
                (Ok(kept), Ok(scanned)) => {
                    assert_eq!(kept, scanned, "kept field identities differ")
                }
                (Err(kept), Err(scanned)) => assert_eq!(kept.to_string(), scanned.to_string()),
                (kept, scanned) => {
                    panic!("kept field identities give {kept:?}, a scan {scanned:?}")
                }
            }
        }
        inventory
    }
}

/// The text field identities of every XML part of `package`, taking those
/// `parsed` kept for a part that holds the bytes read.
fn field_identities(package: &OpcPackage, parsed: Option<&Parsed>) -> Result<HashSet<String>> {
    let mut ids = HashSet::new();
    for (name, bytes) in &package.parts {
        if !name.ends_with(".xml") {
            continue;
        }
        match parsed.and_then(|parsed| parsed.field_ids(package, name)) {
            Some(found) => ids.extend(found.iter().cloned()),
            None => ids.extend(part_field_identities(name, bytes)?),
        }
    }
    Ok(ids)
}

/// The identities of the DrawingML text fields of one XML part, uppercase.
pub(super) fn part_field_identities(name: &str, bytes: &[u8]) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut reader = quick_xml::Reader::from_reader(bytes);
    // Keep the legacy vendor declaration rules and raw URI matching, while
    // tracking logical depth independently of its u16 namespace level.
    let mut declarations = quick_xml::name::NamespaceResolver::default();
    let mut bindings: HashMap<Vec<u8>, bool> = HashMap::new();
    let mut undo: Vec<(Vec<u8>, Option<bool>)> = Vec::new();
    let mut frames = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| footer_malformed(name, error))?;
        let closes = matches!(event, XmlEvent::Empty(_) | XmlEvent::End(_));
        match event {
            XmlEvent::Start(start) | XmlEvent::Empty(start) => {
                declarations
                    .push(&start)
                    .map_err(|error| footer_malformed(name, error))?;
                frames.push(undo.len());
                for attribute in start.attributes().with_checks(false) {
                    let Ok(attribute) = attribute else { break };
                    let Some(prefix) = attribute.key.as_namespace_binding() else {
                        continue;
                    };
                    let prefix = match prefix {
                        quick_xml::name::PrefixDeclaration::Default => Vec::new(),
                        quick_xml::name::PrefixDeclaration::Named(prefix) => prefix.to_vec(),
                    };
                    let previous = bindings
                        .insert(prefix.clone(), attribute.value.as_ref() == A_NS.as_bytes());
                    undo.push((prefix, previous));
                }
                declarations.pop();
                let drawing_namespace = match start.name().prefix() {
                    Some(prefix) if prefix.as_ref().is_empty() => false,
                    Some(prefix) => bindings.get(prefix.as_ref()) == Some(&true),
                    None => bindings.get(&b""[..]) == Some(&true),
                };
                if drawing_namespace && xml_local_name(start.name().as_ref()) == b"fld" {
                    for attr in start.attributes() {
                        let attr = attr.map_err(|error| footer_malformed(name, error))?;
                        if attr.key.as_ref() == b"id" {
                            ids.push(
                                attr.decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| footer_malformed(name, error))?
                                .to_uppercase(),
                            );
                        }
                    }
                }
            }
            XmlEvent::Eof => break,
            _ => {}
        }
        if closes && let Some(mark) = frames.pop() {
            while undo.len() > mark {
                let (prefix, previous) = undo.pop().expect("namespace scope undo entry");
                if let Some(previous) = previous {
                    bindings.insert(prefix, previous);
                } else {
                    bindings.remove(&prefix);
                }
            }
        }
        buffer.clear();
    }
    Ok(ids)
}

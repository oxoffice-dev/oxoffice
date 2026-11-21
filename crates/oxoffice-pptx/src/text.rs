//! Text frames, paragraphs and runs: reading and editing them, collecting slide text, and replacing placeholder text.

use super::*;

/// A refused pair in an ordered transactional replacement batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementCountMismatch {
    pub index: usize,
    pub placeholder: String,
    pub expected: usize,
    pub found: usize,
}

impl Presentation {
    /// Replaces literal text across regular DrawingML runs in every editable shape.
    ///
    /// Replacement text retains the formatting and opaque run content of the
    /// first matched run. Unmatched prefixes and suffixes stay in their
    /// original runs. Groups and table cells are traversed recursively, while
    /// preserved alternate-content branches remain untouched.
    pub fn replace_text(&mut self, placeholder: &str, value: &str) -> usize {
        if placeholder.is_empty() {
            return 0;
        }
        // Stored as the run text setters store it, so it can be saved.
        let value = &oxoffice_drawing::text::escape_invalid_xml_characters(value);
        // A slide without a match keeps its record, so a shared one is not
        // copied and callers can tell the changed slides by pointer identity.
        self.slides
            .iter_mut()
            .filter(|record| slide_record_has_text(record, placeholder))
            .map(|record| {
                replace_text_in_slide_record(Arc::make_mut(record), placeholder, value, true)
            })
            .sum()
    }

    /// Replaces literal slide and speaker-note text on a staged candidate.
    ///
    /// The candidate is serialized before it replaces the live presentation,
    /// so a malformed retained subtree cannot publish a partial mutation.
    pub fn try_replace_text(&mut self, placeholder: &str, value: &str) -> Result<usize> {
        require_replacement_placeholder(placeholder)?;
        let mut candidate = self.clone();
        let count = candidate.replace_text(placeholder, value);
        candidate.staged_package(false)?;
        *self = candidate;
        Ok(count)
    }

    /// Applies ordered literal pairs on one candidate and publishes once.
    ///
    /// Counts refer to each pair's current candidate, so pairs may cascade.
    /// A mismatch or serialization failure leaves the original unchanged.
    pub fn try_replace_all_expected(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
    ) -> Result<std::result::Result<Vec<usize>, ReplacementCountMismatch>> {
        for (placeholder, _, _) in pairs {
            require_replacement_placeholder(placeholder)?;
        }
        if pairs.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let mut candidate = self.clone();
        let mut counts = Vec::with_capacity(pairs.len());
        for (index, &(placeholder, value, expected)) in pairs.iter().enumerate() {
            let found = candidate.replace_text(placeholder, value);
            if let Some(expected) = expected.filter(|expected| *expected != found) {
                return Ok(Err(ReplacementCountMismatch {
                    index,
                    placeholder: placeholder.to_owned(),
                    expected,
                    found,
                }));
            }
            counts.push(found);
        }
        candidate.staged_package(false)?;
        *self = candidate;
        Ok(Ok(counts))
    }

    /// Replaces literal text on one slide, and in its speaker notes when
    /// `notes` is true, and returns the count.
    ///
    /// Matching, formatting and traversal are those of
    /// [`Presentation::try_replace_text`], restricted to that slide. Other
    /// slides and their notes are never touched. The replacement runs on a
    /// copy of the slide and its notes, which must serialize before the copy
    /// replaces them. With `expect`, a different count also leaves the slide
    /// unchanged and is returned for the caller to compare.
    pub fn try_replace_slide_text(
        &mut self,
        slide_index: usize,
        placeholder: &str,
        value: &str,
        notes: bool,
        expect: Option<usize>,
    ) -> Result<usize> {
        self.require_slide_index(slide_index)?;
        require_replacement_placeholder(placeholder)?;
        let mut candidate = SlideRecord::clone(&self.slides[slide_index]);
        let value = &oxoffice_drawing::text::escape_invalid_xml_characters(value);
        let count = replace_text_in_slide_record(&mut candidate, placeholder, value, notes);
        if expect.is_some_and(|expected| expected != count) {
            return Ok(count);
        }
        candidate
            .slide
            .to_xml()
            .map_err(|error| Error::MalformedPart {
                part_name: candidate.part_name.clone(),
                message: error.to_string(),
            })?;
        if let Some(record) = candidate.notes.as_ref().filter(|_| notes) {
            record
                .notes
                .to_xml()
                .map_err(|error| Error::MalformedPart {
                    part_name: record.part_name.clone(),
                    message: error.to_string(),
                })?;
        }
        self.slides[slide_index] = Arc::new(candidate);
        Ok(count)
    }
}

/// A borrowed DrawingML text paragraph.
#[derive(Clone, Copy)]
pub struct TextParagraphRef<'a> {
    paragraph: &'a CT_TextParagraph,
}

/// A borrowed regular DrawingML text run.
#[derive(Clone, Copy)]
pub struct TextRunRef<'a> {
    run: &'a CT_RegularTextRun,
}

impl<'a> TextFrame<'a> {
    /// Returns plain text in paragraph and ordered text-choice order.
    pub fn text(&self) -> String {
        self.body.plain_text()
    }

    /// Replaces the frame content with one paragraph per line, as
    /// [`CT_TextBody::set_text`] describes.
    pub fn set_text(&mut self, text: &str) {
        self.body.set_text(text);
    }

    /// Replaces literal text in this text body only and returns the count.
    ///
    /// Matching and formatting are those of
    /// [`Presentation::try_replace_text`]. An empty placeholder is rejected
    /// before anything changes. With `expect`, the replacement runs on a copy
    /// of the body, and a different count leaves the body unchanged and is
    /// returned for the caller to compare. Nothing is serialized, because
    /// replacing run text cannot make a body fail to serialize.
    pub fn try_replace_text(
        &mut self,
        placeholder: &str,
        value: &str,
        expect: Option<usize>,
    ) -> Result<usize> {
        require_replacement_placeholder(placeholder)?;
        let value = &oxoffice_drawing::text::escape_invalid_xml_characters(value);
        let Some(expected) = expect else {
            return Ok(replace_text_in_body(self.body, placeholder, value));
        };
        let mut candidate = self.body.clone();
        let count = replace_text_in_body(&mut candidate, placeholder, value);
        if count == expected {
            *self.body = candidate;
        }
        Ok(count)
    }

    /// Returns the number of paragraphs in the text body.
    pub fn paragraph_count(&self) -> usize {
        self.body.paragraph_count()
    }

    /// Returns one paragraph without mutating the text body.
    pub fn paragraph(&self, index: usize) -> Option<TextParagraphRef<'_>> {
        self.body
            .paragraphs()
            .get(index)
            .map(|paragraph| TextParagraphRef { paragraph })
    }

    /// Returns one paragraph for in-place mutation.
    pub fn paragraph_mut(&mut self, index: usize) -> Option<TextParagraphMut<'_>> {
        self.body
            .paragraph_mut(index)
            .map(|paragraph| TextParagraphMut { paragraph })
    }

    /// Consumes this handle and returns one paragraph for mutation.
    pub fn into_paragraph_mut(self, index: usize) -> Option<TextParagraphMut<'a>> {
        self.body
            .paragraph_mut(index)
            .map(|paragraph| TextParagraphMut { paragraph })
    }

    /// Appends one empty paragraph.
    pub fn add_paragraph(&mut self) -> TextParagraphMut<'_> {
        TextParagraphMut {
            paragraph: self.body.add_paragraph(),
        }
    }

    /// Replaces all four optional text insets.
    ///
    /// An inset outside the 32-bit `ST_Coordinate32` range is rejected before
    /// any inset changes.
    pub fn set_insets(
        &mut self,
        left: Option<Emu>,
        right: Option<Emu>,
        top: Option<Emu>,
        bottom: Option<Emu>,
    ) -> Result<()> {
        let coordinate = |value: Option<Emu>| {
            value
                .map(|value| {
                    i32::try_from(value.0)
                        .map(Coordinate32Value::Emu)
                        .map_err(|_| Error::InvalidShapeMutation {
                            operation: "set text insets",
                            message: format!("{} EMU is outside the coordinate range", value.0),
                        })
                })
                .transpose()
        };
        let (left, right, top, bottom) = (
            coordinate(left)?,
            coordinate(right)?,
            coordinate(top)?,
            coordinate(bottom)?,
        );
        let properties = &mut self.body.body_properties;
        properties.left_inset = left;
        properties.right_inset = right;
        properties.top_inset = top;
        properties.bottom_inset = bottom;
        Ok(())
    }

    /// Sets or clears the direct vertical anchor.
    pub fn set_vertical_anchor(&mut self, anchor: Option<TextAnchor>) {
        self.body.body_properties.anchor = anchor;
    }

    /// Sets or clears the direct wrap choice, `true` wrapping at the frame width.
    pub fn set_word_wrap(&mut self, wrap: Option<bool>) {
        self.body.body_properties.wrap = wrap.map(|wrap| {
            if wrap {
                TextWrap::Square
            } else {
                TextWrap::None
            }
        });
    }

    /// Sets or clears the direct autofit choice.
    ///
    /// Choosing normal autofit again keeps a stored font scale and line
    /// spacing reduction.
    pub fn set_autofit_mode(&mut self, mode: Option<AutofitMode>) {
        let properties = &mut self.body.body_properties;
        properties.autofit = match mode {
            None => None,
            Some(AutofitMode::None) => Some(TextAutofit::NoAutofit),
            Some(AutofitMode::Shape) => Some(TextAutofit::ShapeAutofit),
            Some(AutofitMode::Normal) => match properties.autofit.take() {
                Some(TextAutofit::Normal(normal)) => Some(TextAutofit::Normal(normal)),
                _ => Some(TextAutofit::Normal(NormalAutofit::default())),
            },
        };
    }
}

impl<'a> TextFrameRef<'a> {
    /// Returns plain text in paragraph and ordered text-choice order.
    pub fn text(&self) -> String {
        self.body.plain_text()
    }

    /// Returns the number of paragraphs in the text body.
    pub fn paragraph_count(&self) -> usize {
        self.body.paragraph_count()
    }

    /// Returns one paragraph by zero-based index.
    pub fn paragraph(&self, index: usize) -> Option<TextParagraphRef<'a>> {
        self.body
            .paragraphs()
            .get(index)
            .map(|paragraph| TextParagraphRef { paragraph })
    }

    /// Returns the direct autofit choice on the text body.
    pub fn autofit_mode(&self) -> Option<AutofitMode> {
        self.body
            .body_properties
            .autofit
            .as_ref()
            .map(|autofit| match autofit {
                TextAutofit::NoAutofit => AutofitMode::None,
                TextAutofit::Normal(_) => AutofitMode::Normal,
                TextAutofit::ShapeAutofit => AutofitMode::Shape,
            })
    }

    /// Returns the optional left, right, top, and bottom text insets in EMU.
    ///
    /// A universal measure such as `0.1in` is converted to the nearest EMU.
    pub fn insets(&self) -> (Option<Emu>, Option<Emu>, Option<Emu>, Option<Emu>) {
        let properties = &self.body.body_properties;
        (
            properties.left_inset.as_ref().and_then(coordinate_emu),
            properties.right_inset.as_ref().and_then(coordinate_emu),
            properties.top_inset.as_ref().and_then(coordinate_emu),
            properties.bottom_inset.as_ref().and_then(coordinate_emu),
        )
    }

    /// Returns the direct vertical anchor.
    pub fn vertical_anchor(&self) -> Option<TextAnchor> {
        self.body.body_properties.anchor
    }

    /// Returns the direct wrap choice, `true` when lines wrap at the frame width.
    pub fn word_wrap(&self) -> Option<bool> {
        self.body
            .body_properties
            .wrap
            .map(|wrap| wrap == TextWrap::Square)
    }
}

fn coordinate_emu(value: &Coordinate32Value) -> Option<Emu> {
    match value {
        Coordinate32Value::Emu(value) => Some(Emu(i64::from(*value))),
        Coordinate32Value::UniversalMeasure(value) => {
            let (number, emu_per_unit) = [
                ("mm", 36_000.0),
                ("cm", 360_000.0),
                ("in", 914_400.0),
                ("pt", 12_700.0),
                ("pc", 152_400.0),
                ("pi", 152_400.0),
            ]
            .into_iter()
            .find_map(|(unit, emu_per_unit)| {
                value
                    .strip_suffix(unit)
                    .map(|number| (number, emu_per_unit))
            })?;
            let number = number.parse::<f64>().ok()?;
            Some(Emu((number * emu_per_unit).round() as i64))
        }
    }
}

impl<'a> TextParagraphRef<'a> {
    /// Returns visible text in ordered run, break, and field order.
    pub fn text(&self) -> String {
        self.paragraph
            .runs
            .iter()
            .map(|run| match run {
                TextRun::Run(run) => run.text.value.as_str(),
                TextRun::Break(_) => "\u{000b}",
                TextRun::Field(field) => field.text.as_ref().map_or("", |text| text.value.as_str()),
            })
            .collect()
    }

    /// Returns the direct paragraph level, defaulting to zero.
    pub fn level(&self) -> u8 {
        self.paragraph
            .properties
            .as_ref()
            .and_then(|properties| properties.level)
            .unwrap_or(0)
    }

    /// Returns direct default-run character properties when present.
    pub fn default_run_properties(&self) -> Option<&'a CT_TextCharacterProperties> {
        self.paragraph
            .properties
            .as_ref()
            .and_then(|properties| properties.default_run_properties.as_ref())
    }

    /// Returns direct paragraph properties when present.
    pub fn properties(&self) -> Option<&'a CT_TextParagraphProperties> {
        self.paragraph.properties.as_ref()
    }

    /// Returns the number of regular runs, excluding breaks and fields.
    pub fn run_count(&self) -> usize {
        self.paragraph
            .runs
            .iter()
            .filter(|run| matches!(run, TextRun::Run(_)))
            .count()
    }

    /// Returns one regular run by zero-based regular-run index.
    pub fn run(&self, index: usize) -> Option<TextRunRef<'a>> {
        self.paragraph
            .runs
            .iter()
            .filter_map(|run| match run {
                TextRun::Run(run) => Some(TextRunRef { run }),
                TextRun::Break(_) | TextRun::Field(_) => None,
            })
            .nth(index)
    }
}

impl TextRunRef<'_> {
    /// Returns the run text.
    pub fn text(&self) -> &str {
        &self.run.text.value
    }

    /// Returns direct character properties when present.
    pub fn properties(&self) -> Option<&CT_TextCharacterProperties> {
        self.run.properties.as_ref()
    }

    /// Returns the direct Latin typeface on this run.
    pub fn font_name(&self) -> Option<&str> {
        self.properties()?
            .latin
            .as_ref()
            .map(|font| font.typeface.as_str())
    }

    /// Returns the direct run size in hundredths of a point.
    pub fn font_size(&self) -> Option<i32> {
        self.properties()?.font_size
    }

    /// Returns the direct sRGB run colour as `RRGGBB`.
    pub fn font_color(&self) -> Option<String> {
        let Fill::Solid(fill) = self.properties()?.fill.as_ref()? else {
            return None;
        };
        let ColorChoice::Srgb { value, .. } = fill.color.as_ref()? else {
            return None;
        };
        Some(value.to_string())
    }
}

/// A mutably borrowed DrawingML text paragraph.
pub struct TextParagraphMut<'a> {
    paragraph: &'a mut CT_TextParagraph,
}

impl TextParagraphMut<'_> {
    /// Replaces fields, breaks, and runs with one regular run, split by an
    /// `a:br` at each line feed, CRLF pair or vertical tab, as python-pptx
    /// assigns paragraph text.
    pub fn set_text(&mut self, text: &str) {
        self.paragraph
            .set_text(&text.replace("\r\n", "\u{b}").replace('\n', "\u{b}"));
    }

    /// Appends a regular run after the existing ordered text choices.
    pub fn add_run(&mut self, text: &str) -> TextRunMut<'_> {
        TextRunMut {
            run: self.paragraph.add_run(text),
        }
    }

    /// Returns one regular run for in-place mutation.
    pub fn run_mut(&mut self, index: usize) -> Option<TextRunMut<'_>> {
        self.paragraph
            .runs
            .iter_mut()
            .filter_map(|run| match run {
                TextRun::Run(run) => Some(TextRunMut { run }),
                TextRun::Break(_) | TextRun::Field(_) => None,
            })
            .nth(index)
    }

    /// Sets the direct paragraph level.
    pub fn set_level(&mut self, level: u8) -> bool {
        if level > 8 {
            return false;
        }
        self.paragraph.properties_mut().level = Some(level);
        true
    }

    /// Returns mutable direct default-run character properties.
    pub fn default_run_properties_mut(&mut self) -> &mut CT_TextCharacterProperties {
        self.paragraph
            .properties_mut()
            .default_run_properties
            .get_or_insert_with(CT_TextCharacterProperties::default)
    }

    /// Returns direct paragraph properties when present.
    pub fn properties(&self) -> Option<&CT_TextParagraphProperties> {
        self.paragraph.properties.as_ref()
    }

    /// Replaces the paragraph's direct typed properties.
    pub fn set_properties(&mut self, properties: CT_TextParagraphProperties) {
        *self.paragraph.properties_mut() = properties;
    }

    /// Sets or clears the direct paragraph bullet.
    ///
    /// A preserved picture or follow-text bullet part that the new value
    /// replaces is removed, so the paragraph keeps one bullet choice.
    pub fn set_bullet(&mut self, bullet: Option<TextBullet>) {
        if let Some(properties) = self.paragraph.properties.as_mut() {
            properties.set_bullet(bullet);
        } else if bullet.is_some() {
            self.paragraph.properties_mut().set_bullet(bullet);
        }
    }
}

/// A mutably borrowed regular DrawingML text run.
pub struct TextRunMut<'a> {
    run: &'a mut CT_RegularTextRun,
}

impl TextRunMut<'_> {
    /// Returns direct character properties when present.
    pub fn properties(&self) -> Option<&CT_TextCharacterProperties> {
        self.run.properties.as_ref()
    }

    /// Replaces text while retaining the run's typed and unmodelled state.
    pub fn set_text(&mut self, text: &str) {
        self.run.set_text(text);
    }

    /// Replaces the run's direct typed character properties.
    pub fn set_properties(&mut self, properties: CT_TextCharacterProperties) {
        self.run.properties = Some(properties);
    }

    /// Sets or clears the direct Latin typeface.
    pub fn set_font(&mut self, font: Option<TextFont>) {
        if let Some(properties) = self.run.properties.as_mut() {
            properties.latin = font;
        } else if font.is_some() {
            let mut properties = CT_TextCharacterProperties::default();
            properties.latin = font;
            self.run.properties = Some(properties);
        }
    }
}

pub(super) fn collect_text(children: &[ShapeTreeChild], output: &mut Vec<String>) {
    for child in children {
        let shape = shape_ref(child);
        if let Some(text) = shape.text()
            && !text.is_empty()
        {
            output.push(text);
        }
        collect_text(shape.child_slice(), output);
    }
}

fn require_replacement_placeholder(placeholder: &str) -> Result<()> {
    if placeholder.is_empty() {
        return Err(Error::InvalidSlideMutation {
            operation: "replace text",
            message: "placeholder must not be empty".to_owned(),
        });
    }
    Ok(())
}

fn replace_text_in_slide_record(
    record: &mut SlideRecord,
    placeholder: &str,
    value: &str,
    notes: bool,
) -> usize {
    let slide_count = replace_text_in_children(
        &mut record.slide.common_slide_data.shape_tree.children,
        placeholder,
        value,
    );
    let notes_count = record.notes.as_mut().filter(|_| notes).map_or(0, |notes| {
        replace_text_in_children(
            &mut notes.notes.common_slide_data.shape_tree.children,
            placeholder,
            value,
        )
    });
    slide_count + notes_count
}

/// Whether `replace_text_in_slide_record` would replace anything in a slide
/// or its notes: the same run segments, read without copying the record.
fn slide_record_has_text(record: &SlideRecord, placeholder: &str) -> bool {
    fn in_body(body: &CT_TextBody, placeholder: &str) -> bool {
        body.paragraphs().iter().any(|paragraph| {
            paragraph
                .runs
                .split(|run| !matches!(run, TextRun::Run(_)))
                .any(|segment| {
                    let text = segment
                        .iter()
                        .filter_map(|run| match run {
                            TextRun::Run(run) => Some(run.text.value.as_str()),
                            _ => None,
                        })
                        .collect::<String>();
                    text.contains(placeholder)
                })
        })
    }
    fn in_children(children: &[ShapeTreeChild], placeholder: &str) -> bool {
        children.iter().any(|child| match child {
            ShapeTreeChild::Shape(shape) => shape
                .text_body
                .as_ref()
                .is_some_and(|body| in_body(body, placeholder)),
            ShapeTreeChild::GraphicFrame(frame) => match frame.graphic_data.payload() {
                GraphicDataPayload::Table(table) => table
                    .rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .filter_map(|cell| cell.text_body.as_ref())
                    .any(|body| in_body(body, placeholder)),
                _ => false,
            },
            ShapeTreeChild::GroupShape(group) => in_children(&group.children, placeholder),
            ShapeTreeChild::Picture(_)
            | ShapeTreeChild::Connector(_)
            | ShapeTreeChild::AlternateContent(_) => false,
        })
    }
    !placeholder.is_empty()
        && (in_children(
            &record.slide.common_slide_data.shape_tree.children,
            placeholder,
        ) || record.notes.as_ref().is_some_and(|notes| {
            in_children(
                &notes.notes.common_slide_data.shape_tree.children,
                placeholder,
            )
        }))
}

fn replace_text_in_children(
    children: &mut [ShapeTreeChild],
    placeholder: &str,
    value: &str,
) -> usize {
    let mut count = 0;
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape) => {
                if let Some(body) = &mut shape.text_body {
                    count += replace_text_in_body(body, placeholder, value);
                }
            }
            ShapeTreeChild::GraphicFrame(frame) => {
                if let Some(table) = frame.graphic_data.table_mut() {
                    for row in &mut table.rows {
                        for cell in &mut row.cells {
                            if let Some(body) = &mut cell.text_body {
                                count += replace_text_in_body(body, placeholder, value);
                            }
                        }
                    }
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                count += replace_text_in_children(&mut group.children, placeholder, value);
            }
            ShapeTreeChild::Picture(_)
            | ShapeTreeChild::Connector(_)
            | ShapeTreeChild::AlternateContent(_) => {}
        }
    }
    count
}

fn replace_text_in_body(body: &mut CT_TextBody, placeholder: &str, value: &str) -> usize {
    let mut count = 0;
    for index in 0..body.paragraph_count() {
        if let Some(paragraph) = body.paragraph_mut(index) {
            count += replace_text_in_paragraph(paragraph, placeholder, value);
        }
    }
    count
}

fn replace_text_in_paragraph(
    paragraph: &mut CT_TextParagraph,
    placeholder: &str,
    value: &str,
) -> usize {
    let mut count = 0;
    let mut segment_start = 0;
    while segment_start < paragraph.runs.len() {
        while segment_start < paragraph.runs.len()
            && !matches!(paragraph.runs[segment_start], TextRun::Run(_))
        {
            segment_start += 1;
        }
        let mut segment_end = segment_start;
        while segment_end < paragraph.runs.len()
            && matches!(paragraph.runs[segment_end], TextRun::Run(_))
        {
            segment_end += 1;
        }
        count += replace_text_in_run_segment(
            &mut paragraph.runs[segment_start..segment_end],
            placeholder,
            value,
        );
        segment_start = segment_end.saturating_add(1);
    }
    count
}

fn replace_text_in_run_segment(runs: &mut [TextRun], placeholder: &str, value: &str) -> usize {
    if placeholder.is_empty() {
        return 0;
    }
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(runs.len());
    for run in runs.iter() {
        let TextRun::Run(run) = run else {
            unreachable!("run segments contain only regular runs")
        };
        let start = text.len();
        text.push_str(&run.text.value);
        ranges.push((start, text.len()));
    }
    let matches = text
        .match_indices(placeholder)
        .map(|(start, _)| (start, start + placeholder.len()))
        .collect::<Vec<_>>();

    for &(match_start, match_end) in matches.iter().rev() {
        let start_run = ranges
            .iter()
            .position(|&(_, end)| match_start < end)
            .expect("non-empty match has a starting run");
        let end_run = ranges
            .iter()
            .position(|&(start, end)| match_end > start && match_end <= end)
            .expect("non-empty match has an ending run");
        let start_offset = match_start - ranges[start_run].0;
        let end_offset = match_end - ranges[end_run].0;

        if start_run == end_run {
            let TextRun::Run(run) = &mut runs[start_run] else {
                unreachable!("run segments contain only regular runs")
            };
            run.text
                .value
                .replace_range(start_offset..end_offset, value);
            continue;
        }

        let TextRun::Run(first) = &mut runs[start_run] else {
            unreachable!("run segments contain only regular runs")
        };
        let first_len = first.text.value.len();
        first
            .text
            .value
            .replace_range(start_offset..first_len, value);
        for run in &mut runs[start_run + 1..end_run] {
            let TextRun::Run(run) = run else {
                unreachable!("run segments contain only regular runs")
            };
            run.set_text("");
        }
        let TextRun::Run(last) = &mut runs[end_run] else {
            unreachable!("run segments contain only regular runs")
        };
        last.text.value.replace_range(..end_offset, "");
    }
    matches.len()
}

//! Section handles: body content and section references, section properties and their checks.

use super::*;

/// One body child exposed through the compatibility reader facade.
#[non_exhaustive]
pub enum BodyContentRef<'a> {
    /// A body paragraph.
    Paragraph(ParagraphRef<'a>),
    /// A body table.
    Table(TableRef<'a>),
    /// Content that the compatibility facade does not model.
    UnsupportedXml(UnsupportedXmlRef<'a>),
}

impl<'a> SectionRef<'a> {
    /// Return this section's zero-based document-order position.
    pub fn ordinal(&self) -> usize {
        self.ordinal
    }

    /// Return whether this section owns the schema-final body properties.
    pub fn is_final(&self) -> bool {
        self.is_final
    }

    /// Return the explicitly configured page orientation.
    pub fn orientation(&self) -> Option<ST_PageOrientation> {
        self.inner.orientation
    }

    /// Return the explicitly configured page width and height.
    pub fn page_size(&self) -> Option<(Length, Length)> {
        Some((
            Length::twips(self.inner.page_width?.0),
            Length::twips(self.inner.page_height?.0),
        ))
    }

    /// Return the explicitly configured top, right, bottom, and left margins.
    pub fn margins(&self) -> Option<(Length, Length, Length, Length)> {
        Some((
            Length::twips(self.inner.margin_top?.0),
            Length::twips(self.inner.margin_right?.0),
            Length::twips(self.inner.margin_bottom?.0),
            Length::twips(self.inner.margin_left?.0),
        ))
    }

    /// Return the explicitly configured gutter.
    pub fn gutter(&self) -> Option<Length> {
        self.inner.gutter.map(|value| Length::twips(value.0))
    }

    /// Return the equal-width column count and spacing when modeled by M23.
    pub fn columns(&self) -> Option<(u32, Length)> {
        let columns = self.inner.columns.as_ref()?;
        if columns.equal_width == Some(false) || !columns.columns.is_empty() {
            return None;
        }
        Some((
            columns.num.unwrap_or(1),
            Length::twips(columns.space.map_or(720, |value| value.0)),
        ))
    }

    /// Return the explicit column tracks, as `(width, trailing space)` pairs.
    ///
    /// `None` when the section has no columns or leaves them equal width, which
    /// is what `columns` already reports.
    pub fn column_widths(&self) -> Option<Vec<(Length, Length)>> {
        let columns = self.inner.columns.as_ref()?;
        if columns.columns.is_empty() {
            return None;
        }
        Some(
            columns
                .columns
                .iter()
                .map(|column| {
                    (
                        Length::twips(column.width.map_or(0, |value| value.0)),
                        Length::twips(column.space.map_or(0, |value| value.0)),
                    )
                })
                .collect(),
        )
    }

    /// Return whether a rule is drawn between this section's columns.
    pub fn column_separator(&self) -> Option<bool> {
        self.inner.columns.as_ref()?.sep
    }

    /// Borrow this section's footnote configuration.
    pub fn footnote_properties(&self) -> Option<&'a CT_NoteProperties> {
        self.inner.footnote_pr.as_deref()
    }

    /// Borrow this section's endnote configuration.
    pub fn endnote_properties(&self) -> Option<&'a CT_NoteProperties> {
        self.inner.endnote_pr.as_deref()
    }

    /// Return the printer trays feeding the first page and every later page.
    pub fn paper_source(&self) -> Option<(Option<u32>, Option<u32>)> {
        let source = self.inner.paper_source.as_deref()?;
        Some((source.first, source.other))
    }

    /// Borrow this section's page border frame.
    pub fn page_borders(&self) -> Option<&'a CT_PageBorders> {
        self.inner.page_borders.as_deref()
    }

    /// Borrow this section's margin line numbering.
    pub fn line_numbers(&self) -> Option<&'a CT_LineNumber> {
        self.inner.line_numbers.as_deref()
    }

    /// Return the vertical alignment of the body band within the page.
    pub fn vertical_alignment(&self) -> Option<ST_VerticalJc> {
        self.inner.vertical_alignment
    }

    /// Return this section's `w:textDirection`, exactly as it was authored.
    pub fn text_direction(&self) -> Option<&'a str> {
        self.inner.text_direction.as_deref()
    }

    /// Return this section's `w:docGrid` character grid.
    pub fn doc_grid(&self) -> Option<&'a CT_DocGrid> {
        self.inner.doc_grid.as_deref()
    }

    /// Return the explicitly configured page-number restart.
    pub fn page_number_start(&self) -> Option<u32> {
        self.inner.page_number.as_ref()?.start
    }

    /// Return the explicitly configured header and footer distances.
    pub fn header_footer_distance(&self) -> Option<(Length, Length)> {
        Some((
            Length::twips(self.inner.header_distance?.0),
            Length::twips(self.inner.footer_distance?.0),
        ))
    }

    /// Return the explicitly configured title-page state.
    pub fn different_first_page(&self) -> Option<bool> {
        self.inner.title_pg
    }

    /// Return the explicitly configured section break type.
    pub fn break_type(&self) -> Option<ST_SectionType> {
        self.inner.section_type
    }

    /// Borrow the complete section properties.
    pub fn properties(&self) -> &'a CT_SectPr {
        self.inner
    }
}

impl Section<'_> {
    /// Return this section's zero-based document-order position.
    pub fn ordinal(&self) -> usize {
        self.ordinal
    }

    /// Return whether this section owns the schema-final body properties.
    pub fn is_final(&self) -> bool {
        self.is_final
    }

    /// Return the explicitly configured page orientation.
    pub fn orientation(&self) -> Option<ST_PageOrientation> {
        self.inner.orientation
    }

    /// Return the explicitly configured page width and height.
    pub fn page_size(&self) -> Option<(Length, Length)> {
        SectionRef {
            ordinal: self.ordinal,
            is_final: self.is_final,
            inner: self.inner,
        }
        .page_size()
    }

    /// Set this section's positive page dimensions.
    pub fn set_page_size(&mut self, width: Length, height: Length) -> Result<()> {
        let width = checked_positive_section_twips(width, "page width")?;
        let height = checked_positive_section_twips(height, "page height")?;
        self.inner.page_width = Some(width);
        self.inner.page_height = Some(height);
        Ok(())
    }

    /// Set this section's orientation and normalize its page dimensions.
    pub fn set_orientation(&mut self, orientation: ST_PageOrientation) {
        self.inner.orientation = Some(orientation);
        if let (Some(width), Some(height)) = (self.inner.page_width, self.inner.page_height) {
            let dimensions_need_swap = match orientation {
                ST_PageOrientation::Landscape => width.0 < height.0,
                ST_PageOrientation::Portrait => width.0 > height.0,
            };
            if dimensions_need_swap {
                self.inner.page_width = Some(height);
                self.inner.page_height = Some(width);
            }
        }
    }

    /// Return the explicitly configured top, right, bottom, and left margins.
    pub fn margins(&self) -> Option<(Length, Length, Length, Length)> {
        SectionRef {
            ordinal: self.ordinal,
            is_final: self.is_final,
            inner: self.inner,
        }
        .margins()
    }

    /// Set this section's top, right, bottom, and left margins.
    ///
    /// Any other `w:pgMar` value the section lacks takes its layout default, so
    /// the written element carries every attribute `CT_PageMar` requires.
    pub fn set_margins(
        &mut self,
        top: Length,
        right: Length,
        bottom: Length,
        left: Length,
    ) -> Result<()> {
        let top = checked_section_twips(top, "top margin")?;
        let right = checked_section_twips(right, "right margin")?;
        let bottom = checked_section_twips(bottom, "bottom margin")?;
        let left = checked_section_twips(left, "left margin")?;
        self.inner.margin_top = Some(top);
        self.inner.margin_right = Some(right);
        self.inner.margin_bottom = Some(bottom);
        self.inner.margin_left = Some(left);
        complete_page_margins(self.inner);
        Ok(())
    }

    /// Return the explicitly configured gutter.
    pub fn gutter(&self) -> Option<Length> {
        self.inner.gutter.map(|value| Length::twips(value.0))
    }

    /// Set this section's nonnegative gutter.
    ///
    /// Any other `w:pgMar` value the section lacks takes its layout default, so
    /// the written element carries every attribute `CT_PageMar` requires.
    pub fn set_gutter(&mut self, gutter: Length) -> Result<()> {
        let gutter = checked_nonnegative_section_twips(gutter, "gutter")?;
        self.inner.gutter = Some(gutter);
        complete_page_margins(self.inner);
        Ok(())
    }

    /// Return the equal-width column count and spacing when modeled by M23.
    pub fn columns(&self) -> Option<(u32, Length)> {
        SectionRef {
            ordinal: self.ordinal,
            is_final: self.is_final,
            inner: self.inner,
        }
        .columns()
    }

    /// Set positive-count equal-width columns with nonnegative spacing.
    pub fn set_columns(&mut self, count: u32, spacing: Length) -> Result<()> {
        if count == 0 {
            return Err(Error::Other(
                "section column count must be positive".to_owned(),
            ));
        }
        let spacing = checked_nonnegative_section_twips(spacing, "column spacing")?;
        let separator = self.inner.columns.as_ref().and_then(|columns| columns.sep);
        self.inner.columns = Some(CT_Columns {
            num: Some(count),
            space: Some(spacing),
            equal_width: Some(true),
            sep: separator,
            columns: Vec::new(),
        });
        Ok(())
    }

    /// Return the explicit column tracks, as `(width, trailing space)` pairs.
    pub fn column_widths(&self) -> Option<Vec<(Length, Length)>> {
        SectionRef {
            ordinal: self.ordinal,
            is_final: self.is_final,
            inner: self.inner,
        }
        .column_widths()
    }

    /// Set explicit column tracks of positive width and nonnegative spacing.
    ///
    /// Authoring tracks marks the section unequal width, which is what makes
    /// the resolver read the list rather than divide the text measure.
    pub fn set_column_widths(&mut self, tracks: &[(Length, Length)]) -> Result<()> {
        if tracks.is_empty() {
            return Err(Error::Other(
                "section column tracks must not be empty".to_owned(),
            ));
        }
        let mut columns = Vec::with_capacity(tracks.len());
        for (width, space) in tracks {
            columns.push(CT_Column {
                width: Some(checked_positive_section_twips(*width, "column width")?),
                space: Some(checked_nonnegative_section_twips(*space, "column spacing")?),
            });
        }
        let count = u32::try_from(tracks.len())
            .map_err(|_| Error::Other("section column count exceeds the range".to_owned()))?;
        let separator = self.inner.columns.as_ref().and_then(|columns| columns.sep);
        self.inner.columns = Some(CT_Columns {
            num: Some(count),
            space: None,
            equal_width: Some(false),
            sep: separator,
            columns,
        });
        Ok(())
    }

    /// Return whether a rule is drawn between this section's columns.
    pub fn column_separator(&self) -> Option<bool> {
        SectionRef {
            ordinal: self.ordinal,
            is_final: self.is_final,
            inner: self.inner,
        }
        .column_separator()
    }

    /// Configure the rule drawn between this section's columns.
    pub fn set_column_separator(&mut self, enabled: bool) {
        match &mut self.inner.columns {
            Some(columns) => columns.sep = Some(enabled),
            None => {
                self.inner.columns = Some(CT_Columns {
                    sep: Some(enabled),
                    ..CT_Columns::default()
                });
            }
        }
    }

    /// Borrow this section's footnote configuration.
    pub fn footnote_properties(&self) -> Option<&CT_NoteProperties> {
        self.inner.footnote_pr.as_deref()
    }

    /// Set this section's footnote configuration.
    pub fn set_footnote_properties(&mut self, properties: CT_NoteProperties) {
        self.inner.footnote_pr = Some(Box::new(properties));
    }

    /// Borrow this section's endnote configuration.
    pub fn endnote_properties(&self) -> Option<&CT_NoteProperties> {
        self.inner.endnote_pr.as_deref()
    }

    /// Set this section's endnote configuration.
    pub fn set_endnote_properties(&mut self, properties: CT_NoteProperties) {
        self.inner.endnote_pr = Some(Box::new(properties));
    }

    /// Set a checked section override while retaining unmodelled note-property children.
    pub fn set_note_policy(&mut self, family: NoteFamily, policy: NotePolicy) -> Result<()> {
        let checked = policy.properties(family)?;
        let slot = match family {
            NoteFamily::Footnote => &mut self.inner.footnote_pr,
            NoteFamily::Endnote => &mut self.inner.endnote_pr,
        };
        let properties = slot.get_or_insert_with(Box::default);
        properties.pos = checked.pos;
        properties.num_fmt = checked.num_fmt;
        properties.num_start = checked.num_start;
        properties.num_restart = checked.num_restart;
        Ok(())
    }

    /// Remove an explicit section note override so document defaults apply.
    pub fn remove_note_policy(&mut self, family: NoteFamily) -> Option<CT_NoteProperties> {
        let slot = match family {
            NoteFamily::Footnote => &mut self.inner.footnote_pr,
            NoteFamily::Endnote => &mut self.inner.endnote_pr,
        };
        let properties = slot.as_mut()?;
        if properties.pos.is_none()
            && properties.num_fmt.is_none()
            && properties.num_start.is_none()
            && properties.num_restart.is_none()
        {
            return None;
        }
        let removed = (**properties).clone();
        properties.pos = None;
        properties.num_fmt = None;
        properties.num_start = None;
        properties.num_restart = None;
        if properties.special_references.is_empty() && properties.extra_xml.is_empty() {
            *slot = None;
        }
        Some(removed)
    }

    /// Return the printer trays feeding the first page and every later page.
    pub fn paper_source(&self) -> Option<(Option<u32>, Option<u32>)> {
        SectionRef {
            ordinal: self.ordinal,
            is_final: self.is_final,
            inner: self.inner,
        }
        .paper_source()
    }

    /// Select the printer trays this section prints from.
    ///
    /// Tray selection is a print-time choice, so page geometry and page count
    /// are identical with and without it.
    pub fn set_paper_source(&mut self, first: Option<u32>, other: Option<u32>) {
        let retained = self
            .inner
            .paper_source
            .take()
            .map(|source| source.extra_attributes)
            .unwrap_or_default();
        self.inner.paper_source = Some(Box::new(CT_PaperSource {
            first,
            other,
            extra_attributes: retained,
        }));
    }

    /// Borrow this section's page border frame.
    pub fn page_borders(&self) -> Option<&CT_PageBorders> {
        self.inner.page_borders.as_deref()
    }

    /// Set this section's page border frame.
    pub fn set_page_borders(&mut self, borders: CT_PageBorders) {
        self.inner.page_borders = Some(Box::new(borders));
    }

    /// Edit one page-frame edge without replacing other edges or opaque owners.
    /// Width is in eighth-points and spacing in points, as specified by OOXML.
    pub fn set_page_border(
        &mut self,
        edge: &str,
        style: &str,
        width: u32,
        space: u32,
        color: &str,
    ) -> Result<()> {
        let style = oxoffice_docx_oxml::shared::ST_Border::from_str(style)?;
        if !(2..=96).contains(&width) || space > 31 {
            return Err(Error::Other(
                "page border width must be 2..96 eighth-points and spacing 0..31 points".into(),
            ));
        }
        if color.len() != 6 || !color.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::Other(
                "page border color requires six hexadecimal digits".into(),
            ));
        }
        if !matches!(edge, "top" | "left" | "bottom" | "right") {
            return Err(Error::Other(
                "page border edge must be top, left, bottom or right".into(),
            ));
        }
        let borders = self.inner.page_borders.get_or_insert_with(Box::default);
        let slot = match edge {
            "top" => &mut borders.top,
            "left" => &mut borders.left,
            "bottom" => &mut borders.bottom,
            "right" => &mut borders.right,
            _ => unreachable!(),
        };
        let mut border = slot
            .clone()
            .unwrap_or_else(|| oxoffice_docx_oxml::borders::CT_BorderEdge::new(style));
        border.set_literal_color(color)?;
        border.val = style;
        border.sz = Some(width);
        border.space = Some(space);
        *slot = Some(border);
        Ok(())
    }

    /// Borrow this section's margin line numbering.
    pub fn line_numbers(&self) -> Option<&CT_LineNumber> {
        self.inner.line_numbers.as_deref()
    }

    /// Set this section's margin line numbering.
    pub fn set_line_numbers(&mut self, numbering: CT_LineNumber) {
        self.inner.line_numbers = Some(Box::new(numbering));
    }

    /// Return the vertical alignment of the body band within the page.
    pub fn vertical_alignment(&self) -> Option<ST_VerticalJc> {
        self.inner.vertical_alignment
    }

    /// Set the vertical alignment of the body band within the page.
    ///
    /// `ST_VerticalJc::Both` keeps its source value and lays out as `Top`.
    /// True vertical distribution is a named follow-up.
    pub fn set_vertical_alignment(&mut self, alignment: ST_VerticalJc) {
        self.inner.vertical_alignment = Some(alignment);
    }

    /// Return this section's `w:textDirection`, exactly as it was authored.
    pub fn text_direction(&self) -> Option<&str> {
        self.inner.text_direction.as_deref()
    }

    /// Set this section's `w:textDirection`.
    ///
    /// Authors and preserves the value. Layout projects it onto the rendered
    /// page, so nothing here changes what is drawn. A character XML 1.0
    /// cannot carry is dropped.
    pub fn set_text_direction(&mut self, direction: &str) {
        self.inner.text_direction = Some(oxoffice_core::xml::without_non_xml_characters(direction));
    }

    /// Return this section's `w:docGrid` character grid.
    pub fn doc_grid(&self) -> Option<&CT_DocGrid> {
        self.inner.doc_grid.as_deref()
    }

    /// Set or clear this section's `w:docGrid` character grid.
    ///
    /// A `default` grid, and an absent one, keep the ordinary line and
    /// character metrics. `lines`, `linesAndChars` and `snapToChars` put line
    /// advance on the grid pitch, and the last two put character advance on
    /// the character space as well.
    pub fn set_doc_grid(&mut self, grid: Option<CT_DocGrid>) {
        self.inner.doc_grid = grid.map(Box::new);
    }

    /// Return the explicitly configured page-number restart.
    pub fn page_number_start(&self) -> Option<u32> {
        self.inner.page_number.as_ref()?.start
    }

    /// Set the positive displayed page number at which this section starts.
    pub fn set_page_number_start(&mut self, start: u32) -> Result<()> {
        if start == 0 {
            return Err(Error::Other(
                "section page-number start must be positive".to_owned(),
            ));
        }
        match &mut self.inner.page_number {
            Some(page_number) => page_number.start = Some(start),
            None => {
                self.inner.page_number =
                    Some(oxoffice_docx_oxml::document::CT_PageNumberType::new(start))
            }
        }
        Ok(())
    }

    /// Return the explicitly configured header and footer distances.
    pub fn header_footer_distance(&self) -> Option<(Length, Length)> {
        SectionRef {
            ordinal: self.ordinal,
            is_final: self.is_final,
            inner: self.inner,
        }
        .header_footer_distance()
    }

    /// Set nonnegative header and footer distances from the page edges.
    ///
    /// Any other `w:pgMar` value the section lacks takes its layout default, so
    /// the written element carries every attribute `CT_PageMar` requires.
    pub fn set_header_footer_distance(&mut self, header: Length, footer: Length) -> Result<()> {
        let header = checked_nonnegative_section_twips(header, "header distance")?;
        let footer = checked_nonnegative_section_twips(footer, "footer distance")?;
        self.inner.header_distance = Some(header);
        self.inner.footer_distance = Some(footer);
        complete_page_margins(self.inner);
        Ok(())
    }

    /// Return the explicitly configured title-page state.
    pub fn different_first_page(&self) -> Option<bool> {
        self.inner.title_pg
    }

    /// Configure whether this section uses first-page header and footer variants.
    pub fn set_different_first_page(&mut self, enabled: bool) {
        self.inner.title_pg = Some(enabled);
    }

    /// Return the explicitly configured section break type.
    pub fn break_type(&self) -> Option<ST_SectionType> {
        self.inner.section_type
    }

    /// Set this section's break type.
    pub fn set_break_type(&mut self, break_type: ST_SectionType) {
        self.inner.section_type = Some(break_type);
    }

    /// Borrow the complete section properties.
    pub fn properties(&self) -> &CT_SectPr {
        self.inner
    }

    /// Mutably borrow the complete section properties.
    pub fn properties_mut(&mut self) -> &mut CT_SectPr {
        self.inner
    }
}

/// Give every `w:pgMar` attribute a value once a setter has written one.
///
/// `CT_PageMar` requires all seven attributes. A value the section already has
/// is kept, and an absent one takes the value layout assumes when it is
/// missing, which is the `CT_SectPr::default_letter` value, so the section
/// renders exactly as before.
fn complete_page_margins(properties: &mut CT_SectPr) {
    let defaults = CT_SectPr::default_letter();
    properties.margin_top = properties.margin_top.or(defaults.margin_top);
    properties.margin_right = properties.margin_right.or(defaults.margin_right);
    properties.margin_bottom = properties.margin_bottom.or(defaults.margin_bottom);
    properties.margin_left = properties.margin_left.or(defaults.margin_left);
    properties.gutter = properties.gutter.or(defaults.gutter);
    properties.header_distance = properties.header_distance.or(defaults.header_distance);
    properties.footer_distance = properties.footer_distance.or(defaults.footer_distance);
}

fn checked_positive_section_twips(value: Length, name: &str) -> Result<Twips> {
    if value.to_emu() <= 0 {
        return Err(Error::Other(format!("section {name} must be positive")));
    }
    let twips = checked_section_twips(value, name)?;
    if twips.0 == 0 {
        return Err(Error::Other(format!(
            "section {name} must be at least one twip"
        )));
    }
    Ok(twips)
}

fn checked_nonnegative_section_twips(value: Length, name: &str) -> Result<Twips> {
    if value.to_emu() < 0 {
        return Err(Error::Other(format!("section {name} must be nonnegative")));
    }
    checked_section_twips(value, name)
}

fn checked_section_twips(value: Length, name: &str) -> Result<Twips> {
    let twips = value.to_emu() / 635;
    let twips = i32::try_from(twips)
        .map_err(|_| Error::Other(format!("section {name} exceeds the twip range")))?;
    Ok(Twips(twips))
}

pub(super) fn empty_section_properties() -> CT_SectPr {
    CT_SectPr {
        page_width: None,
        page_height: None,
        orientation: None,
        margin_top: None,
        margin_right: None,
        margin_bottom: None,
        margin_left: None,
        gutter: None,
        header_distance: None,
        footer_distance: None,
        section_type: None,
        columns: None,
        page_number: None,
        footnote_pr: None,
        endnote_pr: None,
        paper_source: None,
        page_borders: None,
        line_numbers: None,
        vertical_alignment: None,
        text_direction: None,
        doc_grid: None,
        title_pg: None,
        header_refs: Vec::new(),
        footer_refs: Vec::new(),
        extra_xml: Vec::new(),
        extra_xml_positions: Vec::new(),
        change: None,
    }
}

//! Styles and document defaults: adding, changing, linking and resolving them.

use super::*;

fn update_reciprocal_style_link(
    styles: &mut CT_Styles,
    style_id: &str,
    old_link: Option<&str>,
    new_link: Option<&str>,
) {
    if old_link != new_link
        && let Some(old_target) = old_link
            .and_then(|target_id| styles.styles.iter_mut().find(|s| s.style_id == target_id))
        && old_target.linked_style.as_deref() == Some(style_id)
    {
        old_target.linked_style = None;
    }
    if let Some(new_target) =
        new_link.and_then(|target_id| styles.styles.iter_mut().find(|s| s.style_id == target_id))
        && new_target
            .linked_style
            .as_deref()
            .is_none_or(|linked| linked == style_id)
    {
        new_target.linked_style = Some(style_id.to_owned());
    }
}

fn merge_style_update(
    existing: &oxoffice_docx_oxml::styles::CT_Style,
    authored: &mut oxoffice_docx_oxml::styles::CT_Style,
    cleared: u32,
    removed_regions: &[oxoffice_docx_oxml::styles::TableStyleRegion],
) {
    authored.is_default = existing.is_default;
    if authored.based_on.is_none() && cleared & style::CLEAR_BASED_ON == 0 {
        authored.based_on = existing.based_on.clone();
    }
    if authored.next_style.is_none() && cleared & style::CLEAR_NEXT_STYLE == 0 {
        authored.next_style = existing.next_style.clone();
    }
    if authored.linked_style.is_none() && cleared & style::CLEAR_LINKED_STYLE == 0 {
        authored.linked_style = existing.linked_style.clone();
    }
    if authored.auto_redefine.is_none() && cleared & style::CLEAR_AUTO_REDEFINE == 0 {
        authored.auto_redefine = existing.auto_redefine;
    }
    if authored.hidden.is_none() && cleared & style::CLEAR_HIDDEN == 0 {
        authored.hidden = existing.hidden;
    }
    if authored.ui_priority.is_none() && cleared & style::CLEAR_PRIORITY == 0 {
        authored.ui_priority = existing.ui_priority;
    }
    if authored.semi_hidden.is_none() && cleared & style::CLEAR_SEMI_HIDDEN == 0 {
        authored.semi_hidden = existing.semi_hidden;
    }
    if authored.unhide_when_used.is_none() && cleared & style::CLEAR_UNHIDE_WHEN_USED == 0 {
        authored.unhide_when_used = existing.unhide_when_used;
    }
    if authored.quick_format.is_none() && cleared & style::CLEAR_QUICK_FORMAT == 0 {
        authored.quick_format = existing.quick_format;
    }
    if authored.locked.is_none() && cleared & style::CLEAR_LOCKED == 0 {
        authored.locked = existing.locked;
    }
    if let Some(properties) = &authored.ppr {
        if cleared & style::REPLACE_PARAGRAPH_PROPERTIES == 0 {
            let mut updated = existing.ppr.clone().unwrap_or_default();
            updated.merge_from(properties);
            authored.ppr = Some(updated);
        }
    } else if cleared & style::CLEAR_PARAGRAPH_PROPERTIES == 0 {
        authored.ppr = existing.ppr.clone();
    }
    if let Some(properties) = &authored.rpr {
        let mut updated = existing.rpr.clone().unwrap_or_default();
        updated.merge_from(properties);
        if cleared & style::CLEAR_FONT_THEME != 0 {
            updated.font_ascii_theme = None;
            updated.font_hansi_theme = None;
            updated.font_east_asia_theme = None;
            updated.font_cs_theme = None;
        }
        if cleared & style::CLEAR_COLOR_THEME != 0 {
            updated.color_theme = None;
            updated.color_theme_tint = None;
            updated.color_theme_shade = None;
        }
        authored.rpr = Some(updated);
    } else if cleared & style::CLEAR_RUN_PROPERTIES == 0 {
        authored.rpr = existing.rpr.clone();
    }
    authored.extra_xml = existing.extra_xml.clone();
    authored.extra_attributes = existing.extra_attributes.clone();
    authored.modeled_xml = existing.modeled_xml.clone();
    if let Some(properties) = &authored.table_properties {
        authored.table_properties = Some(merge_table_style_properties(
            existing.table_properties.as_ref(),
            properties,
        ));
    } else if cleared & style::CLEAR_TABLE_PROPERTIES == 0 {
        authored.table_properties = existing.table_properties.clone();
    }
    authored.table_properties_original = existing.table_properties_original.clone();
    authored.table_properties_xml = existing.table_properties_xml.clone();
    if authored.table_row_properties.is_none() && cleared & style::CLEAR_TABLE_ROW_PROPERTIES == 0 {
        authored.table_row_properties = existing.table_row_properties.clone();
    }
    if authored.table_cell_properties.is_none() && cleared & style::CLEAR_TABLE_CELL_PROPERTIES == 0
    {
        authored.table_cell_properties = existing.table_cell_properties.clone();
    }
    let authored_regions = std::mem::take(&mut authored.conditional_table_styles);
    let mut merged_regions = if cleared & style::CLEAR_CONDITIONAL_TABLE_STYLES == 0 {
        existing.conditional_table_styles.clone()
    } else {
        Vec::new()
    };
    merged_regions.retain(|conditional| {
        conditional
            .region
            .is_none_or(|region| !removed_regions.contains(&region))
    });
    let mut authored_region_names = std::collections::HashSet::new();
    for authored_region in authored_regions {
        if !authored_region_names.insert(authored_region.region) {
            merged_regions.push(authored_region);
            continue;
        }
        if let Some(index) = merged_regions
            .iter()
            .position(|current| current.region == authored_region.region)
        {
            merged_regions[index] =
                merge_conditional_table_style(&merged_regions[index], &authored_region);
        } else {
            merged_regions.push(authored_region);
        }
    }
    authored.conditional_table_styles = merged_regions;
}

fn merge_conditional_table_style(
    existing: &oxoffice_docx_oxml::styles::CT_TblStylePr,
    authored: &oxoffice_docx_oxml::styles::CT_TblStylePr,
) -> oxoffice_docx_oxml::styles::CT_TblStylePr {
    let mut updated = existing.clone();
    if let Some(properties) = &authored.paragraph_properties {
        let target = updated.paragraph_properties.get_or_insert_default();
        target.merge_from(properties);
    }
    if let Some(properties) = &authored.run_properties {
        let target = updated.run_properties.get_or_insert_default();
        target.merge_from(properties);
    }
    if let Some(properties) = &authored.table_properties {
        updated.table_properties = Some(merge_table_style_properties(
            updated.table_properties.as_ref(),
            properties,
        ));
    }
    if let Some(properties) = &authored.row_properties {
        // A row layer has no typed merge of its own, so an authored row layer
        // replaces the existing one outright.
        updated.row_properties = Some(properties.clone());
    }
    if let Some(properties) = &authored.cell_properties {
        updated.cell_properties = Some(merge_table_cell_style_properties(
            updated.cell_properties.as_ref(),
            properties,
        ));
    }
    updated
}

fn merge_table_borders(
    existing: Option<&oxoffice_docx_oxml::table::CT_TblBorders>,
    authored: &oxoffice_docx_oxml::table::CT_TblBorders,
) -> oxoffice_docx_oxml::table::CT_TblBorders {
    let mut updated = existing.cloned().unwrap_or_default();
    if authored.top.is_some() {
        updated.top.clone_from(&authored.top);
    }
    if authored.bottom.is_some() {
        updated.bottom.clone_from(&authored.bottom);
    }
    if authored.left.is_some() {
        updated.left.clone_from(&authored.left);
    }
    if authored.right.is_some() {
        updated.right.clone_from(&authored.right);
    }
    if authored.inside_h.is_some() {
        updated.inside_h.clone_from(&authored.inside_h);
    }
    if authored.inside_v.is_some() {
        updated.inside_v.clone_from(&authored.inside_v);
    }
    for raw in &authored.extra_xml {
        if !updated.extra_xml.contains(raw) {
            updated.extra_xml.push(raw.clone());
        }
    }
    updated
}

fn merge_table_cell_style_properties(
    existing: Option<&oxoffice_docx_oxml::table::CT_TcPr>,
    authored: &oxoffice_docx_oxml::table::CT_TcPr,
) -> oxoffice_docx_oxml::table::CT_TcPr {
    let mut updated = existing.cloned().unwrap_or_default();
    if authored.width.is_some() {
        updated.width.clone_from(&authored.width);
    }
    if authored.grid_span.is_some() {
        updated.grid_span = authored.grid_span;
    }
    if authored.h_merge.is_some() {
        updated.h_merge.clone_from(&authored.h_merge);
    }
    if authored.v_merge.is_some() {
        updated.v_merge = authored.v_merge;
    }
    if let Some(borders) = &authored.borders {
        updated.borders = Some(merge_table_borders(updated.borders.as_ref(), borders));
    }
    if authored.shading.is_some() {
        updated.shading.clone_from(&authored.shading);
    }
    if authored.v_align.is_some() {
        updated.v_align = authored.v_align;
    }
    if authored.no_wrap.is_some() {
        updated.no_wrap = authored.no_wrap;
    }
    if authored.text_direction.is_some() {
        updated.text_direction.clone_from(&authored.text_direction);
    }
    if authored.cnf_style.is_some() {
        updated.cnf_style.clone_from(&authored.cnf_style);
    }
    for raw in &authored.extra_xml {
        if !updated.extra_xml.contains(raw) {
            updated.extra_xml.push(raw.clone());
        }
    }
    updated
}

fn merge_table_style_properties(
    existing: Option<&oxoffice_docx_oxml::table::CT_TblPr>,
    authored: &oxoffice_docx_oxml::table::CT_TblPr,
) -> oxoffice_docx_oxml::table::CT_TblPr {
    let mut updated = existing.cloned().unwrap_or_default();
    if authored.style_id.is_some() {
        updated.style_id.clone_from(&authored.style_id);
    }
    if authored.row_band_size.is_some() {
        updated.row_band_size = authored.row_band_size;
    }
    if authored.column_band_size.is_some() {
        updated.column_band_size = authored.column_band_size;
    }
    if authored.width.is_some() {
        updated.width.clone_from(&authored.width);
    }
    if authored.jc.is_some() {
        updated.jc = authored.jc;
    }
    if let Some(borders) = &authored.borders {
        updated.borders = Some(merge_table_borders(updated.borders.as_ref(), borders));
    }
    if let Some(margins) = &authored.cell_margin {
        let target = updated.cell_margin.get_or_insert_default();
        if margins.top.is_some() {
            target.top = margins.top;
        }
        if margins.bottom.is_some() {
            target.bottom = margins.bottom;
        }
        if margins.left.is_some() {
            target.left = margins.left;
        }
        if margins.right.is_some() {
            target.right = margins.right;
        }
    }
    if authored.layout.is_some() {
        updated.layout.clone_from(&authored.layout);
    }
    if authored.indent.is_some() {
        updated.indent.clone_from(&authored.indent);
    }
    if authored.shading.is_some() {
        updated.shading.clone_from(&authored.shading);
    }
    if let Some(look) = &authored.look {
        let target = updated.look.get_or_insert_default();
        if look.val.is_some() {
            target.val.clone_from(&look.val);
        }
        if look.first_row.is_some() {
            target.first_row = look.first_row;
        }
        if look.last_row.is_some() {
            target.last_row = look.last_row;
        }
        if look.first_column.is_some() {
            target.first_column = look.first_column;
        }
        if look.last_column.is_some() {
            target.last_column = look.last_column;
        }
        if look.no_h_band.is_some() {
            target.no_h_band = look.no_h_band;
        }
        if look.no_v_band.is_some() {
            target.no_v_band = look.no_v_band;
        }
    }
    if authored.change.is_some() {
        updated.change.clone_from(&authored.change);
    }
    for raw in &authored.revision_xml {
        if !updated.revision_xml.contains(raw) {
            updated.revision_xml.push(raw.clone());
        }
    }
    for raw in &authored.extra_xml {
        if !updated.extra_xml.contains(raw) {
            updated.extra_xml.push(raw.clone());
        }
    }
    updated
}

fn body_references_style(content: &[BodyContent], style_id: &str) -> bool {
    content.iter().any(|item| match item {
        BodyContent::Paragraph(paragraph) => paragraph_references_style(paragraph, style_id),
        BodyContent::Table(table) => table_references_style(table, style_id),
        BodyContent::ContentControl(control) => control_references_style(control, style_id),
        BodyContent::RawXml(_) => false,
    })
}

fn paragraph_references_style(paragraph: &CT_P, style_id: &str) -> bool {
    paragraph
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref())
        == Some(style_id)
        || paragraph.runs.iter().any(|run| {
            run.properties
                .as_ref()
                .and_then(|properties| properties.style_id.as_deref())
                == Some(style_id)
        })
        || paragraph
            .content_controls
            .iter()
            .any(|(_, _, _, control)| control_references_style(control, style_id))
}

fn table_references_style(table: &CT_Tbl, style_id: &str) -> bool {
    table
        .properties
        .as_ref()
        .and_then(|properties| properties.style_id.as_deref())
        == Some(style_id)
        || table.rows.iter().any(|row| {
            row.cells.iter().any(|cell| {
                cell.content.iter().any(|item| match item {
                    CellContent::Paragraph(paragraph) => {
                        paragraph_references_style(paragraph, style_id)
                    }
                    CellContent::Table(table) => table_references_style(table, style_id),
                    CellContent::ContentControl(control) => {
                        control_references_style(control, style_id)
                    }
                })
            })
        })
        || table
            .content_controls
            .iter()
            .any(|(_, _, control)| control_references_style(control, style_id))
}

fn control_references_style(control: &CT_Sdt, style_id: &str) -> bool {
    control.content.iter().any(|item| match item {
        SdtContent::Paragraph(paragraph) => paragraph_references_style(paragraph, style_id),
        SdtContent::Table(table) => table_references_style(table, style_id),
        SdtContent::Row(row) => row.cells.iter().any(|cell| {
            cell.content.iter().any(|item| match item {
                CellContent::Paragraph(paragraph) => {
                    paragraph_references_style(paragraph, style_id)
                }
                CellContent::Table(table) => table_references_style(table, style_id),
                CellContent::ContentControl(control) => control_references_style(control, style_id),
            })
        }),
        SdtContent::Cell(cell) => cell.content.iter().any(|item| match item {
            CellContent::Paragraph(paragraph) => paragraph_references_style(paragraph, style_id),
            CellContent::Table(table) => table_references_style(table, style_id),
            CellContent::ContentControl(control) => control_references_style(control, style_id),
        }),
        SdtContent::Run(run) => {
            run.properties
                .as_ref()
                .and_then(|properties| properties.style_id.as_deref())
                == Some(style_id)
        }
        SdtContent::ContentControl(nested) => control_references_style(nested, style_id),
        SdtContent::RawXml(_) => false,
    })
}

pub(super) fn xml_references_style(xml: &[u8], style_id: &str) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == WORD_NAMESPACE.as_bytes())
                    && matches!(
                        element.local_name().as_ref(),
                        b"pStyle" | b"rStyle" | b"tblStyle"
                    ) =>
            {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == WORD_NAMESPACE.as_bytes())
                        && local.as_ref() == b"val"
                        && attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|error| Error::Other(error.to_string()))?
                            .as_ref()
                            == style_id
                    {
                        return Ok(true);
                    }
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

impl Document {
    pub(super) fn reserve_styles_bundle(&mut self) -> Result<()> {
        let existing = self.styles_part_name.clone();
        let part_name = self
            .reserve_document_part_bundle(
                existing.as_deref(),
                DEFAULT_STYLES_PART,
                rel_types::STYLES,
                STYLES_CONTENT_TYPE,
            )
            .map_err(|error| {
                Error::Other(format!("styles relationship allocation failed: {error}"))
            })?;
        self.styles_part_name = Some(part_name);
        Ok(())
    }
}

impl Document {
    /// The direct ASCII document default font, without changing Normal.
    pub fn default_font_name(&self) -> Option<&str> {
        self.styles
            .doc_defaults
            .as_ref()?
            .rpr
            .as_ref()?
            .font_ascii
            .as_deref()
    }

    /// Stage a general four-slot default-font update, preserving unrelated defaults.
    pub fn set_default_font_name_value(&mut self, name: Option<&str>) -> Result<()> {
        if name.is_none()
            && self
                .styles
                .doc_defaults
                .as_ref()
                .and_then(|value| value.rpr.as_ref())
                .is_none()
        {
            return Ok(());
        }
        if let Some(name) = name {
            oxoffice_core::xml::reject_non_xml_characters("default_font_name", name)?;
        }
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        let defaults = Arc::make_mut(&mut candidate.styles)
            .doc_defaults
            .get_or_insert_with(Default::default);
        let properties = defaults.rpr.get_or_insert_with(Default::default);
        properties.font_ascii = name.map(str::to_owned);
        properties.font_hansi = name.map(str::to_owned);
        properties.font_east_asia = name.map(str::to_owned);
        properties.font_cs = name.map(str::to_owned);
        properties.font_ascii_theme = None;
        properties.font_hansi_theme = None;
        properties.font_east_asia_theme = None;
        properties.font_cs_theme = None;
        if candidate.styles == self.styles {
            return Ok(());
        }
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// The normal-script direct default font size.
    pub fn default_font_size(&self) -> Option<Length> {
        self.styles
            .doc_defaults
            .as_ref()?
            .rpr
            .as_ref()?
            .sz
            .map(|size| Length::pt(size.to_pt()))
    }

    /// Set both normal and complex-script half-point defaults after preflight.
    pub fn set_default_font_size_value(&mut self, size: Option<Length>) -> Result<()> {
        let half_points = size
            .map(|size| {
                if size.to_emu() < 0 {
                    return Err(Error::Other(
                        "default_font_size cannot be negative".to_owned(),
                    ));
                }
                u32::try_from(size.to_emu() / 6350)
                    .map(oxoffice_docx_oxml::units::HalfPoint)
                    .map_err(|_| {
                        Error::Other("default_font_size exceeds half-point range".to_owned())
                    })
            })
            .transpose()?;
        if size.is_none()
            && self
                .styles
                .doc_defaults
                .as_ref()
                .and_then(|value| value.rpr.as_ref())
                .is_none()
        {
            return Ok(());
        }
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        let defaults = Arc::make_mut(&mut candidate.styles)
            .doc_defaults
            .get_or_insert_with(Default::default);
        let properties = defaults.rpr.get_or_insert_with(Default::default);
        properties.sz = half_points;
        properties.sz_cs = half_points;
        if candidate.styles == self.styles {
            return Ok(());
        }
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// The document default paragraph properties, `w:pPrDefault`.
    ///
    /// The value is the stored `CT_PPr`, including every parsed line rule.
    pub fn default_paragraph_properties(&self) -> Option<&CT_PPr> {
        self.styles.doc_defaults.as_ref()?.ppr.as_ref()
    }

    /// Replace `w:pPrDefault`. `None` removes the paragraph defaults and
    /// keeps `w:rPrDefault` plus unrelated `w:docDefaults` source.
    pub fn set_default_paragraph_properties(&mut self, properties: Option<CT_PPr>) -> Result<()> {
        if self.default_paragraph_properties() == properties.as_ref() {
            return Ok(());
        }
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        Arc::make_mut(&mut candidate.styles)
            .doc_defaults
            .get_or_insert_with(Default::default)
            .ppr = properties;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }
}

impl Document {
    // ---- Style access ----

    /// Get all styles.
    pub fn styles(&self) -> Vec<Style<'_>> {
        self.styles
            .styles
            .iter()
            .map(|s| Style { inner: s })
            .collect()
    }

    /// Find a style by its ID.
    pub fn style(&self, style_id: &str) -> Option<Style<'_>> {
        self.styles.get_by_id(style_id).map(|s| Style { inner: s })
    }

    // ---- Style manipulation ----

    /// Add a custom style after validating the change to the style graph.
    ///
    /// A defect the styles part already has, such as a style ID a producer
    /// repeated, is retained, and [`Document::validate_style_graph`] still
    /// reports it. Only a defect the change introduces rejects it, and an ID
    /// that already exists is refused. The style mutations resolve a repeated
    /// ID to its first definition, as layout and TOC rebuilding do. A value
    /// holding a character XML 1.0 cannot carry is refused: the ID, the name,
    /// a based-on, next or linked style, and any font, colour or other
    /// property the builder sets.
    pub fn add_style(&mut self, builder: StyleBuilder) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        let (authored, _, _) = builder.build();
        oxoffice_core::xml::reject_non_xml_characters("style ID", &authored.style_id)?;
        for (what, value) in [
            ("style name", &authored.name),
            ("based-on style", &authored.based_on),
            ("next style", &authored.next_style),
            ("linked style", &authored.linked_style),
        ] {
            if let Some(value) = value {
                oxoffice_core::xml::reject_non_xml_characters(what, value)?;
            }
        }
        refuse_non_xml_style_values(&authored)?;
        if candidate.styles.get_by_id(&authored.style_id).is_some() {
            return Err(Error::Other(format!(
                "style '{}' already exists",
                authored.style_id
            )));
        }
        if authored
            .ppr
            .as_ref()
            .is_some_and(|properties| properties.num_id.is_some() || properties.num_ilvl.is_some())
        {
            return Err(Error::Other(format!(
                "style '{}' numbering must be established with link_style_to_numbering",
                authored.style_id
            )));
        }
        let linked_style = authored.linked_style.clone();
        let style_id = authored.style_id.clone();
        Arc::make_mut(&mut candidate.styles).styles.push(authored);
        update_reciprocal_style_link(
            Arc::make_mut(&mut candidate.styles),
            &style_id,
            None,
            linked_style.as_deref(),
        );
        style::validate_style_graph_change(&self.styles, &candidate.styles)?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Replace an existing style after validating the change to the style graph.
    ///
    /// A repeated style ID updates its first definition. Validation retains
    /// existing defects as in [`Document::add_style`]. A value holding a
    /// character XML 1.0 cannot carry is refused, as [`Document::add_style`]
    /// refuses it.
    pub fn set_style(&mut self, builder: StyleBuilder) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        let (mut authored, cleared, removed_regions) = builder.build();
        refuse_non_xml_style_values(&authored)?;
        let index = candidate
            .styles
            .styles
            .iter()
            .position(|style| style.style_id == authored.style_id)
            .ok_or_else(|| Error::Other(format!("style '{}' does not exist", authored.style_id)))?;
        let existing = &candidate.styles.styles[index];
        if existing.style_type != authored.style_type {
            return Err(Error::Other(format!(
                "style '{}' has type '{}', not '{}'",
                authored.style_id,
                existing.style_type.to_str(),
                authored.style_type.to_str()
            )));
        }
        let old_link = existing.linked_style.clone();
        merge_style_update(existing, &mut authored, cleared, &removed_regions);
        // The levels are read as layout reads them, in the table before and
        // after the change: writing the level a `w:numId` without `w:ilvl`
        // shows, or a new `w:basedOn` that gives the same level, changes no
        // numbering. A style below this one in a `w:basedOn` chain may take
        // its level from it, so every style is compared.
        let numbering_before = super::numbering::style_numbering_levels(&candidate.styles);
        let style_id = authored.style_id.clone();
        let new_link = authored.linked_style.clone();
        Arc::make_mut(&mut candidate.styles).styles[index] = authored;
        let numbering_after = super::numbering::style_numbering_levels(&candidate.styles);
        if let Some(changed) = numbering_after
            .iter()
            .zip(&numbering_before)
            .position(|(after, before)| after != before)
        {
            return Err(Error::Other(format!(
                "style '{}' numbering must be changed with link_style_to_numbering or unlink_style_from_numbering",
                candidate.styles.styles[changed].style_id
            )));
        }
        update_reciprocal_style_link(
            Arc::make_mut(&mut candidate.styles),
            &style_id,
            old_link.as_deref(),
            new_link.as_deref(),
        );
        style::validate_style_graph_change(&self.styles, &candidate.styles)?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Select the sole default style for one style type.
    ///
    /// A repeated style ID makes its first definition the default. Validation
    /// retains existing defects as in [`Document::add_style`].
    pub fn set_default_style(&mut self, style_type: StyleType, style_id: &str) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        let index = candidate
            .styles
            .styles
            .iter()
            .position(|style| style.style_id == style_id)
            .ok_or_else(|| Error::Other(format!("style '{style_id}' does not exist")))?;
        let target = &candidate.styles.styles[index];
        if target.style_type != style_type {
            return Err(Error::Other(format!(
                "style '{style_id}' has type '{}', not '{}'",
                target.style_type.to_str(),
                style_type.to_str()
            )));
        }
        for (position, style) in Arc::make_mut(&mut candidate.styles)
            .styles
            .iter_mut()
            .enumerate()
        {
            if style.style_type == style_type {
                style.is_default = position == index;
            }
        }
        style::validate_style_graph_change(&self.styles, &candidate.styles)?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    /// Remove an unreferenced style, returning whether it existed.
    ///
    /// A linked pair goes as one: removing either side of a paragraph style
    /// and the character style that links back to it removes both. A
    /// numbering level the removed style points at, and that names it back,
    /// stops naming it, as [`Document::unlink_style_from_numbering`] does.
    /// The call is refused while content, a numbering level or a style
    /// outside the pair still names either side; a one-way link counts as a
    /// use.
    ///
    /// Every definition of a repeated style ID is removed, so no later one takes
    /// over the ID. Validation retains existing defects as in
    /// [`Document::add_style`].
    pub fn remove_style(&mut self, style_id: &str) -> Result<bool> {
        if self.styles.get_by_id(style_id).is_none() {
            return Ok(false);
        }
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let styles = Arc::clone(&self.styles);
        let styles_part_name = self.styles_part_name.clone();
        // The numbering journal undoes a released level and the
        // reservations; a refusal also puts the styles back.
        let restore = |document: &mut Self| {
            document.styles = Arc::clone(&styles);
            document.styles_part_name = styles_part_name.clone();
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.edit_numbering(|document| {
                let result = document.remove_style_in_place(style_id);
                if result.is_err() {
                    restore(document);
                }
                result
            })
        }));
        let result = match result {
            Ok(result) => result,
            Err(panic) => {
                restore(self);
                std::panic::resume_unwind(panic)
            }
        };
        if result.is_ok() {
            self.invalidate_layout();
        }
        #[cfg(debug_assertions)]
        self.assert_style_removal_matches_staged(before, style_id, &result);
        result.map(|()| true)
    }

    /// The styles [`Self::remove_style`] removes: `style_id` and the style
    /// linked to it that links back.
    fn styles_removed_with(&self, style_id: &str) -> Vec<String> {
        let links = |from: &str, to: &str| {
            self.styles
                .styles
                .iter()
                .any(|style| style.style_id == from && style.linked_style.as_deref() == Some(to))
        };
        let mut removed = vec![style_id.to_string()];
        for style in &self.styles.styles {
            if style.style_id != style_id
                && !removed.contains(&style.style_id)
                && links(&style.style_id, style_id)
                && links(style_id, &style.style_id)
            {
                removed.push(style.style_id.clone());
            }
        }
        removed
    }

    /// [`Self::remove_style`] on this document. A refusal leaves the styles
    /// and the numbering to the caller, which puts them back. Releasing a
    /// level's `w:pStyle` is checked with the numbering graph as the
    /// staged route did; the save writes the numbering model.
    fn remove_style_in_place(&mut self, style_id: &str) -> Result<()> {
        let removed = self.styles_removed_with(style_id);
        let released = self.release_numbering_levels_naming(&removed);
        if released {
            self.reserve_numbering_bundle()?;
        }
        for id in &removed {
            self.check_style_unreferenced(id, &removed)?;
        }
        self.reserve_styles_bundle()?;
        let previous = Arc::clone(&self.styles);
        Arc::make_mut(&mut self.styles)
            .styles
            .retain(|style| !removed.contains(&style.style_id));
        style::validate_style_graph_change(&previous, &self.styles)?;
        if released {
            self.validate_numbering_graph()?;
        }
        Ok(())
    }

    /// Debug proof that [`Self::remove_style`] in place leaves the styles
    /// and the numbering the staged route leaves, or refuses with its error
    /// and leaves them as they were.
    #[cfg(debug_assertions)]
    fn assert_style_removal_matches_staged(
        &self,
        before: Self,
        style_id: &str,
        outcome: &Result<()>,
    ) {
        let mut staged = before.clone_for_staging();
        let removed = staged.styles_removed_with(style_id);
        let staged_outcome = (|| -> Result<()> {
            let released = staged.release_numbering_levels_naming(&removed);
            if released {
                staged.reserve_numbering_bundle()?;
            }
            for id in &removed {
                staged.check_style_unreferenced(id, &removed)?;
            }
            staged.reserve_styles_bundle()?;
            Arc::make_mut(&mut staged.styles)
                .styles
                .retain(|style| !removed.contains(&style.style_id));
            style::validate_style_graph_change(&before.styles, &staged.styles)?;
            if released {
                staged.validate_numbering_graph()?;
            }
            Ok(())
        })();
        let expected = match (outcome, staged_outcome) {
            (Ok(()), Ok(())) => &staged,
            (Err(direct), Err(staged_error)) => {
                assert_eq!(
                    direct.to_string(),
                    staged_error.to_string(),
                    "in-place style removal refused otherwise"
                );
                &before
            }
            (Ok(()), Err(error)) => panic!("in-place style removal hid the staged error {error}"),
            (Err(error), Ok(())) => panic!("in-place style removal refused with {error}"),
        };
        assert_eq!(
            expected.styles, self.styles,
            "in-place style removal left other styles"
        );
        assert_eq!(expected.styles_part_name, self.styles_part_name);
        assert_eq!(expected.numbering, self.numbering);
        assert_eq!(expected.numbering_part_name, self.numbering_part_name);
    }

    /// Clear the `w:pStyle` of each numbering level that one of `styles`
    /// points at and that names that style back, returning whether any was
    /// cleared. A style without `w:ilvl` points at the level layout reads.
    fn release_numbering_levels_naming(&mut self, styles: &[String]) -> bool {
        let pointers: Vec<(String, u32, u32)> = self
            .styles
            .styles
            .iter()
            .filter(|style| styles.contains(&style.style_id))
            .filter_map(|style| {
                let (num_id, level) = super::numbering::style_numbering_level(&self.styles, style);
                Some((style.style_id.clone(), num_id?, level.unwrap_or(0)))
            })
            .collect();
        let Some(numbering) = self.numbering.as_mut() else {
            return false;
        };
        let mut released = false;
        for (style_id, num_id, level) in pointers {
            let Some(instance) = numbering.nums.iter_mut().find(|n| n.num_id == num_id) else {
                continue;
            };
            let definition_id = instance.abstract_num_id;
            if let Some(value) = instance
                .level_overrides
                .iter_mut()
                .find(|value| value.ilvl == level && value.level.is_some())
            {
                let level = value.level.as_mut().expect("selected for its level");
                if level.p_style.as_deref() == Some(style_id.as_str()) {
                    level.p_style = None;
                    released = true;
                }
                continue;
            }
            if let Some(level) = numbering
                .abstract_nums
                .iter_mut()
                .filter(|definition| definition.abstract_num_id == definition_id)
                .flat_map(|definition| definition.levels.iter_mut())
                .find(|value| value.ilvl == level)
                && level.p_style.as_deref() == Some(style_id.as_str())
            {
                level.p_style = None;
                released = true;
            }
        }
        released
    }

    /// Refuse when anything outside `removed` names `style_id`: another
    /// style, document content or a numbering level.
    fn check_style_unreferenced(&self, style_id: &str, removed: &[String]) -> Result<()> {
        if let Some(owner) = self.styles.styles.iter().find(|style| {
            !removed.contains(&style.style_id)
                && (style.based_on.as_deref() == Some(style_id)
                    || style.next_style.as_deref() == Some(style_id)
                    || style.linked_style.as_deref() == Some(style_id))
        }) {
            return Err(Error::Other(format!(
                "style '{style_id}' is referenced by style '{}'",
                owner.style_id
            )));
        }
        if self.document_references_style(style_id)? {
            return Err(Error::Other(format!(
                "style '{style_id}' is referenced by document content"
            )));
        }
        if self.numbering.as_ref().is_some_and(|numbering| {
            numbering.abstract_nums.iter().any(|definition| {
                definition
                    .levels
                    .iter()
                    .any(|level| level.p_style.as_deref() == Some(style_id))
            }) || numbering.nums.iter().any(|instance| {
                instance.level_overrides.iter().any(|value| {
                    value
                        .level
                        .as_ref()
                        .is_some_and(|level| level.p_style.as_deref() == Some(style_id))
                })
            })
        }) {
            return Err(Error::Other(format!(
                "style '{style_id}' is referenced by a numbering level"
            )));
        }
        Ok(())
    }

    /// Add the Word built-in styles most documents use and python-docx's
    /// default template defines, returning how many were added.
    ///
    /// They are `heading 2` to `heading 9`, `Title`, `Subtitle`,
    /// `No Spacing`, `Quote`, `List Paragraph`, `caption` and `Table Grid`,
    /// none of them a default style, so content that names none of them lays
    /// out as before. A style whose ID, or whose name regardless of case, the
    /// document already defines is skipped. Most are based on `Normal`, so a
    /// document without a `Normal` paragraph style is rejected unchanged.
    pub fn add_common_styles(&mut self) -> Result<usize> {
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        let mut added = 0;
        for common in style::common_styles() {
            let defined = candidate.styles.styles.iter().any(|style| {
                style.style_id == common.style_id
                    || style
                        .name
                        .as_deref()
                        .zip(common.name.as_deref())
                        .is_some_and(|(name, common)| name.eq_ignore_ascii_case(common))
            });
            if !defined {
                Arc::make_mut(&mut candidate.styles).styles.push(common);
                added += 1;
            }
        }
        if added == 0 {
            return Ok(0);
        }
        style::validate_style_graph(&candidate.styles)?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(added)
    }

    /// Materialize the requested supported built-in table style, without changing defaults.
    pub fn ensure_builtin_table_style(&mut self, name: &str) -> Result<bool> {
        if !matches!(name, "LightShading-Accent1" | "Light Shading Accent 1") {
            return Ok(false);
        }
        if self.styles.styles.iter().any(|s| {
            s.style_id == "LightShading-Accent1"
                || s.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
        }) {
            return Ok(false);
        }
        // python-docx 1.2.0 default.docx, SHA-256 2094b5bddffe9cf973d61fe03388413804f034160718494a65db7e98da40d35d.
        // The existing TableNormal base is retained. A copied absent base is nondefault.
        let definitions = CT_Styles::from_xml(br#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="table" w:styleId="TableNormal">
    <w:name w:val="Normal Table"/>
    <w:uiPriority w:val="99"/>
    <w:semiHidden/>
    <w:unhideWhenUsed/>
    <w:tblPr>
      <w:tblInd w:w="0" w:type="dxa"/>
      <w:tblCellMar>
        <w:top w:w="0" w:type="dxa"/>
        <w:left w:w="108" w:type="dxa"/>
        <w:bottom w:w="0" w:type="dxa"/>
        <w:right w:w="108" w:type="dxa"/>
      </w:tblCellMar>
    </w:tblPr>
  </w:style><w:style w:type="table" w:styleId="LightShading-Accent1">
    <w:name w:val="Light Shading Accent 1"/>
    <w:basedOn w:val="TableNormal"/>
    <w:uiPriority w:val="60"/>
    <w:rsid w:val="00FC693F"/>
    <w:pPr>
      <w:spacing w:after="0" w:line="240" w:lineRule="auto"/>
    </w:pPr>
    <w:rPr>
      <w:color w:val="365F91" w:themeColor="accent1" w:themeShade="BF"/>
    </w:rPr>
    <w:tblPr>
      <w:tblStyleRowBandSize w:val="1"/>
      <w:tblStyleColBandSize w:val="1"/>
      <w:tblInd w:w="0" w:type="dxa"/>
      <w:tblBorders>
        <w:top w:val="single" w:sz="8" w:space="0" w:color="4F81BD" w:themeColor="accent1"/>
        <w:bottom w:val="single" w:sz="8" w:space="0" w:color="4F81BD" w:themeColor="accent1"/>
      </w:tblBorders>
      <w:tblCellMar>
        <w:top w:w="0" w:type="dxa"/>
        <w:left w:w="108" w:type="dxa"/>
        <w:bottom w:w="0" w:type="dxa"/>
        <w:right w:w="108" w:type="dxa"/>
      </w:tblCellMar>
    </w:tblPr>
    <w:tblStylePr w:type="firstRow">
      <w:pPr>
        <w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="auto"/>
      </w:pPr>
      <w:rPr>
        <w:b/>
        <w:bCs/>
      </w:rPr>
      <w:tblPr/>
      <w:tcPr>
        <w:tcBorders>
          <w:top w:val="single" w:sz="8" w:space="0" w:color="4F81BD" w:themeColor="accent1"/>
          <w:left w:val="nil"/>
          <w:bottom w:val="single" w:sz="8" w:space="0" w:color="4F81BD" w:themeColor="accent1"/>
          <w:right w:val="nil"/>
          <w:insideH w:val="nil"/>
          <w:insideV w:val="nil"/>
        </w:tcBorders>
      </w:tcPr>
    </w:tblStylePr>
    <w:tblStylePr w:type="lastRow">
      <w:pPr>
        <w:spacing w:before="0" w:after="0" w:line="240" w:lineRule="auto"/>
      </w:pPr>
      <w:rPr>
        <w:b/>
        <w:bCs/>
      </w:rPr>
      <w:tblPr/>
      <w:tcPr>
        <w:tcBorders>
          <w:top w:val="single" w:sz="8" w:space="0" w:color="4F81BD" w:themeColor="accent1"/>
          <w:left w:val="nil"/>
          <w:bottom w:val="single" w:sz="8" w:space="0" w:color="4F81BD" w:themeColor="accent1"/>
          <w:right w:val="nil"/>
          <w:insideH w:val="nil"/>
          <w:insideV w:val="nil"/>
        </w:tcBorders>
      </w:tcPr>
    </w:tblStylePr>
    <w:tblStylePr w:type="firstCol">
      <w:rPr>
        <w:b/>
        <w:bCs/>
      </w:rPr>
    </w:tblStylePr>
    <w:tblStylePr w:type="lastCol">
      <w:rPr>
        <w:b/>
        <w:bCs/>
      </w:rPr>
    </w:tblStylePr>
    <w:tblStylePr w:type="band1Vert">
      <w:tblPr/>
      <w:tcPr>
        <w:tcBorders>
          <w:left w:val="nil"/>
          <w:right w:val="nil"/>
          <w:insideH w:val="nil"/>
          <w:insideV w:val="nil"/>
        </w:tcBorders>
        <w:shd w:val="clear" w:color="auto" w:fill="D3DFEE" w:themeFill="accent1" w:themeFillTint="3F"/>
      </w:tcPr>
    </w:tblStylePr>
    <w:tblStylePr w:type="band1Horz">
      <w:tblPr/>
      <w:tcPr>
        <w:tcBorders>
          <w:left w:val="nil"/>
          <w:right w:val="nil"/>
          <w:insideH w:val="nil"/>
          <w:insideV w:val="nil"/>
        </w:tcBorders>
        <w:shd w:val="clear" w:color="auto" w:fill="D3DFEE" w:themeFill="accent1" w:themeFillTint="3F"/>
      </w:tcPr>
    </w:tblStylePr>
  </w:style></w:styles>"#)?;
        let mut candidate = self.clone_for_staging();
        candidate.reserve_styles_bundle()?;
        for definition in definitions.styles {
            if let Some(existing) = candidate
                .styles
                .styles
                .iter()
                .find(|s| s.style_id == definition.style_id)
            {
                if existing.style_type != definition.style_type {
                    return Err(Error::Other(
                        "built-in table style base has a conflicting type".into(),
                    ));
                }
            } else {
                Arc::make_mut(&mut candidate.styles).styles.push(definition);
            }
        }
        style::validate_style_graph(&candidate.styles)?;
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
        Ok(true)
    }

    /// Validate all style IDs, defaults, links, next styles, and inheritance chains.
    pub fn validate_style_graph(&self) -> Result<()> {
        style::validate_style_graph(&self.styles)
    }

    fn document_references_style(&self, style_id: &str) -> Result<bool> {
        if self.main_part_references_style(style_id)
            || self
                .footnotes
                .footnotes
                .iter()
                .flat_map(|note| &note.paragraphs)
                .any(|paragraph| paragraph_references_style(paragraph, style_id))
            || self.comments.as_ref().is_some_and(|comments| {
                comments
                    .comments
                    .iter()
                    .flat_map(|comment| &comment.paragraphs)
                    .any(|paragraph| paragraph_references_style(paragraph, style_id))
            })
            || self.glossary.as_ref().is_some_and(|glossary| {
                glossary
                    .doc_parts
                    .iter()
                    .any(|part| body_references_style(&part.body.content, style_id))
            })
        {
            return Ok(true);
        }

        let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) else {
            return Ok(false);
        };
        for relationship in &relationships.items {
            if relationship_is_internal(relationship)
                && matches!(
                    relationship.rel_type.as_str(),
                    rel_types::HEADER
                        | rel_types::FOOTER
                        | rel_types::FOOTNOTES
                        | rel_types::ENDNOTES
                        | rel_types::COMMENTS
                        | rel_types::GLOSSARY_DOCUMENT
                )
            {
                let part_name =
                    OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                if let Some(xml) = self.package.get_part(&part_name)
                    && xml_references_style(xml, style_id)?
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Resolve the effective paragraph properties for a given style ID,
    /// walking the full inheritance chain (docDefaults → basedOn → ...).
    pub fn resolve_paragraph_properties(&self, style_id: Option<&str>) -> CT_PPr {
        style::resolve_paragraph_properties(style_id, &self.styles)
    }

    /// Resolve inherited, numbering-level, and direct properties for a
    /// concrete paragraph.
    pub fn effective_paragraph_properties(&self, paragraph: &ParagraphRef<'_>) -> CT_PPr {
        let direct = paragraph.inner.properties.as_ref();
        let style_id = direct
            .and_then(|properties| properties.style_id.as_deref())
            .or_else(|| {
                self.styles
                    .get_default(StyleType::Paragraph)
                    .map(|style| style.style_id.as_str())
            });
        let mut effective = style::resolve_paragraph_properties(style_id, &self.styles);
        // `w:numId` and `w:ilvl` merge on their own: a direct `w:numId` keeps
        // the level the style chain gives.
        effective.num_id = direct
            .and_then(|properties| properties.num_id)
            .or(effective.num_id);
        effective.num_ilvl = direct
            .and_then(|properties| properties.num_ilvl)
            .or(effective.num_ilvl);

        // A `w:numId` without `w:ilvl` numbers at level 0, as Word reads it.
        if let Some(num_id) = effective.num_id
            && let Some(definition) =
                self.resolved_numbering_level_definition(num_id, effective.num_ilvl.unwrap_or(0))
            && let Some(properties) = &definition.ppr
        {
            effective.merge_from(properties);
        }
        if let Some(properties) = direct {
            effective.merge_from(properties);
        }
        effective
    }

    /// Resolve the effective run properties for the given paragraph and character styles,
    /// walking the full inheritance chain.
    pub fn resolve_run_properties(
        &self,
        para_style_id: Option<&str>,
        run_style_id: Option<&str>,
    ) -> CT_RPr {
        style::resolve_run_properties(para_style_id, run_style_id, &self.styles)
    }

    /// Resolve inherited, paragraph-mark, and direct properties for one
    /// concrete body run.
    pub fn effective_run_properties(
        &self,
        paragraph: &ParagraphRef<'_>,
        run: &RunRef<'_>,
    ) -> CT_RPr {
        self.effective_run_properties_from_direct(paragraph, run.inner.properties.as_ref())
    }

    /// Resolve inherited, paragraph-mark, and direct properties for one cached
    /// complex-field display segment.
    pub fn effective_field_segment_properties(
        &self,
        paragraph: &ParagraphRef<'_>,
        segment: &FieldDisplaySegmentRef<'_>,
    ) -> CT_RPr {
        self.effective_run_properties_from_direct(paragraph, segment.properties())
    }

    fn effective_run_properties_from_direct(
        &self,
        paragraph: &ParagraphRef<'_>,
        direct: Option<&CT_RPr>,
    ) -> CT_RPr {
        let paragraph_properties = paragraph.inner.properties.as_ref();
        let mut effective = style::resolve_run_properties(
            paragraph_properties.and_then(|properties| properties.style_id.as_deref()),
            direct.and_then(|properties| properties.style_id.as_deref()),
            &self.styles,
        );
        if let Some(properties) =
            paragraph_properties.and_then(|properties| properties.rpr.as_ref())
        {
            effective.merge_from(properties);
        }
        if let Some(properties) = direct {
            effective.merge_from(properties);
        }
        effective
    }

    fn resolved_numbering_level_definition(
        &self,
        num_id: u32,
        level: u32,
    ) -> Option<&oxoffice_docx_oxml::numbering::CT_Lvl> {
        if num_id == 0 {
            return None;
        }
        self.numbering
            .as_ref()?
            .get_abstract_num_for(num_id)?
            .levels
            .iter()
            .find(|definition| definition.ilvl == level)
    }
}

/// Refuses a style that would write a character XML 1.0 cannot carry, in any
/// value the builder set: the style is serialized as it will be saved, and
/// the error names the style and the attribute or element that holds it.
fn refuse_non_xml_style_values(style: &oxoffice_docx_oxml::styles::CT_Style) -> Result<()> {
    let mut writer = quick_xml::Writer::new(Vec::new());
    style.to_xml(&mut writer)?;
    let xml = String::from_utf8_lossy(writer.get_ref());
    let Some((at, character)) = xml
        .char_indices()
        .find(|(_, character)| !oxoffice_core::xml::is_xml_1_0_character(*character))
    else {
        return Ok(());
    };
    // The value sits in the last attribute or element opened before it.
    let before = &xml[..at];
    let element = before[before.rfind('<').map_or(0, |start| start + 1)..]
        .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .next()
        .unwrap_or_default();
    let attribute = before
        .rfind('=')
        .filter(|equals| {
            before.rfind('<').is_some_and(|open| open < *equals) && !before[*equals..].contains('>')
        })
        .map(|equals| {
            let name = before[..equals].trim_end();
            &name[name.rfind(char::is_whitespace).map_or(0, |space| space + 1)..]
        });
    let place = match attribute {
        Some(attribute) => format!("{element}/@{attribute}"),
        None => element.to_owned(),
    };
    Err(oxoffice_core::OxmlError::InvalidValue(format!(
        "style {} value {place} holds U+{:04X}, {}",
        oxoffice_core::xml::strip_non_xml_characters(&style.style_id),
        u32::from(character),
        oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL
    ))
    .into())
}

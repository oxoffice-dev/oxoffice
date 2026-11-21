//! Inserting a table of contents.

use super::*;

impl Document {
    // ---- Table of Contents ----

    /// Insert a Table of Contents at the given body content index.
    ///
    /// Scans the document for heading paragraphs (styles named `heading 1`
    /// to `heading N`, whatever their localised id, where N <= max_level),
    /// inserts bookmark markers at each heading, and generates TOC entry paragraphs
    /// with internal hyperlinks and dot-leader tab stops.
    ///
    /// # Arguments
    /// * `index` - Body content index at which to insert the TOC
    /// * `max_level` - Maximum heading level to include (1-9, typically 3)
    pub fn insert_toc(&mut self, index: usize, max_level: u32) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.insert_toc_in_place(index, max_level)?;
        self.commit_staged_mutation(candidate);
        Ok(())
    }

    fn insert_toc_in_place(&mut self, index: usize, max_level: u32) -> Result<()> {
        use oxoffice_docx_oxml::borders::{CT_TabStop, CT_Tabs};
        use oxoffice_docx_oxml::shared::{ST_TabJc, ST_TabLeader};
        use oxoffice_docx_oxml::text::HyperlinkSpan;
        use oxoffice_docx_oxml::units::Twips;

        let max_level = max_level.clamp(1, 9);

        // Step 1: Collect heading info from the document body
        struct HeadingInfo {
            content_index: usize,
            level: u32,
            text: String,
            bookmark_name: String,
            bookmark_id: i32,
        }

        // Calling insert_toc twice must not mint bookmarks that collide with
        // the ones the first call left behind — duplicate `w:name` values make
        // the internal links ambiguous. Continue numbering past whatever is
        // already there.
        let mut occupied_suffixes = self.toc_bookmark_suffixes();
        let mut toc_counter = occupied_suffixes.iter().copied().max().unwrap_or(0);
        let mut identifiers = self.identifiers.clone();
        let mut duplicate_typed_id = false;
        let mut typed_ids = HashSet::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |paragraph| {
            for marker in &paragraph.bookmark_markers {
                if marker.is_start()
                    && let Some(id) = marker.id()
                    && id >= 0
                    && !typed_ids.insert(id)
                {
                    duplicate_typed_id = true;
                }
            }
        });
        if duplicate_typed_id {
            return Err(Error::Other("duplicate heading bookmark ID".to_owned()));
        }
        identifiers.bookmark_ids.extend(typed_ids);

        let mut headings = Vec::new();

        for (idx, content) in self.document.body.content.iter().enumerate() {
            if let BodyContent::Paragraph(p) = content
                && let Some(level) = self.detect_heading_level_for_toc(p)
                && level <= max_level
            {
                let text = p.accepted_text();
                if !text.trim().is_empty() {
                    let Some(suffix) = next_toc_bookmark_suffix(&occupied_suffixes, toc_counter)
                    else {
                        return Err(Error::Other(
                            "TOC bookmark suffixes are exhausted".to_owned(),
                        ));
                    };
                    toc_counter = suffix;
                    occupied_suffixes.insert(suffix);
                    let Some(preferred_id) = suffix
                        .checked_add(99)
                        .and_then(|candidate| i32::try_from(candidate).ok())
                    else {
                        return Err(Error::Other("TOC bookmark IDs are exhausted".to_owned()));
                    };
                    let bookmark_id = identifiers.reserve_preferred_bookmark_id(preferred_id)?;
                    headings.push(HeadingInfo {
                        content_index: idx,
                        level,
                        text,
                        bookmark_name: format!("_Toc{suffix}"),
                        bookmark_id,
                    });
                }
            }
        }

        // Step 2: Insert typed bookmark markers at each heading paragraph.
        for heading in &headings {
            if let Some(BodyContent::Paragraph(p)) =
                self.document.body.content.get_mut(heading.content_index)
            {
                let run_count = p.runs.len();
                let inserted_start =
                    p.insert_bookmark_start(0, heading.bookmark_id, &heading.bookmark_name);
                let inserted_end = p.insert_bookmark_end(run_count, heading.bookmark_id);
                if !inserted_start || !inserted_end {
                    return Err(Error::Other(
                        "TOC heading bookmark insertion failed".to_owned(),
                    ));
                }
            }
        }

        // Step 3: Build TOC entry paragraphs.
        // The dot leader runs to the right text margin, which depends on the
        // section's page size and margins rather than being a fixed 6.5".
        let right_tab = CT_Tabs {
            tabs: vec![CT_TabStop {
                val: ST_TabJc::Right,
                pos: Twips(self.text_width_twips()),
                leader: Some(ST_TabLeader::Dot),
                source_occurrence: None,
            }],
        };

        let mut toc_paragraphs: Vec<CT_P> = Vec::new();

        // TOC title
        let mut title_p = CT_P::new();
        let mut title_r = CT_R::new("Table of Contents");
        title_r.properties = Some(CT_RPr {
            bold: Some(true),
            ..Default::default()
        });
        title_p.runs.push(title_r);
        title_p.properties = Some(CT_PPr {
            space_after: Some(Twips(120)),
            ..Default::default()
        });
        toc_paragraphs.push(title_p);

        for heading in &headings {
            let mut p = CT_P::new();

            // Indentation based on heading level (each level indented 360 twips = 0.25")
            let indent = Twips(360 * (heading.level as i32 - 1));

            p.properties = Some(CT_PPr {
                tabs: Some(right_tab.clone()),
                ind_left: if indent.0 > 0 { Some(indent) } else { None },
                ..Default::default()
            });

            // Run with heading text
            let text_run = CT_R::new(&heading.text);
            p.runs.push(text_run);

            // Tab run (separates text from page number)
            p.runs.push(CT_R {
                alt_drawings: Vec::new(),
                properties: None,
                content: vec![oxoffice_docx_oxml::text::RunContent::Tab],
                extra_xml: Vec::new(),
                extra_xml_positions: Vec::new(),
            });

            // Wrap the text run in a hyperlink to the bookmark
            p.hyperlinks.push(HyperlinkSpan {
                rel_id: None,
                anchor: Some(heading.bookmark_name.clone()),
                tooltip: None,
                doc_location: None,
                run_start: 0,
                run_end: 1, // Just the text run, not the tab
                extra_attributes: Vec::new(),
                extra_xml: Vec::new(),
                preserved_raw_before: None,
            });

            toc_paragraphs.push(p);
        }

        if headings.is_empty() {
            toc_paragraphs.push(CT_P::new());
        }

        let field_run = |xml: String| {
            let mut run = CT_R::new("");
            run.content.clear();
            run.extra_xml.push(xml.into_bytes());
            run.extra_xml_positions.push(0);
            run
        };
        toc_paragraphs[0]
            .runs
            .push(field_run("<w:fldChar w:fldCharType=\"begin\"/>".to_owned()));
        toc_paragraphs[0].runs.push(field_run(format!(
            "<w:instrText>TOC \\o \"1-{max_level}\" \\h</w:instrText>"
        )));
        toc_paragraphs[0].runs.push(field_run(
            "<w:fldChar w:fldCharType=\"separate\"/>".to_owned(),
        ));
        toc_paragraphs
            .last_mut()
            .expect("the TOC title exists")
            .runs
            .push(field_run("<w:fldChar w:fldCharType=\"end\"/>".to_owned()));

        // Step 4: Insert TOC paragraphs at the specified index
        let insert_at = index.min(self.document.body.content.len());
        for (i, p) in toc_paragraphs.into_iter().enumerate() {
            self.document
                .body
                .content
                .insert(insert_at + i, BodyContent::Paragraph(Box::new(p)));
        }
        self.identifiers = identifiers;
        self.invalidate_layout();
        Ok(())
    }

    /// Numeric `_TocN` bookmark suffixes already present in the body.
    fn toc_bookmark_suffixes(&self) -> HashSet<u64> {
        let mut suffixes = HashSet::new();
        visit_body_paragraphs(&self.document.body.content, false, &mut |p| {
            for marker in &p.bookmark_markers {
                let Some(name) = marker.name() else {
                    continue;
                };
                let Some(after) = name.strip_prefix("_Toc") else {
                    continue;
                };
                if let Ok(suffix) = after.parse::<u64>() {
                    suffixes.insert(suffix);
                }
            }
        });
        suffixes
    }

    /// Width of the text column in twips: page width less both side margins.
    ///
    /// Falls back to the US Letter default (6.5") when the section does not
    /// specify a size, and never returns a non-positive width.
    fn text_width_twips(&self) -> i32 {
        const DEFAULT_TEXT_WIDTH: i32 = 9360;

        let Some(sect) = self.document.body.sect_pr.as_ref() else {
            return DEFAULT_TEXT_WIDTH;
        };
        let page_width = sect.page_width.map(|w| w.0).unwrap_or(12240);
        let left = sect.margin_left.map(|m| m.0).unwrap_or(1440);
        let right = sect.margin_right.map(|m| m.0).unwrap_or(1440);

        let width = i64::from(page_width) - i64::from(left) - i64::from(right);
        match i32::try_from(width) {
            Ok(width) if width > 0 => width,
            _ => DEFAULT_TEXT_WIDTH,
        }
    }

    /// The heading level of a paragraph, see [`paragraph_heading_level`].
    pub(super) fn detect_heading_level_for_toc(&self, para: &CT_P) -> Option<u32> {
        paragraph_heading_level(para, &self.styles)
    }
}

fn next_toc_bookmark_suffix(occupied: &HashSet<u64>, after: u64) -> Option<u64> {
    after
        .checked_add(1)
        .filter(|candidate| !occupied.contains(candidate))
        .or_else(|| (1..=u64::MAX).find(|candidate| !occupied.contains(candidate)))
}

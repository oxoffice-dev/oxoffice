//! The body: its items, paragraphs, text and revisions, and the page background.

use super::*;

impl Document {
    // ---- Paragraph access ----

    /// Iterate over direct body items in source order.
    ///
    /// Unlike [`Self::paragraphs`] and [`Self::tables`], this retains the
    /// interleaving of paragraphs, tables, content controls, and preserved
    /// unmodelled XML.
    pub fn body_items(&self) -> impl Iterator<Item = BodyItemRef<'_>> {
        self.document.body.content.iter().map(|item| match item {
            BodyContent::Paragraph(paragraph) => {
                BodyItemRef::Paragraph(ParagraphRef { inner: paragraph })
            }
            BodyContent::Table(table) => BodyItemRef::Table(TableRef { inner: table }),
            BodyContent::ContentControl(control) => {
                BodyItemRef::ContentControl(ContentControlRef::borrowed(control))
            }
            BodyContent::RawXml(raw) => BodyItemRef::UnsupportedXml(raw),
        })
    }

    /// Iterate over paragraphs, tables, and unsupported content in body order.
    ///
    /// This compatibility view reports content controls as modeled unsupported
    /// facts without fabricating raw XML bytes.
    pub fn body_content(&self) -> impl Iterator<Item = BodyContentRef<'_>> {
        self.document.body.content.iter().map(|item| match item {
            BodyContent::Paragraph(paragraph) => {
                BodyContentRef::Paragraph(ParagraphRef { inner: paragraph })
            }
            BodyContent::Table(table) => BodyContentRef::Table(TableRef { inner: table }),
            BodyContent::ContentControl(control) => {
                BodyContentRef::UnsupportedXml(UnsupportedXmlRef::modeled(
                    "w:sdt",
                    WORD_NAMESPACE,
                    "sdt",
                    control.has_child_content(),
                ))
            }
            BodyContent::RawXml(raw) => BodyContentRef::UnsupportedXml(UnsupportedXmlRef::raw(
                raw,
                &self.body_namespace_bindings,
            )),
        })
    }

    /// Whether the document declares a page background that may affect its
    /// visible appearance.
    pub fn has_document_background(&self) -> bool {
        self.document.background_xml.is_some()
    }

    /// Publish page color and the relationship-resolved display setting together.
    pub fn set_page_color(&mut self, color: &str) -> Result<()> {
        if !is_rgb_hex(color) {
            return Err(Error::Other(
                "page color must be six hexadecimal digits".to_owned(),
            ));
        }
        let mut candidate = self.settings_mutation_candidate()?;
        if let Some(background) = candidate.document.background_xml.as_ref() {
            let source = candidate
                .package
                .get_part(&candidate.doc_part_name)
                .ok_or_else(|| Error::Other("main document part is missing".to_owned()))?;
            let scope = document_namespace_scopes(source)?
                .root_declarations
                .into_iter()
                .collect();
            candidate.document.background_xml =
                Some(close_content_fragment_namespaces(background, &scope)?);
        }
        candidate.document.set_background_color(color)?;
        candidate
            .settings
            .get_or_insert_with(CT_Settings::new)
            .set_display_background_shape(true)?;
        candidate.invalidate_layout();
        let reopened = candidate.prepare_and_reopen_staged()?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }
}

impl Document {
    /// Return whether each facade paragraph contains displayed content.
    ///
    /// Uses an unpublished source snapshot and preserves the live document.
    pub fn paragraph_visibility(&self) -> Result<Vec<bool>> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_document_to_package()?;
        let source = candidate
            .package
            .get_part(&candidate.doc_part_name)
            .ok_or(Error::NoDocumentPart)?;
        paragraph_visibility_from_main_source(
            &candidate,
            source,
            ParagraphVisibilityScope::FacadeRecursive,
        )
    }

    /// Get immutable references to all paragraphs.
    pub fn paragraphs(&self) -> Vec<ParagraphRef<'_>> {
        self.document
            .body
            .paragraphs()
            .map(|p| ParagraphRef { inner: p })
            .collect()
    }

    /// Get an immutable reference to a paragraph by its index in
    /// [`Self::paragraphs`]. That index skips body tables and enters block
    /// content controls, so it is not the direct body index.
    pub fn paragraph(&self, index: usize) -> Option<ParagraphRef<'_>> {
        self.document
            .body
            .paragraphs()
            .nth(index)
            .map(|p| ParagraphRef { inner: p })
    }
}

impl Document {
    /// Add a paragraph with the given text and return a mutable reference.
    /// A character XML 1.0 cannot carry is dropped;
    /// [`Self::try_add_paragraph`] refuses it instead.
    pub fn add_paragraph(&mut self, text: &str) -> Paragraph<'_> {
        self.invalidate_layout();
        self.document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(
                Self::detached_text_paragraph(text),
            )));
        match self.document.body.content.last_mut().unwrap() {
            BodyContent::Paragraph(p) => Paragraph { inner: p },
            _ => unreachable!(),
        }
    }

    /// Get the number of paragraphs.
    pub fn paragraph_count(&self) -> usize {
        self.document.body.paragraphs().count()
    }

    /// Return every valid modeled main-document revision in document order.
    ///
    /// This borrowed projection covers the main body, its tables, cells, and
    /// content controls. [`Document::story_revisions`] lists every story.
    pub fn revisions(&self) -> Vec<RevisionRef<'_>> {
        self.document
            .revisions()
            .into_iter()
            .map(|inner| RevisionRef { inner })
            .collect()
    }

    /// Return every modeled revision that accepting or rejecting all revisions
    /// resolves, each with the story that holds it.
    ///
    /// The list covers the main document, headers, footers, comments, normal
    /// footnotes, endnotes, and the text boxes inside them. It has one entry per
    /// revision element, so its length is the count [`Document::accept_all`]
    /// and [`Document::reject_all`] report. The main document comes first, then
    /// headers and footers in section order, then comments, footnotes, and
    /// endnotes. Revisions keep document order within each part.
    ///
    /// Each story is one returned by [`Document::stories`]. A revision in a
    /// table cell belongs to the story that holds the table. A text box that
    /// `stories` does not report, such as one under `mc:AlternateContent` or
    /// one whose namespace binding the typed serialization drops, belongs to
    /// the story that holds its drawing. A revision outside every story owner,
    /// such as one inside a footnote separator, is an error, and so is a
    /// document that resolution cannot stage.
    pub fn story_revisions(&self) -> Result<Vec<StoryRevision>> {
        // Resolution scans the parts staging leaves in the package. The main
        // part there keeps its original bytes, or its namespace declarations
        // replayed, which the typed serialization that `stories` scans lacks.
        let mut staged = self.clone_for_staging();
        staged.prepare_staged_package()?;
        let mut parts = vec![(StoryKind::Body, staged.doc_part_name.clone())];
        parts.extend(crate::comparison::revision_story_parts(&staged)?);
        let mut seen = HashSet::new();
        let mut revisions = Vec::new();
        for (root_kind, part_name) in parts {
            if !seen.insert(part_name.clone()) {
                continue;
            }
            let scanned = staged
                .package
                .get_part(&part_name)
                .ok_or_else(|| Error::Other(format!("missing revision story part {part_name}")))?;
            let reported = if part_name == self.doc_part_name {
                Some(Cow::Owned(self.document.to_xml()?))
            } else {
                self.package.get_part(&part_name).map(Cow::Borrowed)
            };
            let owners = paired_story_owners(&part_name, root_kind, scanned, reported.as_deref())?;
            revisions.extend(crate::revision::story_part_revisions(
                &part_name, scanned, &owners,
            )?);
        }
        if staged
            .package
            .contains_part(crate::comparison::COMMENT_COMPARISON_PART)
        {
            let body = staged
                .stories()?
                .into_iter()
                .find(|story| story.kind() == StoryKind::Body)
                .ok_or_else(|| Error::Other("comment comparison has no body story".to_owned()))?;
            if let Some(revision) = crate::revision::comment_comparison_revision(&staged, body)? {
                revisions.push(revision);
            }
        }
        Ok(revisions)
    }

    /// Get the plain text of body paragraphs and table cells in document order.
    ///
    /// A body paragraph normally ends with a newline. A table row is one line in which
    /// every cell paragraph ends with a tab, paragraphs of nested tables
    /// included. Content controls at every level contribute the paragraphs,
    /// rows and cells they wrap at the position they occupy.
    /// Each paragraph contributes its accepted-view text, the same text as
    /// [`ParagraphRef::text`]: tracked insertions are included and tracked
    /// deletions are left out. A body paragraph whose mark is deleted or moved
    /// away joins the next body paragraph without a newline when
    /// `CT_Body::accepted_paragraph_joins_next` says so, as accepting it does,
    /// so one with no accepted text leaves no line at all. A table row that
    /// accepting removes, as
    /// `CT_Row::accepted_view_removes` reports it, contributes no line, so a
    /// table whose rows are all deleted contributes nothing.
    pub fn text(&self) -> String {
        let mut result = String::new();
        for (index, content) in self.document.body.content.iter().enumerate() {
            match content {
                BodyContent::Paragraph(paragraph)
                    if self.document.body.accepted_paragraph_joins_next(index) =>
                {
                    result.push_str(&ParagraphRef { inner: paragraph }.text());
                }
                BodyContent::Paragraph(paragraph) => push_paragraph_line(paragraph, &mut result),
                BodyContent::Table(table) => push_table_text(table, &mut result),
                BodyContent::ContentControl(control) => push_control_text(control, &mut result),
                BodyContent::RawXml(_) => {}
            }
        }
        result
    }

    /// Get a mutable reference to a paragraph by the same index as
    /// [`Self::paragraph`].
    pub fn paragraph_mut(&mut self, index: usize) -> Option<Paragraph<'_>> {
        self.invalidate_layout();
        let mut remaining = index;
        nth_paragraph_in_body(&mut self.document.body.content, &mut remaining)
            .map(|inner| Paragraph { inner })
    }

    /// Split a direct body paragraph run at a Unicode scalar offset of its
    /// literal text and return the resulting run boundary.
    ///
    /// `body_index` is the direct body child index that `RunPosition`,
    /// [`Self::find_content_index`], [`Self::add_comment`] and
    /// [`Self::insert_paragraph`] use, where a table or a block content
    /// control counts as one child. An index that names a table, a block
    /// content control or preserved XML is an error that names its kind. A
    /// paragraph inside a block content control is split through
    /// `paragraph_mut(i).split_run`.
    ///
    /// Zero returns the boundary before the selected run and the literal text
    /// length returns the boundary after it without changing the document.
    /// Interior offsets create a continuation with cloned run properties while
    /// preserving ordered non-text content, hyperlinks, and range markers.
    /// Every failure leaves the document unchanged.
    pub fn split_run(
        &mut self,
        body_index: usize,
        run_index: usize,
        character_offset: usize,
    ) -> Result<usize> {
        let not_a_paragraph = |kind: &str| {
            Error::Other(format!(
                "body index {body_index} is {kind}, not a paragraph"
            ))
        };
        let original = match self.document.body.content.get(body_index) {
            Some(BodyContent::Paragraph(paragraph)) => paragraph,
            Some(BodyContent::Table(_)) => return Err(not_a_paragraph("a table")),
            Some(BodyContent::ContentControl(_)) => {
                return Err(not_a_paragraph("a block content control"));
            }
            Some(BodyContent::RawXml(_)) => return Err(not_a_paragraph("preserved XML")),
            None => {
                return Err(Error::Other(format!(
                    "body index {body_index} is out of range"
                )));
            }
        };
        let mut candidate = original.clone();
        let boundary = Paragraph {
            inner: &mut candidate,
        }
        .split_run(run_index, character_offset)?;
        if candidate == *original {
            return Ok(boundary);
        }

        let Some(BodyContent::Paragraph(paragraph)) =
            self.document.body.content.get_mut(body_index)
        else {
            unreachable!("body index {body_index} was checked to be a paragraph");
        };
        *paragraph = candidate;
        self.invalidate_layout();
        Ok(boundary)
    }
}

//! Native snapshots of stories, story items, sections and styles, and the owners they bind.

use super::*;

impl PyDocument {
    /// `picture` as the staged routes compare it: a picture an in-place
    /// route returned carries its story without the fingerprint (a read of
    /// the whole main part), so its story is looked up by kind, part name
    /// and owner index among the stories as they are now. The caller checked
    /// the handle's revision: it names the story's occurrence now. A
    /// fingerprinted picture is returned as it is.
    pub(super) fn current_picture_occurrence(
        &self,
        py: Python<'_>,
        picture: &oxoffice_docx::PictureOccurrence,
    ) -> PyResult<oxoffice_docx::PictureOccurrence> {
        let mut occurrence = picture.clone();
        let story = occurrence.location.story().clone();
        if story
            == oxoffice_docx::StoryId::new(story.kind(), story.part_name(), story.owner_index())
            && let Some(current) = self
                .inner
                .stories()
                .map_err(|error| docx_to_pyerr(py, error))?
                .into_iter()
                .find(|candidate| {
                    candidate.kind() == story.kind()
                        && candidate.part_name() == story.part_name()
                        && candidate.owner_index() == story.owner_index()
                })
        {
            occurrence.location = oxoffice_docx::ContentLocation::new(
                current,
                occurrence.location.item_kind(),
                occurrence.location.index_path().to_vec(),
            );
        }
        Ok(occurrence)
    }

    /// The live native owner that a `Story` snapshot names.
    ///
    /// A snapshot carries no fingerprint, so it resolves by kind, part name
    /// and owner index against the document as it is now.
    pub(super) fn native_story(
        &self,
        py: Python<'_>,
        story: &PyStory,
    ) -> PyResult<oxoffice_docx::StoryId> {
        self.validate_owned_story(py, story)?;
        self.inner
            .stories_in_part(&story.part_name)
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|candidate| story_snapshot(candidate) == *story)
            .ok_or_else(|| {
                docx_to_pyerr(
                    py,
                    oxoffice_docx::Error::Other(format!(
                        "document has no {} story {} at owner index {}",
                        story.kind, story.part_name, story.owner_index
                    )),
                )
            })
    }

    /// The checked physical owner of a captured `Hyperlink` record: the
    /// story item that holds its link, the link's index among that item's
    /// links and among its story's. `None` for a constructed record.
    pub(super) fn hyperlink_owner(
        &self,
        py: Python<'_>,
        hyperlink: &PyHyperlink,
    ) -> PyResult<Option<(oxoffice_docx::ContentLocation, usize, usize)>> {
        let Some((path, state)) = &hyperlink.owner else {
            return Ok(None);
        };
        if !path.belongs_to(&self.revisions) {
            return Err(PyValueError::new_err(
                "hyperlink belongs to a different document",
            ));
        }
        path.validate_scopes(
            &self.revisions,
            "hyperlink",
            "re-fetch it with document.hyperlinks.",
        )
        .map_err(|error| crate::stale_to_pyerr(py, error))?;
        let owner = self
            .owners
            .iter()
            .find(|owner| owner.owner.same_state(state))
            .ok_or_else(|| {
                PyValueError::new_err("hyperlink physical owner is no longer current")
            })?;
        let WordOwnerLocation::Hyperlink(location, local) = &*owner.location() else {
            return Err(PyValueError::new_err(
                "hyperlink physical owner has the wrong kind",
            ));
        };
        let index = owner
            .projection_ordinal()
            .ok_or_else(|| PyValueError::new_err("hyperlink source position is absent"))?;
        Ok(Some((location.clone(), *local, index)))
    }

    /// The live story of a `Hyperlink` record and the position of its link.
    ///
    /// A captured snapshot resolves through its current checked physical owner
    /// state, independently of the source Story snapshot's global epoch.
    /// A constructed record resolves only when exactly one current link matches.
    pub(super) fn native_hyperlink(
        &self,
        py: Python<'_>,
        hyperlink: &PyHyperlink,
    ) -> PyResult<(oxoffice_docx::StoryId, usize)> {
        if let Some((location, _, index)) = self.hyperlink_owner(py, hyperlink)? {
            let story = self
                .inner
                .stories()
                .map_err(|error| docx_to_pyerr(py, error))?
                .into_iter()
                .find(|story| same_story_owner(story, location.story()))
                .ok_or_else(|| {
                    PyValueError::new_err("hyperlink story owner is no longer current")
                })?;
            return Ok((story, index));
        }
        let story = self.native_story(py, &hyperlink.story)?;
        let links = self
            .inner
            .story_links(&story)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let matches =
            |(location, link): &(oxoffice_docx::ContentLocation, oxoffice_docx::LinkInfo)| {
                location.index_path() == hyperlink.index_path.as_slice()
                    && link.text == hyperlink.text
                    && link.url == hyperlink.url
                    && link.anchor == hyperlink.anchor
                    && link.rel_id == hyperlink.relationship_id
            };
        let index =
            match hyperlink.position {
                Some((index, layout)) => links
                    .get(index)
                    .filter(|entry| {
                        matches(entry)
                            && story_link_layout(links.iter().map(|(location, link)| {
                                (location.index_path(), link.text.as_str())
                            })) == layout
                    })
                    .map(|_| index),
                None => {
                    let mut found = links
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| matches(entry))
                        .map(|(index, _)| index);
                    match (found.next(), found.next()) {
                        (Some(index), None) => Some(index),
                        _ => None,
                    }
                }
            };
        index.map(|index| (story, index)).ok_or_else(|| {
            docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other(
                    "the hyperlink does not match exactly one link of the document, re-fetch it with document.hyperlinks"
                        .to_owned(),
                ),
            )
        })
    }

    pub(super) fn body_story(&self, py: Python<'_>) -> PyResult<oxoffice_docx::StoryId> {
        self.inner
            .stories()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|story| story.kind() == oxoffice_docx::StoryKind::Body)
            .ok_or_else(|| {
                docx_to_pyerr(
                    py,
                    oxoffice_docx::Error::Other("document body story is missing".to_owned()),
                )
            })
    }

    pub(super) fn native_location(
        &self,
        py: Python<'_>,
        item: &PyStoryItem,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.validate_owned_item(py, item)?;
        let story = self.native_story(py, &item.story)?;
        if item.binding.is_none()
            && item.story.binding.is_none()
            && item.revision != self.revisions.current()
        {
            return Err(crate::stale_to_pyerr(
                py,
                StaleElementError {
                    element_kind: "story item".to_owned(),
                    captured_revision: item.revision,
                    current_revision: self.revisions.current(),
                    recovery_hint: "Causal provenance is unavailable for this explicit numeric selector. Re-fetch it with document.story_items.".to_owned(),
                },
            ));
        }
        Ok(oxoffice_docx::ContentLocation::new(
            story,
            story_item_kind_from_name(&item.kind)?,
            item.index_path.clone(),
        ))
    }

    pub(super) fn body_location(
        &self,
        py: Python<'_>,
        index: usize,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        self.body_locations(py, &[index])?
            .pop()
            .ok_or_else(|| PyIndexError::new_err("content index out of range"))
    }

    pub(super) fn body_locations(
        &self,
        py: Python<'_>,
        indices: &[usize],
    ) -> PyResult<Vec<oxoffice_docx::ContentLocation>> {
        let content_count = self.inner.content_count();
        if indices.iter().any(|index| *index > content_count) {
            return Err(PyIndexError::new_err("content index out of range"));
        }
        let story = self.body_story(py)?;
        let mut locations = Vec::with_capacity(indices.len());
        for index in indices {
            if *index == content_count {
                locations.push(oxoffice_docx::ContentLocation::end(story.clone()));
                continue;
            }
            let location = self
                .inner
                .direct_body_item_location(*index)
                .map_err(|error| docx_to_pyerr(py, error))?
                .filter(|location| location.story() == &story)
                .ok_or_else(|| {
                    docx_to_pyerr(
                        py,
                        oxoffice_docx::Error::Other(format!(
                            "direct body content at index {index} has no checked location"
                        )),
                    )
                })?;
            locations.push(location);
        }
        Ok(locations)
    }

    pub(super) fn comment_story_position(
        &self,
        py: Python<'_>,
        position: &oxoffice_docx::StoryRunPosition,
        items: &mut HashMap<oxoffice_docx::ContentLocation, oxoffice_docx::StoryItemSnapshot>,
        document: &std::sync::Arc<Py<PyDocument>>,
    ) -> PyResult<PyStoryRunPosition> {
        if !items.contains_key(&position.location) {
            let snapshot = self
                .inner
                .story_range_paragraph_snapshot(&position.location)
                .map_err(|error| docx_to_pyerr(py, error))?;
            items.insert(position.location.clone(), snapshot);
        }
        let item = &items[&position.location];
        Ok(PyStoryRunPosition {
            item: PyStoryItem {
                story: self.owned_story_snapshot(item.location().story())?,
                kind: story_item_kind_name(item.location().item_kind()).to_owned(),
                index_path: item.location().index_path().to_vec(),
                direct_body_index: item.direct_body_index(),
                text: item.text().map(str::to_owned),
                xml: item.xml().to_vec(),
                revision: self.revisions.current(),
                binding: Some(self.capture_snapshot(smallvec![])?),
                document: Some(document.clone()),
            },
            run_index: position.run_index,
        })
    }

    fn story_item_snapshot(
        &self,
        py: Python<'_>,
        location: &oxoffice_docx::ContentLocation,
        document: std::sync::Arc<Py<PyDocument>>,
    ) -> PyResult<PyStoryItem> {
        let item = self
            .inner
            .standalone_story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|item| item.location() == location)
            .ok_or_else(|| PyIndexError::new_err("inserted story item was not found"))?;
        Ok(PyStoryItem {
            story: self.owned_story_snapshot(item.location().story())?,
            kind: story_item_kind_name(item.location().item_kind()).to_owned(),
            index_path: item.location().index_path().to_vec(),
            direct_body_index: item.direct_body_index(),
            text: item.text().map(str::to_owned),
            xml: item.xml().to_vec(),
            revision: self.revisions.current(),
            binding: Some(self.capture_snapshot(smallvec![])?),
            document: Some(document),
        })
    }

    /// Snapshot a checked body paragraph, including a paragraph inside a block control.
    pub(super) fn paragraph_story_item(
        py: Python<'_>,
        paragraph: &PyParagraph,
    ) -> PyResult<PyStoryItem> {
        let ParagraphLocation::Body(paragraph_index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "StoryRunPosition does not accept a table cell paragraph handle",
            ));
        };
        let document = paragraph.document.borrow(py);
        let location = document
            .inner
            .paragraph_story_location(paragraph_index)
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyIndexError::new_err("paragraph index out of range"))?;
        let owner = std::sync::Arc::new(paragraph.document.clone_ref(py));
        let [_, _] = location.index_path() else {
            return document.story_item_snapshot(py, &location, owner);
        };
        let item = document
            .inner
            .story_range_paragraph_snapshot(&location)
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyStoryItem {
            story: document.owned_story_snapshot(item.location().story())?,
            kind: story_item_kind_name(item.location().item_kind()).to_owned(),
            index_path: item.location().index_path().to_vec(),
            direct_body_index: item.direct_body_index(),
            text: item.text().map(str::to_owned),
            xml: item.xml().to_vec(),
            revision: document.revisions.current(),
            binding: Some(document.capture_snapshot(smallvec![])?),
            document: Some(owner),
        })
    }
}

pub(super) fn story_kind_name(kind: oxoffice_docx::StoryKind) -> &'static str {
    match kind {
        oxoffice_docx::StoryKind::Body => "body",
        oxoffice_docx::StoryKind::TableCell => "table_cell",
        oxoffice_docx::StoryKind::Header => "header",
        oxoffice_docx::StoryKind::Footer => "footer",
        oxoffice_docx::StoryKind::Footnote => "footnote",
        oxoffice_docx::StoryKind::Endnote => "endnote",
        oxoffice_docx::StoryKind::Comment => "comment",
        oxoffice_docx::StoryKind::TextBox => "text_box",
        _ => "unknown",
    }
}

pub(super) fn story_snapshot(story: &oxoffice_docx::StoryId) -> PyStory {
    PyStory {
        binding: None,
        kind: story_kind_name(story.kind()).to_owned(),
        part_name: story.part_name().to_owned(),
        owner_index: story.owner_index(),
    }
}

pub(super) fn story_item_kind_name(kind: oxoffice_docx::StoryItemKind) -> &'static str {
    match kind {
        oxoffice_docx::StoryItemKind::Paragraph => "paragraph",
        oxoffice_docx::StoryItemKind::Table => "table",
        oxoffice_docx::StoryItemKind::ContentControl => "content_control",
        oxoffice_docx::StoryItemKind::Field => "field",
        oxoffice_docx::StoryItemKind::Drawing => "drawing",
        oxoffice_docx::StoryItemKind::PreservedNode => "preserved_node",
        _ => "unknown",
    }
}

pub(super) fn story_item_kind_from_name(name: &str) -> PyResult<oxoffice_docx::StoryItemKind> {
    match name {
        "paragraph" => Ok(oxoffice_docx::StoryItemKind::Paragraph),
        "table" => Ok(oxoffice_docx::StoryItemKind::Table),
        "content_control" => Ok(oxoffice_docx::StoryItemKind::ContentControl),
        "field" => Ok(oxoffice_docx::StoryItemKind::Field),
        "drawing" => Ok(oxoffice_docx::StoryItemKind::Drawing),
        "preserved_node" => Ok(oxoffice_docx::StoryItemKind::PreservedNode),
        _ => Err(PyValueError::new_err(format!(
            "unsupported story item kind {name:?}"
        ))),
    }
}

pub(super) fn header_footer_kind_from_name(
    name: &str,
) -> PyResult<oxoffice_docx::HeaderFooterKind> {
    match name {
        "header" => Ok(oxoffice_docx::HeaderFooterKind::Header),
        "footer" => Ok(oxoffice_docx::HeaderFooterKind::Footer),
        _ => Err(PyValueError::new_err(format!(
            "kind must be header or footer, not {name:?}"
        ))),
    }
}

pub(super) fn header_footer_variant_from_name(name: &str) -> PyResult<oxoffice_docx::HdrFtrType> {
    // `HdrFtrType::from_str` reads every unknown name as the default variant.
    match name {
        "default" => Ok(oxoffice_docx::HdrFtrType::Default),
        "first" => Ok(oxoffice_docx::HdrFtrType::First),
        "even" => Ok(oxoffice_docx::HdrFtrType::Even),
        _ => Err(PyValueError::new_err(format!(
            "variant must be default, first or even, not {name:?}"
        ))),
    }
}

pub(super) fn section_snapshot(section: oxoffice_docx::SectionRef<'_>) -> PySection {
    let (page_width, page_height) = section.page_size().map_or((None, None), |(width, height)| {
        (Some(width.to_emu()), Some(height.to_emu()))
    });
    let (margin_top, margin_right, margin_bottom, margin_left) =
        section
            .margins()
            .map_or((None, None, None, None), |(top, right, bottom, left)| {
                (
                    Some(top.to_emu()),
                    Some(right.to_emu()),
                    Some(bottom.to_emu()),
                    Some(left.to_emu()),
                )
            });
    let (column_count, column_spacing) =
        section.columns().map_or((None, None), |(count, spacing)| {
            (Some(count), Some(spacing.to_emu()))
        });
    let (header_distance, footer_distance) = section
        .header_footer_distance()
        .map_or((None, None), |(header, footer)| {
            (Some(header.to_emu()), Some(footer.to_emu()))
        });
    PySection {
        ordinal: section.ordinal(),
        is_final: section.is_final(),
        orientation: section.orientation().map(|value| value.to_str().to_owned()),
        page_width,
        page_height,
        margin_top,
        margin_right,
        margin_bottom,
        margin_left,
        gutter: section.gutter().map(oxoffice_docx::Length::to_emu),
        column_count,
        column_spacing,
        page_number_start: section.page_number_start(),
        header_distance,
        footer_distance,
        different_first_page: section.different_first_page(),
        break_type: section.break_type().map(|value| value.to_str().to_owned()),
    }
}

pub(super) fn style_snapshot(style: oxoffice_docx::style::Style<'_>) -> PyStyle {
    PyStyle {
        style_id: style.style_id().to_owned(),
        name: style.name().map(str::to_owned),
        based_on: style.based_on().map(str::to_owned),
        style_type: style.style_type().to_str().to_owned(),
        linked_style: style.linked_style().map(str::to_owned),
        next_style: style.next_style().map(str::to_owned),
        priority: style.priority(),
        auto_redefine: style.auto_redefine(),
        hidden: style.hidden(),
        semi_hidden: style.semi_hidden(),
        unhide_when_used: style.unhide_when_used(),
        quick_format: style.quick_format(),
        locked: style.locked(),
        is_default: style.is_default(),
    }
}

// Native snapshots bind causal state without changing their visible value equality.

impl PyDocument {
    pub(super) fn owned_story_snapshot(&self, story: &oxoffice_docx::StoryId) -> PyResult<PyStory> {
        let mut snapshot = story_snapshot(story);
        snapshot.binding = Some(self.capture_snapshot(smallvec![])?);
        Ok(snapshot)
    }

    pub(super) fn validate_owned_story(&self, py: Python<'_>, story: &PyStory) -> PyResult<()> {
        let Some(path) = &story.binding else {
            return Ok(());
        };
        if !path.belongs_to(&self.revisions) {
            return Err(PyValueError::new_err(
                "story belongs to a different document",
            ));
        }
        path.validate_scopes(
            &self.revisions,
            "story",
            "Re-fetch it with document.stories.",
        )
        .map_err(|error| crate::stale_to_pyerr(py, error))
    }

    pub(super) fn validate_owned_item(&self, py: Python<'_>, item: &PyStoryItem) -> PyResult<()> {
        if let Some(path) = &item.binding {
            if !path.belongs_to(&self.revisions) {
                return Err(PyValueError::new_err(
                    "story item belongs to a different document",
                ));
            }
            path.validate_scopes(
                &self.revisions,
                "story item",
                "Re-fetch it with document.story_items.",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
        }
        self.validate_owned_story(py, &item.story)
    }
}

impl PartialEq for PyStory {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.part_name == other.part_name
            && self.owner_index == other.owner_index
    }
}
impl Eq for PyStory {}
impl PartialEq for PyStoryItem {
    fn eq(&self, other: &Self) -> bool {
        self.story == other.story
            && self.kind == other.kind
            && self.index_path == other.index_path
            && self.direct_body_index == other.direct_body_index
            && self.text == other.text
            && self.xml == other.xml
            && self.revision == other.revision
    }
}
impl Eq for PyStoryItem {}

//! Appending and inserting other documents: their styles, numbering and parts.

use super::*;

fn equivalent_abstract_numbering(left: &CT_AbstractNum, right: &CT_AbstractNum) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.abstract_num_id = 0;
    right.abstract_num_id = 0;
    // Parsed leaf snapshots record prior typed values. Compare the actual
    // owned XML projection, including opaque attributes and children, rather
    // than the preservation bookkeeping used to produce that projection.
    let mut left_xml = quick_xml::Writer::new(Vec::new());
    let mut right_xml = quick_xml::Writer::new(Vec::new());
    left.to_xml(&mut left_xml).is_ok()
        && right.to_xml(&mut right_xml).is_ok()
        && left_xml.into_inner() == right_xml.into_inner()
}

fn equivalent_numbering_instance(left: &CT_Num, right: &CT_Num, abstract_num_id: u32) -> bool {
    if left.abstract_num_id != abstract_num_id {
        return false;
    }
    let mut left = left.clone();
    let mut right = right.clone();
    left.num_id = 0;
    right.num_id = 0;
    left.abstract_num_id = abstract_num_id;
    right.abstract_num_id = abstract_num_id;
    let mut left_xml = quick_xml::Writer::new(Vec::new());
    let mut right_xml = quick_xml::Writer::new(Vec::new());
    left.to_xml(&mut left_xml).is_ok()
        && right.to_xml(&mut right_xml).is_ok()
        && left_xml.into_inner() == right_xml.into_inner()
}

impl Document {
    // ---- Document Merging ----

    /// Append the content of another document to this document.
    ///
    /// Copies all body content (paragraphs and tables) from the other document.
    /// Handles style deduplication and numbering remapping.
    pub fn append(&mut self, other: &Document) {
        let mut candidate = self.clone_for_staging();
        candidate
            .append_document_content(other)
            .expect("document append preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Append the content of another document with a section break.
    pub fn append_with_break(&mut self, other: &Document, break_type: crate::SectionBreak) {
        let mut candidate = self.clone_for_staging();
        candidate.invalidate_layout();
        // Insert a section break paragraph before the merged content
        let mut p = CT_P::new();
        let sect_pr = match break_type {
            crate::SectionBreak::NextPage => CT_SectPr::default_letter(),
            crate::SectionBreak::Continuous => {
                let mut sp = CT_SectPr::default_letter();
                sp.section_type = Some(ST_SectionType::Continuous);
                sp
            }
            crate::SectionBreak::EvenPage => {
                let mut sp = CT_SectPr::default_letter();
                sp.section_type = Some(ST_SectionType::EvenPage);
                sp
            }
            crate::SectionBreak::OddPage => {
                let mut sp = CT_SectPr::default_letter();
                sp.section_type = Some(ST_SectionType::OddPage);
                sp
            }
        };
        p.properties = Some(CT_PPr {
            sect_pr: Some(sect_pr),
            ..Default::default()
        });
        candidate
            .document
            .body
            .content
            .push(BodyContent::Paragraph(Box::new(p)));
        candidate
            .append_document_content(other)
            .expect("document append-with-break preflight failed");
        self.commit_staged_mutation(candidate);
    }

    /// Insert the content of another document at a specified body index.
    ///
    /// An `index` past the end is clamped to the end rather than panicking.
    pub fn insert_document(&mut self, index: usize, other: &Document) {
        if other.content_count() == 0 {
            return;
        }
        let start = other
            .direct_body_item_location(0)
            .expect("document insertion source preflight failed")
            .expect("nonempty source body");
        let end = ContentLocation::end(start.story().clone());
        let fragment = DocumentFragment::from_range(other, &start, &end, false)
            .expect("document insertion source preflight failed");
        let index = index.min(self.content_count());
        let destination = self
            .direct_body_item_location(index)
            .expect("document insertion destination preflight failed")
            .unwrap_or_else(|| {
                ContentLocation::end(
                    self.stories()
                        .expect("document insertion destination stories")
                        .into_iter()
                        .find(|story| story.kind() == StoryKind::Body)
                        .expect("body story"),
                )
            });
        self.import_fragment(
            &destination,
            &fragment,
            FragmentConflictPolicy::for_native_insert_document(None),
        )
        .expect("document insertion preflight failed");
    }

    pub(crate) fn insert_document_fragment_content_staged(
        &mut self,
        index: usize,
        other: &Document,
        reuse_equivalent_numbering: bool,
        source_owned_numbering: bool,
    ) -> Result<HashMap<u32, u32>> {
        self.insert_document_content_staged_with_numbering_policy(
            index,
            other,
            reuse_equivalent_numbering,
            source_owned_numbering,
        )
    }

    fn insert_document_content_staged_with_numbering_policy(
        &mut self,
        index: usize,
        other: &Document,
        reuse_equivalent_numbering: bool,
        source_owned_numbering: bool,
    ) -> Result<HashMap<u32, u32>> {
        self.reserve_merge_bundles(other)?;
        self.invalidate_layout();
        self.merge_styles(other)?;

        let insert_at = index.min(self.document.body.content.len());
        for (i, content) in other.document.body.content.iter().enumerate() {
            self.document
                .body
                .content
                .insert(insert_at + i, content.clone());
        }

        self.remap_merged_numbering(
            other,
            insert_at,
            reuse_equivalent_numbering,
            source_owned_numbering,
        )
    }

    fn append_document_content(&mut self, other: &Document) -> Result<()> {
        self.reserve_merge_bundles(other)?;
        self.invalidate_layout();
        self.merge_styles(other)?;
        let start_idx = self.document.body.content.len();
        self.document
            .body
            .content
            .extend(other.document.body.content.iter().cloned());
        self.remap_merged_numbering(other, start_idx, false, false)
            .map(|_| ())
    }

    fn reserve_merge_bundles(&mut self, other: &Document) -> Result<()> {
        let needs_styles = other
            .styles
            .styles
            .iter()
            .any(|style| self.styles.get_by_id(&style.style_id).is_none());
        let needs_numbering = other.numbering.is_some();
        let additional_relationships = usize::from(
            needs_styles
                && self.document_bundle_relationship_is_missing(
                    self.styles_part_name.as_deref(),
                    rel_types::STYLES,
                ),
        ) + usize::from(
            needs_numbering
                && self.document_bundle_relationship_is_missing(
                    self.numbering_part_name.as_deref(),
                    rel_types::NUMBERING,
                ),
        );
        self.identifiers
            .ensure_relationship_capacity(&self.doc_part_name, additional_relationships)?;
        if needs_styles {
            self.reserve_styles_bundle()?;
        }
        if needs_numbering {
            self.reserve_numbering_bundle()?;
        }
        Ok(())
    }

    fn document_bundle_relationship_is_missing(
        &self,
        part_name: Option<&str>,
        rel_type: &str,
    ) -> bool {
        let Some(part_name) = part_name else {
            return true;
        };
        !self
            .package
            .get_part_rels(&self.doc_part_name)
            .is_some_and(|relationships| {
                relationships.items.iter().any(|relationship| {
                    relationship.rel_type == rel_type
                        && relationship_is_internal(relationship)
                        && OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target)
                            == part_name
                })
            })
    }

    /// Merge styles from another document, avoiding duplicates.
    ///
    /// A defect the destination styles already have is retained. Only a
    /// defect the merged styles introduce rejects the merge.
    fn merge_styles(&mut self, other: &Document) -> Result<()> {
        let source = self.styles.clone();
        for style in &other.styles.styles {
            if self.styles.get_by_id(&style.style_id).is_none() {
                Arc::make_mut(&mut self.styles).styles.push(style.clone());
            }
        }
        style::validate_style_graph_change(&source, &self.styles).map(|_| ())
    }

    // Only the same-document template range path selects this mode. Exact
    // original identities and their effective definitions must still agree.
    pub(crate) fn source_owned_numbering_remap(
        &self,
        other: &Document,
    ) -> Result<HashMap<u32, u32>> {
        let Some(source) = &other.numbering else {
            return Ok(HashMap::new());
        };
        let destination = self.numbering.as_ref().ok_or_else(|| {
            Error::Other("source-owned template numbering part disappeared".into())
        })?;
        for definition in &source.abstract_nums {
            if !destination.abstract_nums.iter().any(|existing| {
                existing.abstract_num_id == definition.abstract_num_id
                    && equivalent_abstract_numbering(existing, definition)
            }) {
                return Err(Error::Other(format!(
                    "source-owned template numbering definition {} changed",
                    definition.abstract_num_id
                )));
            }
        }
        let mut ids = HashMap::new();
        for instance in &source.nums {
            if !destination.nums.iter().any(|existing| {
                existing.num_id == instance.num_id
                    && equivalent_numbering_instance(existing, instance, instance.abstract_num_id)
            }) {
                return Err(Error::Other(format!(
                    "source-owned template numbering instance {} changed",
                    instance.num_id
                )));
            }
            ids.insert(instance.num_id, instance.num_id);
        }
        Ok(ids)
    }

    pub(crate) fn equivalent_numbering_dependency_id(
        &self,
        other: &Document,
        num_id: u32,
    ) -> Option<u32> {
        let (left_numbering, right_numbering) =
            self.numbering.as_ref().zip(other.numbering.as_ref())?;
        let right = right_numbering
            .nums
            .iter()
            .find(|instance| instance.num_id == num_id)?;
        let right_abstract = right_numbering
            .abstract_nums
            .iter()
            .find(|definition| definition.abstract_num_id == right.abstract_num_id)?;
        left_numbering.nums.iter().find_map(|left| {
            let left_abstract = left_numbering
                .abstract_nums
                .iter()
                .find(|definition| definition.abstract_num_id == left.abstract_num_id)?;
            (equivalent_abstract_numbering(left_abstract, right_abstract)
                && equivalent_numbering_instance(left, right, left.abstract_num_id))
            .then_some(left.num_id)
        })
    }

    /// Merge numbering from another document and remap IDs in the merged content.
    /// `start_idx` is the index where the other document's content starts in self.
    fn remap_merged_numbering(
        &mut self,
        other: &Document,
        start_idx: usize,
        reuse_equivalent: bool,
        source_owned_numbering: bool,
    ) -> Result<HashMap<u32, u32>> {
        if source_owned_numbering {
            return self.source_owned_numbering_remap(other);
        }
        let Some(other_numbering) = &other.numbering else {
            return Ok(HashMap::new());
        };
        let existing_numbering = self.numbering.clone();
        let mut abstract_remap = HashMap::new();
        let mut new_abstract_ids = HashSet::new();
        for abs_num in &other_numbering.abstract_nums {
            let new_id = existing_numbering
                .as_ref()
                .filter(|_| reuse_equivalent)
                .and_then(|numbering| {
                    numbering.abstract_nums.iter().find_map(|existing| {
                        equivalent_abstract_numbering(existing, abs_num)
                            .then_some(existing.abstract_num_id)
                    })
                })
                .map(Ok)
                .unwrap_or_else(|| self.identifiers.reserve_abstract_numbering_id())?;
            if !existing_numbering.as_ref().is_some_and(|numbering| {
                numbering
                    .abstract_nums
                    .iter()
                    .any(|existing| existing.abstract_num_id == new_id)
            }) {
                new_abstract_ids.insert(abs_num.abstract_num_id);
            }
            abstract_remap.insert(abs_num.abstract_num_id, new_id);
        }
        let mut num_remap = HashMap::new();
        let mut new_num_ids = HashSet::new();
        for num in &other_numbering.nums {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                abstract_remap.entry(num.abstract_num_id)
            {
                let allocated = self.identifiers.reserve_abstract_numbering_id()?;
                new_abstract_ids.insert(num.abstract_num_id);
                entry.insert(allocated);
            }
            let mapped_abstract = abstract_remap[&num.abstract_num_id];
            let new_id = existing_numbering
                .as_ref()
                .filter(|_| reuse_equivalent)
                .and_then(|numbering| {
                    numbering.nums.iter().find_map(|existing| {
                        equivalent_numbering_instance(existing, num, mapped_abstract)
                            .then_some(existing.num_id)
                    })
                })
                .map(Ok)
                .unwrap_or_else(|| self.identifiers.reserve_numbering_instance_id())?;
            if !existing_numbering.as_ref().is_some_and(|numbering| {
                numbering
                    .nums
                    .iter()
                    .any(|existing| existing.num_id == new_id)
            }) {
                new_num_ids.insert(num.num_id);
            }
            num_remap.insert(num.num_id, new_id);
        }

        for source_style in &other.styles.styles {
            let Some(source_num_id) = source_style
                .ppr
                .as_ref()
                .and_then(|properties| properties.num_id)
            else {
                continue;
            };
            let Some(mapped_num_id) = num_remap.get(&source_num_id).copied() else {
                continue;
            };
            let Some(destination_style) = std::sync::Arc::make_mut(&mut self.styles)
                .styles
                .iter_mut()
                .find(|style| style.style_id == source_style.style_id)
            else {
                continue;
            };
            if destination_style == source_style
                && let Some(properties) = destination_style.ppr.as_mut()
            {
                properties.num_id = Some(mapped_num_id);
                properties.num_id_raw = None;
            }
        }

        let numbering =
            self.numbering
                .get_or_insert_with(|| oxoffice_docx_oxml::numbering::CT_Numbering {
                    abstract_nums: Vec::new(),
                    nums: Vec::new(),
                    root_attributes: Vec::new(),
                    extra_xml: Vec::new(),
                });
        for abs_num in &other_numbering.abstract_nums {
            if !new_abstract_ids.contains(&abs_num.abstract_num_id) {
                continue;
            }
            let mut new_abs = abs_num.clone();
            new_abs.abstract_num_id = abstract_remap[&abs_num.abstract_num_id];
            numbering.abstract_nums.push(new_abs);
        }
        for num in &other_numbering.nums {
            if !new_num_ids.contains(&num.num_id) {
                continue;
            }
            let mut new_num = num.clone();
            new_num.num_id = num_remap[&num.num_id];
            new_num.abstract_num_id = abstract_remap[&num.abstract_num_id];
            new_num.abstract_num_id_raw = None;
            numbering.nums.push(new_num);
        }
        let incoming_count = other.document.body.content.len();
        visit_body_paragraphs_mut(
            &mut self.document.body.content[start_idx..start_idx + incoming_count],
            &mut |paragraph| {
                if let Some(num_id) = paragraph
                    .properties
                    .as_mut()
                    .and_then(|properties| properties.num_id.as_mut())
                    && let Some(updated) = num_remap.get(num_id)
                {
                    *num_id = *updated;
                }
            },
        );
        Ok(num_remap)
    }
}

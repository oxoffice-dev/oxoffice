//! The serialized main part, its canonical source and its blocks.

use super::*;

impl Document {
    /// Compare current writer bytes without constructing an owned result when
    /// the existing writer memo proves every current child and the full frame.
    pub(super) fn serialized_main_part_matches(&self, expected: &[u8]) -> Result<bool> {
        let scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(matched) = scans
            .serialized
            .as_ref()
            .and_then(|record| record.writer_bytes_match(&self.document, expected))
        {
            return Ok(matched);
        }
        drop(scans);
        self.serialized_main_part().map(|(xml, _)| xml == expected)
    }

    /// Current writer bytes and pieces, without claiming a flushed part.
    pub(super) fn serialized_main_part(&self) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        self.serialized_main_part_in(None)
    }

    pub(super) fn serialized_main_part_in(
        &self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        self.document.validate_resource_limits()?;
        let mut scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(record) = scans.serialized.as_ref()
            && let Some(kept) = self.kept_main_part()
            && let Some(expected) = kept.serialization_proof.as_deref()
            && record.writer_bytes_match(&self.document, expected) == Some(true)
        {
            // These are an opening's normalized writer bytes, not its raw
            // source. Requalify the complete current writer before copying.
            let xml = if let Some(template) = &mut template
                && let Some(records) = template.lineage.records
                && let Some(source) = records.source_for_part(template.operation, expected)?
            {
                let copied = template.operation.repeat_source(&source)?;
                let bytes = template.operation.finish_source(copied)?;
                template.sources.source = Some(Arc::new(source));
                bytes
            } else {
                if let Some(template) = &mut template {
                    template.sources.source = None;
                }
                expected.to_vec()
            };
            let mut record = record.clone();
            record.part = None;
            drop(scans);
            return Ok((xml, Some(record)));
        }
        let (xml, record) = MainPartBlocks::serialize_in(
            &self.document,
            &self.document.body.content,
            scans.serialized.as_ref().or(self.main_part_blocks.as_ref()),
            template,
        )?;
        scans.serialized = (!record.head.is_empty()).then(|| record.clone());
        drop(scans);
        Ok((xml, (!record.head.is_empty()).then_some(record)))
    }

    /// Temporary main-part bytes, as the save would write them. The returned record
    /// proves these bytes, without replacing the package or flushed record.
    pub(super) fn canonical_source(&self) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        self.canonical_source_in(None)
    }

    pub(super) fn canonical_source_in(
        &self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        let (xml, mut record) = self.serialized_main_part_in(template.as_deref_mut())?;
        if let Some(record) = &mut record {
            let part = if let Some(template) = &mut template
                && let Some(source) = &template.sources.source
            {
                let copied = template.operation.repeat_source(source)?;
                template.operation.finish_source(copied)?
            } else {
                xml.clone()
            };
            record.part = Some(Arc::new(part));
        }
        Ok((xml, record))
    }

    pub(super) fn prepared_main_read(&self) -> Result<PreparedMainRead<'_>> {
        let (xml, writer) = self.canonical_source()?;
        let (xml, content, children) = match writer.as_ref() {
            Some(record) => {
                let (content, children) = record.body_spans()?;
                (xml, content, children)
            }
            None => {
                let (xml, content, children) = self.document.to_xml_with_body_spans()?;
                (xml, content, children)
            }
        };
        Ok(PreparedMainRead {
            document: self,
            xml,
            writer,
            content,
            children,
        })
    }

    #[allow(clippy::type_complexity)]
    pub(crate) fn main_part_with_body_spans(
        &self,
    ) -> Result<(Vec<u8>, Range<usize>, Vec<Range<usize>>)> {
        let (xml, record) = self.serialized_main_part()?;
        if let Some(record) = record {
            let (content, children) = record.body_spans()?;
            let start = content.start;
            let end = content.end;
            #[cfg(debug_assertions)]
            {
                // The canonical spans must point at the exact canonical
                // blocks. This holds for every child, including a direct
                // paragraph whose root keeps a foreign `w` binding.
                for (range, block) in children.iter().zip(record.blocks.iter()) {
                    assert_eq!(
                        xml.get(range.clone()),
                        Some(block.bytes.as_slice()),
                        "serialized pieces changed the body spans"
                    );
                }
                // `to_xml_with_body_spans` is a public call and stays
                // Publication. The canonical Source writer emits a direct
                // paragraph with a root-local foreign `w` binding
                // differently, so compare the public bytes of every other
                // child against its canonical block.
                if let Ok((public, _, public_children)) = self.document.to_xml_with_body_spans() {
                    assert_eq!(
                        public_children.len(),
                        children.len(),
                        "serialized pieces changed the body spans"
                    );
                    for (public_range, block) in public_children.iter().zip(record.blocks.iter()) {
                        if !root_declares_foreign_word(&block.bytes) {
                            assert_eq!(
                                public.get(public_range.clone()),
                                Some(block.bytes.as_slice()),
                                "serialized pieces changed the body spans"
                            );
                        }
                    }
                }
            }
            return Ok((xml, start..end, children));
        }
        self.document.to_xml_with_body_spans().map_err(Error::from)
    }

    /// Reconstruct a source edit from actual parsed child facts. A changed
    /// frame, an unclosed element or an unproved child declines and
    /// leaves the whole parser authoritative.
    pub(super) fn source_blocks(
        &self,
        xml: &Arc<Vec<u8>>,
    ) -> Option<(CT_Document, MainPartBlocks)> {
        if let Some(record) = &self.main_part_blocks
            && let Some(current) = &record.part
            && self.package.get_part(&self.doc_part_name) == Some(current.as_slice())
            && let Some(found) = self.source_blocks_in(current, record.clone(), xml)
        {
            return Some(found);
        }
        let (mut current, record) = self.canonical_source().ok()?;
        let record = record?;
        if let Some(found) = self.source_blocks_in(&current, record.clone(), xml) {
            return Some(found);
        }
        if let Some(kept) = self.kept_main_part() {
            let source = kept.main_part_xml()?;
            if let Some(producer) = kept
                .source_blocks
                .get_or_init(|| self.producer_source_blocks(source, &record))
                && let Some(found) = self.source_blocks_in(source, producer.clone(), xml)
            {
                return Some(found);
            }
        }
        // Bootstrap a producer's body-leading whitespace under the exact
        // canonical root and body start. Root aliases and other frame changes
        // still require the whole reader. The empty frame is actually parsed
        // and its normalized output must match the proved canonical shell.
        if !xml.starts_with(&record.head) {
            return None;
        }
        let after_head = xml.get(record.head.len()..)?;
        let whitespace = after_head
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
            .count();
        if whitespace == 0 {
            return None;
        }
        let gap = after_head.get(..whitespace)?;
        let mut head = record.head.clone();
        head.extend_from_slice(gap);
        let shell =
            CT_Document::from_xml(&[head.as_slice(), record.tail.as_slice()].concat()).ok()?;
        let canonical_shell = record.frame.get()?.as_ref()?;
        if !shell.body.content.is_empty()
            || shell.to_xml().ok()? != canonical_shell.to_xml().ok()?
        {
            return None;
        }
        current.splice(record.head.len()..record.head.len(), gap.iter().copied());
        let retained = MainPartBlocks {
            head,
            root: record.root,
            tail: record.tail,
            blocks: Arc::new(
                record
                    .blocks
                    .iter()
                    .map(BodyBlockScan::fresh_context_piece)
                    .collect(),
            ),
            part: Some(Arc::new(current.clone())),
            frame: OnceLock::from(Some(shell)),
            nested_namespace_owners: OnceLock::new(),
            identifier_frame: OnceLock::new(),
            canonical_frame: OnceLock::new(),
            namespace_frame: OnceLock::new(),
            namespace_frame_spans: OnceLock::new(),
        };
        self.source_blocks_in(&current, retained, xml)
    }

    /// Partition a producer frame using physical owner ranges and actual
    /// one-child reads. These facts do not certify canonical writer bytes.
    fn producer_source_blocks(
        &self,
        source: &[u8],
        normal: &MainPartBlocks,
    ) -> Option<MainPartBlocks> {
        let owners = scan_story_owners(source, StoryKind::Body).ok()?;
        let body = single_frame_body(&owners, None).ok()?;
        let body_start = source.get(body.full.start..)?;
        if !body_start.starts_with(b"<w:body>") && !body_start.starts_with(b"<w:body ") {
            return None;
        }
        let items = direct_story_content_items(source, body).ok()?;
        let first = items.first()?.full.start;
        let end = items.last()?.full.end;
        let head = source.get(..first)?.to_vec();
        let tail = source.get(end..)?.to_vec();
        let mut root = root_start_tag(&head)?;
        root.start += head
            .get(root.clone())?
            .iter()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        let tag = head.get(root.clone())?;
        if !tag.starts_with(b"<w:document ") {
            return None;
        }
        let frame = CT_Document::from_xml(&[head.as_slice(), tail.as_slice()].concat()).ok()?;
        if !frame.body.content.is_empty()
            || frame.to_xml().ok()? != normal.frame.get()?.as_ref()?.to_xml().ok()?
        {
            return None;
        }
        let scopes = document_namespace_scopes(source).ok()?;
        if !scopes.body_declarations.is_empty()
            || scopes
                .root_declarations
                .iter()
                .any(|(name, _)| name == "xmlns")
        {
            return None;
        }
        let mut blocks = Vec::with_capacity(items.len());
        let mut at = first;
        for item in items {
            if !matches!(
                item.kind,
                StoryItemKind::Paragraph | StoryItemKind::Table | StoryItemKind::ContentControl
            ) || item.full.start < at
                || !source
                    .get(at..item.full.start)?
                    .iter()
                    .all(u8::is_ascii_whitespace)
            {
                return None;
            }
            let mut block = BodyBlockScan::from_source(source.get(at..item.full.end)?.to_vec());
            let child = block.read_source_child(&head, &tail, &scopes)?;
            if block.holds_field_words || !child.owner_facts_reusable_by_value() {
                return None;
            }
            // The proof above is an actual read under the producer scope.
            // Canonical serialization remains a separate authority.
            block.standalone_value = OnceLock::new();
            block.staged = None;
            blocks.push(Arc::new(block));
            at = item.full.end;
        }
        let record = MainPartBlocks {
            head,
            root,
            tail,
            blocks: Arc::new(blocks),
            part: Some(Arc::new(source.to_vec())),
            frame: OnceLock::from(Some(frame)),
            nested_namespace_owners: OnceLock::new(),
            identifier_frame: OnceLock::new(),
            canonical_frame: OnceLock::new(),
            namespace_frame: OnceLock::new(),
            namespace_frame_spans: OnceLock::new(),
        };
        // Complete actual source equality is read authority only.
        let source_matches = record.proved_document_matches(source, &self.document)?;
        if record.blocks.len() != normal.blocks.len() {
            return None;
        }
        // Preserve the original fallible frame/resource preflight before either route.
        let context = scoped_normalization_context(&self.document, &record, normal)?;
        if source_matches {
            let lexical_and_private = record
                .blocks
                .iter()
                .zip(normal.blocks.iter())
                .zip(&self.document.body.content)
                .all(|((left, right), item)| {
                    scoped_normalization_reuse_allowed(&record, normal, left, right, item)
                });
            if lexical_and_private && let Some(actual_normal_part) = &normal.part {
                let writer_matches = normal.writer_bytes_match(&self.document, actual_normal_part);
                if writer_matches == Some(true) {
                    // No scoped correspondence memo is created by this different proof.
                    return Some(record);
                }
            }
        }
        for children in record.blocks.iter().zip(normal.blocks.iter()) {
            let seed = producer_scoped_normalization_seed(
                &record, normal, children.0, children.1, &scopes,
            );
            if !scoped_children_correspond(
                &self.document,
                &record,
                normal,
                children,
                &context,
                None,
                seed,
            ) {
                return None;
            }
        }
        Some(record)
    }

    /// Rebuild one exact source partition. Kept children supply actual parse
    /// facts, and replacing children are read under this same source frame.
    pub(super) fn source_blocks_in(
        &self,
        current: &[u8],
        mut record: MainPartBlocks,
        xml: &Arc<Vec<u8>>,
    ) -> Option<(CT_Document, MainPartBlocks)> {
        let mut parsed = record.proved_document(current)?;
        let head = record.head.len();
        let content_end = current.len().checked_sub(record.tail.len())?;
        let new_content_end = xml.len().checked_sub(record.tail.len())?;
        let same = current
            .iter()
            .zip(xml.iter())
            .take_while(|(a, b)| a == b)
            .count()
            .min(content_end)
            .min(new_content_end);
        if same < head {
            return None;
        }
        let suffix = current
            .get(same..)?
            .iter()
            .rev()
            .zip(xml.get(same..)?.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if suffix < record.tail.len() {
            return None;
        }
        let mut boundaries = Vec::with_capacity(record.blocks.len() + 1);
        boundaries.push(head);
        for block in record.blocks.iter() {
            boundaries.push(boundaries.last()?.checked_add(block.bytes.len())?);
        }
        if boundaries.last().copied()? != content_end {
            return None;
        }
        let first = boundaries[1..].partition_point(|end| *end <= same);
        let changed_end = current.len().checked_sub(suffix)?;
        let last = boundaries[..record.blocks.len()]
            .partition_point(|start| *start < changed_end)
            .max(first);
        let start = *boundaries.get(first)?;
        let end = *boundaries.get(last)?;
        let new_end = xml.len().checked_sub(current.len().checked_sub(end)?)?;
        let replacing = xml.get(start..new_end)?;
        let scopes = document_namespace_scopes(current).ok()?;
        let mut reader = oxoffice_core::xml_reader::Reader::from_reader(replacing);
        let mut buffer = Vec::new();
        let mut depth = 0usize;
        let mut element = 0usize;
        let mut ranges = Vec::new();
        loop {
            let at = reader.buffer_position() as usize;
            let event = reader.read_event_into(&mut buffer).ok()?;
            let after = reader.buffer_position() as usize;
            match event {
                Event::Eof if depth == 0 => break,
                Event::Eof => return None,
                Event::Start(_) => {
                    if depth == 0 {
                        element = at;
                    }
                    depth = depth.checked_add(1)?;
                }
                Event::End(_) => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        ranges.push(element..after);
                    }
                }
                Event::Empty(_) if depth == 0 => ranges.push(at..after),
                _ if depth == 0 => return None,
                _ => {}
            }
            buffer.clear();
        }
        let mut items = Vec::with_capacity(ranges.len());
        let mut replacement_blocks = Vec::with_capacity(ranges.len());
        for range in ranges {
            let bytes = replacing.get(range)?.to_vec();
            let mut block = BodyBlockScan::from_source(bytes);
            let item = block.read_source_child(&record.head, &record.tail, &scopes)?;
            items.push(item);
            replacement_blocks.push(Arc::new(block));
        }
        parsed.body.content.splice(first..last, items);
        parsed.validate_resource_limits().ok()?;
        Arc::make_mut(&mut record.blocks).splice(first..last, replacement_blocks);
        record.part = Some(Arc::clone(xml));
        record.nested_namespace_owners = OnceLock::new();
        if !record.part_partition_matches(xml) {
            return None;
        }
        Some((parsed, record))
    }

    pub(super) fn package_main_part_blocks(&self) -> Option<MainPartBlocks> {
        let source = self.package.get_part(&self.doc_part_name)?;
        if let Some(record) = &self.main_part_blocks
            && record
                .part
                .as_deref()
                .is_some_and(|bytes| bytes.as_slice() == source)
        {
            return Some(record.clone());
        }
        self.source_blocks(&Arc::new(source.to_vec()))
            .map(|(_, record)| record)
    }

    pub(super) fn main_part_parses_to_model(&self, kept: Option<&Parsed>) -> bool {
        let Some(source) = self.package.get_part(&self.doc_part_name) else {
            return false;
        };
        if let Some(parsed) = self
            .main_part_blocks
            .as_ref()
            .and_then(|record| record.proved_document_matches(source, &self.document))
        {
            return parsed;
        }
        if let Some(parsed) = kept
            .filter(|kept| {
                kept.main_part == self.doc_part_name && kept.main_part_xml() == Some(source)
            })
            .and_then(Parsed::main_document)
        {
            return parsed == &*self.document;
        }
        CT_Document::from_xml(source).ok().as_ref() == Some(&*self.document)
    }

    /// Same temporary record for a document that is not `self.document`.
    /// `kept` supplies neighbor bytes only. It is not stored as history.
    pub(super) fn canonical_source_from(
        document: &CT_Document,
        kept: Option<&MainPartBlocks>,
    ) -> Result<(Vec<u8>, Option<MainPartBlocks>)> {
        document.validate_resource_limits()?;
        let (xml, mut record) = MainPartBlocks::serialize(document, &document.body.content, kept)?;
        record.part = Some(Arc::new(xml.clone()));
        Ok((xml, (!record.head.is_empty()).then_some(record)))
    }

    /// The main part bytes from the current block record, or the whole writer
    /// when the frame or a child cannot be proved. A writer error is returned.
    /// It is not an empty part.
    pub(super) fn main_part_xml(&mut self) -> Result<Vec<u8>> {
        self.main_part_xml_in(None)
    }

    pub(super) fn main_part_xml_in(
        &mut self,
        template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<Vec<u8>> {
        let (xml, record) = self.canonical_source_in(template)?;
        if let Some(record) = &record {
            let _ = record.proved_document_matches(&xml, &self.document);
        }
        self.main_part_blocks = record;
        Ok(xml)
    }

    pub(super) fn install_carried_blocks(&mut self, carried: Option<MainPartBlocks>) {
        let Some(carried) = carried else {
            return;
        };
        let Some(part) = carried.part.as_deref() else {
            return;
        };
        let Some(current) = self.package.get_part(&self.doc_part_name) else {
            return;
        };
        if current != part.as_slice() {
            return;
        }
        if self.document.validate_resource_limits().is_ok()
            && self
                .document
                .body
                .content
                .iter()
                .all(BodyContent::owner_facts_reusable_by_value)
            && carried.blocks.iter().all(|block| !block.reads_anew())
            && carried.proved_document_matches(current, &self.document) == Some(true)
            && self.opened_main_still_serializes()
        {
            self.main_part_blocks = Some(carried);
            return;
        }
        let Ok(xml) = self.document.to_xml() else {
            return;
        };
        // A retained source child may normalize on write. Its actual parse
        // must normalize to this reopened model before its read facts carry.
        let Some(parsed) = carried.proved_document(current) else {
            return;
        };
        if parsed.to_xml().ok().as_ref() != Some(&xml) {
            return;
        }
        self.main_part_blocks = Some(carried);
    }
}

impl Document {
    /// The last opening, while the main part still holds those bytes.
    pub(super) fn kept_main_part(&self) -> Option<Arc<Parsed>> {
        let parsed = self.parsed.as_ref()?;
        let kept = parsed.main_part_xml()?;
        let current = self.package.get_part(&self.doc_part_name)?;
        (parsed.main_part == self.doc_part_name && (std::ptr::eq(kept, current) || kept == current))
            .then(|| Arc::clone(parsed))
    }
}

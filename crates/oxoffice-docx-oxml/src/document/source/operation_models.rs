//! Scoped operations that source model items: body items, documents, notes and comments.

use super::*;

impl<'a> ScopedWordOperation<'a> {
    /// Emit this real body child through the same ancestor-aware writer.
    pub fn source_model_body_item(
        &mut self,
        frame: &CT_Document,
        item: &BodyContent,
        ancestors: &[&[(String, String)]],
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        let owner = match item {
            BodyContent::Paragraph(value) => Some(WordOwner::Paragraph(value)),
            BodyContent::Table(value) => Some(WordOwner::Table(value)),
            BodyContent::ContentControl(value) => Some(WordOwner::Control(value)),
            BodyContent::RawXml(_) => None,
        };
        if let Some(owner) = owner {
            admit_word_model(owner)?;
        }
        let field_origins =
            self.template_model_field_origins(owner.into_iter().collect(), field_origins);
        self.register_template_model_fields(owner.into_iter().collect(), &field_origins)?;
        let (output, lease) = self.field_work.template_word_source_output(
            text_origins,
            &field_origins,
            raw_origins,
        )?;
        let mut writer = Writer::new(output);
        frame.write_body_item_in(item, ancestors, &mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let admitted = admit_word_source(&bytes, &[], None, None)?;
        Ok(ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: Some(admitted.constructors),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    /// Emit the actual full-document fallback through its existing part policy.
    pub fn source_model_document(
        &mut self,
        document: &CT_Document,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        document.validate_resource_limits()?;
        let field_origins =
            self.template_model_field_origins(vec![WordOwner::Body(&document.body)], field_origins);
        self.register_template_model_fields(vec![WordOwner::Body(&document.body)], &field_origins)?;
        let (output, lease) = self.field_work.template_word_source_output(
            text_origins,
            &field_origins,
            raw_origins,
        )?;
        let mut writer = Writer::new(output);
        document.write_part_contents(
            &mut writer,
            PartWrite::Whole,
            &mut |_| {},
            Some(lease.work),
        )?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            let admitted = admit_word_source(&source.bytes, &[], None, None)?;
            source.constructors = Some(admitted.constructors);
        }
        Ok(source)
    }

    /// Emit the actual document paired with this operation's successful reader.
    #[doc(hidden)]
    pub fn source_model_document_from_read(
        &mut self,
        document: &CT_Document,
        source: &ScopedWordSource,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        document.validate_resource_limits()?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let reads = reads
            .as_ref()
            .ok_or_else(|| OxmlError::InvalidValue("source has no successful model read".into()))?;
        let mut texts = Vec::new();
        let mut raws = Vec::new();
        for (index, maps) in &reads.body {
            let item = document
                .body
                .content
                .get(*index)
                .ok_or_else(|| OxmlError::InvalidValue("returned body path changed".into()))?;
            if let Some(owner) = returned_body_owner(item) {
                let (child_texts, child_raws) = returned_block_inputs(owner, maps)?;
                texts.extend(child_texts);
                raws.extend(child_raws);
            }
        }
        let fields = self.template_model_field_origins(vec![WordOwner::Body(&document.body)], &[]);
        self.register_template_model_fields(vec![WordOwner::Body(&document.body)], &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        document.write_part_contents(
            &mut writer,
            PartWrite::Whole,
            &mut |_| {},
            Some(lease.work),
        )?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            source.constructors =
                Some(admit_word_source(&source.bytes, &[], None, None)?.constructors);
        }
        Ok(source)
    }

    /// Emit an actual selected item paired with the reader's physical body slot.
    #[doc(hidden)]
    pub fn source_model_body_item_from_read(
        &mut self,
        frame: &CT_Document,
        item: &BodyContent,
        source: &ScopedWordSource,
        body_index: usize,
        ancestors: &[&[(String, String)]],
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let reads = reads
            .as_ref()
            .ok_or_else(|| OxmlError::InvalidValue("source has no successful model read".into()))?;
        let owner = returned_body_owner(item);
        if let Some(owner) = owner {
            admit_word_model(owner)?;
        }
        let (texts, raws) = if let (Some(owner), Some((_, maps))) = (
            owner,
            reads.body.iter().find(|(index, _)| *index == body_index),
        ) {
            returned_block_inputs(owner, maps)?
        } else {
            (Vec::new(), Vec::new())
        };
        let fields = self.template_model_field_origins(owner.into_iter().collect(), &[]);
        self.register_template_model_fields(owner.into_iter().collect(), &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        frame.write_body_item_in(item, ancestors, &mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let admitted = admit_word_source(&bytes, &[], None, None)?;
        Ok(ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: Some(admitted.constructors),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    /// Emit the actual related-note family through its existing root writer.
    pub fn source_model_notes(
        &mut self,
        notes: &crate::footnotes::CT_Footnotes,
        endnotes: bool,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
        prepare_drawing_namespaces: bool,
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        let owners = notes
            .footnotes
            .iter()
            .flat_map(|note| note.paragraphs.iter())
            .map(WordOwner::Paragraph)
            .collect::<Vec<_>>();
        for owner in &owners {
            admit_word_model(*owner)?;
        }
        let field_origins = self.template_model_field_origins(owners.clone(), field_origins);
        self.register_template_model_fields(owners, &field_origins)?;
        let (output, lease) = self.field_work.template_word_source_output(
            text_origins,
            &field_origins,
            raw_origins,
        )?;
        let mut writer = Writer::new(output);
        let (root, item) = if endnotes {
            ("w:endnotes", "w:endnote")
        } else {
            ("w:footnotes", "w:footnote")
        };
        if prepare_drawing_namespaces {
            lease.work.prepare_note_drawing_bindings();
        }
        notes.write_xml_root_in(&mut writer, root, item, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            source.constructors =
                Some(admit_word_source(&source.bytes, &[], None, None)?.constructors);
        }
        Ok(source)
    }

    /// Emit the existing whole-comments writer with actual frozen clone facts.
    #[doc(hidden)]
    pub fn source_model_comments(
        &mut self,
        comments: &crate::comments::CT_Comments,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        comments.validate_writer()?;
        let owners = comments
            .comments
            .iter()
            .flat_map(|comment| &comment.paragraphs)
            .map(WordOwner::Paragraph)
            .collect::<Vec<_>>();
        let fields = self.template_model_field_origins(owners.clone(), field_origins);
        self.register_template_model_fields(owners, &fields)?;
        let (output, lease) =
            self.field_work
                .template_word_source_output(text_origins, &fields, raw_origins)?;
        let mut writer = Writer::new(output);
        lease.work.prepare_note_drawing_bindings();
        comments.write_xml_in(&mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        self.declare_source_w14_on_root(source)
    }

    /// Parse the same successful comments factory and retain its actual owners.
    #[doc(hidden)]
    pub fn source_comments(
        &mut self,
        source: &ScopedWordSource,
    ) -> Result<crate::comments::CT_Comments> {
        self.check_source(source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?
            .with_publication_collector(&source.publication);
        let admitted = admit_reader_source(
            &source.bytes,
            WordSourceReader::Part(PartReaderKind::Comments),
        )?
        .with_source_map(map)?;
        let mut controls = admitted.controls_for(&source.bytes).with_field_origins(
            std::sync::Arc::clone(&self.field_origins),
            source.initial_delivery,
        );
        let mut facts = crate::comments::CommentsReadFacts::default();
        let comments = crate::comments::CT_Comments::from_xml_with_read_output(
            &source.bytes,
            &mut controls,
            Some(&mut facts),
        )?;
        for comment in &comments.comments {
            for paragraph in &comment.paragraphs {
                self.field_origins
                    .lock()
                    .expect("field origin registry poisoned")
                    .commit_publication_owner(
                        WordOwner::Paragraph(paragraph),
                        &admitted.source_map,
                    );
            }
        }
        *source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned") = Some(ReturnedWordRead {
            comments: Some(facts),
            ..Default::default()
        });
        Ok(comments)
    }

    /// Emit the comments model paired with this operation's successful reader.
    #[doc(hidden)]
    pub fn source_model_comments_from_read(
        &mut self,
        comments: &crate::comments::CT_Comments,
        source: &ScopedWordSource,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let reads = reads
            .as_ref()
            .and_then(|reads| reads.comments.as_ref())
            .ok_or_else(|| {
                OxmlError::InvalidValue("source has no successful comments read".into())
            })?;
        comments.validate_writer()?;
        let mut texts = Vec::new();
        let mut raws = Vec::new();
        let mut owners = Vec::new();
        for ((_, raw), map) in comments.extra_xml.iter().zip(&reads.raws) {
            if let Some(map) = map
                && map.len() == raw.len()
            {
                raws.push((raw.as_slice(), map.clone()));
            }
        }
        for (index, comment) in comments.comments.iter().enumerate() {
            owners.extend(comment.paragraphs.iter().map(WordOwner::Paragraph));
            if let Some(read) = reads.comments.get(index) {
                for ((_, raw), map) in comment.extra_xml.iter().zip(&read.raws) {
                    if let Some(map) = map
                        && map.len() == raw.len()
                    {
                        raws.push((raw.as_slice(), map.clone()));
                    }
                }
                for (index, maps) in &read.paragraphs {
                    let paragraph = comment.paragraphs.get(*index).ok_or_else(|| {
                        OxmlError::InvalidValue("returned comment paragraph path changed".into())
                    })?;
                    let (child_texts, child_raws) =
                        returned_block_inputs(WordOwner::Paragraph(paragraph), maps)?;
                    texts.extend(child_texts);
                    raws.extend(child_raws);
                }
            }
        }
        let fields = self.template_model_field_origins(owners.clone(), &[]);
        self.register_template_model_fields(owners, &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        lease.work.prepare_note_drawing_bindings();
        comments.write_xml_in(&mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        self.declare_source_w14_on_root(source)
    }
}

impl<'a> ScopedWordOperation<'a> {
    /// Emit exactly the selected note from its actual reader vector position.
    #[doc(hidden)]
    pub fn source_model_note_from_read(
        &mut self,
        note: &crate::footnotes::CT_Footnote,
        endnotes: bool,
        source: &ScopedWordSource,
        note_index: usize,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let paragraphs = reads
            .as_ref()
            .and_then(|reads| reads.notes.get(note_index))
            .ok_or_else(|| OxmlError::InvalidValue("source has no selected note read".into()))?;
        let mut texts = Vec::new();
        let mut raws = Vec::new();
        let owners = note
            .paragraphs
            .iter()
            .map(WordOwner::Paragraph)
            .collect::<Vec<_>>();
        for owner in &owners {
            admit_word_model(*owner)?;
        }
        for (index, maps) in paragraphs {
            let paragraph = note.paragraphs.get(*index).ok_or_else(|| {
                OxmlError::InvalidValue("returned selected note path changed".into())
            })?;
            let (child_texts, child_raws) =
                returned_block_inputs(WordOwner::Paragraph(paragraph), maps)?;
            texts.extend(child_texts);
            raws.extend(child_raws);
        }
        let fields = self.template_model_field_origins(owners.clone(), &[]);
        self.register_template_model_fields(owners, &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        let (root, item) = if endnotes {
            ("w:endnotes", "w:endnote")
        } else {
            ("w:footnotes", "w:footnote")
        };
        crate::footnotes::CT_Footnotes::write_notes_root_in(
            &[note],
            &mut writer,
            root,
            item,
            Some(lease.work),
        )?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            source.constructors =
                Some(admit_word_source(&source.bytes, &[], None, None)?.constructors);
        }
        Ok(source)
    }
}

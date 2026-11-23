//! Reading a paragraph: CT_P from XML, fragments, word inputs and read outputs.

use super::*;

#[allow(non_snake_case)]
impl CT_P {
    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_word_input(reader, None, None, None, None)
    }

    /// The legacy consumed-opening paragraph role with its actual same-capture
    /// companion. Root declarations remain intentionally outside this role.
    pub(crate) fn from_xml_with_word_input(
        reader: &mut Reader<&[u8]>,
        input: Option<(&crate::document::WordFieldInput, &[u8])>,
        extra_maps: Option<&mut WordExtraSourceMaps>,
        text_facts: Option<&mut WordParagraphTextFacts>,
        nested_maps: Option<&mut WordBlockSourceMaps>,
    ) -> Result<Self> {
        let admitted = crate::document::admit_word_reader(reader, &["w".to_owned()], b"p", None)?;
        let mut controls = admitted.controls();
        if let Some((input, bytes)) = input {
            let (map, registry, initial) = input.projection_parts();
            controls = controls
                .with_field_origins(registry, initial)
                .bind_capture(bytes, Arc::new(map.clone()));
        }
        Self::from_xml_with_read_output(
            reader,
            &[
                "w".to_string(),
                format!("\0r\0{R_NS}"),
                format!("\0mc\0{}", crate::namespace::MC_NS),
            ],
            None,
            Some(&mut controls),
            WordParagraphReadOutput {
                extra_maps,
                text_facts,
                nested_maps,
                ..Default::default()
            },
        )
    }

    /// Parse a standalone paragraph fragment with declarations on its root.
    #[doc(hidden)]
    pub fn from_xml_fragment(xml: &[u8]) -> Result<Self> {
        let admitted = crate::document::admit_reader_source(
            xml,
            crate::document::WordSourceReader::StandaloneParagraph,
        )?;
        let mut controls = admitted.controls_for(xml);
        controls.request_standalone_selectors();
        Self::from_xml_fragment_admitted(xml, Some(&mut controls), None, None)
    }

    pub(crate) fn from_xml_fragment_admitted(
        xml: &[u8],
        controls: Option<&mut crate::document::ControlSourceCursor<'_>>,
        extra_maps: Option<&mut WordExtraSourceMaps>,
        text_facts: Option<&mut WordParagraphTextFacts>,
    ) -> Result<Self> {
        Self::from_xml_fragment_with_properties(xml, controls, extra_maps, text_facts, None, None)
    }

    pub(crate) fn from_xml_fragment_with_properties(
        xml: &[u8],
        mut controls: Option<&mut crate::document::ControlSourceCursor<'_>>,
        mut extra_maps: Option<&mut WordExtraSourceMaps>,
        mut text_facts: Option<&mut WordParagraphTextFacts>,
        mut ordinary: Option<&mut Vec<crate::document::WordOrdinaryFact>>,
        nested_maps: Option<&mut WordBlockSourceMaps>,
    ) -> Result<Self> {
        if let Some(output) = &mut ordinary {
            output.clear();
        }
        if let Some(output) = &mut text_facts {
            output.clear();
        }
        if let Some(output) = &mut extra_maps {
            output.clear();
        }
        validate_word_owner_fragment(xml, b"p", false)?;
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buffer)? {
                Event::Start(element) => {
                    let prefixes = word_prefixes_at(&element, &[])?;
                    if !is_word_element(element.name().as_ref(), b"p", &prefixes) {
                        return Err(OxmlError::MissingElement("w:p root".to_owned()));
                    }
                    if let Some(controls) = controls.as_deref_mut() {
                        controls.confirm_standalone_paragraph(
                            &reader,
                            event_start..reader.buffer_position(),
                        );
                    }
                    return Self::from_xml_with_read_output(
                        &mut reader,
                        &prefixes,
                        Some(&element),
                        controls.as_deref_mut(),
                        WordParagraphReadOutput {
                            extra_maps,
                            text_facts,
                            ordinary,
                            nested_maps,
                            ..Default::default()
                        },
                    );
                }
                Event::Empty(element) => {
                    let prefixes = word_prefixes_at(&element, &[])?;
                    if is_word_element(element.name().as_ref(), b"p", &prefixes) {
                        let paragraph = Self::from_empty_root(&element, &prefixes)?;
                        if let Some(output) = extra_maps {
                            output.resize(paragraph.extra_xml.len(), None);
                        }
                        return Ok(paragraph);
                    }
                    return Err(OxmlError::MissingElement("w:p root".to_owned()));
                }
                Event::Eof => return Err(OxmlError::MissingElement("w:p root".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    /// Parse one namespace-complete paragraph for checked owner replacement.
    #[doc(hidden)]
    pub fn from_xml_fragment_checked(xml: &[u8]) -> Result<Self> {
        let admitted = crate::document::admit_word_source(xml, &[], None, None)?;
        validate_word_owner_fragment(xml, b"p", true)?;
        let mut paragraph = Self::from_xml_fragment_admitted(
            xml,
            Some(&mut admitted.controls_for(xml)),
            None,
            None,
        )?;
        // Checked replacement regenerates the paragraph root. Move only its
        // disappearing declarations onto preserved drawing owners. Ordinary
        // source snapshots must retain their original physical byte identity.
        let mut reader = Reader::from_reader(xml);
        let root = loop {
            match reader.read_event()? {
                Event::Start(root) | Event::Empty(root) => break root,
                Event::Eof => return Err(OxmlError::MissingElement("w:p root".into())),
                _ => {}
            }
        };
        let prefixes = word_prefixes_at(&root, &[])?;
        let bindings = bindings_outside_retained_root(
            &raw_namespace_declarations(&root)?,
            Some(&root),
            &prefixes,
        )?;
        for run in &mut paragraph.runs {
            let (retained_root, _) =
                run_xml_root(run, None, &mut Vec::new(), XmlWriteRole::Publication)?;
            let retained_bindings = raw_namespace_declarations(&retained_root)?;
            let bindings = bindings
                .iter()
                .filter(|binding| !retained_bindings.contains(binding))
                .cloned()
                .collect::<Vec<_>>();
            for drawing in run.content.iter_mut().filter_map(|content| match content {
                RunContent::Drawing(drawing) => Some(drawing),
                _ => None,
            }) {
                for raw in drawing
                    .inline
                    .iter_mut()
                    .filter_map(|inline| inline.raw_xml.as_mut())
                    .chain(
                        drawing
                            .anchor
                            .iter_mut()
                            .filter_map(|anchor| anchor.raw_xml.as_mut()),
                    )
                {
                    *raw = raw_with_external_bindings(raw, &bindings)?;
                }
            }
        }
        Ok(paragraph)
    }

    /// Return exact source bytes for complex fields admitted by the typed paragraph parser.
    #[doc(hidden)]
    pub fn story_complex_field_sources(
        xml: &[u8],
        inherited_word_prefixes: &[String],
    ) -> Result<Vec<Vec<u8>>> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        let paragraph = loop {
            match reader.read_event_into(&mut buffer)? {
                Event::Start(element) => {
                    let prefixes = word_prefixes_at(&element, inherited_word_prefixes)?;
                    if !is_word_element(element.name().as_ref(), b"p", &prefixes) {
                        return Err(OxmlError::MissingElement("w:p root".to_owned()));
                    }
                    break Self::from_xml_with_prefixes_and_root(
                        &mut reader,
                        &prefixes,
                        Some(&element),
                    )?;
                }
                Event::Empty(element) => {
                    let prefixes = word_prefixes_at(&element, inherited_word_prefixes)?;
                    if is_word_element(element.name().as_ref(), b"p", &prefixes) {
                        break Self::from_empty_root(&element, &prefixes)?;
                    }
                    return Err(OxmlError::MissingElement("w:p root".to_owned()));
                }
                Event::Eof => return Err(OxmlError::MissingElement("w:p root".to_owned())),
                _ => {}
            }
            buffer.clear();
        };
        let mut sources = Vec::new();
        collect_story_complex_field_sources(&paragraph, &mut sources);
        Ok(sources)
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_root(reader, word_prefixes, None)
    }

    pub(crate) fn from_xml_with_prefixes_and_root(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        root: Option<&BytesStart<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_revision_facts(reader, word_prefixes, root, None, None, None, None)
    }

    pub(crate) fn from_xml_with_revision_facts(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        root: Option<&BytesStart<'_>>,
        source_facts: Option<&mut crate::revision::RevisionSourceFacts>,
        control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        extra_maps: Option<&mut WordExtraSourceMaps>,
        text_facts: Option<&mut WordParagraphTextFacts>,
    ) -> Result<Self> {
        Self::from_xml_with_read_output(
            reader,
            word_prefixes,
            root,
            control_facts,
            WordParagraphReadOutput {
                source_facts,
                extra_maps,
                text_facts,
                ordinary: None,
                nested_maps: None,
            },
        )
    }

    pub(crate) fn from_xml_with_read_output(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        root: Option<&BytesStart<'_>>,
        mut control_facts: Option<&mut crate::document::ControlSourceCursor<'_>>,
        output: WordParagraphReadOutput<'_>,
    ) -> Result<Self> {
        let WordParagraphReadOutput {
            mut source_facts,
            extra_maps,
            text_facts,
            mut ordinary,
            mut nested_maps,
        } = output;
        if let Some(output) = &mut ordinary {
            output.clear();
        }
        let mut selected_property = None;
        reader.config_mut().trim_text(false);
        let mut properties = None;
        let mut runs = Vec::new();
        let mut run_sources = Vec::new();
        let mut simple_field_summaries = Vec::new();
        let mut comment_sources = Vec::new();
        let mut control_sources = Vec::new();
        let mut hyperlink_sources = Vec::new();
        let mut run_projection_ends = Vec::new();
        let mut hyperlinks = Vec::new();
        let mut comment_ranges = Vec::new();
        let mut bookmark_markers = Vec::new();
        let mut extra_xml = Vec::new();
        let mut content_controls = Vec::new();
        let mut revisions = Vec::new();
        let mut rubies = Vec::new();
        let mut projected_run_count = 0usize;
        let mut tracked_run_count = 0usize;
        let mut buf = Vec::new();
        let mut projection_maps = control_facts.as_deref().and_then(|facts| {
            facts.field_registry().map(|registry| {
                WordProjectionSourceMaps::new(
                    registry,
                    facts.initial_field_delivery(),
                    text_facts.is_some(),
                )
            })
        });
        if let Some(maps) = &mut projection_maps {
            maps.collect_properties = ordinary.is_some();
        }
        // Only the legacy consumed paragraph API lacks an actual Word scope.
        // Actual starts and source-backed scopes must not promote unbound XML.
        let legacy_consumed_owner = root.is_none()
            && !namespace_bindings(word_prefixes)
                .iter()
                .any(|(_, namespace)| namespace == crate::namespace::W_NS);

        loop {
            let event_start = reader.buffer_position();
            let before = [
                run_sources.len(),
                extra_xml.len(),
                comment_sources.len(),
                control_sources.len(),
                hyperlink_sources.len(),
            ];
            if let Some(maps) = &mut projection_maps {
                maps.fill_gaps(before);
            }
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"pPr", &prefixes) {
                        let (raw, mapped) = if ordinary.is_some() {
                            if let Some(cursor) = control_facts.as_deref() {
                                cursor.capture_element(reader, e, event_start)?
                            } else {
                                (capture_element(reader, e)?, None)
                            }
                        } else {
                            (capture_element(reader, e)?, None)
                        };
                        let mut observed = None;
                        properties = Some(crate::numbering::parse_scoped_ppr_with_observation(
                            &raw,
                            word_prefixes,
                            mapped.as_ref().map(|_| &mut observed),
                        )?);
                        selected_property =
                            observed.and_then(|range| mapped.as_ref()?.property_observation(range));
                    } else if is_word_element(name.as_ref(), b"r", &prefixes)
                        || (legacy_consumed_owner
                            && matches_local_name(name.as_ref(), b"r")
                            && !element_prefix_has_binding(name.as_ref(), &prefixes))
                    {
                        let recovered = !is_word_element(name.as_ref(), b"r", &prefixes);
                        let prefixes = if recovered {
                            Cow::Owned(prefixes_with_assumed_word_owner(name.as_ref(), &prefixes)?)
                        } else {
                            prefixes
                        };
                        let (raw, mapped) = if let Some(cursor) = control_facts.as_deref() {
                            cursor.capture_element(reader, e, event_start)?
                        } else {
                            (capture_element(reader, e)?, None)
                        };
                        // A run holding a phonetic guide, as Word writes it,
                        // reads as the guide: its base runs are the
                        // paragraph's, as for a `w:ruby` read in `w:p`.
                        let mut base_texts = Vec::new();
                        let ruby = if recovered {
                            None
                        } else {
                            CT_Ruby::from_run(
                                &raw,
                                &prefixes,
                                text_facts.as_ref().map(|_| &mut base_texts),
                            )?
                        };
                        if let Some((mut ruby, base_runs)) = ruby {
                            if let Some(maps) = &mut projection_maps
                                && maps.text_facts_enabled
                            {
                                maps.texts.extend(base_texts);
                            }
                            ruby.base_start = runs.len();
                            ruby.base_end = runs.len() + base_runs.len();
                            projected_run_count += base_runs.len();
                            tracked_run_count += base_runs.len();
                            run_sources.extend(base_runs.iter().map(|_| None));
                            runs.extend(base_runs);
                            rubies.push(ruby);
                        } else {
                            let mut parsed_texts = Vec::new();
                            let mut property_fact = None;
                            runs.push(parse_run_raw_with_observation(
                                &raw,
                                &prefixes,
                                text_facts.as_ref().map(|_| &mut parsed_texts),
                                mapped.as_ref(),
                                ordinary.as_ref().map(|_| &mut property_fact),
                            )?);
                            if let Some(maps) = &mut projection_maps {
                                let run_map = mapped
                                    .as_ref()
                                    .map(|input| Arc::new(input.projection_parts().0.clone()));
                                maps.runs.resize_with(runs.len() - 1, || None);
                                maps.runs.push(run_map.clone());
                                if maps.collect_properties {
                                    maps.ordinary_runs.resize_with(runs.len() - 1, || None);
                                    maps.ordinary_runs.push(property_fact);
                                }
                                if maps.text_facts_enabled {
                                    // This is the successful actual run push,
                                    // including recovered and empty Started runs.
                                    maps.texts.resize_with(runs.len() - 1, || None);
                                    maps.texts
                                        .push(run_map.map(|source_map| ParsedWordRunTexts {
                                            source: 0..raw.len(),
                                            source_map: Some(source_map),
                                            texts: parsed_texts,
                                        }));
                                }
                            }
                            if let Some(facts) = source_facts.as_deref_mut() {
                                facts.push_run(
                                    runs.last().expect("just parsed physical run"),
                                    &raw,
                                )?;
                            }
                            run_sources.push(Some(raw));
                            projected_run_count += 1;
                            tracked_run_count += 1;
                            // Preserve the legacy recovery branch's existing
                            // projection-boundary policy exactly.
                            if !recovered {
                                run_projection_ends.push((
                                    runs.len() - 1,
                                    projected_run_count,
                                    tracked_run_count,
                                ));
                            }
                        }
                    } else if is_word_element(name.as_ref(), b"sdt", &prefixes) {
                        let (raw, captured_map) = if ordinary.is_some() {
                            if let Some(cursor) = control_facts.as_deref() {
                                let (raw, mapped) =
                                    cursor.capture_element(reader, e, event_start)?;
                                let map = mapped
                                    .map(|input| Arc::new(input.projection_parts().0.clone()));
                                (raw, map)
                            } else {
                                (capture_element(reader, e)?, None)
                            }
                        } else {
                            let raw = capture_element(reader, e)?;
                            let map = control_facts.as_deref().and_then(|facts| {
                                facts.captured_map(
                                    reader,
                                    event_start..reader.buffer_position(),
                                    &raw,
                                )
                            });
                            (raw, map)
                        };
                        let mut property_maps =
                            (ordinary.is_some() || nested_maps.is_some()).then(|| {
                                WordBlockSourceMaps {
                                    collect_properties: ordinary.is_some(),
                                    collect_word_texts: nested_maps.is_some(),
                                    ..Default::default()
                                }
                            });
                        if let Some(sdt) = CT_Sdt::from_raw_with_mapped_context_and_maps(
                            &raw,
                            &prefixes,
                            crate::content_control::SdtOwner::Inline,
                            control_facts.as_deref_mut(),
                            captured_map,
                            property_maps.as_mut(),
                        ) {
                            let raw_before = raw_xml_count_at(&extra_xml, runs.len());
                            let markers_before = comment_ranges
                                .iter()
                                .filter(|marker: &&CommentRangeMarker| {
                                    marker.run_index() == runs.len()
                                        && marker.raw_before() == raw_before
                                })
                                .count();
                            let projection = accepted_control_bookmark_projection(&sdt);
                            append_nested_bookmark_projection(
                                &mut bookmark_markers,
                                &mut projected_run_count,
                                &mut tracked_run_count,
                                projection,
                                runs.len(),
                                raw_before,
                            );
                            control_sources.push((runs.len(), raw_before, markers_before, raw));
                            content_controls.push((runs.len(), raw_before, markers_before, sdt));
                            if let Some(mut child) = property_maps {
                                if let Some(maps) = &mut projection_maps
                                    && maps.collect_properties
                                {
                                    maps.ordinary_controls
                                        .resize_with(content_controls.len() - 1, Vec::new);
                                    maps.ordinary_controls
                                        .push(std::mem::take(&mut child.ordinary));
                                }
                                if let Some(output) = nested_maps.as_deref_mut() {
                                    output.append(
                                        WordBlockComponent::ParagraphControl(
                                            content_controls.len() - 1,
                                        ),
                                        child,
                                    );
                                }
                            }
                        } else {
                            extra_xml.push((runs.len(), raw));
                        }
                    } else if is_word_element(name.as_ref(), b"hyperlink", &prefixes) {
                        let (rel_id, anchor, tooltip, doc_location, extra_attributes) =
                            parse_hyperlink_attributes(e, &prefixes)?;
                        let (raw, mapped) = if ordinary.is_some() {
                            if let Some(cursor) = control_facts.as_deref() {
                                cursor.capture_element(reader, e, event_start)?
                            } else {
                                (capture_element(reader, e)?, None)
                            }
                        } else {
                            let raw = capture_element(reader, e)?;
                            let mapped = control_facts.as_deref().and_then(|facts| {
                                facts.field_input(
                                    reader,
                                    event_start..reader.buffer_position(),
                                    &raw,
                                )
                            });
                            (raw, mapped)
                        };
                        let parsed = parse_hyperlink_children(
                            &raw,
                            &prefixes,
                            source_facts.as_deref_mut(),
                            mapped.as_ref(),
                            text_facts.is_some(),
                            ordinary.is_some(),
                        )?;
                        let run_start = runs.len();
                        if parsed.runs.is_empty() && parsed.revisions.is_empty() {
                            // A hyperlink with no run stays raw, and its
                            // bookmarks still count where it sits.
                            append_nested_bookmark_projection(
                                &mut bookmark_markers,
                                &mut projected_run_count,
                                &mut tracked_run_count,
                                AcceptedBookmarkProjection {
                                    markers: parsed.bookmark_markers,
                                    projected_run_count: 0,
                                    tracked_run_count: 0,
                                },
                                run_start,
                                raw_xml_count_at(&extra_xml, run_start),
                            );
                            extra_xml.push((run_start, raw));
                        } else {
                            append_nested_bookmark_projection(
                                &mut bookmark_markers,
                                &mut projected_run_count,
                                &mut tracked_run_count,
                                AcceptedBookmarkProjection {
                                    markers: parsed.bookmark_markers.clone(),
                                    projected_run_count: parsed.projected_run_count,
                                    tracked_run_count: parsed.tracked_run_count,
                                },
                                run_start,
                                0,
                            );
                            let hyperlink_index = hyperlinks.len();
                            let run_end = run_start + parsed.runs.len();
                            if run_end > run_start {
                                hyperlink_sources.push((run_start, run_end, raw.clone()));
                            }
                            let preserved_raw_before = parsed
                                .runs
                                .is_empty()
                                .then(|| raw_xml_count_at(&extra_xml, run_start));
                            if let Some(maps) = &mut projection_maps {
                                maps.runs.extend(parsed.run_maps);
                                if maps.collect_properties {
                                    maps.ordinary_runs.resize_with(run_start, || None);
                                    maps.ordinary_runs.extend(parsed.ordinary_runs);
                                }
                                if maps.text_facts_enabled {
                                    maps.texts.extend(parsed.text_facts);
                                }
                                maps.hyperlink_extras.push(parsed.extra_maps);
                            }
                            runs.extend(parsed.runs);
                            run_sources.extend(parsed.run_sources);
                            hyperlinks.push(HyperlinkSpan {
                                rel_id,
                                anchor,
                                tooltip,
                                doc_location,
                                run_start,
                                run_end,
                                extra_attributes,
                                extra_xml: parsed.extra_xml,
                                preserved_raw_before,
                            });
                            revisions.extend(parsed.revisions.into_iter().map(|(at, revision)| {
                                (
                                    run_start + at,
                                    hyperlink_revision_slot(hyperlink_index),
                                    revision,
                                )
                            }));
                            if preserved_raw_before.is_some() {
                                extra_xml.push((run_start, raw));
                            }
                        }
                    } else if is_word_element(name.as_ref(), b"ruby", &prefixes) {
                        let raw = capture_element(reader, e)?;
                        let mut base_texts = Vec::new();
                        let parsed = if text_facts.is_some() {
                            let mapped = control_facts.as_deref().and_then(|facts| {
                                facts.field_input(
                                    reader,
                                    event_start..reader.buffer_position(),
                                    &raw,
                                )
                            });
                            CT_Ruby::from_raw_with_texts(
                                &raw,
                                &prefixes,
                                mapped.as_ref(),
                                Some(&mut base_texts),
                            )?
                        } else {
                            CT_Ruby::from_raw(&raw, &prefixes)?
                        };
                        if let Some((mut ruby, base_runs)) = parsed {
                            if let Some(maps) = &mut projection_maps
                                && maps.text_facts_enabled
                            {
                                maps.texts.extend(base_texts);
                            }
                            ruby.base_start = runs.len();
                            ruby.base_end = runs.len() + base_runs.len();
                            projected_run_count += base_runs.len();
                            tracked_run_count += base_runs.len();
                            run_sources.extend(base_runs.iter().map(|_| None));
                            runs.extend(base_runs);
                            rubies.push(ruby);
                        } else {
                            extra_xml.push((runs.len(), raw));
                        }
                    } else if is_word_element(name.as_ref(), b"fldSimple", &prefixes) {
                        let prepared_header = if source_facts.is_none() {
                            control_facts.as_deref().and_then(|facts| {
                                facts.simple_header(
                                    reader,
                                    event_start..reader.buffer_position(),
                                    e,
                                    &prefixes,
                                )
                            })
                        } else {
                            None
                        };
                        let raw = capture_element(reader, e)?;
                        if let Some(facts) = source_facts.as_deref_mut() {
                            facts.simple_fields_deferred = true;
                            extra_xml.push((runs.len(), raw));
                        } else {
                            let mapped = control_facts.as_deref().and_then(|facts| {
                                facts.field_input(
                                    reader,
                                    event_start..reader.buffer_position(),
                                    &raw,
                                )
                            });
                            let prepared_selector = control_facts.as_deref().and_then(|facts| {
                                facts.simple_selector(
                                    reader,
                                    event_start..reader.buffer_position(),
                                    &raw,
                                )
                            });
                            if let Some((field, summary)) = parse_simple_field_with_summary(
                                &raw,
                                &prefixes,
                                prepared_header,
                                prepared_selector,
                                mapped.as_ref(),
                            )? {
                                simple_field_summaries.push((runs.len(), summary));
                                runs.push(field_run(field, None));
                                run_sources.push(Some(raw));
                                projected_run_count += 1;
                                tracked_run_count += 1;
                            } else {
                                extra_xml.push((runs.len(), raw));
                            }
                        }
                    } else if is_word_element(name.as_ref(), b"commentRangeStart", &prefixes)
                        || is_word_element(name.as_ref(), b"commentRangeEnd", &prefixes)
                    {
                        let id = required_word_i32_attribute(e, b"id", &prefixes)?;
                        let raw = capture_element(reader, e)?;
                        push_comment_marker(
                            &mut comment_ranges,
                            &extra_xml,
                            runs.len(),
                            id,
                            is_word_element(name.as_ref(), b"commentRangeStart", &prefixes),
                            raw_element_has_child_content(&raw),
                        );
                        comment_sources
                            .push((comment_ranges.last().expect("captured marker").clone(), raw));
                    } else if is_word_element(name.as_ref(), b"bookmarkStart", &prefixes)
                        || is_word_element(name.as_ref(), b"bookmarkEnd", &prefixes)
                    {
                        let start = is_word_element(name.as_ref(), b"bookmarkStart", &prefixes);
                        let id = optional_word_attribute(e, b"id", &prefixes)
                            .and_then(|value| value.parse().ok());
                        let bookmark_name = optional_word_attribute(e, b"name", &prefixes);
                        let raw = capture_element(reader, e)?;
                        bookmark_markers.push(BookmarkMarker {
                            start,
                            id,
                            name: bookmark_name,
                            run_index: runs.len(),
                            raw_before: raw_xml_count_at(&extra_xml, runs.len()),
                            projected_run_index: projected_run_count,
                            tracked_run_index: tracked_run_count,
                            word_prefixes: prefixes.to_vec(),
                            has_child_content: raw_element_has_child_content(&raw),
                        });
                        extra_xml.push((runs.len(), raw));
                    } else if is_word_element(name.as_ref(), b"ins", &prefixes)
                        || is_word_element(name.as_ref(), b"del", &prefixes)
                        || is_word_element(name.as_ref(), b"moveFrom", &prefixes)
                        || is_word_element(name.as_ref(), b"moveTo", &prefixes)
                    {
                        let raw_before = raw_xml_count_at(&extra_xml, runs.len());
                        let (raw, mapped) = if let Some(cursor) = control_facts.as_deref() {
                            cursor.capture_element(reader, e, event_start)?
                        } else {
                            (capture_element(reader, e)?, None)
                        };
                        let mut revision_maps =
                            nested_maps.as_ref().map(|_| WordBlockSourceMaps::default());
                        if let Some(revision) = CT_Revision::from_raw_with_read_output(
                            raw.clone(),
                            &prefixes,
                            false,
                            mapped.as_ref(),
                            revision_maps.as_mut(),
                        )? {
                            if let (Some(output), Some(child)) =
                                (nested_maps.as_deref_mut(), revision_maps)
                            {
                                output.append(
                                    WordBlockComponent::ParagraphRevision(revisions.len()),
                                    child,
                                );
                            }
                            if let Some(facts) = source_facts.as_deref_mut() {
                                facts.push_revision(&revision)?;
                            }
                            let projection = accepted_revision_bookmark_projection(&revision);
                            append_nested_bookmark_projection(
                                &mut bookmark_markers,
                                &mut projected_run_count,
                                &mut tracked_run_count,
                                projection,
                                runs.len(),
                                raw_before,
                            );
                            if let (Some(output), Some(mapped)) =
                                (nested_maps.as_deref_mut(), mapped.as_ref())
                            {
                                output.word_raws.push((
                                    vec![WordBlockComponent::ParagraphRevision(revisions.len())],
                                    Arc::new(mapped.projection_parts().0.clone()),
                                ));
                            }
                            revisions.push((runs.len(), raw_before, revision));
                        }
                        extra_xml.push((runs.len(), raw));
                    } else {
                        // Capture unknown elements (bookmarks, comments, etc.) as raw XML.
                        let raw = capture_element(reader, e)?;
                        if let Some(facts) = source_facts.as_deref_mut()
                            && crate::revision::revision_kind(e.name().as_ref(), &prefixes)
                                .is_some()
                            && let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes)
                        {
                            facts.push_revision(&revision)?;
                        }
                        extra_xml.push((runs.len(), raw));
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"pPr", &prefixes)
                        && e.attributes().next().is_none()
                    {
                        properties.get_or_insert_default();
                    } else if is_word_element(name.as_ref(), b"commentRangeStart", &prefixes)
                        || is_word_element(name.as_ref(), b"commentRangeEnd", &prefixes)
                    {
                        let id = required_word_i32_attribute(e, b"id", &prefixes)?;
                        push_comment_marker(
                            &mut comment_ranges,
                            &extra_xml,
                            runs.len(),
                            id,
                            is_word_element(name.as_ref(), b"commentRangeStart", &prefixes),
                            false,
                        );
                        comment_sources.push((
                            comment_ranges.last().expect("captured marker").clone(),
                            capture_empty_element(e)?,
                        ));
                    } else if is_word_element(name.as_ref(), b"bookmarkStart", &prefixes)
                        || is_word_element(name.as_ref(), b"bookmarkEnd", &prefixes)
                    {
                        let start = is_word_element(name.as_ref(), b"bookmarkStart", &prefixes);
                        let id = optional_word_attribute(e, b"id", &prefixes)
                            .and_then(|value| value.parse().ok());
                        let bookmark_name = optional_word_attribute(e, b"name", &prefixes);
                        bookmark_markers.push(BookmarkMarker {
                            start,
                            id,
                            name: bookmark_name,
                            run_index: runs.len(),
                            raw_before: raw_xml_count_at(&extra_xml, runs.len()),
                            projected_run_index: projected_run_count,
                            tracked_run_index: tracked_run_count,
                            word_prefixes: prefixes.to_vec(),
                            has_child_content: false,
                        });
                        extra_xml.push((runs.len(), capture_empty_element(e)?));
                    } else if is_word_element(name.as_ref(), b"fldSimple", &prefixes) {
                        let raw = capture_empty_element(e)?;
                        let mapped = control_facts.as_deref().and_then(|facts| {
                            facts.field_input(reader, event_start..reader.buffer_position(), &raw)
                        });
                        let field = if source_facts.is_none() {
                            parse_empty_simple_field(
                                e,
                                &raw,
                                prefixes.into_owned(),
                                mapped.as_ref(),
                            )?
                            .map(|(field, _)| field)
                        } else {
                            source_facts
                                .as_deref_mut()
                                .expect("physical revision facts")
                                .simple_fields_deferred = true;
                            None
                        };
                        if let Some(field) = field {
                            if let Some(mapped) = mapped {
                                mapped.register(&field)?;
                            }
                            runs.push(field_run(field, None));
                            run_sources.push(None);
                            projected_run_count += 1;
                            tracked_run_count += 1;
                        } else {
                            extra_xml.push((runs.len(), raw));
                        }
                    } else if !matches_local_name(name.as_ref(), b"p") {
                        let raw_before = raw_xml_count_at(&extra_xml, runs.len());
                        let raw = capture_empty_element(e)?;
                        if (is_word_element(name.as_ref(), b"ins", &prefixes)
                            || is_word_element(name.as_ref(), b"del", &prefixes)
                            || is_word_element(name.as_ref(), b"moveFrom", &prefixes)
                            || is_word_element(name.as_ref(), b"moveTo", &prefixes))
                            && let Some(revision) = CT_Revision::from_raw_with_input(
                                raw.clone(),
                                &prefixes,
                                false,
                                control_facts
                                    .as_deref()
                                    .and_then(|facts| {
                                        facts.field_input(
                                            reader,
                                            event_start..reader.buffer_position(),
                                            &raw,
                                        )
                                    })
                                    .as_ref(),
                            )?
                        {
                            if let Some(facts) = source_facts.as_deref_mut() {
                                facts.push_revision(&revision)?;
                            }
                            let projection = accepted_revision_bookmark_projection(&revision);
                            append_nested_bookmark_projection(
                                &mut bookmark_markers,
                                &mut projected_run_count,
                                &mut tracked_run_count,
                                projection,
                                runs.len(),
                                raw_before,
                            );
                            revisions.push((runs.len(), raw_before, revision));
                        }
                        if let Some(facts) = source_facts.as_deref_mut()
                            && !is_content_revision_element(name.as_ref(), &prefixes)
                            && crate::revision::revision_kind(name.as_ref(), &prefixes).is_some()
                            && let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes)
                        {
                            facts.push_revision(&revision)?;
                        }
                        extra_xml.push((runs.len(), raw));
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"p") => {
                    break;
                }
                Ok(Event::Text(ref text)) if is_xml_whitespace(text.as_ref()) => {
                    extra_xml.push((runs.len(), text.as_ref().to_vec()));
                }
                Ok(Event::Comment(ref comment)) => {
                    extra_xml.push((
                        runs.len(),
                        capture_standalone_event(Event::Comment(comment.to_owned().into_owned()))?,
                    ));
                }
                Ok(Event::PI(ref instruction)) => {
                    extra_xml.push((
                        runs.len(),
                        capture_standalone_event(Event::PI(instruction.to_owned().into_owned()))?,
                    ));
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            if let (Some(maps), Some(facts)) = (&mut projection_maps, control_facts.as_deref()) {
                if maps.text_facts_enabled {
                    maps.texts.resize_with(run_sources.len(), || None);
                }
                let captured = event_start..reader.buffer_position();
                maps.runs
                    .extend(run_sources[maps.runs.len()..].iter().map(|raw| {
                        raw.as_ref()
                            .and_then(|raw| facts.captured_map(reader, captured.clone(), raw))
                    }));
                maps.extras.extend(
                    extra_xml[before[1]..]
                        .iter()
                        .map(|(_, raw)| facts.captured_map(reader, captured.clone(), raw)),
                );
                maps.comments.extend(
                    comment_sources[before[2]..]
                        .iter()
                        .map(|(_, raw)| facts.captured_map(reader, captured.clone(), raw)),
                );
                maps.controls.extend(
                    control_sources[before[3]..]
                        .iter()
                        .map(|(_, _, _, raw)| facts.captured_map(reader, captured.clone(), raw)),
                );
                maps.hyperlink_sources.extend(
                    hyperlink_sources[before[4]..]
                        .iter()
                        .map(|(_, _, raw)| facts.captured_map(reader, captured.clone(), raw)),
                );
                for link in hyperlinks.iter().skip(maps.hyperlink_extras.len()) {
                    maps.hyperlink_extras.push(vec![None; link.extra_xml.len()]);
                }
            }
            buf.clear();
        }

        if nested_maps.is_some()
            && let Some(maps) = &mut projection_maps
        {
            maps.retained_control_slots = Some((0..content_controls.len()).collect());
            maps.retained_revision_slots = Some((0..revisions.len()).collect());
        }
        let retain_legacy_namespace_frame = project_complex_fields(ComplexFieldProjection {
            source_maps: projection_maps.as_mut(),
            inline_complete_only: false,
            legacy_consumed_owner,
            run_coverage: None,
            runs: &mut runs,
            run_sources: &mut run_sources,
            simple_field_summaries: &simple_field_summaries,
            extra_xml: &mut extra_xml,
            comment_ranges: &mut comment_ranges,
            comment_sources: &comment_sources,
            control_sources: &control_sources,
            hyperlink_sources: &hyperlink_sources,
            run_projection_ends: &run_projection_ends,
            bookmark_markers: &mut bookmark_markers,
            content_controls: &mut content_controls,
            revisions: &mut revisions,
            hyperlinks: &mut hyperlinks,
            rubies: &mut rubies,
            word_prefixes,
        })?;
        if let (Some(output), Some(maps)) = (nested_maps, projection_maps.as_ref())
            && let (Some(controls), Some(revisions)) =
                (&maps.retained_control_slots, &maps.retained_revision_slots)
        {
            let controls = controls
                .iter()
                .enumerate()
                .map(|(new, old)| (*old, new))
                .collect::<std::collections::HashMap<_, _>>();
            let revisions = revisions
                .iter()
                .enumerate()
                .map(|(new, old)| (*old, new))
                .collect::<std::collections::HashMap<_, _>>();
            let remap = |path: &mut Vec<WordBlockComponent>| match path.first_mut() {
                Some(WordBlockComponent::ParagraphControl(index)) => {
                    if let Some(new) = controls.get(index) {
                        *index = *new;
                        true
                    } else {
                        false
                    }
                }
                Some(WordBlockComponent::ParagraphRevision(index)) => {
                    if let Some(new) = revisions.get(index) {
                        *index = *new;
                        true
                    } else {
                        false
                    }
                }
                _ => true,
            };
            output.paragraphs.retain_mut(|(path, _)| remap(path));
            output.word_texts.retain_mut(|(path, _)| remap(path));
            output.word_runs.retain_mut(|(path, _)| remap(path));
            output.word_raws.retain_mut(|(path, _)| remap(path));
        }
        if let Some(maps) = &mut projection_maps {
            // retain_extras selects run boundaries, not extra-vector slots.
            // Pair this actual byte predicate with the corresponding map slot.
            maps.extras = extra_xml
                .iter()
                .enumerate()
                .filter(|(_, (_, raw))| !is_xml_whitespace(raw))
                .map(|(index, _)| maps.extras.get(index).cloned().flatten())
                .collect();
        }
        extra_xml.retain(|(_, raw)| !is_xml_whitespace(raw));
        for hyperlink in &mut hyperlinks {
            hyperlink
                .extra_xml
                .retain(|(_, _, raw)| !is_xml_whitespace(raw));
        }

        let mut inherited_bindings = None;
        let mut equations = Vec::new();
        for run_index in 0..=runs.len() {
            for (raw_before, raw) in extra_xml
                .iter()
                .filter(|(position, _)| *position == run_index)
                .map(|(_, raw)| raw)
                .enumerate()
            {
                let bindings =
                    inherited_bindings.get_or_insert_with(|| namespace_bindings(word_prefixes));
                if let Some(equation) = OfficeMath::from_raw(raw, bindings)? {
                    equations.push((run_index, raw_before, equation));
                }
            }
        }

        if let Some(root_start) = root
            && let Some(record) = capture_root_attribute_record(root_start, word_prefixes)?
        {
            let record = retain_foreign_word_alias_declaration(record, root_start, word_prefixes)?;
            extra_xml.push((ROOT_ATTRIBUTES_POSITION, record));
        }

        if retain_legacy_namespace_frame {
            // The consumed runs previously carried bindings for their raw
            // children. Retain the actual paragraph frame, including the
            // external end marker and opaque siblings. Local declarations
            // still shadow it, and no child scope is hoisted here.
            let bindings = bindings_outside_retained_root(
                &namespace_bindings(word_prefixes),
                root,
                word_prefixes,
            )?;
            if !bindings.is_empty() {
                let mut record = BytesStart::new(std::str::from_utf8(ROOT_ATTRIBUTES_ELEMENT)?);
                for (prefix, uri) in bindings {
                    let name = if prefix.is_empty() {
                        "xmlns".into()
                    } else {
                        format!("xmlns:{prefix}")
                    };
                    record.push_attribute((name.as_str(), uri.as_str()));
                }
                extra_xml.push((
                    ROOT_ATTRIBUTES_POSITION,
                    capture_standalone_event(Event::Empty(record))?,
                ));
            }
        }
        if let Some(output) = ordinary {
            if let Some(fact) = selected_property {
                output.push(fact);
            }
            if let Some(maps) = &mut projection_maps {
                output.extend(
                    std::mem::take(&mut maps.ordinary_runs)
                        .into_iter()
                        .flatten(),
                );
                output.extend(
                    std::mem::take(&mut maps.ordinary_controls)
                        .into_iter()
                        .flatten(),
                );
            }
        }
        if let Some(output) = text_facts {
            *output = projection_maps
                .as_mut()
                .map_or_else(Vec::new, |maps| std::mem::take(&mut maps.texts));
            output.resize_with(runs.len(), || None);
        }
        if let Some(output) = extra_maps {
            *output = projection_maps.map_or_else(Vec::new, |maps| maps.extras);
            output.resize(extra_xml.len(), None);
        }
        Ok(CT_P {
            properties,
            runs,
            hyperlinks,
            comment_ranges,
            bookmark_markers,
            extra_xml,
            content_controls,
            revisions,
            equations,
            rubies,
        })
    }
}

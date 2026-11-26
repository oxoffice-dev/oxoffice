//! Comparing runs and fields: instructions, results, conversions between simple and complex.

use super::*;

pub(super) fn publish_run_source(
    original: &CT_R,
    content: &CT_R,
    generated: String,
    source: Option<&RunSource>,
    content_source: Option<&RunSource>,
    selection: Option<RunSourceSelection<'_>>,
    destination_parent: &NamespaceBindings,
    metadata: &Metadata<'_>,
) -> Result<String> {
    let source = source.or_else(|| metadata.run_source(original));
    if let Some(source) = source {
        let content_source = content_source
            .or_else(|| metadata.run_source(content))
            .unwrap_or(source);
        let complete;
        let (raw, typed) = if let Some(selection) = selection {
            selection
        } else {
            complete = complete_run_selection(content);
            (&complete.0[..], &complete.1[..])
        };
        preserve_run_source(
            &generated,
            &run_property_xml(original.properties.as_ref())?,
            [source, content_source],
            content,
            raw,
            typed,
            destination_parent,
            &metadata.ids,
        )
    } else {
        metadata.ids.generated(generated)
    }
}

pub(super) fn compared_run_xml(
    original: &CT_R,
    edited: &CT_R,
    source: Option<&RunSource>,
    selection: Option<RunSourceSelection<'_>>,
    location: &str,
    destination_parent: &NamespaceBindings,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if let (Some(original_field), Some(edited_field)) = (run_field(original), run_field(edited)) {
        // A virtual Field is not a physical run owner. Its producer qualifies
        // new revision nodes without rebinding the retained field's namespace.
        return if metadata.options.ignore_fields {
            field_xml(original_field)
        } else {
            compare_field_xml(original_field, edited_field, metadata)
        };
    }
    let source = source.or_else(|| metadata.run_source(original)).cloned();
    let generated = compared_run_xml_impl(original, edited, location, metadata, diagnostics)?;
    publish_run_source(
        original,
        original,
        generated,
        source.as_ref(),
        None,
        selection,
        destination_parent,
        metadata,
    )
}

fn compared_run_xml_impl(
    original: &CT_R,
    edited: &CT_R,
    location: &str,
    metadata: &mut Metadata<'_>,
    diagnostics: &mut Vec<ComparisonDiagnostic>,
) -> Result<String> {
    if metadata.options.ignore_fields && (run_is_field(original) || run_is_field(edited)) {
        return metadata.ids.generated(paragraph_owned_run_xml(original)?);
    }
    if metadata.options.ignore_formatting {
        return metadata.ids.generated(paragraph_owned_run_xml(original)?);
    }
    let original_properties = modeled_run_properties(original);
    let edited_properties = modeled_run_properties(edited);
    if original_properties == edited_properties {
        if run_formatting(original) != run_formatting(edited) {
            formatting_diagnostic(diagnostics, location.to_owned());
        }
        return metadata.ids.generated(paragraph_owned_run_xml(original)?);
    }
    let previous = run_property_xml(original_properties.as_ref())?;
    let change =
        metadata
            .ids
            .revision("rPrChange", metadata.author, metadata.timestamp, &previous)?;
    if unmodeled_run_properties_differ(original, edited) {
        formatting_diagnostic(diagnostics, location.to_owned());
    }
    let mut current_run = edited.clone();
    preserve_unmodeled_run_properties(&mut current_run, original);
    let mut current = run_xml(&current_run)?;
    let properties = direct_word_element_spans(&current, "rPr")?;
    if let Some(span) = properties.first() {
        let updated = append_word_child(&current[span.clone()], "rPr", &change)?;
        current.replace_range(span.clone(), &updated);
    } else {
        current = insert_first_child(&current, &format!("<w:rPr>{change}</w:rPr>"))?;
    }
    metadata.ids.generated(current)
}

fn field_result_scope(
    field: &oxoffice_docx_oxml::text::Field,
    metadata: &Metadata<'_>,
) -> NamespaceBindings {
    metadata
        .source_frames
        .iter()
        .rev()
        .find_map(|frame| {
            frame
                .field_scopes
                .get(&(std::ptr::from_ref(field) as usize))
        })
        .cloned()
        .unwrap_or_else(|| vec![("xmlns:w".into(), W_NS.into())])
}

pub(super) fn comparison_instruction_eq(
    left: &oxoffice_docx_oxml::text::FieldInstruction,
    right: &oxoffice_docx_oxml::text::FieldInstruction,
) -> bool {
    use oxoffice_docx_oxml::text::FieldArgument;
    fn same_operand(left: &FieldArgument, right: &FieldArgument) -> bool {
        match (left, right) {
            (FieldArgument::Text(left), FieldArgument::Text(right)) => left == right,
            (FieldArgument::Nested(left), FieldArgument::Nested(right)) => {
                comparison_instruction_eq(&left.instruction, &right.instruction)
            }
            (FieldArgument::Joined(left), FieldArgument::Joined(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| same_operand(left, right))
            }
            _ => false,
        }
    }
    left.name == right.name
        && left.arguments.len() == right.arguments.len()
        && left
            .arguments
            .iter()
            .zip(&right.arguments)
            .all(|(left, right)| same_operand(left, right))
        && left.switches.len() == right.switches.len()
        && left
            .switches
            .iter()
            .zip(&right.switches)
            .all(|(left, right)| {
                left.name == right.name
                    && match (&left.argument, &right.argument) {
                        (None, None) => true,
                        (Some(left), Some(right)) => same_operand(left, right),
                        _ => false,
                    }
            })
}

fn compare_field_xml(
    original: &oxoffice_docx_oxml::text::Field,
    edited: &oxoffice_docx_oxml::text::Field,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let original_xml = field_xml(original)?;
    let edited_xml = field_xml(edited)?;
    let old_scope = field_result_scope(original, metadata);
    let new_scope = field_result_scope(edited, metadata);

    let original_complex_form = actual_field_is_complex(&original_xml, &old_scope)?;
    let edited_complex_form = actual_field_is_complex(&edited_xml, &new_scope)?;
    let same_instruction = comparison_instruction_eq(
        &original.effective_instruction(),
        &edited.effective_instruction(),
    );
    let same_form = original_complex_form == edited_complex_form;
    // Representation dispatch precedes global Field equality, which deliberately
    // omits simple/complex form. Protected source facts remain independent.
    let same_source_facts =
        metadata.field_source_facts(original) == metadata.field_source_facts(edited);
    if same_instruction && same_form && original == edited && same_source_facts {
        return Ok(original_xml);
    }
    if same_instruction && same_form {
        if original_complex_form {
            let (before, old_result, after) = complex_field_result(&original_xml)?;
            let (_, new_result, _) = complex_field_result(&edited_xml)?;
            return Ok(format!(
                "{before}{}{after}",
                tracked_field_result(
                    [original, edited],
                    &old_result,
                    &new_result,
                    [&old_scope, &new_scope],
                    metadata
                )?
            ));
        }
        let original_xml = append_word_child(&original_xml, "fldSimple", "")?;
        let edited_xml = append_word_child(&edited_xml, "fldSimple", "")?;
        let original_inner = element_inner_range_any_prefix(&original_xml, "fldSimple")?;
        let edited_inner = element_inner_range_any_prefix(&edited_xml, "fldSimple")?;
        let old_data = direct_word_element_spans_in_scope(&original_xml, "fldData", &old_scope)?;
        let new_data = direct_word_element_spans_in_scope(&edited_xml, "fldData", &new_scope)?;
        if old_data.len() > 1 || new_data.len() > 1 {
            return Err(Error::Other(
                "field result comparison has ambiguous fldData ownership".into(),
            ));
        }
        let old_start = old_data
            .first()
            .map_or(original_inner.start, |span| span.end);
        let new_start = new_data.first().map_or(edited_inner.start, |span| span.end);
        let tracked = tracked_field_result(
            [original, edited],
            &original_xml[old_start..original_inner.end],
            &edited_xml[new_start..edited_inner.end],
            [&old_scope, &new_scope],
            metadata,
        )?;
        let mut output = original_xml;
        output.replace_range(old_start..original_inner.end, &tracked);
        return Ok(output);
    }

    if let Some(story) = &metadata.paired_field_story {
        let actual = |field: &oxoffice_docx_oxml::text::Field| {
            metadata.source_frames.iter().rev().find_map(|frame| {
                frame
                    .field_occurrences
                    .get(&(std::ptr::from_ref(field) as usize))
            })
        };
        let left = actual(original).ok_or_else(|| {
            Error::Other("paired field lacks its actual original source occurrence".into())
        })?;
        let right = actual(edited).ok_or_else(|| {
            Error::Other("paired field lacks its actual edited source occurrence".into())
        })?;
        if left.story != *story
            || right.story != *story
            || left.input_role != 0
            || right.input_role != 1
            || left.owner_path != right.owner_path
            || left.field_slot != right.field_slot
            || left.bounds.is_empty()
            || right.bounds.is_empty()
        {
            return Err(Error::Other(
                "paired field does not belong to the checked host interval".into(),
            ));
        }
        // The outer checked host owns the replacement. Its actual selected side
        // remains its own representation, with no ordinary conversion witness.
        return Ok(edited_xml);
    }

    let original_complex = if original_complex_form {
        original_xml
    } else {
        clean_simple_complex_source(&original_xml, &old_scope, Some(original))?
    };
    let edited_complex = if edited_complex_form {
        edited_xml
    } else {
        clean_simple_complex_source(&edited_xml, &new_scope, Some(edited))?
    };
    let deleted_source =
        qualified_complete_field_payload(&original_complex, &old_scope, true, false, true, &mut 0)?;
    let inserted_source =
        qualified_complete_field_payload(&edited_complex, &new_scope, false, false, true, &mut 0)?;
    let deleted_id = metadata.ids.allocate()?;
    let inserted_id = metadata.ids.allocate()?;
    let deleted = metadata.ids.generated(IdAllocator::revision_with_id(
        "del",
        metadata.author,
        metadata.timestamp,
        &deleted_source,
        deleted_id,
    ))?;
    let inserted = metadata.ids.generated(IdAllocator::revision_with_id(
        "ins",
        metadata.author,
        metadata.timestamp,
        &inserted_source,
        inserted_id,
    ))?;
    if !original_complex_form || !edited_complex_form {
        let occurrence = |field: &oxoffice_docx_oxml::text::Field| {
            metadata
                .source_frames
                .iter()
                .rev()
                .find_map(|frame| {
                    frame
                        .field_occurrences
                        .get(&(std::ptr::from_ref(field) as usize))
                })
                .cloned()
        };
        let left = occurrence(original).ok_or_else(|| {
            Error::Other(
                "ordinary field conversion has no original physical source occurrence".into(),
            )
        })?;
        let right = occurrence(edited).ok_or_else(|| {
            Error::Other(
                "ordinary field conversion has no edited physical source occurrence".into(),
            )
        })?;
        {
            if left.input_role != 0 || right.input_role != 1 || left.story != right.story {
                return Err(Error::Other(
                    "field conversion input roles or physical story association changed".into(),
                ));
            }
            metadata.field_conversions.push(FieldConversionWitness {
                sources: [left, right],
                instruction_changed: !same_instruction,
                form_changed: !same_form,
                input_complex: [original_complex_form, edited_complex_form],
                revision_ids: [deleted_id, inserted_id],
                generated_spans: [deleted_source, inserted_source],
            });
        }
    }
    Ok(format!("{deleted}{inserted}"))
}

pub(super) fn actual_field_source_fragment(
    xml: &str,
    bindings: &NamespaceBindings,
) -> Result<String> {
    let scope = bindings
        .iter()
        .map(|(name, uri)| {
            (
                name.strip_prefix("xmlns:").unwrap_or("").to_owned(),
                uri.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    String::from_utf8(crate::document::close_content_fragment_namespaces(
        xml.as_bytes(),
        &scope,
    )?)
    .map_err(utf8_error)
}

pub(super) fn actual_field_is_complex(xml: &str, bindings: &NamespaceBindings) -> Result<bool> {
    let closed = actual_field_source_fragment(xml, bindings)?;
    let mut reader = NsReader::from_str(&closed);
    let (namespace, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(element) | Event::Empty(element)) = event else {
        return Err(Error::Other("field has no actual source root".into()));
    };
    if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()) {
        return Err(Error::Other(
            "field source root has no actual Word binding".into(),
        ));
    }
    match element.local_name().as_ref() {
        b"fldSimple" => Ok(false),
        b"r" => Ok(true),
        _ => Err(Error::Other(
            "field source root is not a qualified field carrier".into(),
        )),
    }
}

/// The namespace bindings of `xml`'s root that its names use, but the Word
/// binding: Word 2019 declares a default namespace on the document root that
/// no name uses, and a version from another producer does not, which is no
/// change.
pub(super) fn conversion_namespace_facts(
    bindings: NamespaceBindings,
    xml: &str,
) -> Result<BTreeMap<String, String>> {
    // XML that does not read keeps every binding, as before.
    let used = used_name_prefixes(xml).ok();
    Ok(bindings
        .into_iter()
        .filter(|(name, uri)| !(name == "xmlns:w" && uri == W_NS))
        .filter(|(name, _)| {
            let prefix = if name == "xmlns" {
                ""
            } else {
                name.strip_prefix("xmlns:").unwrap_or(name)
            };
            used.as_ref().is_none_or(|used| used.contains(prefix))
        })
        .collect())
}

pub(super) fn field_source_sequence_budget(
    source: &str,
    scope: &NamespaceBindings,
) -> Result<Vec<String>> {
    let wrapped = format!("<sourceBudget>{source}</sourceBudget>");
    let mut output = Vec::new();
    let mut cursor = "<sourceBudget>".len();
    for span in direct_element_spans(&wrapped)? {
        output.push(format!("outside:{:?}", &wrapped[cursor..span.start]));
        let (lexical, bindings) = raw_occurrence_representation(&wrapped[span.clone()], scope)?;
        output.push(format!(
            "root:{:?}:{lexical:?}",
            conversion_namespace_facts(bindings, &wrapped[span.clone()])?
        ));
        cursor = span.end;
    }
    output.push(format!(
        "outside:{:?}",
        &wrapped[cursor..wrapped.len() - "</sourceBudget>".len()]
    ));
    Ok(output)
}

pub(super) fn field_carrier_source_budget(
    source: &str,
    scope: &NamespaceBindings,
) -> Result<Vec<String>> {
    let result = actual_field_result_range(source, scope)?;
    if actual_field_is_complex(source, scope)? {
        let mut facts = field_source_sequence_budget(&source[..result.start], scope)?;
        facts.push("result-boundary".into());
        facts.extend(field_source_sequence_budget(&source[result.end..], scope)?);
        Ok(facts)
    } else {
        let (lexical, bindings) = raw_occurrence_representation(source, scope)?;
        let growth = element_opening(source)?
            .0
            .checked_sub(element_opening(&lexical)?.0)
            .ok_or_else(|| {
                Error::Other("field carrier namespace projection grew its opening".into())
            })?;
        let start = result
            .start
            .checked_sub(growth)
            .ok_or_else(|| Error::Other("field carrier result precedes opening".into()))?;
        let end = result
            .end
            .checked_sub(growth)
            .ok_or_else(|| Error::Other("field carrier result precedes opening".into()))?;
        Ok(vec![
            format!("scope:{:?}", conversion_namespace_facts(bindings, source)?),
            format!("before:{:?}", &lexical[..start]),
            format!("after:{:?}", &lexical[end..]),
        ])
    }
}

/// The namespace prefixes the names of an element and its descendants use,
/// `""` for an unprefixed element, with the prefixes a markup-compatibility
/// attribute lists.
fn used_name_prefixes(xml: &str) -> Result<HashSet<String>> {
    let mut reader = Reader::from_str(xml);
    let mut prefixes = HashSet::new();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    loop {
        let event = reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let element = match &event {
            Event::Start(element) | Event::Empty(element) => element,
            Event::Eof => return Ok(prefixes),
            _ => continue,
        };
        let name = element.name();
        prefixes.insert(
            name.prefix()
                .map_or(String::new(), |prefix| text(prefix.as_ref())),
        );
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            let key = attribute.key;
            if key.as_ref() == b"xmlns" || key.as_ref().starts_with(b"xmlns:") {
                continue;
            }
            if let Some(prefix) = key.prefix() {
                prefixes.insert(text(prefix.as_ref()));
            }
            if matches!(
                key.local_name().as_ref(),
                b"Ignorable"
                    | b"Requires"
                    | b"ProcessContent"
                    | b"MustUnderstand"
                    | b"PreserveElements"
                    | b"PreserveAttributes"
            ) {
                let value = oxoffice_core::xml::attribute_text(&attribute)
                    .map_err(|error| Error::Other(error.to_string()))?;
                for token in value.split_whitespace() {
                    prefixes.insert(token.split(':').next().unwrap_or(token).to_owned());
                }
            }
        }
    }
}

/// Read encoding facts from an actual carrier. The guard never invokes the
/// comparison converter to produce expected source or an expected model.
pub(super) fn conversion_source_inventory(
    source: &str,
    inherited: &NamespaceBindings,
) -> Result<(String, BTreeMap<String, String>, String, String)> {
    fn element(
        source: &str,
        scope: &NamespaceBindings,
        expected: &str,
        allowed: &[&str],
    ) -> Result<BTreeMap<String, (String, String)>> {
        let closed = actual_field_source_fragment(source, scope)?;
        let mut reader = NsReader::from_str(&closed);
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let (Event::Start(root) | Event::Empty(root)) = event else {
            return Err(Error::Other("conversion inventory has no element".into()));
        };
        if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            || root.local_name().as_ref() != expected.as_bytes()
        {
            return Err(Error::Other(
                "conversion inventory has an unproved qualified carrier".into(),
            ));
        }
        let mut attributes = BTreeMap::new();
        for attribute in root.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
                continue;
            }
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            let xml_space = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == b"http://www.w3.org/XML/1998/namespace")
                && local.as_ref() == b"space";
            let name = if xml_space {
                "xml:space".to_owned()
            } else {
                if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                {
                    return Err(Error::Other(
                        "unconsumed conversion carrier attribute namespace".into(),
                    ));
                }
                std::str::from_utf8(local.as_ref())
                    .map_err(utf8_error)?
                    .to_owned()
            };
            if !allowed.contains(&name.as_str()) {
                return Err(Error::Other(
                    "unconsumed conversion carrier attribute".into(),
                ));
            }
            let lexical = std::str::from_utf8(attribute.value.as_ref())
                .map_err(utf8_error)?
                .to_owned();
            let decoded = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
                .map_err(|error| Error::Other(error.to_string()))?
                .into_owned();
            if attributes.insert(name, (lexical, decoded)).is_some() {
                return Err(Error::Other(
                    "duplicate conversion carrier attribute".into(),
                ));
            }
        }
        Ok(attributes)
    }
    fn run_child<'a>(run: &'a str, scope: &NamespaceBindings) -> Result<&'a str> {
        element(run, scope, "r", &[])?;
        let inner = element_inner_range_any_prefix(run, "r")?;
        let children = direct_element_spans(run)?;
        if children.len() != 1
            || !run[inner.start..children[0].start].trim().is_empty()
            || !run[children[0].end..inner.end].trim().is_empty()
        {
            return Err(Error::Other(
                "conversion marker run has additional source payload".into(),
            ));
        }
        Ok(&run[children[0].clone()])
    }
    fn marker(
        run: &str,
        scope: &NamespaceBindings,
        kind: &str,
    ) -> Result<(BTreeMap<String, String>, String)> {
        let raw = run_child(run, scope)?;
        let mut attributes = element(raw, scope, "fldChar", &["fldCharType", "dirty", "fldLock"])?;
        if attributes
            .remove("fldCharType")
            .map(|(_, value)| value)
            .as_deref()
            != Some(kind)
        {
            return Err(Error::Other(
                "conversion marker kind differs from actual carrier geometry".into(),
            ));
        }
        if kind != "begin" && !attributes.is_empty() {
            return Err(Error::Other(
                "conversion non-begin marker has unexpected flags".into(),
            ));
        }
        let data = if element_opening(raw)?.1 {
            String::new()
        } else {
            let inner = element_inner_range_any_prefix(raw, "fldChar")?;
            let children = direct_element_spans(raw)?;
            if kind != "begin"
                || children.len() != 1
                || !raw[inner.start..children[0].start].trim().is_empty()
                || !raw[children[0].end..inner.end].trim().is_empty()
            {
                return Err(Error::Other(
                    "conversion marker has unconsumed owned source".into(),
                ));
            }
            let data = &raw[children[0].clone()];
            let closed = actual_field_source_fragment(data, scope)?;
            let mut reader = NsReader::from_str(&closed);
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|error| Error::Other(error.to_string()))?;
            if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                || !matches!(event, Event::Start(ref root) | Event::Empty(ref root) if root.local_name().as_ref() == b"fldData")
            {
                return Err(Error::Other(
                    "conversion marker data has no actual fldData owner".into(),
                ));
            }
            data.to_owned()
        };
        Ok((
            attributes
                .into_iter()
                .map(|(name, (lexical, _))| (name, lexical))
                .collect(),
            data,
        ))
    }
    // Namespace declarations introduced by closure may differ at a result root.
    // Compare their actual effective bindings, every other root attribute and
    // the complete untouched descendant bytes independently.
    // A field nested in the result is read as its own facts, in either form:
    // the converter writes a nested simple field as a complex one.
    fn result_facts(result: &str, scope: &NamespaceBindings) -> Result<String> {
        fn fld_char_kind(run: &str, scope: &NamespaceBindings) -> Option<String> {
            let raw = run_child(run, scope).ok()?;
            let attributes =
                element(raw, scope, "fldChar", &["fldCharType", "dirty", "fldLock"]).ok()?;
            attributes
                .get("fldCharType")
                .map(|(_, value)| value.clone())
        }
        let wrapped = format!("<result>{result}</result>");
        let spans = direct_element_spans(&wrapped)?;
        let mut output = String::new();
        let mut cursor = "<result>".len();
        let mut index = 0;
        while index < spans.len() {
            let span = spans[index].clone();
            let nested_end = if element(
                &wrapped[span.clone()],
                scope,
                "fldSimple",
                &["instr", "dirty", "fldLock"],
            )
            .is_ok()
            {
                Some(index)
            } else if fld_char_kind(&wrapped[span.clone()], scope).as_deref() == Some("begin") {
                let mut depth = 0usize;
                spans[index..]
                    .iter()
                    .position(|run| {
                        match fld_char_kind(&wrapped[run.clone()], scope).as_deref() {
                            Some("begin") => depth += 1,
                            Some("end") => depth -= 1,
                            _ => {}
                        }
                        depth == 0
                    })
                    .map(|offset| index + offset)
            } else {
                None
            };
            if let Some(end) = nested_end {
                let source = &wrapped[span.start..spans[end].end];
                if let Ok(facts) = conversion_source_inventory(source, scope) {
                    output.push_str(&wrapped[cursor..span.start]);
                    output.push_str(&format!("\0field{facts:?}"));
                    cursor = spans[end].end;
                    index = end + 1;
                    continue;
                }
            }
            index += 1;
            output.push_str(&wrapped[cursor..span.start]);
            let (lexical, bindings) = raw_occurrence_representation(&wrapped[span.clone()], scope)?;
            let bindings = conversion_namespace_facts(bindings, &wrapped[span.clone()])?;
            output.push_str(&format!("{bindings:?}"));
            output.push_str(&lexical);
            cursor = span.end;
        }
        output.push_str(&wrapped[cursor..wrapped.len() - "</result>".len()]);
        Ok(output)
    }
    let scope = actual_root_bindings(source, inherited)?;
    if !actual_field_is_complex(source, inherited)? {
        let mut attributes = element(source, &scope, "fldSimple", &["instr", "dirty", "fldLock"])?;
        let instruction = attributes
            .remove("instr")
            .ok_or_else(|| Error::Other("simple conversion source lacks instruction".into()))?
            .1;
        let expanded = append_word_child(source, "fldSimple", "")?;
        let inner = element_inner_range_any_prefix(&expanded, "fldSimple")?;
        let data = direct_word_element_spans_in_scope(&expanded, "fldData", &scope)?;
        if data.len() > 1
            || data
                .first()
                .is_some_and(|span| !expanded[inner.start..span.start].trim().is_empty())
        {
            return Err(Error::Other(
                "simple conversion source has ambiguous data".into(),
            ));
        }
        let start = data.first().map_or(inner.start, |span| span.end);
        return Ok((
            instruction,
            attributes
                .into_iter()
                .map(|(name, (raw, _))| (name, raw))
                .collect(),
            data.first()
                .map_or(String::new(), |span| expanded[span.clone()].to_owned()),
            result_facts(&expanded[start..inner.end], &scope)?,
        ));
    }
    let wrapped = format!("<carrier>{source}</carrier>");
    let spans = direct_element_spans(&wrapped)?;
    if spans.len() < 4 {
        return Err(Error::Other(
            "converted complex source lacks pure carrier runs".into(),
        ));
    }
    let (flags, data) = marker(&wrapped[spans[0].clone()], &scope, "begin")?;
    let instruction = run_child(&wrapped[spans[1].clone()], &scope)?;
    let attributes = element(instruction, &scope, "instrText", &["xml:space"])?;
    if attributes.get("xml:space").map(|(_, value)| value.as_str()) != Some("preserve") {
        return Err(Error::Other(
            "converted instruction lost its whitespace contract".into(),
        ));
    }
    let inner = element_inner_range_any_prefix(instruction, "instrText")?;
    if !direct_element_spans(instruction)?.is_empty() {
        return Err(Error::Other(
            "converted instruction has unconsumed child source".into(),
        ));
    }
    let instruction = quick_xml::escape::unescape(&instruction[inner])
        .map_err(|error| Error::Other(error.to_string()))?
        .into_owned();
    marker(&wrapped[spans[2].clone()], &scope, "separate")?;
    let last = spans.last().unwrap();
    marker(&wrapped[last.clone()], &scope, "end")?;
    if !wrapped["<carrier>".len()..spans[0].start].trim().is_empty()
        || !wrapped[last.end..wrapped.len() - "</carrier>".len()]
            .trim()
            .is_empty()
        || !wrapped[spans[0].end..spans[1].start].trim().is_empty()
        || !wrapped[spans[1].end..spans[2].start].trim().is_empty()
    {
        return Err(Error::Other(
            "converted carrier has unconsumed outside source".into(),
        ));
    }
    Ok((
        instruction,
        flags,
        data,
        result_facts(&wrapped[spans[2].end..last.start], &scope)?,
    ))
}

/// Map only the qualified clean source roles. This is an encoding change, not
/// Field re-authoring: the XML-normalized attribute value supplies instrText.
fn clean_simple_complex_source(
    xml: &str,
    inherited: &NamespaceBindings,
    field: Option<&oxoffice_docx_oxml::text::Field>,
) -> Result<String> {
    let closed = actual_field_source_fragment(xml, inherited)?;
    let scope = actual_root_bindings(xml, inherited)?;
    let mut reader = NsReader::from_str(&closed);
    let (namespace, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()) {
        return Err(Error::Other(
            "unconsumed simple-field source root namespace".into(),
        ));
    }
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return Err(Error::Other(
            "simple-field conversion has no actual source root".into(),
        ));
    };
    if root.local_name().as_ref() != b"fldSimple" {
        return Err(Error::Other(
            "simple-field conversion source is not fldSimple".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut instruction = None;
    let mut word_prefix = None;
    let mut flags = Vec::new();
    for attribute in root.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        if attribute.key.as_ref() == b"xmlns" || attribute.key.as_ref().starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let uri = match namespace {
            ResolveResult::Bound(uri) => uri.as_ref().to_vec(),
            ResolveResult::Unbound => Vec::new(),
            ResolveResult::Unknown(prefix) => {
                return Err(Error::Other(format!(
                    "unbound simple-field source attribute prefix {prefix:?}"
                )));
            }
        };
        if !seen.insert((uri.clone(), local.as_ref().to_vec())) {
            return Err(Error::Other(
                "duplicate expanded simple-field source attribute".into(),
            ));
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
            .map_err(|error| Error::Other(error.to_string()))?;
        if uri != W_NS.as_bytes() {
            return Err(Error::Other(format!(
                "unconsumed simple-field root attribute {}",
                String::from_utf8_lossy(attribute.key.as_ref())
            )));
        }
        match local.as_ref() {
            b"instr" => {
                let name = std::str::from_utf8(attribute.key.as_ref()).map_err(utf8_error)?;
                word_prefix = name.split_once(':').map(|(prefix, _)| prefix.to_owned());
                instruction = Some(value.into_owned());
            }
            b"dirty" | b"fldLock" => {
                if oxoffice_docx_oxml::shared::ST_OnOff::parse(&value).is_none() {
                    return Err(Error::Other(format!(
                        "unmappable simple-field flag {}={value:?}",
                        String::from_utf8_lossy(local.as_ref())
                    )));
                }
                flags.push((
                    std::str::from_utf8(attribute.key.as_ref())
                        .map_err(utf8_error)?
                        .to_owned(),
                    std::str::from_utf8(attribute.value.as_ref())
                        .map_err(utf8_error)?
                        .to_owned(),
                ));
            }
            _ => {
                return Err(Error::Other(format!(
                    "unconsumed simple-field root attribute {}",
                    String::from_utf8_lossy(attribute.key.as_ref())
                )));
            }
        }
    }
    let prefix = word_prefix.ok_or_else(|| {
        Error::Other("simple-field instruction lacks a qualified actual Word prefix".into())
    })?;
    let instruction = instruction
        .ok_or_else(|| Error::Other("simple-field source instruction is missing".into()))?;
    let expanded = append_word_child(xml, "fldSimple", "")?;
    let inner = element_inner_range_any_prefix(&expanded, "fldSimple")?;
    let data = direct_word_element_spans_in_scope(&expanded, "fldData", &scope)?;
    if data.len() > 1
        || data
            .first()
            .is_some_and(|span| !expanded[inner.start..span.start].trim().is_empty())
    {
        return Err(Error::Other(
            "multiple or late simple-field fldData source owners".into(),
        ));
    }
    let result_start = data.first().map_or(inner.start, |span| span.end);
    let result = nested_simple_fields_as_complex(&expanded[result_start..inner.end], &scope)?;
    let result = qualified_complete_field_payload(&result, &scope, false, false, true, &mut 0)?;
    let mut namespace_root = BytesStart::new(format!("{prefix}:r"));
    for (name, uri) in &scope {
        namespace_root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(namespace_root))
        .map_err(|error| Error::Other(error.to_string()))?;
    let run_open = String::from_utf8(writer.into_inner()).map_err(utf8_error)?;
    let run_close = format!("</{prefix}:r>");
    let mut begin = format!("<{prefix}:fldChar {prefix}:fldCharType=\"begin\"");
    for (name, value) in flags {
        begin.push_str(&format!(" {name}=\"{value}\""));
    }
    if let Some(span) = data.first() {
        begin.push('>');
        begin.push_str(&expanded[span.clone()]);
        begin.push_str(&format!("</{prefix}:fldChar>"));
    } else {
        begin.push_str("/>");
    }
    let output = format!(
        "{run_open}{begin}{run_close}{run_open}<{prefix}:instrText xml:space=\"preserve\">{}</{prefix}:instrText>{run_close}{run_open}<{prefix}:fldChar {prefix}:fldCharType=\"separate\"/>{run_close}{result}{run_open}<{prefix}:fldChar {prefix}:fldCharType=\"end\"/>{run_close}",
        quick_xml::escape::escape(&instruction)
    );
    // A nested field is proved through the root field that holds it.
    let Some(field) = field else {
        return Ok(output);
    };
    // An independent read of emitted source must actually group a root Field.
    // No witness is allowed to fill a missing parsed owner or cached display.
    let root = format!("<w:p xmlns:w=\"{W_NS}\">{output}</w:p>");
    let model = story_document(&root, &Vec::new())?;
    let Some(BodyContent::Paragraph(paragraph)) = model.body.content.first() else {
        return Err(Error::Other(
            "converted source has no actual root paragraph".into(),
        ));
    };
    let fields = paragraph
        .runs
        .iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| {
            if let RunContent::Field(field) = content {
                Some(field)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if fields.len() != 1
        || !fields[0].is_complex()
        || fields[0].effective_instruction().name != field.effective_instruction().name
        || fields[0].effective_instruction().arguments != field.effective_instruction().arguments
        || fields[0].effective_instruction().switches != field.effective_instruction().switches
        || fields[0].cached_result != field.cached_result
        || fields[0].dirty != field.dirty
        || fields[0].locked() != field.locked()
    {
        return Err(Error::Other(
            "converted source did not reconstruct its actual root field instruction/cache/flags"
                .into(),
        ));
    }
    Ok(output)
}

/// A simple field's result with each simple field it holds written as a
/// complex field too: Word nests complex fields only, and a whole field
/// payload holds runs.
fn nested_simple_fields_as_complex(result: &str, scope: &NamespaceBindings) -> Result<String> {
    let (opening, closing) = ("<nested>", "</nested>");
    let wrapped = format!("{opening}{result}{closing}");
    let spans = direct_word_element_spans_in_scope(&wrapped, "fldSimple", scope)?;
    let mut output = wrapped.clone();
    for span in spans.into_iter().rev() {
        let nested = clean_simple_complex_source(&wrapped[span.clone()], scope, None)?;
        output.replace_range(span, &nested);
    }
    // One reached through a control, smart tag or custom XML would stay a
    // simple field, whose text a deletion cannot mark (Word repairs it).
    let mut reader = Reader::from_str(&output);
    loop {
        match reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?
        {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == b"fldSimple" =>
            {
                return Err(Error::Other(
                    "comparison cannot track a whole paragraph's simple field nested in another field's result inside another owner"
                        .into(),
                ));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(output[opening.len()..output.len() - closing.len()].to_owned())
}

/// One simple field of a paragraph inserted or deleted whole, as the complex
/// field that `compare_field_xml` writes for a change of form, in a wrapper
/// of its own under one new revision id. The witness names the occurrence of
/// the input that keeps the paragraph; with fields ignored, the guard
/// compares no field and none is recorded.
pub(super) fn whole_simple_field(
    field: &oxoffice_docx_oxml::text::Field,
    deleted: bool,
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let scope = field_result_scope(field, metadata);
    let complex = clean_simple_complex_source(&field_xml(field)?, &scope, Some(field))?;
    let payload = qualified_complete_field_payload(&complex, &scope, deleted, false, true, &mut 0)?;
    let id = metadata.ids.allocate()?;
    let wrapper = metadata.ids.generated(IdAllocator::revision_with_id(
        if deleted { "del" } else { "ins" },
        metadata.author,
        metadata.timestamp,
        &payload,
        id,
    ))?;
    if !metadata.options.ignore_fields {
        let source = metadata
            .source_frames
            .iter()
            .rev()
            .find_map(|frame| {
                frame
                    .field_occurrences
                    .get(&(std::ptr::from_ref(field) as usize))
            })
            .cloned()
            .ok_or_else(|| {
                Error::Other("whole-paragraph field has no physical source occurrence".into())
            })?;
        if source.input_role != usize::from(!deleted) {
            return Err(Error::Other(
                "whole-paragraph field occurrence is on the other input".into(),
            ));
        }
        metadata.whole_field_conversions.push(WholeFieldWitness {
            source,
            revision_id: id,
            generated_span: payload,
        });
    }
    Ok(wrapper)
}

/// Each direct hyperlink of a paragraph inserted or deleted whole, written as
/// one `HYPERLINK` complex field, the whole of it in an `ins` (or, `deleted`,
/// a `del`) of its own under one new revision id: the hyperlink element
/// cannot carry a mark, and Word keeps it, emptied, when it resolves the
/// paragraph away. That is the shape Word writes itself for such a link.
///
/// `xml` is the paragraph's XML and `published` the wrapper spans already in
/// it; the new wrappers join them. A link holding one of them (a simple field
/// written whole, in a wrapper of its own) keeps its element: Word nests no
/// revision wrapper in another. A link with an attribute no field switch
/// carries is refused, except `w:history`, which Word drops too.
pub(super) fn whole_paragraph_hyperlinks(
    paragraph: &CT_P,
    xml: &mut String,
    published: &mut Vec<Range<usize>>,
    deleted: bool,
    metadata: &mut Metadata<'_>,
) -> Result<()> {
    if paragraph.hyperlinks.is_empty() {
        return Ok(());
    }
    // The writer's own prefixes, and those the paragraph's source binds: a
    // link attribute kept as read keeps the prefix of its source
    // (`ox0:history`), declared on the part's root.
    let mut scope: NamespaceBindings = match metadata.paragraph_source(paragraph) {
        Some(source) => actual_root_bindings(source, &Vec::new())?
            .into_iter()
            .filter(|(name, _)| name != "xmlns:w" && name != "xmlns:r")
            .collect(),
        None => Vec::new(),
    };
    scope.push(("xmlns:w".into(), W_NS.into()));
    scope.push(("xmlns:r".into(), R_NS.into()));
    let scope = actual_root_bindings(xml, &scope)?;
    let spans = direct_word_element_spans_in_scope(xml, "hyperlink", &scope)?;
    if spans.len() != paragraph.hyperlinks.len() {
        return Err(Error::Other(
            "whole-paragraph hyperlinks differ from the paragraph's modeled links".into(),
        ));
    }
    for span in spans.into_iter().rev() {
        if published
            .iter()
            .any(|range| range.start < span.end && span.start < range.end)
        {
            continue;
        }
        let link = &xml[span.clone()];
        let (opening_end, empty, _) = element_opening(link)?;
        if empty {
            continue;
        }
        let (instruction, opening) =
            hyperlink_field_instruction(&link[..opening_end], &scope, metadata)?;
        let inner = element_inner_range_any_prefix(link, "hyperlink")?;
        let complex = format!(
            r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
            quick_xml::escape::partial_escape(&instruction),
            &link[inner]
        );
        let start = span.start + opening_end;
        let parent = metadata
            .ids
            .revision_parent_scope(&ParentScopes::new(xml, vec![start]).at(start)?);
        let payload =
            qualified_complete_field_payload(&complex, &parent, deleted, false, true, &mut 0)?;
        let id = metadata.ids.allocate()?;
        let wrapper = metadata.ids.generated(IdAllocator::revision_with_id(
            if deleted { "del" } else { "ins" },
            metadata.author,
            metadata.timestamp,
            &payload,
            id,
        ))?;
        let shift = wrapper.len() as isize - span.len() as isize;
        for range in published.iter_mut() {
            if range.start >= span.end {
                *range = range.start.saturating_add_signed(shift)
                    ..range.end.saturating_add_signed(shift);
            }
        }
        published.push(span.start..span.start + wrapper.len());
        xml.replace_range(span, &wrapper);
        metadata
            .whole_hyperlinks
            .witnesses
            .push(WholeHyperlinkWitness {
                owner: metadata.whole_hyperlinks.owner.clone(),
                input_role: usize::from(!deleted),
                revision_id: id,
                generated_span: payload,
                opening,
            });
    }
    published.sort_by_key(|range| range.start);
    Ok(())
}

/// The `HYPERLINK` instruction that carries the link whose start tag is
/// `opening`, and the qualified names of its attributes in order, with the
/// value of those no switch carries (see `WholeHyperlinkWitness`).
fn hyperlink_field_instruction(
    opening: &str,
    scope: &NamespaceBindings,
    metadata: &Metadata<'_>,
) -> Result<(String, HyperlinkOpening)> {
    // The declarations the link itself writes, not those closing it adds.
    let mut written = HashSet::new();
    if let Event::Start(element) = Reader::from_str(opening)
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?
    {
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            written.insert(attribute.key.as_ref().to_vec());
        }
    }
    let closed = actual_field_source_fragment(&format!("{opening}</w:hyperlink>"), scope)?;
    let mut reader = NsReader::from_str(&closed);
    let (_, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let Event::Start(element) = event else {
        return Err(Error::Other(
            "whole-paragraph hyperlink has no start tag".into(),
        ));
    };
    let mut names = Vec::new();
    let mut target = None;
    let mut switches = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        if attribute.key.as_namespace_binding().is_some() {
            if !written.contains(attribute.key.as_ref()) {
                continue;
            }
            names.push((
                String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
                Some(
                    attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?
                        .into_owned(),
                ),
            ));
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| Error::Other(error.to_string()))?
            .into_owned();
        // The name as the link writes it: its prefix may be another than `w`.
        let written = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        match (namespace, local.as_ref()) {
            (ResolveResult::Bound(Namespace(uri)), b"id") if uri == R_NS.as_bytes() => {
                let (address, external) = metadata
                    .whole_hyperlinks
                    .targets
                    .get(&metadata.whole_hyperlinks.owner)
                    .and_then(|targets| targets.get(&value))
                    .ok_or_else(|| {
                        Error::Other(format!(
                            "a hyperlink of a paragraph added or removed whole names relationship {value}, which has no target"
                        ))
                    })?;
                if !external {
                    return Err(Error::Other(format!(
                        "a hyperlink of a paragraph added or removed whole targets a part of the package ({address}), which no HYPERLINK field names"
                    )));
                }
                target = Some(address.clone());
            }
            (ResolveResult::Bound(Namespace(uri)), local) if uri == W_NS.as_bytes() => {
                match local {
                    b"anchor" => switches.push(("l", value)),
                    b"tooltip" => switches.push(("o", value)),
                    b"tgtFrame" => switches.push(("t", value)),
                    b"history" => {
                        names.push((written, Some(value)));
                        continue;
                    }
                    _ => {
                        return Err(Error::Other(format!(
                            "a hyperlink of a paragraph added or removed whole has w:{}, which no HYPERLINK field switch carries",
                            String::from_utf8_lossy(local)
                        )));
                    }
                }
            }
            _ => {
                return Err(Error::Other(format!(
                    "a hyperlink of a paragraph added or removed whole has {}, which no HYPERLINK field switch carries",
                    String::from_utf8_lossy(attribute.key.as_ref())
                )));
            }
        }
        names.push((written, None));
    }
    let quoted = |value: &str| format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    let mut instruction = String::from("HYPERLINK");
    if let Some(target) = target {
        instruction.push(' ');
        instruction.push_str(&quoted(&target));
    }
    for (switch, value) in switches {
        instruction.push_str(&format!(" \\{switch} {}", quoted(&value)));
    }
    Ok((instruction, names))
}

/// Qualify an ordinary whole-field run-revision payload at its actual owners.
/// Direct runs use the existing qualified child scanner. Opaque run payloads
/// and real nested control/wrapper custody retain their own source authority.
fn qualified_complete_field_payload(
    source: &str,
    scope: &NamespaceBindings,
    deleted: bool,
    paragraph_content: bool,
    close_roots: bool,
    field_depth: &mut usize,
) -> Result<String> {
    let initial_depth = *field_depth;
    let mut root = BytesStart::new("fieldPayload");
    for (name, uri) in scope {
        root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(root))
        .map_err(|error| Error::Other(error.to_string()))?;
    let opening = String::from_utf8(writer.into_inner()).map_err(utf8_error)?;
    let wrapped = format!("{opening}{source}</fieldPayload>");
    let mut output = String::new();
    let mut cursor = opening.len();
    for span in direct_element_spans(&wrapped)? {
        if !wrapped[cursor..span.start].trim().is_empty() {
            return Err(Error::Other(
                "unconsumed direct field-result source event".into(),
            ));
        }
        output.push_str(&wrapped[cursor..span.start]);
        let raw = &wrapped[span.clone()];
        let closed = actual_field_source_fragment(raw, scope)?;
        let mut reader = NsReader::from_str(&closed);
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let (Event::Start(element) | Event::Empty(element)) = event else {
            return Err(Error::Other("field-result source child is missing".into()));
        };
        if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()) {
            return Err(Error::Other(format!(
                "unproved direct field-result owner {}",
                String::from_utf8_lossy(element.name().as_ref())
            )));
        }
        let local = element.local_name();
        if !(paragraph_content && matches!(local.as_ref(), b"fldSimple" | b"hyperlink"))
            && !matches!(
                local.as_ref(),
                b"r" | b"sdt"
                    | b"customXml"
                    | b"smartTag"
                    | b"dir"
                    | b"bdo"
                    | b"proofErr"
                    | b"permStart"
                    | b"permEnd"
                    | b"bookmarkStart"
                    | b"bookmarkEnd"
                    | b"commentRangeStart"
                    | b"commentRangeEnd"
                    | b"moveFromRangeStart"
                    | b"moveFromRangeEnd"
                    | b"moveToRangeStart"
                    | b"moveToRangeEnd"
                    | b"customXmlInsRangeStart"
                    | b"customXmlInsRangeEnd"
                    | b"customXmlDelRangeStart"
                    | b"customXmlDelRangeEnd"
                    | b"customXmlMoveFromRangeStart"
                    | b"customXmlMoveFromRangeEnd"
                    | b"customXmlMoveToRangeStart"
                    | b"customXmlMoveToRangeEnd"
                    | b"ins"
                    | b"del"
                    | b"moveFrom"
                    | b"moveTo"
            )
        {
            return Err(Error::Other(format!(
                "unproved whole-field run-revision child {}",
                String::from_utf8_lossy(element.name().as_ref())
            )));
        }
        let local = local.as_ref().to_vec();
        let child_scope = actual_root_bindings(raw, scope)?;
        let mut child = if close_roots { closed } else { raw.to_owned() };
        if local == b"sdt" {
            let children = direct_element_spans(&child)?;
            let mut previous = None;
            let mut seen = HashSet::new();
            for span in children.into_iter().rev() {
                let raw = &child[span.clone()];
                let scoped = actual_field_source_fragment(raw, &child_scope)?;
                let mut reader = NsReader::from_str(&scoped);
                let (namespace, event) = reader
                    .read_resolved_event()
                    .map_err(|error| Error::Other(error.to_string()))?;
                let (Event::Start(element) | Event::Empty(element)) = event else {
                    return Err(Error::Other("control source child is missing".into()));
                };
                if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                {
                    return Err(Error::Other(
                        "unproved foreign direct inline-control child".into(),
                    ));
                }
                let role = match element.local_name().as_ref() {
                    b"sdtPr" => 0,
                    b"sdtEndPr" => 1,
                    b"sdtContent" => 2,
                    _ => {
                        return Err(Error::Other(
                            "unproved block/opaque inline-control child".into(),
                        ));
                    }
                };
                if !seen.insert(role) || previous.is_some_and(|next| role >= next) {
                    return Err(Error::Other(
                        "inline-control source order or multiplicity is unproved".into(),
                    ));
                }
                previous = Some(role);
                if role == 2 && !element_opening(raw)?.1 {
                    let content_scope = actual_root_bindings(raw, &child_scope)?;
                    let content = element_inner_range_any_prefix(raw, "sdtContent")?;
                    let replacement = qualified_complete_field_payload(
                        &raw[content.clone()],
                        &content_scope,
                        deleted,
                        true,
                        false,
                        field_depth,
                    )?;
                    let mut current = raw.to_owned();
                    current.replace_range(content, &replacement);
                    child.replace_range(span, &current);
                }
            }
        } else if matches!(
            local.as_slice(),
            b"customXml"
                | b"smartTag"
                | b"dir"
                | b"bdo"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
                | b"hyperlink"
                | b"fldSimple"
        ) && !element_opening(&child)?.1
        {
            let name = std::str::from_utf8(&local).map_err(utf8_error)?;
            let inner = element_inner_range_any_prefix(&child, name)?;
            let property = match local.as_slice() {
                b"customXml" => Some("customXmlPr"),
                b"smartTag" => Some("smartTagPr"),
                b"fldSimple" => Some("fldData"),
                _ => None,
            };
            let mut start = inner.start;
            if let Some(property) = property {
                let properties =
                    direct_word_element_spans_in_scope(&child, property, &child_scope)?;
                if properties.len() > 1
                    || properties
                        .first()
                        .is_some_and(|span| !child[start..span.start].trim().is_empty())
                {
                    return Err(Error::Other(format!(
                        "unproved {name} property order or multiplicity"
                    )));
                }
                if let Some(property) = properties.first() {
                    start = property.end;
                }
            }
            let paragraph = !matches!(local.as_slice(), b"ins" | b"del" | b"moveFrom" | b"moveTo");
            let replacement = qualified_complete_field_payload(
                &child[start..inner.end],
                &child_scope,
                deleted && local != b"fldSimple",
                paragraph,
                false,
                field_depth,
            )?;
            child.replace_range(start..inner.end, &replacement);
        }
        if local == b"r" {
            for forbidden in [
                "fldSimple",
                "hyperlink",
                "sdt",
                "p",
                "tbl",
                "ins",
                "del",
                "moveFrom",
                "moveTo",
            ] {
                if !direct_word_element_spans_in_scope(&child, forbidden, &child_scope)?.is_empty()
                {
                    return Err(Error::Other(format!(
                        "unproved {forbidden} inside an actual result run"
                    )));
                }
            }
        }
        if local == b"r" {
            let mut renames = Vec::new();
            for direct in direct_element_spans(&child)? {
                let raw = &child[direct.clone()];
                let scoped = actual_field_source_fragment(raw, &child_scope)?;
                let mut reader = NsReader::from_str(&scoped);
                let (namespace, event) = reader
                    .read_resolved_event()
                    .map_err(|error| Error::Other(error.to_string()))?;
                let (Event::Start(element) | Event::Empty(element)) = event else {
                    continue;
                };
                if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                {
                    continue;
                }
                let local = element.local_name();
                if local.as_ref() == b"fldChar" {
                    let mut kind = None;
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                        if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                            && name.as_ref() == b"fldCharType"
                        {
                            kind = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .map_err(|error| Error::Other(error.to_string()))?
                                    .into_owned(),
                            );
                        }
                    }
                    match kind.as_deref() {
                        Some("begin") => {
                            *field_depth = field_depth.checked_add(1).ok_or_else(|| {
                                Error::Other("field deletion depth overflow".into())
                            })?
                        }
                        Some("end") => {
                            *field_depth = field_depth.checked_sub(1).ok_or_else(|| {
                                Error::Other("field deletion has an unmatched end".into())
                            })?
                        }
                        Some("separate") if *field_depth > 0 => {}
                        _ => {
                            return Err(Error::Other(
                                "unproved field marker ownership during complete-span publication"
                                    .into(),
                            ));
                        }
                    }
                } else if deleted && *field_depth > 0 {
                    // Every text of a deleted field is deleted text, that of
                    // a field nested in it too: Word repairs a deletion that
                    // holds a w:t or a w:instrText.
                    let target = match local.as_ref() {
                        b"t" => Some("delText"),
                        b"instrText" => Some("delInstrText"),
                        _ => None,
                    };
                    if let Some(target) = target {
                        renames.push((direct, target));
                    }
                }
            }
            for (text, target) in renames.into_iter().rev() {
                let raw = &child[text.clone()];
                let (_, empty, name) = element_opening(raw)?;
                let target = name
                    .rsplit_once(':')
                    .map_or(target.to_owned(), |(prefix, _)| {
                        format!("{prefix}:{target}")
                    });
                let mut renamed = raw.to_owned();
                if !empty {
                    let close = paragraph_close_start(raw)?;
                    renamed.replace_range(close + 2..close + 2 + name.len(), &target);
                }
                renamed.replace_range(1..1 + name.len(), &target);
                child.replace_range(text, &renamed);
            }
        }
        output.push_str(&child);
        cursor = span.end;
    }
    let end = wrapped.len() - "</fieldPayload>".len();
    if !wrapped[cursor..end].trim().is_empty() {
        return Err(Error::Other(
            "unconsumed trailing field-result source event".into(),
        ));
    }
    output.push_str(&wrapped[cursor..end]);
    if *field_depth != initial_depth {
        return Err(Error::Other(
            "field markers cross their actual enclosing source owner".into(),
        ));
    }
    Ok(output)
}

fn field_xml(field: &oxoffice_docx_oxml::text::Field) -> Result<String> {
    paragraph_owned_run_xml(&CT_R {
        properties: None,
        content: vec![RunContent::Field(field.clone())],
        extra_xml: Vec::new(),
        extra_xml_positions: Vec::new(),
        alt_drawings: Vec::new(),
    })
}

pub(super) fn prepared_field_result_owner(
    field: &oxoffice_docx_oxml::text::Field,
    source: &str,
) -> Result<CT_P> {
    if let Some(owner) = field.cached_result_owner_projection()? {
        return Ok(owner);
    }
    if field.cached_result.is_empty() && source.as_bytes().iter().all(u8::is_ascii_whitespace) {
        return Ok(CT_P::default());
    }
    let runs = field.cached_result_physical_runs().ok_or_else(|| {
        Error::Other(
            "field result has neither a current ordered owner nor a proved contiguous run owner"
                .into(),
        )
    })?;
    Ok(CT_P {
        runs: runs.to_vec(),
        ..Default::default()
    })
}

pub(super) fn field_result_source_paragraph(
    source: &str,
    scope: &NamespaceBindings,
) -> Result<String> {
    let prefix = scope
        .iter()
        .find(|(_, uri)| uri == W_NS)
        .map(|(name, _)| name.strip_prefix("xmlns:").unwrap_or(""))
        .ok_or_else(|| Error::Other("field result has no actual Word namespace binding".into()))?;
    let name = if prefix.is_empty() {
        "p".to_owned()
    } else {
        format!("{prefix}:p")
    };
    let mut root = BytesStart::new(name.as_str());
    for (name, uri) in scope {
        root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(root))
        .map_err(|error| Error::Other(error.to_string()))?;
    writer.get_mut().extend_from_slice(source.as_bytes());
    writer
        .write_event(Event::End(quick_xml::events::BytesEnd::new(name.as_str())))
        .map_err(|error| Error::Other(error.to_string()))?;
    String::from_utf8(writer.into_inner()).map_err(utf8_error)
}

fn tracked_field_result(
    fields: [&oxoffice_docx_oxml::text::Field; 2],
    original: &str,
    edited: &str,
    scopes: [&NamespaceBindings; 2],
    metadata: &mut Metadata<'_>,
) -> Result<String> {
    let left_model = prepared_field_result_owner(fields[0], original)?;
    let right_model = prepared_field_result_owner(fields[1], edited)?;
    let left = field_result_source_paragraph(original, scopes[0])?;
    let right = field_result_source_paragraph(edited, scopes[1])?;
    let outer = fields
        .iter()
        .map(|field| {
            metadata
                .source_frames
                .iter()
                .rev()
                .find_map(|frame| {
                    frame
                        .field_occurrences
                        .get(&(std::ptr::from_ref(*field) as usize))
                })
                .cloned()
                .ok_or_else(|| {
                    Error::Other("field result has no actual outer source occurrence".into())
                })
        })
        .collect::<Result<Vec<_>>>()?;
    if outer[0].input_role != 0 || outer[1].input_role != 1 || outer[0].story != outer[1].story {
        return Err(Error::Other(
            "field result outer occurrence association changed".into(),
        ));
    }
    let mut result_sources = SourceSnapshots {
        story: outer[0].story.clone(),
        ..Default::default()
    };
    for (role, (model, source)) in [(&left_model, &left), (&right_model, &right)]
        .into_iter()
        .enumerate()
    {
        result_sources.input_role = role;
        let key = std::ptr::from_ref(model) as usize;
        let mut path = outer[role].owner_path.clone();
        path.push((7, outer[role].field_slot));
        result_sources.owner_paths.insert(key, path);
        if !fields[role].cached_fields_in_source_order().is_empty() {
            let result = if role == 0 { original } else { edited };
            match actual_field_result_range(&outer[role].source, &outer[role].scope) {
                Ok(interval) if outer[role].source.get(interval.clone()) == Some(result) => {
                    let mut ancestors = outer[role].result_ancestors.clone();
                    ancestors.push(interval.clone());
                    result_sources.result_ancestors.insert(key, ancestors);
                    result_sources
                        .result_origins
                        .insert(key, (interval, element_opening(source)?.0));
                }
                _ => {
                    result_sources.unproved_result_owners.insert(key);
                }
            }
        }
        result_sources
            .roots
            .insert(key, (source.clone(), scopes[role].clone()));
        result_sources.paragraph(model, source)?;
    }
    let mut options = metadata.options.clone();
    // Cached-result properties are owned by the Field, before the ordinary
    // paragraph ignore-formatting policy. The actual legal child owners recurse
    // through the existing qualified paragraph/control/hyperlink publishers.
    options.ignore_formatting = false;
    let mut inner = Metadata {
        author: metadata.author,
        timestamp: metadata.timestamp,
        options: &options,
        ids: metadata.ids.clone(),
        existing_revisions: metadata.existing_revisions,
        source_frames: vec![result_sources],
        projected_textbox_noop: false,
        leaf_capture: false,
        paired_field_story: metadata
            .paired_field_story
            .as_ref()
            .filter(|story| outer[0].story == **story && outer[1].story == **story)
            .cloned(),
        leaf_publications: Vec::new(),
        field_conversions: Vec::new(),
        whole_field_conversions: Vec::new(),
        shared_bookmarks: HashSet::new(),
        point_bookmarks: HashSet::new(),
        relocated_bookmarks: HashSet::new(),
        whole_hyperlinks: metadata.whole_hyperlinks.scope(),
    };
    let mut diagnostics = Vec::new();
    let result = compare_paragraph(
        &left_model,
        &right_model,
        Some(&left),
        &format!("{}/field[{}]/result", outer[0].story, outer[0].field_slot),
        &mut inner,
        &mut diagnostics,
    )?;
    metadata.ids = inner.ids;
    metadata.field_conversions.extend(inner.field_conversions);
    metadata
        .whole_field_conversions
        .extend(inner.whole_field_conversions);
    metadata
        .whole_hyperlinks
        .witnesses
        .extend(inner.whole_hyperlinks.witnesses);
    let inner_range = element_inner_range_any_prefix(&result, "p")?;
    Ok(result[inner_range].to_owned())
}

/// Locate an unchanged physical result interval without splitting producer runs.
/// Only complete pure marker geometry supplies a nested occurrence anchor.
pub(super) fn actual_field_result_range(
    source: &str,
    inherited: &NamespaceBindings,
) -> Result<Range<usize>> {
    let scope = actual_root_bindings(source, inherited)?;
    if !actual_field_is_complex(source, inherited)? {
        if element_opening(source)?.1 {
            return Err(Error::Other(
                "empty field root has no physical result interval".into(),
            ));
        }
        let inner = element_inner_range_any_prefix(source, "fldSimple")?;
        let data = direct_word_element_spans_in_scope(source, "fldData", &scope)?;
        if data.len() > 1
            || data
                .first()
                .is_some_and(|span| !source[inner.start..span.start].trim().is_empty())
        {
            return Err(Error::Other(
                "field result data boundary is ambiguous".into(),
            ));
        }
        return Ok(data.first().map_or(inner.start, |span| span.end)..inner.end);
    }
    let opening = "<fieldSource>";
    let wrapped = format!("{opening}{source}</fieldSource>");
    let spans = direct_element_spans(&wrapped)?;
    let mut depth = 0usize;
    let mut start = None;
    let mut end = None;
    for (index, span) in spans.iter().enumerate() {
        let run = &wrapped[span.clone()];
        let run_scope = actual_root_bindings(run, &scope)?;
        let closed = actual_field_source_fragment(run, &scope)?;
        let mut reader = NsReader::from_str(&closed);
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
            || !matches!(event, Event::Start(ref root) if root.local_name().as_ref() == b"r")
        {
            continue;
        }
        let children = direct_element_spans(run)?;
        let properties = direct_word_element_spans_in_scope(run, "rPr", &run_scope)?;
        let inner = element_inner_range_any_prefix(run, "r")?;
        let mut cursor = inner.start;
        let mut trivia = true;
        for child in &children {
            trivia &= run[cursor..child.start]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
            cursor = child.end;
        }
        trivia &= run[cursor..inner.end]
            .bytes()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'));
        let pure = trivia
            && properties.len() <= 1
            && children.len() == 1 + properties.len()
            && properties
                .first()
                .is_none_or(|property| children.first() == Some(property));
        for child in &children {
            let raw = actual_field_source_fragment(&run[child.clone()], &run_scope)?;
            let mut reader = NsReader::from_str(&raw);
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|error| Error::Other(error.to_string()))?;
            let (Event::Start(root) | Event::Empty(root)) = event else {
                continue;
            };
            if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                || root.local_name().as_ref() != b"fldChar"
            {
                continue;
            }
            let mut kind = None;
            for attribute in root.attributes() {
                let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes())
                    && name.as_ref() == b"fldCharType"
                {
                    if kind.is_some() {
                        return Err(Error::Other("duplicate physical field marker kind".into()));
                    }
                    kind = Some(
                        attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, root.decoder())
                            .map_err(|error| Error::Other(error.to_string()))?
                            .into_owned(),
                    );
                }
            }
            match kind.as_deref() {
                Some("begin") => {
                    if depth == 0 && (index != 0 || !pure) {
                        return Err(Error::Other(
                            "nested result anchor has a shared or partial begin run".into(),
                        ));
                    }
                    depth = depth
                        .checked_add(1)
                        .ok_or_else(|| Error::Other("physical field depth overflow".into()))?;
                }
                Some("separate") if depth == 1 => {
                    if start.is_some() || !pure {
                        return Err(Error::Other(
                            "nested result anchor has a shared or duplicate separate run".into(),
                        ));
                    }
                    start = Some(span.end - opening.len());
                }
                Some("separate") if depth > 1 => {}
                Some("end") if depth > 0 => {
                    if depth == 1 {
                        if end.is_some() || !pure || index + 1 != spans.len() {
                            return Err(Error::Other(
                                "nested result anchor has a shared or partial end run".into(),
                            ));
                        }
                        end = Some(span.start - opening.len());
                    }
                    depth -= 1;
                }
                _ => {
                    return Err(Error::Other(
                        "unbalanced qualified physical field marker".into(),
                    ));
                }
            }
        }
    }
    let range = start
        .zip(end)
        .filter(|(start, end)| start <= end && depth == 0)
        .map(|(start, end)| start..end)
        .ok_or_else(|| {
            Error::Other("field has no complete proved physical result interval".into())
        })?;
    if !source[..spans.first().unwrap().start - opening.len()]
        .trim()
        .is_empty()
        || !source[spans.last().unwrap().end - opening.len()..]
            .trim()
            .is_empty()
    {
        return Err(Error::Other(
            "field anchor has independent outside source".into(),
        ));
    }
    Ok(range)
}

fn complex_field_result(xml: &str) -> Result<(String, String, String)> {
    let separate = field_character(xml, "separate")
        .ok_or_else(|| Error::Other("complex field source has no separate boundary".to_owned()))?;
    // A producer may pack a whole field in one run, as Google Docs writes page
    // fields. Ending a run after `separate` and starting one at `end` reads it
    // as the same field written one run per part.
    let xml = split_field_run(xml, separate, true)?;
    let (_, result_start) = containing_run(&xml, separate)?;
    let end_marker = field_character(&xml, "end")
        .ok_or_else(|| Error::Other("complex field source has no end boundary".to_owned()))?;
    let xml = split_field_run(&xml, end_marker, false)?;
    // The split may have moved the `end` character further along.
    let end_marker = field_character(&xml, "end")
        .ok_or_else(|| Error::Other("complex field source has no end boundary".to_owned()))?;
    let (result_end, _) = containing_run(&xml, end_marker)?;
    Ok((
        xml[..result_start].to_owned(),
        xml[result_start..result_end].to_owned(),
        xml[result_end..].to_owned(),
    ))
}

/// Where the field this source begins has its `separate` or `end` character:
/// the characters of a field nested in its instruction or its result are
/// skipped.
fn field_character(xml: &str, kind: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (at, _) in xml.match_indices("fldCharType=") {
        let value = &xml[at + "fldCharType=".len()..];
        let quote = value.chars().next()?;
        let value = value[1..].split(quote).next()?;
        match value {
            "begin" => depth += 1,
            "separate" if depth == 1 && kind == "separate" => return Some(at),
            "end" if depth == 1 && kind == "end" => return Some(at),
            "end" => depth = depth.checked_sub(1)?,
            _ => {}
        }
    }
    None
}

/// Split the run holding the field character at `marker` so that the
/// character ends its run (`after`) or starts it.
///
/// Both runs repeat the original start tag and run properties. A run with no
/// content on that side of the character is returned unchanged.
fn split_field_run(xml: &str, marker: usize, after: bool) -> Result<String> {
    let (run_start, run_end) = containing_run(xml, marker)?;
    let run = &xml[run_start..run_end];
    let children = direct_element_spans(run)?;
    let character = children
        .iter()
        .position(|child| child.contains(&(marker - run_start)))
        .ok_or_else(|| Error::Other("complex field character is not a run child".to_owned()))?;
    let is_properties = |child: &Range<usize>| {
        let mut reader = Reader::from_reader(run[child.clone()].as_bytes());
        matches!(
            reader.read_event(),
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"rPr"
        )
    };
    let first_content = usize::from(children.first().is_some_and(is_properties));
    let split = if after { character + 1 } else { character };
    if split <= first_content || split >= children.len() {
        return Ok(xml.to_owned());
    }
    let head = &run[..children[first_content].start];
    let span = oxoffice_core::xml_element::element_span(run)?;
    if span.is_self_closing() {
        return Err(Error::Other("complex field run has no end tag".to_owned()));
    }
    let close = &run[span.end_tag.start..];
    let at = run_start + children[split].start;
    Ok(format!("{}{close}{head}{}", &xml[..at], &xml[at..]))
}

pub(super) fn containing_run(xml: &str, at: usize) -> Result<(usize, usize)> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(usize, bool)>::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                stack.push((before, element.local_name().as_ref() == b"r"));
            }
            Event::End(_) => {
                let Some((start, run)) = stack.pop() else {
                    return Err(Error::Other("complex field XML is unbalanced".to_owned()));
                };
                if run && start <= at && at < after {
                    return Ok((start, after));
                }
            }
            Event::Empty(_) => {}
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Err(Error::Other(
        "complex field boundary has no owning run".to_owned(),
    ))
}

pub(super) fn run_is_field(run: &CT_R) -> bool {
    matches!(run.content.as_slice(), [RunContent::Field(_)])
}

pub(super) fn run_field(run: &CT_R) -> Option<&oxoffice_docx_oxml::text::Field> {
    match run.content.as_slice() {
        [RunContent::Field(field)] => Some(field),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instruction_with_a_joined_operand_equals_its_copy_only() {
        let paragraph = |name: &str| {
            let xml = format!(
                r#"<w:p xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF "Mr </w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD {name} </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>x</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve">" = "Mr Smith" "a" "b" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>b</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
            );
            oxoffice_docx_oxml::text::CT_P::from_xml_fragment(xml.as_bytes()).unwrap()
        };
        let instruction = |paragraph: &oxoffice_docx_oxml::text::CT_P| {
            paragraph
                .runs
                .iter()
                .flat_map(|run| &run.content)
                .find_map(|content| match content {
                    oxoffice_docx_oxml::text::RunContent::Field(field) => {
                        Some(field.effective_instruction())
                    }
                    _ => None,
                })
                .expect("one field")
        };
        let name = instruction(&paragraph("Name"));
        assert!(matches!(
            name.arguments.first(),
            Some(oxoffice_docx_oxml::text::FieldArgument::Joined(_))
        ));
        assert!(comparison_instruction_eq(
            &name,
            &instruction(&paragraph("Name"))
        ));
        assert!(!comparison_instruction_eq(
            &name,
            &instruction(&paragraph("Other"))
        ));
    }
}

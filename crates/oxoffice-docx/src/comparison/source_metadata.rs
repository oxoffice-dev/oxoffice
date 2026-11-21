//! The metadata of a source body that a comparison must not lose.

use super::*;

/// Opaque owning-classifier facts, indexed by normalized content coverage.
/// The kind separates overlapping root and leaf facts. Physical run partitions
/// are source witnesses, not equality keys.
pub(super) type ContentMetadata = BTreeMap<String, Vec<(usize, usize, Vec<String>)>>;

// Ordinary proof inputs omit only the existing qualified producer identities.
// The original source and the strict held-owner residual bridge stay untouched.
fn ordinary_identity_filtered_source(xml: &str, inherited: &NamespaceBindings) -> Result<String> {
    let closed = actual_field_source_fragment(xml, inherited)?;
    let mut reader = NsReader::from_str(&closed);
    let (namespace, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let empty = matches!(event, Event::Empty(_));
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return Ok(closed);
    };
    if !matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes()) {
        return Ok(closed);
    }
    let owner = root.local_name();
    let mut filtered = root.clone().into_owned();
    filtered.clear_attributes();
    for attribute in root.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        let identity = match namespace {
            ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes() => {
                match owner.as_ref() {
                    b"p" => matches!(
                        local.as_ref(),
                        b"rsidRPr" | b"rsidR" | b"rsidDel" | b"rsidP" | b"rsidRDefault"
                    ),
                    b"r" => matches!(local.as_ref(), b"rsidRPr" | b"rsidDel" | b"rsidR"),
                    b"tr" => matches!(
                        local.as_ref(),
                        b"rsidR" | b"rsidTr" | b"rsidRPr" | b"rsidDel"
                    ),
                    _ => false,
                }
            }
            ResolveResult::Bound(Namespace(uri))
                if uri == b"http://schemas.microsoft.com/office/word/2010/wordml" =>
            {
                matches!(owner.as_ref(), b"p" | b"tr")
                    && matches!(local.as_ref(), b"paraId" | b"textId")
            }
            _ => false,
        };
        if !identity {
            filtered.push_attribute(attribute);
        }
    }
    let tail = reader.buffer_position() as usize;
    let mut writer = Writer::new(Vec::new());
    writer.write_event(if empty {
        Event::Empty(filtered)
    } else {
        Event::Start(filtered)
    })?;
    writer
        .get_mut()
        .extend_from_slice(&closed.as_bytes()[tail..]);
    String::from_utf8(writer.into_inner()).map_err(utf8_error)
}

pub(super) fn source_root_metadata(
    xml: &str,
    inherited: &NamespaceBindings,
) -> Result<Vec<String>> {
    let (opening, closing) = paragraph_shell(Some(xml))?;
    let filtered = ordinary_identity_filtered_source(&format!("{opening}{closing}"), inherited)?;
    Ok(oxoffice_docx_oxml::properties::protected_source_residual(
        filtered.as_bytes(),
        inherited,
    )?)
}

/// Paragraph and control fields share one source-qualified residual policy.
pub(super) fn field_source_metadata(view: &str, complex: bool) -> Result<Vec<String>> {
    Ok(if !complex {
        let mut remainder = view.to_owned();
        let simple = direct_word_element_spans(view, "fldSimple")?;
        if simple.len() != 1 {
            return Err(Error::Other(
                "simple field source has no unique actual owner".into(),
            ));
        }
        let field_xml = source_child(view, simple[0].clone())?;
        let runs = direct_word_element_spans(&field_xml, "r")?;
        let mut result_facts = Vec::new();
        for span in &runs {
            let actual = source_child(&field_xml, span.clone())?;
            result_facts.extend(oxoffice_docx_oxml::properties::protected_source_residual(
                actual.as_bytes(),
                &[],
            )?);
        }
        let mut shell = field_xml;
        for span in runs.into_iter().rev() {
            shell.replace_range(span, "");
        }
        remainder.replace_range(simple[0].clone(), &shell);
        let mut facts =
            oxoffice_docx_oxml::properties::protected_source_residual(remainder.as_bytes(), &[])?;
        facts.extend(result_facts);
        facts
    } else {
        oxoffice_docx_oxml::properties::protected_source_residual(view.as_bytes(), &[])?
    })
}

/// Omit actual Word property roles while retaining foreign and opaque events.
pub(super) fn source_without_text_properties(
    xml: &str,
    inherited: &NamespaceBindings,
) -> Result<String> {
    let closed = source_fragment(xml, inherited)?;
    let mut reader = NsReader::from_str(&closed);
    let mut owners = Vec::<(Vec<u8>, bool)>::new();
    let mut omitted = None;
    let mut spans = Vec::new();
    let mut identity_edits = Vec::new();
    let property = |word: bool, local: &[u8], owners: &[(Vec<u8>, bool)]| {
        word && owners.last().is_some_and(|(parent, admitted)| {
            *admitted && matches!((parent.as_slice(), local), (b"r", b"rPr") | (b"p", b"pPr"))
        })
    };
    let admitted_owner = |word: bool, local: &[u8], owners: &[(Vec<u8>, bool)]| {
        word && match owners.last() {
            None => local == b"p",
            Some((parent, true)) => match parent.as_slice() {
                b"p" | b"fldSimple" => matches!(local, b"r" | b"fldSimple" | b"hyperlink" | b"sdt"),
                b"hyperlink" | b"sdtContent" => {
                    matches!(local, b"r" | b"fldSimple" | b"sdt")
                }
                b"sdt" => local == b"sdtContent",
                _ => false,
            },
            Some((_, false)) => false,
        }
    };
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let word =
            matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == W_NS.as_bytes());
        match event {
            Event::Start(element) => {
                let local = element.local_name().as_ref().to_vec();
                if omitted.is_none() && property(word, &local, &owners) {
                    omitted = Some((before, owners.len()));
                }
                // Only the current field-result routes expose modeled properties.
                // An opaque ancestor, prior revision or drawing stays opaque.
                let admitted = admitted_owner(word, &local, &owners);
                if admitted && omitted.is_none() && matches!(local.as_slice(), b"p" | b"r") {
                    let after = reader.buffer_position() as usize;
                    let shell = format!(
                        "{}</{}>",
                        &closed[before..after],
                        std::str::from_utf8(element.name().as_ref()).map_err(utf8_error)?
                    );
                    let filtered = ordinary_identity_filtered_source(
                        &shell,
                        &resolved_source_bindings(&reader)?,
                    )?;
                    let opening_end = element_opening(&filtered)?.0;
                    identity_edits.push((before..after, filtered[..opening_end].to_owned()));
                }
                owners.push((local, admitted));
            }
            Event::Empty(element)
                if omitted.is_none() && property(word, element.local_name().as_ref(), &owners) =>
            {
                spans.push(before..reader.buffer_position() as usize);
            }
            Event::Empty(element)
                if omitted.is_none()
                    && admitted_owner(word, element.local_name().as_ref(), &owners)
                    && element.local_name().as_ref() == b"r" =>
            {
                let after = reader.buffer_position() as usize;
                let filtered = ordinary_identity_filtered_source(
                    &closed[before..after],
                    &resolved_source_bindings(&reader)?,
                )?;
                identity_edits.push((before..after, filtered));
            }
            Event::End(_) => {
                owners.pop();
                if let Some((start, level)) = omitted
                    && level == owners.len()
                {
                    spans.push(start..reader.buffer_position() as usize);
                    omitted = None;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut output = closed;
    let mut edits = identity_edits;
    edits.extend(spans.into_iter().map(|span| (span, String::new())));
    edits.sort_by_key(|(span, _)| span.start);
    for (span, replacement) in edits.into_iter().rev() {
        output.replace_range(span, &replacement);
    }
    Ok(output)
}

pub(super) fn source_without_run_root_attributes(xml: &str) -> Result<String> {
    let mut reader = NsReader::from_str(xml);
    let mut output = String::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = matches!(namespace,ResolveResult::Bound(Namespace(uri)) if uri==W_NS.as_bytes());
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(root) | Event::Empty(root)
                if word && root.local_name().as_ref() == b"r" =>
            {
                let attributes = root
                    .attributes()
                    .map(|attribute| attribute.map_err(|error| Error::Other(error.to_string())))
                    .collect::<Result<Vec<_>>>()?;
                let namespaces = attributes
                    .iter()
                    .filter(|attribute| {
                        attribute.key.as_ref() == b"xmlns"
                            || attribute.key.as_ref().starts_with(b"xmlns:")
                    })
                    .map(|attribute| {
                        (
                            attribute.key.as_ref().to_vec(),
                            attribute.value.as_ref().to_vec(),
                        )
                    })
                    .collect::<Vec<_>>();
                let mut root = root.into_owned();
                root.clear_attributes();
                for (key, value) in &namespaces {
                    root.push_attribute((key.as_slice(), value.as_slice()));
                }
                let mut writer = Writer::new(Vec::new());
                writer.write_event(if empty {
                    Event::Empty(root)
                } else {
                    Event::Start(root)
                })?;
                output.push_str(&String::from_utf8(writer.into_inner()).map_err(utf8_error)?);
            }
            Event::Eof => return Ok(output),
            _ => output.push_str(&xml[before..reader.buffer_position() as usize]),
        }
    }
}

fn normalized_content_width(content: &RunContent, options: &ComparisonOptions) -> usize {
    if ignored_run_content(content, options) {
        return 0;
    }
    match content {
        RunContent::Text(text) | RunContent::DeletedText(text) => text
            .text
            .chars()
            .filter(|character| !options.ignore_whitespace || !character.is_whitespace())
            .count(),
        _ => 1,
    }
}

impl SourceMetadata {
    fn record(&mut self, kind: &str, start: usize, length: usize, facts: Vec<String>) {
        if !facts.is_empty() {
            self.facts
                .entry(kind.to_owned())
                .or_default()
                .push((start, length, facts));
        }
    }

    fn root(&mut self, kind: &str, xml: Option<&String>, start: usize) -> Result<()> {
        if let Some(xml) = xml {
            self.record(
                kind,
                start,
                self.cursor - start,
                source_root_metadata(xml, &Vec::new())?,
            );
        }
        Ok(())
    }

    fn run(
        &mut self,
        run: &CT_R,
        source: &SourceSnapshots,
        options: &ComparisonOptions,
    ) -> Result<()> {
        let start = self.cursor;
        let actual = source.runs.get(&(run as *const CT_R as usize));
        for (ordinal, content) in run.content.iter().enumerate() {
            let length = normalized_content_width(content, options);
            if let RunContent::Field(field) = content {
                self.has_fields |= !options.ignore_fields;
                if let Some(facts) = source.fields.get(&(field as *const _ as usize)) {
                    let record =
                        (!facts.is_empty()).then(|| self.facts.get("field").map_or(0, Vec::len));
                    self.field_records.insert(
                        std::ptr::from_ref(field) as usize,
                        (self.cursor, length, record),
                    );
                    self.record("field", self.cursor, length, facts.clone());
                }
            } else if !matches!(content, RunContent::Drawing(_))
                && let Some(actual) = actual
                && let Some(span) = actual.typed_slots.get(ordinal).cloned().flatten()
            {
                self.record(
                    "content",
                    self.cursor,
                    length,
                    source_content_metadata(actual, span)?,
                );
            }
            self.cursor += length;
        }
        if !source
            .shared_root_runs
            .contains(&(run as *const CT_R as usize))
            && let Some(actual) = actual
        {
            self.record(
                "r-root",
                start,
                self.cursor - start,
                source_root_metadata(&actual.scoped, &actual.parent)?,
            );
        }
        Ok(())
    }

    fn paragraph(
        &mut self,
        paragraph: &CT_P,
        source: &SourceSnapshots,
        options: &ComparisonOptions,
    ) -> Result<()> {
        let start = self.cursor;
        for boundary in 0..=paragraph.runs.len() {
            for (_, _, _, control) in paragraph
                .content_controls
                .iter()
                .filter(|(at, _, _, _)| *at == boundary)
            {
                self.control(control, source, options)?;
            }
            if let Some(run) = paragraph.runs.get(boundary) {
                self.run(run, source, options)?;
            }
        }
        self.root(
            "p-root",
            source.paragraphs.get(&(paragraph as *const _ as usize)),
            start,
        )?;
        if let Some(facts) = source
            .shared_root_facts
            .get(&(paragraph as *const CT_P as usize))
        {
            self.record(
                "unproved-shared-r-roots",
                start,
                self.cursor - start,
                facts.clone(),
            );
        }
        self.cursor += 1;
        Ok(())
    }

    fn table(
        &mut self,
        table: &CT_Tbl,
        source: &SourceSnapshots,
        options: &ComparisonOptions,
    ) -> Result<()> {
        let start = self.cursor;
        for boundary in 0..=table.rows.len() {
            for (_, _, control) in table
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == boundary)
            {
                self.control(control, source, options)?;
            }
            if let Some(row) = table.rows.get(boundary) {
                self.row(row, source, options)?;
            }
        }
        self.root(
            "tbl-root",
            source.tables.get(&(table as *const _ as usize)),
            start,
        )?;
        self.cursor += 1;
        Ok(())
    }

    fn row(
        &mut self,
        row: &CT_Row,
        source: &SourceSnapshots,
        options: &ComparisonOptions,
    ) -> Result<()> {
        let start = self.cursor;
        for boundary in 0..=row.cells.len() {
            for (_, _, control) in row
                .content_controls
                .iter()
                .filter(|(at, _, _)| *at == boundary)
            {
                self.control(control, source, options)?;
            }
            if let Some(cell) = row.cells.get(boundary) {
                self.cell(cell, source, options)?;
            }
        }
        self.root(
            "tr-root",
            source.rows.get(&(row as *const _ as usize)),
            start,
        )?;
        self.cursor += 1;
        Ok(())
    }

    fn cell(
        &mut self,
        cell: &CT_Tc,
        source: &SourceSnapshots,
        options: &ComparisonOptions,
    ) -> Result<()> {
        let start = self.cursor;
        for child in &cell.content {
            match child {
                CellContent::Paragraph(p) => self.paragraph(p, source, options)?,
                CellContent::Table(t) => self.table(t, source, options)?,
                CellContent::ContentControl(c) => self.control(c, source, options)?,
            }
        }
        self.root(
            "tc-root",
            source.cells.get(&(cell as *const _ as usize)),
            start,
        )?;
        self.cursor += 1;
        Ok(())
    }

    fn control(
        &mut self,
        control: &CT_Sdt,
        source: &SourceSnapshots,
        options: &ComparisonOptions,
    ) -> Result<()> {
        let start = self.cursor;
        for child in &control.content {
            match child {
                SdtContent::Paragraph(p) => self.paragraph(p, source, options)?,
                SdtContent::Table(t) => self.table(t, source, options)?,
                SdtContent::Row(r) => self.row(r, source, options)?,
                SdtContent::Cell(c) => self.cell(c, source, options)?,
                SdtContent::Run(r) => self.run(r, source, options)?,
                SdtContent::ContentControl(c) => self.control(c, source, options)?,
                SdtContent::RawXml(raw) if raw.iter().all(u8::is_ascii_whitespace) => {}
                SdtContent::RawXml(_) => self.cursor += 1,
            }
        }
        let xml = source.controls.get(&(control as *const _ as usize));
        self.root("sdt-root", xml, start)?;
        if let Some(xml) = xml
            && let Some(span) = direct_word_element_spans(xml, "sdtContent")?
                .into_iter()
                .next()
        {
            let (parent_open, parent_close) = paragraph_shell(Some(xml))?;
            let child = source_child(xml, span)?;
            let (child_open, child_close) = paragraph_shell(Some(&child))?;
            // The actual qualified parent supplies the classifier's grammar.
            // A standalone sdtContent would become an opaque scope-sensitive root.
            let contextual = format!("{parent_open}{child_open}{child_close}{parent_close}");
            self.record(
                "sdtContent-root",
                start,
                self.cursor - start,
                oxoffice_docx_oxml::properties::protected_source_residual(
                    contextual.as_bytes(),
                    &[],
                )?,
            );
        }
        self.cursor += 1;
        Ok(())
    }

    pub(super) fn paragraph_contains_fields(
        paragraph: &CT_P,
        options: &ComparisonOptions,
    ) -> Result<bool> {
        let mut state = Self::default();
        state.paragraph(paragraph, &SourceSnapshots::default(), options)?;
        Ok(state.has_fields)
    }

    pub(super) fn body(
        content: &BodyContent,
        source: &SourceSnapshots,
        options: &ComparisonOptions,
    ) -> Result<Self> {
        let mut state = Self::default();
        match content {
            BodyContent::Paragraph(p) => state.paragraph(p, source, options)?,
            BodyContent::Table(t) => state.table(t, source, options)?,
            BodyContent::ContentControl(c) => state.control(c, source, options)?,
            BodyContent::RawXml(_) => {}
        }
        for (kind, records) in &mut state.facts {
            if kind != "r-root" {
                continue;
            }
            let mut merged: Vec<(usize, usize, Vec<String>)> = Vec::new();
            for (start, length, facts) in std::mem::take(records) {
                if length > 0
                    && let Some((previous, coverage, actual)) = merged.last_mut()
                    && *coverage > 0
                    && *previous + *coverage == start
                    && *actual == facts
                {
                    *coverage += length;
                } else {
                    merged.push((start, length, facts));
                }
            }
            *records = merged;
        }
        Ok(state)
    }
}

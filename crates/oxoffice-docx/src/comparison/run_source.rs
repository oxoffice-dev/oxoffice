//! The source XML of one run and its occurrences.

use super::*;

impl RunSource {
    /// The source of the run at each span of `parent`, in the order given.
    ///
    /// One read of `parent` serves every run. A read per run reads the parent
    /// again up to that run, which is quadratic in the runs it holds. Each
    /// result, an error included, is the one a read for that run alone gives.
    pub(super) fn capture_all(
        runs: &[(Option<&CT_R>, Range<usize>)],
        parent: &str,
    ) -> Vec<Result<Self>> {
        if runs.is_empty() {
            return Vec::new();
        }
        let opening = format!(r#"<scope xmlns:w="{W_NS}">"#);
        let wrapped = format!("{opening}{parent}</scope>");
        let mut reader = NsReader::from_str(&wrapped);
        let mut scopes = Vec::<NamespaceBindings>::new();
        let mut results = runs.iter().map(|_| None).collect::<Vec<_>>();
        // The runs still to reach, the nearest last.
        let mut pending = (0..runs.len()).collect::<Vec<_>>();
        pending.sort_by_key(|index| std::cmp::Reverse(runs[*index].1.start));
        // A run the read passed opens no element. Its read goes on to the end.
        let mut passed = Vec::new();
        let capture = |(run, span): &(Option<&CT_R>, Range<usize>),
                       bindings: &NamespaceBindings,
                       parent_scope: Option<&NamespaceBindings>|
         -> Result<Self> {
            let raw = parent[span.clone()].to_owned();
            let mut source = Self {
                owner: run.map_or(0, |run| run as *const CT_R as usize),
                scoped: source_fragment(&raw, bindings)?,
                raw,
                parent: parent_scope.cloned().unwrap_or_default(),
                raw_slots: Vec::new(),
                typed_slots: Vec::new(),
                projected_raw_slots: HashSet::new(),
                projected_typed_slots: HashSet::new(),
            };
            if let Some(run) = run {
                source.capture_children(run)?;
            }
            Ok(source)
        };
        let failure = loop {
            if pending.is_empty() && passed.is_empty() {
                break None;
            }
            let before = reader.buffer_position() as usize;
            let event = match reader.read_resolved_event() {
                Ok((_, event)) => event,
                Err(error) => break Some(error.to_string()),
            };
            while let Some(&index) = pending.last()
                && opening.len() + runs[index].1.start < before
            {
                pending.pop();
                passed.push(index);
            }
            match event {
                Event::Start(_) | Event::Empty(_) => {
                    let bindings = match resolved_source_bindings(&reader) {
                        Ok(bindings) => bindings,
                        Err(error) => break Some(error.to_string()),
                    };
                    while let Some(&index) = pending.last()
                        && opening.len() + runs[index].1.start == before
                    {
                        pending.pop();
                        results[index] = Some(capture(&runs[index], &bindings, scopes.last()));
                    }
                    if matches!(event, Event::Start(_)) {
                        scopes.push(bindings);
                    }
                }
                Event::End(_) => {
                    scopes.pop();
                }
                Event::Eof => break Some("run source has no actual opening".into()),
                _ => {}
            }
        };
        if let Some(message) = failure {
            for index in pending.into_iter().chain(passed) {
                results[index] = Some(Err(Error::Other(message.clone())));
            }
        }
        results
            .into_iter()
            .map(|result| result.expect("every run is answered"))
            .collect()
    }
}

/// Only root declaration plumbing is projected here, after a physical slot is
/// already selected. Every non-declaration byte and descendant remains exact.
/// Whether two writings of one raw child hold the same element: the same
/// bindings it needs and the same canonical form, however each is written.
pub(super) fn same_raw_occurrence(
    left: &str,
    right: &str,
    context: &[(String, String)],
) -> Result<bool> {
    let (left, left_bindings) = raw_occurrence_representation(left, context)?;
    let (right, right_bindings) = raw_occurrence_representation(right, context)?;
    Ok(left_bindings == right_bindings
        && (left == right || same_xml_fragment(left.as_bytes(), right.as_bytes())))
}

pub(super) fn raw_occurrence_representation(
    xml: &str,
    context: &[(String, String)],
) -> Result<(String, NamespaceBindings)> {
    let mut initial = Reader::from_str(xml);
    if !matches!(
        initial
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?,
        Event::Start(_) | Event::Empty(_)
    ) {
        return Ok((xml.to_owned(), Vec::new()));
    }
    let scoped = source_fragment(xml, context)?;
    let mut reader = NsReader::from_str(&scoped);
    let (_, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let (Event::Start(root) | Event::Empty(root)) = event else {
        return Ok((xml.to_owned(), Vec::new()));
    };
    let mut bindings = resolved_source_bindings(&reader)?;
    bindings.sort();
    let mut ranges = Vec::new();
    for attribute in root.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let key = attribute.key.as_ref();
        if key != b"xmlns" && !key.starts_with(b"xmlns:") {
            continue;
        }
        let start = key.as_ptr() as usize - scoped.as_ptr() as usize;
        let end = attribute.value.as_ptr() as usize - scoped.as_ptr() as usize
            + attribute.value.len()
            + 1;
        if start == 0 || end > scoped.len() || !scoped.as_bytes()[start - 1].is_ascii_whitespace() {
            return Err(Error::Other(
                "raw occurrence declaration range is invalid".into(),
            ));
        }
        ranges.push(start - 1..end);
    }
    let mut lexical = scoped;
    for range in ranges.into_iter().rev() {
        lexical.replace_range(range, "");
    }
    Ok((lexical, bindings))
}

impl RunSource {
    fn capture_children(&mut self, run: &CT_R) -> Result<()> {
        if run.extra_xml.len() != run.extra_xml_positions.len() {
            return Err(Error::Other("run raw ownership is unordered".into()));
        }
        let mut events = run_source_events(&self.scoped)?;
        let word = word_element_starts(&self.scoped)?;
        if run.properties.is_none() {
            events.retain(|span| {
                !(word.contains(&span.start)
                    && element_local_name(&self.scoped[span.clone()]) == "rPr")
            });
        }
        let context = root_namespace_bindings(&self.scoped, "r", &[])?;
        self.raw_slots.resize(run.extra_xml.len(), None);
        let mut cursor = 0;
        let mut boundary = 0;
        let mut typed = Vec::new();
        for (ordinal, (&encoded, modeled_raw)) in run
            .extra_xml_positions
            .iter()
            .zip(&run.extra_xml)
            .enumerate()
        {
            if CT_R::raw_child_is_root_attributes(encoded) {
                continue;
            }
            let target = CT_R::raw_child_position(encoded);
            while boundary < target {
                let span = events
                    .get(cursor)
                    .ok_or_else(|| {
                        Error::Other("source run raw boundary exceeds its children".into())
                    })?
                    .clone();
                cursor += 1;
                if word.contains(&span.start)
                    && element_local_name(&self.scoped[span.clone()]) == "rPr"
                {
                    if run.properties.is_some() {
                        boundary += 1;
                    }
                } else {
                    typed.push(span);
                    boundary += 1;
                }
            }
            let span = events
                .get(cursor)
                .ok_or_else(|| Error::Other("source run has no child for its raw ordinal".into()))?
                .clone();
            let comment = word.contains(&span.start)
                && element_local_name(&self.scoped[span.clone()]) == "commentReference"
                && run.content.iter().any(|content| matches!(content, RunContent::CommentReference {raw_before, ..} if *raw_before == ordinal + 1));
            if !comment {
                let actual = &self.scoped[span.clone()];
                let modeled = std::str::from_utf8(modeled_raw).map_err(utf8_error)?;
                if !same_raw_occurrence(actual, modeled, &context)? {
                    return Err(Error::Other(format!(
                        "source raw ordinal {ordinal} has a different physical role"
                    )));
                }
            }
            self.raw_slots[ordinal] = Some((span.clone(), target, comment));
            cursor += 1;
            if comment {
                typed.push(span);
                boundary += 1;
            }
        }
        for span in &events[cursor..] {
            if !(word.contains(&span.start)
                && element_local_name(&self.scoped[span.clone()]) == "rPr")
            {
                typed.push(span.clone());
            }
        }
        self.typed_slots.resize(run.content.len(), None);
        let widths = run
            .content
            .iter()
            .map(run_content_source_width)
            .collect::<Vec<_>>();
        if widths.iter().sum::<usize>() == typed.len() {
            let mut at = 0;
            for (index, width) in widths.into_iter().enumerate() {
                if width == 1 {
                    self.typed_slots[index] = Some(typed[at].clone());
                }
                at += width;
            }
        }
        Ok(())
    }
}

pub(super) fn run_content_source_width(content: &RunContent) -> usize {
    match content {
        RunContent::FootnoteRef {
            custom_mark: Some(_),
            ..
        }
        | RunContent::EndnoteRef {
            custom_mark: Some(_),
            ..
        } => 2,
        RunContent::Field(_) => 0,
        _ => 1,
    }
}

/// Full direct run events owned by CT_R, excluding ignored inter-element text.
pub(super) fn run_source_events(xml: &str) -> Result<Vec<Range<usize>>> {
    let (opening, _) = paragraph_shell(Some(xml))?;
    let mut reader = Reader::from_str(xml);
    let mut output = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        if before < opening.len() {
            continue;
        }
        match event {
            Event::Start(root) => {
                reader
                    .read_to_end(root.name())
                    .map_err(|error| Error::Other(error.to_string()))?;
            }
            Event::Empty(_) | Event::PI(_) | Event::Comment(_) => {}
            Event::End(_) | Event::Eof => return Ok(output),
            _ => continue,
        }
        output.push(before..reader.buffer_position() as usize);
    }
}

pub(super) fn resolved_source_bindings(reader: &NsReader<&[u8]>) -> Result<NamespaceBindings> {
    reader
        .resolver()
        .bindings()
        .map(|(prefix, uri)| {
            let key = match prefix {
                quick_xml::name::PrefixDeclaration::Default => "xmlns".to_owned(),
                quick_xml::name::PrefixDeclaration::Named(prefix) => {
                    format!("xmlns:{}", std::str::from_utf8(prefix).map_err(utf8_error)?)
                }
            };
            let uri = std::str::from_utf8(uri.as_ref()).map_err(utf8_error)?;
            Ok((
                key,
                quick_xml::escape::unescape(uri)
                    .map_err(|error| Error::Other(error.to_string()))?
                    .into_owned(),
            ))
        })
        .collect()
}

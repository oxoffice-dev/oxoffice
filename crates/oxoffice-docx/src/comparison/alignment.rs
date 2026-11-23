//! Aligning two sequences, pairing gaps by kind, paragraphs carried whole.

use super::*;

pub(super) fn align(original: &[String], edited: &[String]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut lcs = vec![vec![0usize; edited.len() + 1]; original.len() + 1];
    for i in (0..original.len()).rev() {
        for j in (0..edited.len()).rev() {
            lcs[i][j] = if original[i] == edited[j] {
                1 + lcs[i + 1][j + 1]
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut matches = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < original.len() && j < edited.len() {
        if original[i] == edited[j] {
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let mut result = Vec::new();
    let (mut left, mut right) = (0usize, 0usize);
    for (matched_left, matched_right) in matches
        .into_iter()
        .chain(std::iter::once((original.len(), edited.len())))
    {
        let paired = (matched_left - left).min(matched_right - right);
        for offset in 0..paired {
            result.push((Some(left + offset), Some(right + offset)));
        }
        for index in left + paired..matched_left {
            result.push((Some(index), None));
        }
        for index in right + paired..matched_right {
            result.push((None, Some(index)));
        }
        if matched_left < original.len() {
            result.push((Some(matched_left), Some(matched_right)));
        }
        left = matched_left.saturating_add(1);
        right = matched_right.saturating_add(1);
    }
    result
}

/// Pair the owners left over in each gap between matched owners by kind, in
/// order: a paragraph with a paragraph, a table with a table and a control
/// with a control, as the prefix of their signatures names the kind. The
/// others stay deleted, then inserted, before the next pair. [`align`] pairs
/// them by position, which would delete and insert a changed table whole
/// beside a deleted paragraph.
pub(super) fn pair_gaps_by_kind(
    aligned: Alignment,
    original: &[String],
    edited: &[String],
) -> Alignment {
    fn kind(signature: &str) -> &str {
        signature
            .split_once(':')
            .map_or(signature, |(kind, _)| kind)
    }
    fn flush(
        gap: &mut (Vec<usize>, Vec<usize>),
        original: &[String],
        edited: &[String],
        result: &mut Alignment,
    ) {
        let (left, right) = std::mem::take(gap);
        let mut lcs = vec![vec![0usize; right.len() + 1]; left.len() + 1];
        for i in (0..left.len()).rev() {
            for j in (0..right.len()).rev() {
                lcs[i][j] = if kind(&original[left[i]]) == kind(&edited[right[j]]) {
                    1 + lcs[i + 1][j + 1]
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let mut pairs = Vec::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < left.len() && j < right.len() {
            if kind(&original[left[i]]) == kind(&edited[right[j]]) {
                pairs.push((i, j));
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        let (mut deleted, mut inserted) = (0usize, 0usize);
        for (i, j) in pairs
            .into_iter()
            .chain(std::iter::once((left.len(), right.len())))
        {
            result.extend(left[deleted..i].iter().map(|&index| (Some(index), None)));
            result.extend(right[inserted..j].iter().map(|&index| (None, Some(index))));
            if i < left.len() {
                result.push((Some(left[i]), Some(right[j])));
            }
            (deleted, inserted) = (i + 1, j + 1);
        }
    }
    let mut result = Vec::with_capacity(aligned.len());
    let mut gap = (Vec::new(), Vec::new());
    for pair in aligned {
        match pair {
            (Some(i), Some(j)) if original[i] == edited[j] => {
                flush(&mut gap, original, edited, &mut result);
                result.push(pair);
            }
            (left, right) => {
                gap.0.extend(left);
                gap.1.extend(right);
            }
        }
    }
    flush(&mut gap, original, edited, &mut result);
    result
}

/// Replace each changed run of paragraphs that revising in place cannot
/// express, and name the paragraphs whose hyperlinks and simple fields a
/// whole-paragraph revision may carry.
///
/// A run is the consecutive changed entries of paragraphs only, between
/// unchanged owners, tables or controls. When one of its pairs differs in
/// its inline boundary structures, or gains or loses a modeled field, or
/// when it holds a hyperlink or simple field, the run becomes all its
/// original paragraphs deleted, then all its edited paragraphs inserted. The
/// run grows over its neighbouring paragraphs until each side holds every
/// complex field it begins or ends, so a table of contents whose end sits
/// in an unchanged paragraph is still deleted and inserted whole. Word then
/// removes the whole field on each side, with the hyperlinks and simple
/// fields inside it.
///
/// A run keeps its pairs, and their refusal, when a side cannot be closed
/// that way, when one of its hyperlinks or simple fields lies outside such a
/// field, or when both sides share a bookmark or comment range, which the
/// replacement would hold twice.
pub(super) fn replace_changed_paragraph_runs(
    aligned: Vec<(Option<usize>, Option<usize>)>,
    original: &[Option<&CT_P>],
    edited: &[Option<&CT_P>],
    original_signatures: &[String],
    edited_signatures: &[String],
    options: &ComparisonOptions,
) -> Result<(Alignment, CarriedParagraphs)> {
    fn markers<'a>(paragraphs: impl Iterator<Item = &'a CT_P>) -> HashSet<String> {
        let mut markers = HashSet::new();
        for paragraph in paragraphs {
            for marker in &paragraph.bookmark_markers {
                markers.extend(marker.id().map(|id| format!("bookmark id {id}")));
                markers.extend(marker.name().map(|name| format!("bookmark {name}")));
            }
            for marker in &paragraph.comment_ranges {
                let (CommentRangeMarker::Start { id, .. } | CommentRangeMarker::End { id, .. }) =
                    marker;
                markers.insert(format!("comment {id}"));
            }
        }
        markers
    }
    let changed = |(left, right): &(Option<usize>, Option<usize>)| match (left, right) {
        (Some(i), Some(j)) => original_signatures[*i] != edited_signatures[*j],
        _ => true,
    };
    // In-place revision refuses a modeled field it cannot pair with one on
    // the other side.
    let fields = |paragraph: &CT_P| {
        paragraph
            .runs
            .iter()
            .filter(|run| run_is_field(run))
            .count()
    };
    let paragraphs_only = |(left, right): &(Option<usize>, Option<usize>)| {
        left.is_none_or(|i| original[i].is_some()) && right.is_none_or(|j| edited[j].is_some())
    };
    let sides = |range: &Range<usize>| {
        let entries = &aligned[range.clone()];
        (
            entries
                .iter()
                .filter_map(|(left, _)| left.and_then(|i| original[i]))
                .collect::<Vec<_>>(),
            entries
                .iter()
                .filter_map(|(_, right)| right.and_then(|j| edited[j]))
                .collect::<Vec<_>>(),
        )
    };
    // Grow a run until both sides hold their complex fields whole.
    let close = |mut range: Range<usize>| -> Result<Option<Range<usize>>> {
        loop {
            let (originals, edits) = sides(&range);
            let balances = [field_balance(&originals)?, field_balance(&edits)?];
            if balances.contains(&FieldBalance::Closes) {
                if range.start == 0 || !paragraphs_only(&aligned[range.start - 1]) {
                    return Ok(None);
                }
                range.start -= 1;
            } else if balances.contains(&FieldBalance::Opens) {
                if range.end == aligned.len() || !paragraphs_only(&aligned[range.end]) {
                    return Ok(None);
                }
                range.end += 1;
            } else if balances.contains(&FieldBalance::Uncarried)
                || !markers(originals.iter().copied()).is_disjoint(&markers(edits.iter().copied()))
            {
                return Ok(None);
            } else {
                crate::revision::validate_carried_paragraph_fields(&originals)?;
                crate::revision::validate_carried_paragraph_fields(&edits)?;
                return Ok(Some(range));
            }
        }
    };
    let mut accepted: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    for run in aligned.chunk_by(|first, second| {
        (changed(first), paragraphs_only(first)) == (changed(second), paragraphs_only(second))
    }) {
        let range = start..start + run.len();
        start = range.end;
        if !changed(&run[0]) || !paragraphs_only(&run[0]) {
            continue;
        }
        let inexpressible = run.iter().any(|(left, right)| {
            matches!(
                (left.and_then(|i| original[i]), right.and_then(|j| edited[j])),
                (Some(left), Some(right)) if paragraph_boundaries_differ(left, right, options)
                    || (!options.ignore_fields && fields(left) != fields(right))
            )
        });
        let (originals, edits) = sides(&range);
        let mut owners = false;
        for paragraph in originals.iter().chain(&edits) {
            owners |= paragraph_field_events(paragraph)?.contains(&FieldEvent::Owner);
        }
        if !inexpressible && !owners {
            continue;
        }
        let Some(mut range) = close(range)? else {
            continue;
        };
        // A grown run that reaches an accepted one replaces both together.
        while let Some(last) = accepted.last()
            && last.end > range.start
        {
            let merged = last.start.min(range.start)..last.end.max(range.end);
            match close(merged)? {
                Some(merged) => {
                    accepted.pop();
                    range = merged;
                }
                None => break,
            }
        }
        if accepted.last().is_none_or(|last| last.end <= range.start) {
            accepted.push(range);
        }
    }
    let mut replaced = Vec::with_capacity(aligned.len());
    let mut carried = CarriedParagraphs::default();
    // A pair that revising in place cannot express, and that the in-place
    // replacement of `compare_paragraph` cannot take either: its bookmarks
    // differ, or it is the last owner, whose inserted paragraph mark no
    // rejection can remove. It becomes a deleted and an inserted paragraph,
    // which the trailing-paragraph rule then closes on the mark before them.
    // A bookmark or comment range of either side that the other input also
    // holds would then be held twice, as when a producer renumbers its
    // bookmarks, so such a pair keeps its refusal.
    let all_original = markers(original.iter().flatten().copied());
    let all_edited = markers(edited.iter().flatten().copied());
    let unpaired = |position: usize, i: usize, j: usize| -> Result<bool> {
        let (Some(left), Some(right)) = (original[i], edited[j]) else {
            return Ok(false);
        };
        if original_signatures[i] == edited_signatures[j]
            || !paragraph_boundaries_differ(left, right, options)
            || (left.bookmark_markers == right.bookmark_markers && position + 1 != aligned.len())
            || !markers([left].into_iter()).is_disjoint(&all_edited)
            || !markers([right].into_iter()).is_disjoint(&all_original)
        {
            return Ok(false);
        }
        // A simple field goes with its paragraph as a whole complex field
        // (see `paragraph_children_xml`); a complex field could span others.
        for paragraph in [left, right] {
            if paragraph_field_events(paragraph)?
                .iter()
                .any(|event| matches!(event, FieldEvent::Begin | FieldEvent::End))
            {
                return Ok(false);
            }
        }
        Ok(true)
    };
    let mut ranges = accepted.into_iter().peekable();
    let mut index = 0;
    while index < aligned.len() {
        let Some(range) = ranges.next_if(|range| range.start == index) else {
            if let (Some(i), Some(j)) = aligned[index]
                && unpaired(index, i, j)?
            {
                replaced.push((Some(i), None));
                replaced.push((None, Some(j)));
            } else {
                replaced.push(aligned[index]);
            }
            index += 1;
            continue;
        };
        let entries = &aligned[range.clone()];
        for (left, _) in entries {
            if let Some(i) = *left {
                carried.original.insert(i);
                replaced.push((Some(i), None));
            }
        }
        for (_, right) in entries {
            if let Some(j) = *right {
                carried.edited.insert(j);
                replaced.push((None, Some(j)));
            }
        }
        index = range.end;
    }
    Ok((replaced, carried))
}

/// Aligned original and edited owner indices.
pub(super) type Alignment = Vec<(Option<usize>, Option<usize>)>;

/// The whole deleted or inserted paragraphs, by index on each side, that hold
/// a simple field outside a complex field carried with them.
///
/// Their emitters write each such field as one complex field in a wrapper of
/// its own (see [`paragraph_children_xml`]). A move pair gives one id to both
/// ends, which such a wrapper cannot share, so these paragraphs are not moved.
pub(super) fn uncarried_field_owners(
    aligned: &[(Option<usize>, Option<usize>)],
    original: &[Option<&CT_P>],
    edited: &[Option<&CT_P>],
    carried: &CarriedParagraphs,
) -> Result<[HashSet<usize>; 2]> {
    let mut owners = [HashSet::new(), HashSet::new()];
    for (left, right) in aligned {
        let (side, index, paragraph) = match (left, right) {
            (Some(i), None) if !carried.original.contains(i) => (0, *i, original[*i]),
            (None, Some(j)) if !carried.edited.contains(j) => (1, *j, edited[*j]),
            _ => continue,
        };
        if let Some(paragraph) = paragraph
            && paragraph_wraps_simple_field(paragraph)?
        {
            owners[side].insert(index);
        }
    }
    Ok(owners)
}

/// Whether this paragraph holds a simple field a whole-paragraph change must
/// track: as its own child, or inside a hyperlink.
fn paragraph_wraps_simple_field(paragraph: &CT_P) -> Result<bool> {
    let xml = paragraph_xml(paragraph)?;
    let word_starts = word_element_starts(&xml)?;
    let mut reader = Reader::from_reader(xml.as_bytes());
    // For each open element, whether the wrappers go inside it.
    let mut open: Vec<bool> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let word = word_starts.contains(&(reader.buffer_position() as usize));
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison XML scan failed: {error}")))?;
        let (element, empty) = match &event {
            Event::Start(element) => (element, false),
            Event::Empty(element) => (element, true),
            Event::End(_) => {
                open.pop();
                buffer.clear();
                continue;
            }
            Event::Eof => return Ok(false),
            _ => {
                buffer.clear();
                continue;
            }
        };
        let local = element.local_name();
        let parent_wrapped = open.last().copied();
        if word && parent_wrapped == Some(true) && local.as_ref() == b"fldSimple" {
            return Ok(true);
        }
        if !empty {
            open.push(
                parent_wrapped
                    .is_none_or(|wrapped| wrapped && word && local.as_ref() == b"hyperlink"),
            );
        }
        buffer.clear();
    }
}

#[derive(Clone, Copy, PartialEq)]
enum FieldBalance {
    /// Every complex field begun is ended, and every owner lies inside one.
    Whole,
    /// A complex field begun here ends after the paragraphs.
    Opens,
    /// A complex field ended here begins before the paragraphs.
    Closes,
    /// The fields are whole, but an owner lies outside all of them.
    Uncarried,
}

fn field_balance(paragraphs: &[&CT_P]) -> Result<FieldBalance> {
    let mut depth = 0usize;
    let mut uncarried = false;
    for paragraph in paragraphs {
        for event in paragraph_field_events(paragraph)? {
            match event {
                FieldEvent::Begin => depth += 1,
                FieldEvent::End if depth == 0 => return Ok(FieldBalance::Closes),
                FieldEvent::End => depth -= 1,
                FieldEvent::Owner => uncarried |= depth == 0,
            }
        }
    }
    Ok(if depth > 0 {
        FieldBalance::Opens
    } else if uncarried {
        FieldBalance::Uncarried
    } else {
        FieldBalance::Whole
    })
}

pub(super) fn validate_field_alignment(
    aligned: &[(Option<usize>, Option<usize>)],
    original: &[CT_R],
    edited: &[CT_R],
    original_signatures: &[String],
    edited_signatures: &[String],
    location: &str,
) -> Result<()> {
    for (left, right) in aligned {
        let left_field = left.is_some_and(|index| run_is_field(&original[index]));
        let right_field = right.is_some_and(|index| run_is_field(&edited[index]));
        let unchanged_pair = match (*left, *right) {
            (Some(i), Some(j)) => original_signatures[i] == edited_signatures[j],
            _ => false,
        };
        if (left_field || right_field) && !unchanged_pair {
            return Err(Error::Other(format!(
                "comparison cannot revise a modeled field at {location}"
            )));
        }
    }
    Ok(())
}

//! Comments one version adds or removes whole, as Word tracks them (Q6, W9):
//! their range marks stand outside the comparison, which compares their
//! reference runs as runs, written in `w:ins` or `w:del`, and they come back
//! where the version that holds them has them.

use super::*;
use crate::RevisionKind;
use crate::revision::XmlTree;

/// A comment range mark taken out of a main part: whether it starts, its
/// comment, and how many tokens of its version come before it.
pub(super) struct TakenMark {
    start: bool,
    id: i32,
    offset: usize,
}

/// One token of the walk of a main part, or a range mark on the way.
enum Step {
    /// `len` characters of the text element `element` in run `run`.
    Text {
        element: usize,
        run: usize,
        len: usize,
        first: bool,
    },
    /// Another run child (a tab, a break, a drawing, a reference).
    Leaf {
        element: usize,
        run: usize,
        first: bool,
    },
    /// The mark of paragraph `paragraph`.
    End { paragraph: usize },
    /// A comment range mark: whether it starts, and its comment.
    Mark {
        element: usize,
        start: bool,
        id: Option<i32>,
    },
}

/// The tokens of a version in a main part, in order: the characters of its
/// texts, its other run children and its paragraph marks. In a redline,
/// what the version does not hold is left out: the content of the written
/// revisions of the `hidden` kinds, and the paragraph marks and rows they
/// mark.
struct Walk<'a, F> {
    tree: &'a XmlTree<'a>,
    xml: &'a [u8],
    hidden: &'a [RevisionKind],
    written: F,
}

impl<F: Fn(i32) -> bool> Walk<'_, F> {
    fn hides(&self, index: usize) -> bool {
        self.tree.elements[index]
            .revision
            .as_ref()
            .is_some_and(|metadata| {
                self.hidden.contains(&metadata.kind) && (self.written)(metadata.id)
            })
    }

    /// The word child of `index` named `local`.
    fn child(&self, index: usize, local: &str) -> Option<usize> {
        self.tree.elements[index]
            .children
            .iter()
            .copied()
            .find(|&child| {
                let element = &self.tree.elements[child];
                element.word && element.local == local
            })
    }

    /// Whether the properties `properties` (a `w:pPr` or a `w:trPr`) hold a
    /// mark this walk hides, in `w:rPr` for a paragraph's.
    fn marks_hidden(&self, properties: Option<usize>, run_properties: bool) -> bool {
        let owner = match (properties, run_properties) {
            (Some(properties), true) => self.child(properties, "rPr"),
            (properties, _) => properties,
        };
        owner.is_some_and(|owner| {
            self.tree.elements[owner]
                .children
                .iter()
                .any(|&mark| self.hides(mark))
        })
    }

    fn walk(&self, index: usize, steps: &mut Vec<Step>) -> Result<()> {
        let element = &self.tree.elements[index];
        if element.word {
            match element.local.as_str() {
                "ins" | "del" | "moveFrom" | "moveTo" if self.hides(index) => return Ok(()),
                "tr" if self.marks_hidden(self.child(index, "trPr"), false) => return Ok(()),
                "r" => return self.walk_run(index, steps),
                "commentRangeStart" | "commentRangeEnd" => {
                    steps.push(Step::Mark {
                        element: index,
                        start: element.local == "commentRangeStart",
                        id: self
                            .tree
                            .word_attribute(index, "id")?
                            .and_then(|id| id.trim().parse().ok()),
                    });
                    return Ok(());
                }
                local
                    if local.ends_with("Pr")
                        || local.ends_with("PrChange")
                        || matches!(local, "tblGrid" | "sectPr") =>
                {
                    return Ok(());
                }
                _ => {}
            }
        }
        for &child in &element.children {
            self.walk(child, steps)?;
        }
        if element.word
            && element.local == "p"
            && !self.marks_hidden(self.child(index, "pPr"), true)
        {
            steps.push(Step::End { paragraph: index });
        }
        Ok(())
    }

    fn walk_run(&self, run: usize, steps: &mut Vec<Step>) -> Result<()> {
        let mut first = true;
        for &child in &self.tree.elements[run].children {
            let element = &self.tree.elements[child];
            if element.word && matches!(element.local.as_str(), "rPr" | "lastRenderedPageBreak") {
                continue;
            }
            if element.word
                && matches!(
                    element.local.as_str(),
                    "t" | "delText" | "instrText" | "delInstrText"
                )
            {
                let len = text_of(self.xml, element)?.chars().count();
                if len > 0 {
                    steps.push(Step::Text {
                        element: child,
                        run,
                        len,
                        first,
                    });
                    first = false;
                }
                continue;
            }
            steps.push(Step::Leaf {
                element: child,
                run,
                first,
            });
            first = false;
        }
        Ok(())
    }
}

/// The decoded text of a text element.
fn text_of(xml: &[u8], element: &crate::revision::XmlElement) -> Result<String> {
    if element.empty {
        return Ok(String::new());
    }
    let raw = std::str::from_utf8(&xml[element.open_end..element.close_start]).map_err(utf8_error)?;
    Ok(quick_xml::escape::unescape(raw)
        .map_err(|error| Error::Other(error.to_string()))?
        .into_owned())
}

fn steps_of<'a>(
    tree: &'a XmlTree<'a>,
    xml: &'a [u8],
    hidden: &'a [RevisionKind],
    written: impl Fn(i32) -> bool,
) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    Walk {
        tree,
        xml,
        hidden,
        written,
    }
    .walk(tree.root, &mut steps)?;
    Ok(steps)
}

/// The ids of the comments whose two range marks and reference a main part
/// holds in its body text, and of every comment it marks anywhere.
fn body_comments(xml: &[u8]) -> Result<(HashSet<i32>, HashSet<i32>)> {
    let tree = XmlTree::parse(xml)?;
    let steps = steps_of(&tree, xml, &[], |_| false)?;
    let mut counts: HashMap<i32, [usize; 3]> = HashMap::new();
    for step in &steps {
        match step {
            Step::Mark {
                start,
                id: Some(id),
                ..
            } => counts.entry(*id).or_default()[usize::from(!start)] += 1,
            Step::Leaf { element, .. }
                if tree.elements[*element].word
                    && tree.elements[*element].local == "commentReference" =>
            {
                if let Some(id) = tree
                    .word_attribute(*element, "id")?
                    .and_then(|id| id.trim().parse().ok())
                {
                    counts.entry(id).or_default()[2] += 1;
                }
            }
            _ => {}
        }
    }
    let mut anywhere = HashSet::new();
    for (index, element) in tree.elements.iter().enumerate() {
        if element.word
            && matches!(
                element.local.as_str(),
                "commentRangeStart" | "commentRangeEnd" | "commentReference"
            )
            && let Some(id) = tree
                .word_attribute(index, "id")?
                .and_then(|id| id.trim().parse().ok())
        {
            anywhere.insert(id);
        }
    }
    let whole = counts
        .into_iter()
        .filter(|(_, [starts, ends, references])| *starts == 1 && *ends == 1 && *references >= 1)
        .map(|(id, _)| id)
        .collect();
    Ok((whole, anywhere))
}

/// The comments the edited main part adds whole to the original's, and those
/// it removes whole: marked in the body text of one, with both range marks
/// and a reference, and neither marked nor defined in the other.
pub(super) fn one_sided_comments(
    original: (&[u8], &HashSet<i32>),
    edited: (&[u8], &HashSet<i32>),
) -> Result<[HashSet<i32>; 2]> {
    let (original_whole, original_anywhere) = body_comments(original.0)?;
    let (edited_whole, edited_anywhere) = body_comments(edited.0)?;
    let only = |whole: HashSet<i32>, anywhere: &HashSet<i32>, defined: &HashSet<i32>| {
        whole
            .into_iter()
            .filter(|id| !anywhere.contains(id) && !defined.contains(id))
            .collect::<HashSet<_>>()
    };
    Ok([
        only(edited_whole, &original_anywhere, original.1),
        only(original_whole, &edited_anywhere, edited.1),
    ])
}

/// `xml` without the range marks of these comments, and each mark taken
/// with the number of tokens before it.
pub(super) fn take_comment_marks(
    xml: &[u8],
    ids: &HashSet<i32>,
) -> Result<(Vec<u8>, Vec<TakenMark>)> {
    if ids.is_empty() {
        return Ok((xml.to_vec(), Vec::new()));
    }
    let tree = XmlTree::parse(xml)?;
    let steps = steps_of(&tree, xml, &[], |_| false)?;
    let (mut offset, mut taken, mut spans) = (0, Vec::new(), Vec::new());
    for step in steps {
        match step {
            Step::Text { len, .. } => offset += len,
            Step::Leaf { .. } | Step::End { .. } => offset += 1,
            Step::Mark {
                element,
                start,
                id: Some(id),
            } if ids.contains(&id) => {
                taken.push(TakenMark { start, id, offset });
                let element = &tree.elements[element];
                spans.push(element.start..element.end);
            }
            Step::Mark { .. } => {}
        }
    }
    let mut out = Vec::with_capacity(xml.len());
    let mut cursor = 0;
    for span in spans {
        out.extend_from_slice(&xml[cursor..span.start]);
        cursor = span.end;
    }
    out.extend_from_slice(&xml[cursor..]);
    Ok((out, taken))
}

/// Where a mark goes in a redline: before a token of a run, which splits
/// the run there unless the token starts it, or at a paragraph's end.
enum Spot {
    Before(usize),
    Split { run: usize, element: usize, at: usize },
}

/// `xml`, a redline, with `marks` back where the version they were taken
/// from has them: each before the token of the version that followed it,
/// the version's tokens being those the walk keeps when it leaves out the
/// written revisions of the `hidden` kinds. A mark before a run's first
/// token stands before the run, and before the written revision wrappers
/// that start with it.
pub(super) fn place_comment_marks(
    xml: &[u8],
    marks: &[TakenMark],
    hidden: &[RevisionKind],
    written: impl Fn(i32) -> bool + Copy,
) -> Result<Vec<u8>> {
    if marks.is_empty() {
        return Ok(xml.to_vec());
    }
    let tree = XmlTree::parse(xml)?;
    let elements = &tree.elements;
    let steps = steps_of(&tree, xml, hidden, written)?;
    let climb = |mut at: usize| {
        while let Some(parent) = elements[at].parent {
            let wrapper = &elements[parent];
            let starts = wrapper.children.first() == Some(&at);
            if !(starts
                && wrapper.word
                && matches!(wrapper.local.as_str(), "ins" | "del" | "moveFrom" | "moveTo")
                && wrapper
                    .revision
                    .as_ref()
                    .is_some_and(|metadata| written(metadata.id)))
            {
                break;
            }
            at = parent;
        }
        elements[at].start
    };
    // The spot of each mark, by the token that follows it.
    let mut spots: Vec<Option<Spot>> = (0..marks.len()).map(|_| None).collect();
    let mut order = (0..marks.len()).collect::<Vec<_>>();
    order.sort_by_key(|&mark| marks[mark].offset);
    let mut pending = order.iter().peekable();
    let (mut offset, mut last_end) = (0, None);
    for step in &steps {
        let len = match step {
            Step::Text { len, .. } => *len,
            Step::Leaf { .. } | Step::End { .. } => 1,
            Step::Mark { .. } => 0,
        };
        if let Step::End { paragraph } = step {
            last_end = Some(*paragraph);
        }
        while let Some(&&mark) = pending.peek()
            && marks[mark].offset < offset + len
        {
            let at = marks[mark].offset - offset;
            spots[mark] = Some(match *step {
                Step::Text { run, first, .. } if first && at == 0 => Spot::Before(climb(run)),
                Step::Text { element, run, .. } => Spot::Split { run, element, at },
                Step::Leaf {
                    element,
                    run,
                    first,
                } => match first {
                    true => Spot::Before(climb(run)),
                    false => Spot::Split {
                        run,
                        element,
                        at: 0,
                    },
                },
                Step::End { paragraph } => Spot::Before(elements[paragraph].close_start),
                Step::Mark { .. } => unreachable!("a mark has no token"),
            });
            pending.next();
        }
        offset += len;
    }
    // A mark after the last token: at the end of the last paragraph.
    for &mark in pending {
        let paragraph = last_end
            .ok_or_else(|| Error::Other("a comment mark has no paragraph to stand in".into()))?;
        spots[mark] = Some(Spot::Before(elements[paragraph].close_start));
    }
    let written_mark = |mark: &TakenMark| {
        format!(
            r#"<w:{} w:id="{}"/>"#,
            if mark.start {
                "commentRangeStart"
            } else {
                "commentRangeEnd"
            },
            mark.id
        )
    };
    // Edits: marks inserted at a byte position, runs rebuilt around splits.
    let mut inserted: BTreeMap<usize, String> = BTreeMap::new();
    let mut splits: BTreeMap<usize, Vec<(usize, usize, String)>> = BTreeMap::new();
    for (mark, spot) in marks.iter().zip(spots) {
        match spot.ok_or_else(|| Error::Other("a comment mark found no place".into()))? {
            Spot::Before(at) => inserted.entry(at).or_default().push_str(&written_mark(mark)),
            Spot::Split { run, element, at } => {
                splits
                    .entry(run)
                    .or_default()
                    .push((element, at, written_mark(mark)))
            }
        }
    }
    let mut edits: Vec<(Range<usize>, String)> = inserted
        .into_iter()
        .map(|(at, marks)| (at..at, marks))
        .collect();
    for (run, cuts) in splits {
        edits.push((
            elements[run].start..elements[run].end,
            split_run(&tree, xml, run, &cuts)?,
        ));
    }
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    let mut out = Vec::with_capacity(xml.len() + edits.len() * 40);
    let mut cursor = 0;
    for (range, text) in edits {
        if range.start < cursor {
            return Err(Error::Other("comment marks overlap a rebuilt run".into()));
        }
        out.extend_from_slice(&xml[cursor..range.start]);
        out.extend_from_slice(text.as_bytes());
        cursor = range.end;
    }
    out.extend_from_slice(&xml[cursor..]);
    Ok(out)
}

/// Run `run` written again as runs split at `cuts`, each a child element
/// and a character offset in it (0 for a child that is no text), with the
/// marks of each cut between the pieces. Each piece keeps the run's start
/// tag and properties.
fn split_run(
    tree: &XmlTree<'_>,
    xml: &[u8],
    run: usize,
    cuts: &[(usize, usize, String)],
) -> Result<String> {
    let elements = &tree.elements;
    let text = |range: Range<usize>| std::str::from_utf8(&xml[range]).map_err(utf8_error);
    let open = text(elements[run].start..elements[run].open_end)?;
    let close = format!("</{}>", elements[run].name);
    let properties = elements[run]
        .children
        .iter()
        .find(|&&child| elements[child].word && elements[child].local == "rPr")
        .map(|&child| text(elements[child].start..elements[child].end))
        .transpose()?
        .unwrap_or_default();
    let (mut out, mut piece) = (String::new(), String::new());
    let flush = |piece: &mut String, out: &mut String| {
        if !piece.is_empty() {
            out.push_str(&format!("{open}{properties}{piece}{close}"));
            piece.clear();
        }
    };
    for &child in &elements[run].children {
        let element = &elements[child];
        if element.word && element.local == "rPr" {
            continue;
        }
        let mut here = cuts
            .iter()
            .filter(|(at, ..)| *at == child)
            .collect::<Vec<_>>();
        if here.is_empty() {
            piece.push_str(text(element.start..element.end)?);
            continue;
        }
        here.sort_by_key(|(_, offset, _)| *offset);
        let is_text = element.word
            && matches!(
                element.local.as_str(),
                "t" | "delText" | "instrText" | "delInstrText"
            );
        if !is_text {
            flush(&mut piece, &mut out);
            for (.., mark) in here {
                out.push_str(mark);
            }
            piece.push_str(text(element.start..element.end)?);
            continue;
        }
        let value = text_of(xml, element)?.chars().collect::<Vec<_>>();
        let written = |part: &[char]| {
            format!(
                r#"<{name} xml:space="preserve">{}</{name}>"#,
                quick_xml::escape::escape(part.iter().collect::<String>()),
                name = element.name
            )
        };
        let mut from = 0;
        for (_, offset, mark) in here {
            if *offset > from {
                piece.push_str(&written(&value[from..*offset]));
            }
            flush(&mut piece, &mut out);
            out.push_str(mark);
            from = *offset;
        }
        if from < value.len() {
            piece.push_str(&written(&value[from..]));
        }
    }
    flush(&mut piece, &mut out);
    Ok(out)
}

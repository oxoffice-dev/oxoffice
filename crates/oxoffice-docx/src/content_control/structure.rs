//! Locating a control or its parent in a story part, fresh control ids, publishing a candidate, and removing or unwrapping a control.

use super::*;

impl Document {
    pub(super) fn publish_candidate(&mut self, candidate: Document) -> Result<()> {
        let reopened = candidate
            .prepare_and_reopen_staged()
            .map_err(control_fault)?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }

    pub(super) fn smallest_content_control_id(&self) -> Result<i32> {
        let mut used = HashSet::new();
        for xml in self.owned_story_parts()? {
            collect_control_ids(&xml, &mut used)?;
        }
        let mut id = 1i32;
        while used.contains(&id) {
            id = id
                .checked_add(1)
                .ok_or_else(|| Error::Other("content control ids are exhausted".to_owned()))?;
        }
        Ok(id)
    }
}

impl Document {
    pub(super) fn rewrite_control(
        &mut self,
        location: &ContentLocation,
        rewrite: ControlRewrite,
    ) -> Result<()> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_to_package()?;
        let (part, mut xml, range, _) = control_item(&candidate, location)?;
        let replacement = match rewrite {
            ControlRewrite::Remove => {
                refuse_cell_without_paragraph(&candidate, location.story(), &xml, &range, b"")?;
                Vec::new()
            }
            ControlRewrite::Unwrap => {
                let inner = sdt_content_bytes(&xml, &range)?;
                if !unwrap_keeps_repeating_context(&xml, &range)?
                    || !parent_can_hold(&xml, &range, &inner)?
                {
                    return Err(Error::Other(
                        "content control unwrap leaves content the parent cannot hold".to_owned(),
                    ));
                }
                refuse_cell_without_paragraph(&candidate, location.story(), &xml, &range, &inner)?;
                inner
            }
        };
        // A removal takes the comment threads inside it, and from the main
        // part the notes only it referenced.
        let removing = matches!(rewrite, ControlRewrite::Remove);
        let before = (removing && part == candidate.doc_part_name).then(|| xml.clone());
        xml.splice(range, replacement);
        candidate.write_story_part(&part, xml.clone())?;
        if let Some(before) = before {
            candidate.remove_unreferenced_notes(&before, &xml)?;
        }
        if removing {
            candidate.reconcile_comment_removal(self)?;
        }
        self.publish_candidate(candidate)
    }
}

pub(super) enum ControlRewrite {
    Remove,
    Unwrap,
}

pub(super) fn control_fault(error: Error) -> Error {
    match error {
        Error::Story(inner) => Error::Other(inner.to_string()),
        other => other,
    }
}

pub(super) fn require_block_parent(
    document: &Document,
    destination: &ContentLocation,
) -> Result<()> {
    let kind = destination.story().kind();
    if !matches!(
        kind,
        StoryKind::Body
            | StoryKind::TableCell
            | StoryKind::Header
            | StoryKind::Footer
            | StoryKind::Footnote
            | StoryKind::Endnote
            | StoryKind::Comment
            | StoryKind::TextBox
    ) {
        return Err(Error::Other(
            "content control placement does not match its parent".to_owned(),
        ));
    }
    if Document::location_is_end(destination) {
        return Ok(());
    }
    let (_, _, _, direct) = document
        .copy_indexed_story_item(destination, destination.index_path())
        .map_err(control_fault)?;
    if direct {
        Ok(())
    } else {
        Err(Error::Other(
            "content control placement does not match its parent".to_owned(),
        ))
    }
}

pub(super) fn table_parent_range(
    document: &Document,
    destination: &ContentLocation,
) -> Result<PartRange> {
    if Document::location_is_end(destination)
        || destination.item_kind() != StoryItemKind::Table
        || destination.index_path().len() != 1
    {
        return Err(Error::Other(
            "content control placement does not match its parent".to_owned(),
        ));
    }
    let (part, xml, range, _) = document
        .copy_indexed_story_item(destination, destination.index_path())
        .map_err(control_fault)?;
    Ok(PartRange { part, xml, range })
}

pub(super) fn row_parent_range(
    document: &Document,
    destination: &ContentLocation,
) -> Result<PartRange> {
    let path = destination.index_path();
    let [table_index, row_index] = path else {
        return Err(Error::Other(
            "content control placement does not match its parent".to_owned(),
        ));
    };
    if destination.item_kind() != StoryItemKind::Table {
        return Err(Error::Other(
            "content control placement does not match its parent".to_owned(),
        ));
    }
    let (part, xml, table_range, _) = document
        .copy_indexed_story_item(destination, &[*table_index])
        .map_err(control_fault)?;
    let range = nth_direct_child(&xml, &table_range, b"tr", *row_index)?;
    Ok(PartRange { part, xml, range })
}

pub(super) fn paragraph_parent_range(
    document: &Document,
    destination: &ContentLocation,
) -> Result<PartRange> {
    if Document::location_is_end(destination) || destination.item_kind() != StoryItemKind::Paragraph
    {
        return Err(Error::Other(
            "content control placement does not match its parent".to_owned(),
        ));
    }
    let (part, xml, range, _) = document
        .copy_indexed_story_item(destination, destination.index_path())
        .map_err(control_fault)?;
    Ok(PartRange { part, xml, range })
}

pub(super) fn splice_inside(
    document: &mut Document,
    located: PartRange,
    payload: &str,
) -> Result<()> {
    let PartRange {
        part,
        mut xml,
        range,
    } = located;
    let replacement = insert_into_element(&xml, &range, payload.as_bytes())?;
    xml.splice(range, replacement);
    document.write_story_part(&part, xml)
}

pub(super) fn control_item(
    document: &Document,
    location: &ContentLocation,
) -> Result<(String, Vec<u8>, Range<usize>, bool)> {
    document
        .copy_indexed_story_item(location, location.index_path())
        .map_err(control_fault)
}

fn collect_control_ids(xml: &[u8], used: &mut HashSet<i32>) -> Result<()> {
    let elems = index_elements(xml)?;
    for elem in &elems {
        if !(elem.word && elem.local == b"id") {
            continue;
        }
        let Some(parent) = elem
            .parent
            .and_then(|id| elems.iter().find(|item| item.id == id))
        else {
            continue;
        };
        if !(parent.word && parent.local == b"sdtPr") {
            continue;
        }
        if let Some(value) = &elem.val
            && let Ok(id) = value.parse::<i32>()
            && id > 0
        {
            used.insert(id);
        }
    }
    Ok(())
}

fn sdt_content_bytes(xml: &[u8], sdt_range: &Range<usize>) -> Result<Vec<u8>> {
    let elems = index_elements(xml)?;
    let sdt = element_at(&elems, sdt_range)?;
    let Some(content) = elems
        .iter()
        .find(|elem| elem.parent == Some(sdt.id) && elem.word && elem.local == b"sdtContent")
    else {
        return Ok(Vec::new());
    };
    let Some(inner_end) = content.inner_end else {
        return Ok(Vec::new());
    };
    Ok(xml[content.open_end..inner_end].to_vec())
}

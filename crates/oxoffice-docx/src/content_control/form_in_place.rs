//! Form operations on the body blocks that hold their targets: the targeted
//! controls are read from their blocks, the plan is today's, and each changed
//! block is swapped in place, all in one edit.

use super::form_plan::apply_binding_groups;
use super::form_state::read_physical_control;
use super::*;
use crate::{EditUnit, StoryId};

/// What a form operation made in place answers: its counts, and the root
/// stories of the header and footer parts it wrote, whose owners the caller
/// reads again.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormEdit {
    pub counts: Vec<usize>,
    pub stories: Vec<StoryId>,
}

/// What a form operation plans once its targets are selected.
pub(super) enum FormPlanner<'a> {
    Fill(&'a [FormFieldAssignment]),
    Reset,
}

impl Document {
    /// Make a form operation on the direct body blocks that hold its targets
    /// (`EditUnit::BodyBlocks`, one unit per changed block) and on the custom
    /// XML items its bound controls write (`EditUnit::Parts`), and answer its
    /// counts.
    ///
    /// A header or footer part that holds a target is written whole, under
    /// `EditUnit::Parts`.
    ///
    /// `Ok(None)` changes nothing and leaves the operation to the staged
    /// route: a legacy field target, a tag a note or a comment names, a body that
    /// cannot be saved after a typed edit, and a block that does not read
    /// back as it writes. A refusal leaves the document as it was.
    pub(super) fn form_in_blocks(
        &mut self,
        rules: &[FormFieldRule],
        planner: FormPlanner<'_>,
    ) -> Result<Option<FormEdit>> {
        let assignments = match &planner {
            FormPlanner::Fill(assignments) => *assignments,
            FormPlanner::Reset => &[],
        };
        let mut tags = Vec::<String>::new();
        for target in rules
            .iter()
            .map(|rule| &rule.target)
            .chain(assignments.iter().map(|assignment| &assignment.target))
        {
            match target {
                FormFieldTarget::ContentControlTag(tag) => {
                    if !tags.contains(tag) {
                        tags.push(tag.clone());
                    }
                }
                FormFieldTarget::Legacy { .. } => return Ok(None),
            }
        }
        if self.unsavable_body_edit().is_some() {
            return Ok(None);
        }
        let Some(story_parts) = self.header_parts_naming_control_tags(&tags)? else {
            return Ok(None);
        };
        // Every control of a block that names a tag, read as the staged route
        // reads the whole part: a block stands for its part, and the ordinal
        // counts the controls of the block.
        let mut blocks = Vec::new();
        let mut controls = Vec::new();
        for slot in self.body_slots_with_control_tags(&tags) {
            let Some(part) = self.body_block_part(slot)? else {
                return Ok(None);
            };
            let key = format!("{}#{slot}", self.doc_part_name);
            let elems = index_elements(&part.xml)?;
            for (ordinal, range) in Self::part_form_controls(&part.xml, StoryKind::Body)?
                .iter()
                .enumerate()
            {
                let elem = elems
                    .iter()
                    .find(|elem| {
                        elem.word && elem.local == b"sdt" && elem.range.start == range.start
                    })
                    .ok_or_else(|| form_fault("stale form target"))?;
                controls.push(read_physical_control(
                    &part.xml, &elems, elem, &key, ordinal,
                )?);
            }
            blocks.push((slot, key, part));
        }
        // A header or footer part is read whole, as the staged route reads it:
        // its cost is its own.
        let mut headers = Vec::new();
        for (name, root) in story_parts {
            let xml = self
                .package
                .get_part(&name)
                .ok_or_else(|| form_fault(format!("form story part {name} is missing")))?
                .to_vec();
            let elems = index_elements(&xml)?;
            for (ordinal, range) in Self::part_form_controls(&xml, root)?.iter().enumerate() {
                let elem = elems
                    .iter()
                    .find(|elem| {
                        elem.word && elem.local == b"sdt" && elem.range.start == range.start
                    })
                    .ok_or_else(|| form_fault("stale form target"))?;
                controls.push(read_physical_control(&xml, &elems, elem, &name, ordinal)?);
            }
            headers.push((name, root, xml));
        }
        let mut state = FormState {
            doc_part: self.doc_part_name.clone(),
            controls,
            legacy: Vec::new(),
            anchors: Vec::new(),
        };
        let selections = select_rules(self, &mut state, rules)?;
        let plan = match planner {
            FormPlanner::Fill(assignments) => plan_fill(self, &state, assignments, &selections)?,
            FormPlanner::Reset => plan_reset(self, &state, rules, &selections)?,
        };
        if !plan.legacy.is_empty() {
            return Ok(None);
        }
        let placeholder_styles = placeholder_style_ids(&self.styles);
        let mut swaps = Vec::new();
        let mut same_declarations = true;
        for (slot, key, part) in &blocks {
            let mut located = Vec::new();
            for patch in plan.patches.iter().filter(|patch| &patch.part == key) {
                let control = state
                    .controls
                    .iter()
                    .find(|control| control.part == patch.part && control.ordinal == patch.ordinal)
                    .ok_or_else(|| form_fault("stale form target"))?;
                located.push((control.start, &patch.patch));
            }
            located.sort_by_key(|(start, _)| std::cmp::Reverse(*start));
            let mut edited = part.xml.clone();
            for (start, patch) in located {
                apply_control_patch(&mut edited, start, patch, &placeholder_styles)?;
            }
            let (before, after) = (part.block_of(&part.xml), part.block_of(&edited));
            if before == after {
                continue;
            }
            same_declarations &= declares_only_word_extensions(before, after);
            let Some(parsed) = Self::reparse_body_block(&part.head, part.tail_bytes(), after)
            else {
                return Ok(None);
            };
            swaps.push((*slot, parsed));
        }
        let mut written = Vec::new();
        for (name, root, xml) in headers {
            let mut located = Vec::new();
            for patch in plan.patches.iter().filter(|patch| patch.part == name) {
                let control = state
                    .controls
                    .iter()
                    .find(|control| control.part == patch.part && control.ordinal == patch.ordinal)
                    .ok_or_else(|| form_fault("stale form target"))?;
                located.push((control.start, &patch.patch));
            }
            located.sort_by_key(|(start, _)| std::cmp::Reverse(*start));
            let mut edited = xml.clone();
            for (start, patch) in located {
                apply_control_patch(&mut edited, start, patch, &placeholder_styles)?;
            }
            if edited != xml {
                written.push((name, root, edited));
            }
        }
        // One story publication reads the whole inventory and keeps the
        // owners of every other story, so a call that changes a body block
        // and a header or footer part at once keeps today's route, which
        // publishes both together.
        if !swaps.is_empty() && !written.is_empty() {
            return Ok(None);
        }
        let bindings = apply_binding_groups(self, &self.doc_part_name, &plan.bindings)?;
        if swaps.is_empty() && bindings.is_empty() && written.is_empty() {
            return Ok(Some(FormEdit {
                counts: plan.counts,
                stories: Vec::new(),
            }));
        }
        let stories = written
            .iter()
            .map(|(name, root, _)| StoryId::new(*root, name.clone(), 0))
            .collect::<Vec<_>>();
        let mut units = swaps
            .iter()
            .map(|(slot, _)| EditUnit::BodyBlocks {
                slot: *slot,
                count: 1,
            })
            .collect::<Vec<_>>();
        if !bindings.is_empty() || !written.is_empty() {
            units.push(EditUnit::Parts);
        }
        let slots = swaps.iter().map(|(slot, _)| *slot).collect::<Vec<_>>();
        self.edit_in_place(&units, |document| {
            for (slot, block) in swaps {
                document.document.body.content[slot] = block;
            }
            for (part, xml) in written
                .into_iter()
                .map(|(name, _, xml)| (name, xml))
                .chain(bindings)
            {
                document.package.set_part(&part, xml);
            }
            if slots.is_empty() {
                document.invalidate_layout_keeping_body_answer();
            } else {
                document.invalidate_layout();
            }
            Ok(())
        })?;
        if self.take_last_edit_changed() && !slots.is_empty() {
            // The kept answer of one typed insert reads that block alone; for
            // several blocks it holds only when no block changed a namespace
            // declaration.
            if let ([slot], true) = (slots.as_slice(), same_declarations) {
                // `body_block_part` answered that the body saved before the edit.
                self.keep_body_savable_after_typed_insert(*slot, true);
            }
            if same_declarations {
                self.keep_body_savable_after_swap();
            }
        }
        Ok(Some(FormEdit {
            counts: plan.counts,
            stories,
        }))
    }
}

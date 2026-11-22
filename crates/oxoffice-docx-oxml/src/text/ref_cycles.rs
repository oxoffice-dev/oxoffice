//! Bookmarks whose values depend on each other through REF fields.

use std::collections::{BTreeMap, BTreeSet};

use super::{Field, FieldArgument};

/// Add the bookmarks a field's display depends on to `targets`: the target of
/// each REF in its tree, its own and those of the fields nested in its
/// instruction or result, folded to lower case as Word compares bookmark
/// names.
#[doc(hidden)]
pub fn field_ref_targets(field: &Field, targets: &mut BTreeSet<String>) {
    let instruction = field.effective_instruction();
    if instruction.name.eq_ignore_ascii_case("REF")
        && let Some(FieldArgument::Text(target)) = instruction.arguments.first()
    {
        targets.insert(target.to_lowercase());
    }
    for nested in field.all_nested_fields_in_source_order() {
        field_ref_targets(nested, targets);
    }
}

/// The bookmarks whose value depends on the bookmark `name`, `name` included,
/// folded to lower case: those its value reaches through REF fields that reach
/// it back. `targets_of` gives the folded REF targets in a bookmark's value,
/// as [`field_ref_targets`] reads them. A field whose REF targets one of them
/// would copy the bookmark into itself, a self-reference Word refuses.
#[doc(hidden)]
pub fn bookmarks_depending_on(
    name: &str,
    targets_of: &mut dyn FnMut(&str) -> BTreeSet<String>,
) -> BTreeSet<String> {
    let name = name.to_lowercase();
    // The bookmarks the value of `name` reaches, with the targets of each.
    let mut graph = BTreeMap::<String, BTreeSet<String>>::new();
    let mut pending = vec![name.clone()];
    while let Some(current) = pending.pop() {
        if graph.contains_key(&current) {
            continue;
        }
        let targets = targets_of(&current);
        pending.extend(
            targets
                .iter()
                .filter(|target| !graph.contains_key(*target))
                .cloned(),
        );
        graph.insert(current, targets);
    }
    // Those of them that reach `name` back.
    let mut reverse = BTreeMap::<&str, Vec<&str>>::new();
    for (from, targets) in &graph {
        for to in targets {
            reverse.entry(to.as_str()).or_default().push(from.as_str());
        }
    }
    let mut depending = BTreeSet::new();
    let mut pending = vec![name.as_str()];
    while let Some(current) = pending.pop() {
        if depending.insert(current.to_owned())
            && let Some(sources) = reverse.get(current)
        {
            pending.extend(sources.iter().copied());
        }
    }
    depending
}

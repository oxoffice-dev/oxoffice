//! Which layout and master placeholders a slide placeholder follows.
//!
//! Pure lookups over the slide, layout and master models. Rendering resolves
//! inheritance with them, and so do the edits that need an inherited
//! position, which therefore work without the rendering crates.

use oxoffice_drawing::xfrm::CT_Transform2D;

use crate::placeholder::{CT_Placeholder, PhType, PlaceholderKey};
use crate::shape_tree::{CT_Shape, ShapeTreeChild};
use crate::slide_parts::{CT_SlideLayout, CT_SlideMaster};

/// Whether a placeholder is a date, footer or slide number, the types that
/// match by type alone.
pub fn is_latent(ph_type: &PhType) -> bool {
    matches!(
        ph_type,
        PhType::DateTime | PhType::Footer | PhType::SlideNumber
    )
}

/// Whether two placeholder keys name the same placeholder, latent types
/// matching by type alone.
pub fn placeholder_keys_match(left: &PlaceholderKey, right: &PlaceholderKey) -> bool {
    if is_latent(&left.ph_type) && is_latent(&right.ph_type) {
        left.ph_type == right.ph_type
    } else {
        left.matches(right)
    }
}

/// Returns the transform a slide placeholder inherits, ignoring its own.
///
/// The placeholder matches a layout placeholder, whose own key then matches
/// a master placeholder. The first of the two that carries a transform
/// supplies it whole, which is the fallback rendering takes when the slide
/// shape has no transform of its own.
pub fn inherited_xfrm<'a>(
    placeholder: &CT_Placeholder,
    layout: &'a CT_SlideLayout,
    master: &'a CT_SlideMaster,
) -> Option<&'a CT_Transform2D> {
    let (layout, master) = placeholder_chain(placeholder, layout, master);
    [layout, master]
        .into_iter()
        .flatten()
        .find_map(|shape| shape.shape_properties.transform.as_ref())
}

/// The layout placeholder a slide placeholder matches, then the master
/// placeholder that layout placeholder matches.
pub fn placeholder_chain<'a>(
    placeholder: &CT_Placeholder,
    layout: &'a CT_SlideLayout,
    master: &'a CT_SlideMaster,
) -> (Option<&'a CT_Shape>, Option<&'a CT_Shape>) {
    let Some(layout_shape) = find_placeholder(
        &layout.common_slide_data.shape_tree.children,
        &placeholder.key(),
    ) else {
        return (None, None);
    };
    let Some(layout_key) = layout_shape
        .placeholder
        .as_ref()
        .map(|placeholder| placeholder.key())
    else {
        return (None, None);
    };
    let children = &master.common_slide_data.shape_tree.children;
    let mut pending = children.iter().rev().collect::<Vec<_>>();
    let mut candidates = Vec::new();
    let family = PlaceholderKey {
        ph_type: layout_key.ph_type.clone(),
        idx: None,
    };
    while let Some(child) = pending.pop() {
        match child {
            ShapeTreeChild::Shape(shape) => {
                if let Some(ph) = &shape.placeholder {
                    let candidate = PlaceholderKey {
                        ph_type: ph.effective_type(),
                        idx: None,
                    };
                    if family.matches(&candidate) {
                        candidates.push(shape);
                    }
                }
            }
            ShapeTreeChild::GroupShape(group) => pending.extend(group.children.iter().rev()),
            ShapeTreeChild::AlternateContent(alternate) => {
                if let Some(fallback) = alternate.selected_fallback() {
                    pending.extend(fallback.iter().rev());
                }
            }
            _ => {}
        }
    }
    let master_shape = candidates
        .iter()
        .copied()
        .find(|shape| {
            layout_key.idx.is_some()
                && shape
                    .placeholder
                    .as_ref()
                    .is_some_and(|ph| ph.idx == layout_key.idx)
        })
        .or_else(|| {
            candidates.iter().copied().find(|shape| {
                shape
                    .placeholder
                    .as_ref()
                    .is_some_and(|ph| ph.effective_type() == layout_key.ph_type)
            })
        })
        .or_else(|| {
            matches!(
                layout_key.ph_type,
                PhType::Body | PhType::Subtitle | PhType::Object
            )
            .then(|| {
                candidates.iter().copied().find(|shape| {
                    shape
                        .placeholder
                        .as_ref()
                        .is_some_and(|ph| ph.effective_type() == PhType::Body)
                })
            })
            .flatten()
        })
        .or_else(|| candidates.first().copied())
        .or_else(|| find_placeholder(children, &layout_key));
    (Some(layout_shape), master_shape)
}

fn find_placeholder<'a>(
    children: &'a [ShapeTreeChild],
    key: &PlaceholderKey,
) -> Option<&'a CT_Shape> {
    for child in children {
        let found = match child {
            ShapeTreeChild::Shape(shape) => shape
                .placeholder
                .as_ref()
                .is_some_and(|placeholder| placeholder_keys_match(key, &placeholder.key()))
                .then_some(shape),
            ShapeTreeChild::GroupShape(group) => find_placeholder(&group.children, key),
            ShapeTreeChild::AlternateContent(alternate) => alternate
                .selected_fallback()
                .and_then(|fallback| find_placeholder(fallback, key)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

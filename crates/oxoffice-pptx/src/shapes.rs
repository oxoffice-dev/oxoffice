//! Shape handles: borrowed and mutable shapes, shape lists and walks, shapes at an owner, adding shapes to slides and groups, removing shapes, and the shape-tree helpers.

use super::*;

impl Presentation {
    /// Returns the shapes of one slide, or of one group on it, for adding
    /// shapes.
    ///
    /// `group` holds one z-order index per nesting level, from the slide's
    /// own shapes down to the group, and is empty for the slide's own shapes.
    /// A missing slide or a path that does not end at a group returns `None`.
    pub fn shapes_mut(&mut self, slide_index: usize, group: &[usize]) -> Option<ShapesMut<'_>> {
        self.shapes_mut_at(ShapeOwner::Slide(slide_index), group)
            .ok()
    }

    /// Returns the actual shape collection of a slide, layout or master.
    /// Missing owners and paths that do not end at a group return a checked error.
    pub fn shapes_mut_at(&mut self, owner: ShapeOwner, group: &[usize]) -> Result<ShapesMut<'_>> {
        let tree = &mut self.owner_data_mut(owner)?.shape_tree;
        if !group.is_empty() {
            group_at_mut(&mut tree.children, group).ok_or_else(|| {
                invalid_shape_mutation("shape collection", "path is not a physical group")
            })?;
        }
        Ok(ShapesMut {
            presentation: self,
            owner,
            group: group.to_vec(),
        })
    }
}

impl Presentation {
    /// Iterates the top-level shapes of a slide, layout or master in z-order.
    pub fn part_shapes(&self, owner: ShapeOwner) -> Result<Shapes<'_>> {
        Ok(shapes(&self.owner_data(owner)?.shape_tree.children))
    }

    /// Reads one physical child path in a slide, layout or master.
    pub fn part_shape_at(&self, owner: ShapeOwner, path: &[usize]) -> Result<ShapeRef<'_>> {
        let missing = || invalid_shape_mutation("shape owner", "missing physical path");
        let (first, rest) = path.split_first().ok_or_else(missing)?;
        let mut shape = self
            .owner_data(owner)?
            .shape_tree
            .children
            .get(*first)
            .map(shape_ref)
            .ok_or_else(missing)?;
        for index in rest {
            shape = shape.child(*index).ok_or_else(missing)?;
        }
        Ok(shape)
    }

    /// Edits the actual part-owned child, including group descendants.
    pub fn shape_mut_at(&mut self, owner: ShapeOwner, path: &[usize]) -> Result<ShapeMut<'_>> {
        child_at(&self.owner_data(owner)?.shape_tree.children, path)
            .ok_or_else(|| invalid_shape_mutation("shape owner", "missing physical path"))?;
        Ok(owned_shape_mut(self, owner, path.to_vec()))
    }
}

impl Presentation {
    /// Removes one immediate slide child and the package parts only it used.
    ///
    /// Connectors attached to a removed shape are detached, as PowerPoint does
    /// on delete. The media timing a removed media picture owns goes with it.
    /// A shape that other slide animations still target is rejected without
    /// any change.
    pub fn remove_shape(&mut self, slide_index: usize, shape_index: usize) -> Result<()> {
        self.remove_shape_at(ShapeOwner::Slide(slide_index), shape_index)
    }

    /// Removes a direct physical-owner shape with checked relationship cleanup.
    pub fn remove_shape_at(&mut self, owner: ShapeOwner, shape_index: usize) -> Result<()> {
        const OPERATION: &str = "remove shape";
        self.owner_data(owner)?;
        let mut staged = self.clone();
        let slide_part = staged.owner_part_name(owner)?.to_owned();
        let children = &staged.owner_data(owner)?.shape_tree.children;
        let child = children
            .get(shape_index)
            .ok_or_else(|| Error::InvalidSlideMutation {
                operation: OPERATION,
                message: format!(
                    "shape index {shape_index} is out of range for {} shapes",
                    children.len()
                ),
            })?;
        let shape_id = child
            .non_visual_id()
            .ok_or_else(|| Error::UnsupportedShapeMutation {
                operation: OPERATION,
                shape_kind: shape_kind(child),
            })?;
        let mut removed_counts = HashMap::new();
        let mut media_ids = Vec::new();
        collect_subtree_ids(
            std::slice::from_ref(child),
            &mut removed_counts,
            &mut media_ids,
        );
        let mut owner_counts = HashMap::new();
        collect_subtree_ids(children, &mut owner_counts, &mut Vec::new());
        if let Some(ambiguous) = removed_counts.iter().find_map(|(id, count)| {
            (*count != 1 || owner_counts.get(id) != Some(&1)).then_some(*id)
        }) {
            return Err(invalid_shape_mutation(
                OPERATION,
                format!("shape id {ambiguous} is not unique in the owning part"),
            ));
        }
        let removed_ids: HashSet<u32> = removed_counts.into_keys().collect();
        let before = staged.owner_relationship_ids(owner)?;
        let timing = match owner {
            ShapeOwner::Slide(index) => &mut Arc::make_mut(&mut staged.slides[index]).slide.timing,
            ShapeOwner::Layout(index) => {
                &mut Arc::make_mut(&mut staged.layouts[index]).layout.timing
            }
            ShapeOwner::Master(index) => {
                &mut Arc::make_mut(&mut staged.masters[index])
                    .master
                    .as_mut()
                    .expect("owner was checked")
                    .timing
            }
        };
        if let Some(timing) = timing {
            for media_id in media_ids {
                timing
                    .remove_media(media_id)
                    .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
            }
            if let Some(animated) = removed_ids
                .iter()
                .copied()
                .filter(|id| timing.references_shape(*id))
                .min()
            {
                return Err(invalid_shape_mutation(
                    OPERATION,
                    format!("slide animations still target shape id {animated}"),
                ));
            }
        }
        detach_connectors(
            &mut staged.owner_data_mut(owner)?.shape_tree.children,
            &removed_ids,
        );
        staged
            .owner_data_mut(owner)?
            .shape_tree
            .remove_child_by_id(shape_id)
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
        let after = staged.owner_relationship_ids(owner)?;
        let mut candidates = HashSet::new();
        if let Some(relationships) = staged.package.get_part_rels_mut(&slide_part) {
            relationships.items.retain(|relationship| {
                if !before.contains(&relationship.id) || after.contains(&relationship.id) {
                    return true;
                }
                if !relationship_is_external(relationship) {
                    candidates.insert(OpcPackage::resolve_rel_target(
                        &slide_part,
                        &relationship.target,
                    ));
                }
                false
            });
        }
        prune_unreachable_parts(&mut staged.package, &candidates);
        staged.media_store = staged.media_store.rescan(&staged.package);
        if matches!(owner, ShapeOwner::Slide(_)) {
            self.commit_candidate(staged)
        } else {
            staged.commit_owned_parts(None)?;
            *self = staged;
            Ok(())
        }
    }
}

pub(super) fn shape_id_count(children: &[ShapeTreeChild], shape_id: u32) -> usize {
    children
        .iter()
        .map(|child| {
            let nested = match child {
                ShapeTreeChild::GroupShape(group) => shape_id_count(&group.children, shape_id),
                _ => 0,
            };
            usize::from(child.non_visual_id() == Some(shape_id)) + nested
        })
        .sum()
}

fn collect_subtree_ids(
    children: &[ShapeTreeChild],
    ids: &mut HashMap<u32, usize>,
    media_ids: &mut Vec<u32>,
) {
    for child in children {
        if let Some(id) = child.non_visual_id() {
            *ids.entry(id).or_default() += 1;
        }
        match child {
            ShapeTreeChild::Picture(picture) if picture.media.is_some() => {
                media_ids.extend(child.non_visual_id());
            }
            ShapeTreeChild::GroupShape(group) => {
                collect_subtree_ids(&group.children, ids, media_ids);
            }
            ShapeTreeChild::AlternateContent(alternate) => {
                collect_subtree_ids(
                    alternate.selected_fallback().unwrap_or_default(),
                    ids,
                    media_ids,
                );
            }
            _ => {}
        }
    }
}

pub(super) fn detach_connectors(children: &mut [ShapeTreeChild], removed_ids: &HashSet<u32>) {
    for child in children {
        match child {
            ShapeTreeChild::Connector(connector) => {
                if connector
                    .start_connection
                    .as_ref()
                    .is_some_and(|connection| removed_ids.contains(&connection.id))
                {
                    connector.start_connection = None;
                }
                if connector
                    .end_connection
                    .as_ref()
                    .is_some_and(|connection| removed_ids.contains(&connection.id))
                {
                    connector.end_connection = None;
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                detach_connectors(&mut group.children, removed_ids)
            }
            _ => {}
        }
    }
}

impl ShapesMut<'_> {
    /// Appends a no-fill textbox.
    pub fn add_textbox(
        &mut self,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append(|id| textbox_member(id, left, top, width, height))
    }

    /// Appends an ordinary preset shape.
    pub fn add_shape(
        &mut self,
        preset: &str,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append(|id| preset_member(id, preset, left, top, width, height))
    }

    /// Appends a free-standing connector.
    pub fn add_connector(
        &mut self,
        connector: ConnectorType,
        begin_x: Emu,
        begin_y: Emu,
        end_x: Emu,
        end_y: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append(|id| connector_member(id, connector, begin_x, begin_y, end_x, end_y))
    }

    /// Appends an empty group, which the first addition to it fits.
    ///
    /// A group added inside a group gets the zero `a:xfrm` python-pptx
    /// writes, because python-pptx cannot refit a group whose member group
    /// has none. One added to the slide's own shapes has no transform, as
    /// [`SlideMut::add_group_shape`] writes it.
    pub fn add_group_shape(&mut self) -> Result<ShapeMut<'_>> {
        let nested = !self.group.is_empty();
        self.append(|id| Ok(group_member(id, nested)))
    }

    /// Appends a rectangular table.
    pub fn add_table(
        &mut self,
        rows: usize,
        columns: usize,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append(|id| table_member(id, rows, columns, left, top, width, height))
    }

    /// Appends a relationship-backed picture, sized as
    /// [`Presentation::add_picture`] sizes it.
    pub fn add_picture(
        &mut self,
        image_data: &[u8],
        image_filename: &str,
        left: Emu,
        top: Emu,
        width: Option<Emu>,
        height: Option<Emu>,
    ) -> Result<ShapeMut<'_>> {
        let mut candidate = self.presentation.clone();
        let tree = &mut candidate.owner_data_mut(self.owner)?.shape_tree;
        let count = Members::at(tree, &self.group)
            .ok_or_else(|| invalid_shape_mutation("add picture", "missing group"))?
            .children()
            .len();
        candidate.append_picture(
            self.owner,
            &self.group,
            image_data,
            image_filename,
            left,
            top,
            width,
            height,
        )?;
        candidate.refresh_connector_attachments(
            self.owner,
            &self
                .presentation
                .owner_data(self.owner)?
                .shape_tree
                .children,
            &HashSet::new(),
        )?;
        candidate.validate_owned_part(self.owner)?;
        let mut path = self.group.clone();
        path.push(count);
        *self.presentation = candidate;
        Ok(owned_shape_mut(self.presentation, self.owner, path))
    }

    fn append(
        &mut self,
        build: impl FnOnce(u32) -> Result<ShapeTreeChild>,
    ) -> Result<ShapeMut<'_>> {
        if self
            .presentation
            .local_append_eligible(self.owner, &self.group)?
        {
            let index = self
                .presentation
                .append_locally(self.owner, &self.group, build)?;
            let mut path = self.group.clone();
            path.push(index);
            return Ok(owned_shape_mut(self.presentation, self.owner, path));
        }
        let mut candidate = self.presentation.clone();
        let tree = &mut candidate.owner_data_mut(self.owner)?.shape_tree;
        let index = append_new_member(tree, &self.group, build)?;
        candidate.refresh_connector_attachments(
            self.owner,
            &self
                .presentation
                .owner_data(self.owner)?
                .shape_tree
                .children,
            &HashSet::new(),
        )?;
        candidate.validate_owned_part(self.owner)?;
        let mut path = self.group.clone();
        path.push(index);
        *self.presentation = candidate;
        Ok(owned_shape_mut(self.presentation, self.owner, path))
    }
}

pub(super) fn textbox_member(
    id: u32,
    left: Emu,
    top: Emu,
    width: Emu,
    height: Emu,
) -> Result<ShapeTreeChild> {
    CT_Shape::new_textbox(
        id,
        &format!("TextBox {id}"),
        sized_transform("add textbox", left, top, width, height)?,
    )
    .map(ShapeTreeChild::Shape)
    .map_err(|error| invalid_shape_construction("add textbox", error))
}

pub(super) fn preset_member(
    id: u32,
    preset: &str,
    left: Emu,
    top: Emu,
    width: Emu,
    height: Emu,
) -> Result<ShapeTreeChild> {
    let mut shape = CT_Shape::new_preset(
        id,
        &format!("Shape {id}"),
        preset,
        sized_transform("add shape", left, top, width, height)?,
    )
    .map_err(|error| invalid_shape_construction("add shape", error))?;
    shape
        .set_default_style()
        .map_err(|error| invalid_shape_construction("add shape", error))?;
    Ok(ShapeTreeChild::Shape(shape))
}

pub(super) fn connector_member(
    id: u32,
    connector: ConnectorType,
    begin_x: Emu,
    begin_y: Emu,
    end_x: Emu,
    end_y: Emu,
) -> Result<ShapeTreeChild> {
    let transform = connector_transform(begin_x, begin_y, end_x, end_y)?;
    let mut connector = CT_ConnectionShape::new_free_standing(
        id,
        &format!("Connector {id}"),
        connector.preset_name(),
        transform,
    )
    .map_err(|error| invalid_shape_construction("add connector", error))?;
    connector
        .set_default_style()
        .map_err(|error| invalid_shape_construction("add connector", error))?;
    Ok(ShapeTreeChild::Connector(connector))
}

/// Builds an empty group, with a zero `a:xfrm` when `nested` in a group.
pub(super) fn group_member(id: u32, nested: bool) -> ShapeTreeChild {
    let mut group = CT_GroupShape::new_empty(id, &format!("Group {id}"));
    if nested {
        let transform = group.group_transform_mut();
        transform.offset = Some(CT_Point2D::default());
        transform.extent = Some(CT_PositiveSize2D::default());
        transform.child_offset = Some(CT_Point2D::default());
        transform.child_extent = Some(CT_PositiveSize2D::default());
    }
    ShapeTreeChild::GroupShape(Box::new(group))
}

pub(super) fn table_member(
    id: u32,
    rows: usize,
    columns: usize,
    left: Emu,
    top: Emu,
    width: Emu,
    height: Emu,
) -> Result<ShapeTreeChild> {
    let table = CT_Table::new(rows, columns, width, height)
        .map_err(|error| invalid_table_mutation("add table", error.to_string()))?;
    let frame = CT_GraphicFrame::new_table(
        id,
        &format!("Table {id}"),
        positioned_transform(left, top, width, height),
        table,
    )
    .map_err(|error| invalid_table_mutation("add table", error.to_string()))?;
    Ok(ShapeTreeChild::GraphicFrame(Box::new(frame)))
}

impl Presentation {
    /// An empty group with no connector can take one new child without
    /// reading any other slide. A group refit or an existing connector keeps
    /// the package transaction.
    pub(super) fn local_append_eligible(&self, owner: ShapeOwner, group: &[usize]) -> Result<bool> {
        if !group.is_empty() {
            return Ok(false);
        }
        Ok(!tree_has_connector(
            &self.owner_data(owner)?.shape_tree.children,
        ))
    }

    pub(super) fn append_locally(
        &mut self,
        owner: ShapeOwner,
        group: &[usize],
        build: impl FnOnce(u32) -> Result<ShapeTreeChild>,
    ) -> Result<usize> {
        let index = {
            let tree = &mut self.owner_data_mut(owner)?.shape_tree;
            append_new_member(tree, group, build)?
        };
        let part_name = self.owner_part_name(owner)?.to_owned();
        let validated = child_at(&self.owner_data(owner)?.shape_tree.children, &[index])
            .ok_or_else(|| invalid_shape_mutation("append shape", "new shape is absent"))
            .and_then(|child| validate_shape_child(child, &part_name));
        if let Err(error) = validated {
            self.owner_data_mut(owner)?.shape_tree.children.pop();
            return Err(error);
        }
        Ok(index)
    }
}

/// Appends a new member built with an id that is unused across the slide.
pub(super) fn append_new_member(
    tree: &mut CT_ShapeTree,
    group: &[usize],
    build: impl FnOnce(u32) -> Result<ShapeTreeChild>,
) -> Result<usize> {
    oxoffice_pptx_oxml::shape_tree::validate_shape_nesting(&tree.children)
        .map_err(|error| invalid_shape_mutation("append shape", error.to_string()))?;
    let id = ShapeIdAllocator::scan(tree).allocate();
    let member = build(id)?;
    let depth = group.len()
        + oxoffice_pptx_oxml::shape_tree::validate_shape_nesting(std::slice::from_ref(&member))
            .map_err(|error| invalid_shape_mutation("append shape", error.to_string()))?;
    if depth > oxoffice_pptx_oxml::MAX_PRESENTATION_NESTING {
        return Err(invalid_shape_mutation(
            "append shape",
            format!(
                "group and alternate content nesting exceeds limit {}, offending depth {depth}",
                oxoffice_pptx_oxml::MAX_PRESENTATION_NESTING
            ),
        ));
    }
    let count = if group.is_empty() {
        tree.children.len()
    } else {
        group_at_mut(&mut tree.children, group)
            .expect("validated group path")
            .children
            .len()
    };
    append_member(tree, group, member);
    Ok(count)
}

/// Appends `member` to the slide's own shapes, for an empty `group`, or to
/// the group that `group` indexes, then refits that group and each group
/// enclosing it, innermost first.
///
/// Callers pass a path that ends at a group.
pub(super) fn append_member<'t>(
    tree: &'t mut CT_ShapeTree,
    group: &[usize],
    member: ShapeTreeChild,
) -> &'t mut ShapeTreeChild {
    const CHECKED: &str = "the caller checked the group path";
    if group.is_empty() {
        return tree.append_child(member);
    }
    // Only a group that already has a counted member has members whose drawn
    // place its refit must keep.
    let occupied = (1..=group.len())
        .map(|depth| {
            group_at_mut(&mut tree.children, &group[..depth])
                .expect(CHECKED)
                .children
                .iter()
                .any(|child| member_box(child).is_some())
        })
        .collect::<Vec<_>>();
    group_at_mut(&mut tree.children, group)
        .expect(CHECKED)
        .append_child(member);
    for depth in (1..=group.len()).rev() {
        fit_group_to_members(
            group_at_mut(&mut tree.children, &group[..depth]).expect(CHECKED),
            occupied[depth - 1],
        );
    }
    group_at_mut(&mut tree.children, group)
        .expect(CHECKED)
        .children
        .last_mut()
        .expect("the member was appended")
}

/// Returns the group that `path` indexes from `children`, one z-order index
/// per nesting level.
pub(super) fn group_at_mut<'t>(
    children: &'t mut [ShapeTreeChild],
    path: &[usize],
) -> Option<&'t mut CT_GroupShape> {
    let (index, rest) = path.split_first()?;
    let ShapeTreeChild::GroupShape(group) = children.get_mut(*index)? else {
        return None;
    };
    if rest.is_empty() {
        Some(group.as_mut())
    } else {
        group_at_mut(&mut group.children, rest)
    }
}

/// Returns the offset and extent a member adds to its group's union, or
/// `None` for a member without both and for a group without members.
pub(super) fn member_box(child: &ShapeTreeChild) -> Option<(CT_Point2D, CT_PositiveSize2D)> {
    if let ShapeTreeChild::GroupShape(group) = child
        && group.children.is_empty()
    {
        return None;
    }
    let transform = shape_transform(child)?;
    Some((transform.offset?, transform.extent?))
}

/// Refits a group to the union of its members' own boxes, as python-pptx
/// does after each addition.
///
/// `a:chOff` and `a:chExt` become the union in member space, and `a:off` and
/// `a:ext` follow through the group's current mapping from member space to
/// its parent, the one the renderer applies. The group's flips and rotation
/// apply about the centre of `a:off` and `a:ext`, so when the group is
/// `occupied`, meaning it had a counted member before the addition, `a:off`
/// also moves by [`pivot_shift`]. No member drawn before the addition moves
/// on the slide. A group without all four values draws its members unscaled
/// in place, so its four values end equal, as python-pptx writes them. As
/// in python-pptx, member rotation does not widen the union. A member
/// without an offset and an extent does not count, nor does a group without
/// members, and a group without a counted member keeps its transform.
pub(super) fn fit_group_to_members(group: &mut CT_GroupShape, occupied: bool) {
    let mut union: Option<(i64, i64, i64, i64)> = None;
    for (offset, extent) in group.children.iter().filter_map(member_box) {
        let (left, top) = (offset.x.0, offset.y.0);
        let right = left.saturating_add(extent.cx.0);
        let bottom = top.saturating_add(extent.cy.0);
        union = Some(union.map_or((left, top, right, bottom), |(l, t, r, b)| {
            (l.min(left), t.min(top), r.max(right), b.max(bottom))
        }));
    }
    let Some((left, top, right, bottom)) = union else {
        return;
    };
    let width = right.saturating_sub(left).max(0);
    let height = bottom.saturating_sub(top).max(0);
    let transform = group.group_transform_mut();
    let mapping = transform
        .offset
        .zip(transform.extent)
        .zip(transform.child_offset.zip(transform.child_extent));
    let (mut x, cx) = member_span_to_parent(
        left,
        width,
        mapping.map(|((offset, extent), (child_offset, child_extent))| {
            (offset.x.0, extent.cx.0, child_offset.x.0, child_extent.cx.0)
        }),
    );
    let (mut y, cy) = member_span_to_parent(
        top,
        height,
        mapping.map(|((offset, extent), (child_offset, child_extent))| {
            (offset.y.0, extent.cy.0, child_offset.y.0, child_extent.cy.0)
        }),
    );
    if occupied && let Some(((offset, extent), _)) = mapping {
        let centre = |start: i64, length: i64| start as f64 + length as f64 / 2.0;
        let (shift_x, shift_y) = pivot_shift(
            transform,
            centre(x, cx) - centre(offset.x.0, extent.cx.0),
            centre(y, cy) - centre(offset.y.0, extent.cy.0),
        );
        x = x.saturating_add(shift_x);
        y = y.saturating_add(shift_y);
    }
    transform.offset = Some(CT_Point2D {
        x: Emu(x),
        y: Emu(y),
    });
    transform.extent = Some(CT_PositiveSize2D {
        cx: Emu(cx),
        cy: Emu(cy),
    });
    transform.child_offset = Some(CT_Point2D {
        x: Emu(left),
        y: Emu(top),
    });
    transform.child_extent = Some(CT_PositiveSize2D {
        cx: Emu(width),
        cy: Emu(height),
    });
}

/// Returns how far a group's `a:off` must move so that its flips and
/// rotation, which apply about the centre of `a:off` and `a:ext`, place its
/// members as before after that centre moved by `(dx, dy)`.
///
/// PowerPoint flips, then rotates, so for that linear part `L` the shift is
/// `(L - I)(dx, dy)`, which is zero without rotation and flips. The oxoffice
/// renderer applies the same flip-then-rotation order for transformed groups.
fn pivot_shift(transform: &CT_Transform2D, dx: f64, dy: f64) -> (i64, i64) {
    let (sin, cos) = (f64::from(transform.rotation.0) / 60_000.0)
        .to_radians()
        .sin_cos();
    let flip_x = if transform.flip_horizontal { -1.0 } else { 1.0 };
    let flip_y = if transform.flip_vertical { -1.0 } else { 1.0 };
    (
        ((cos * flip_x - 1.0) * dx - sin * flip_y * dy).round() as i64,
        (sin * flip_x * dx + (cos * flip_y - 1.0) * dy).round() as i64,
    )
}

/// Maps one axis of a member-space span onto a group's parent, given the
/// group's offset, extent, child offset, and child extent on that axis, as
/// `offset + (start - child_offset) * extent / child_extent`. A zero child
/// extent scales by one, as in the renderer, and `None` maps unchanged.
fn member_span_to_parent(
    start: i64,
    length: i64,
    mapping: Option<(i64, i64, i64, i64)>,
) -> (i64, i64) {
    let Some((offset, extent, child_offset, child_extent)) = mapping else {
        return (start, length);
    };
    let scale = |value: i64| {
        if child_extent == 0 || extent == child_extent {
            value
        } else {
            (value as f64 * extent as f64 / child_extent as f64).round() as i64
        }
    };
    (
        offset.saturating_add(scale(start.saturating_sub(child_offset))),
        scale(length),
    )
}

/// The geometry family of a free-standing connector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectorType {
    Straight,
    Elbow,
    Curve,
}

impl ConnectorType {
    const fn preset_name(self) -> &'static str {
        match self {
            Self::Straight => "line",
            Self::Elbow => "bentConnector3",
            Self::Curve => "curvedConnector3",
        }
    }
}

pub(super) fn positioned_transform(left: Emu, top: Emu, width: Emu, height: Emu) -> CT_Transform2D {
    let mut transform = CT_Transform2D::default();
    transform.offset = Some(CT_Point2D { x: left, y: top });
    transform.extent = Some(CT_PositiveSize2D {
        cx: width,
        cy: height,
    });
    transform
}

fn connector_transform(
    begin_x: Emu,
    begin_y: Emu,
    end_x: Emu,
    end_y: Emu,
) -> Result<CT_Transform2D> {
    let width =
        i64::try_from(begin_x.0.abs_diff(end_x.0)).map_err(|_| Error::InvalidShapeMutation {
            operation: "add connector",
            message: "horizontal endpoint span exceeds the EMU range".to_owned(),
        })?;
    let height =
        i64::try_from(begin_y.0.abs_diff(end_y.0)).map_err(|_| Error::InvalidShapeMutation {
            operation: "add connector",
            message: "vertical endpoint span exceeds the EMU range".to_owned(),
        })?;
    let mut transform = CT_Transform2D::default();
    transform.offset = Some(CT_Point2D {
        x: Emu(begin_x.0.min(end_x.0)),
        y: Emu(begin_y.0.min(end_y.0)),
    });
    transform.extent = Some(CT_PositiveSize2D {
        cx: Emu(width),
        cy: Emu(height),
    });
    transform.flip_horizontal = begin_x.0 > end_x.0;
    transform.flip_vertical = begin_y.0 > end_y.0;
    Ok(transform)
}

pub(super) fn invalid_shape_construction(operation: &'static str, error: OxmlError) -> Error {
    Error::InvalidShapeMutation {
        operation,
        message: error.to_string(),
    }
}

/// The structural kind of one PresentationML shape-tree child.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShapeKind {
    Shape,
    Picture,
    GraphicFrame,
    Group,
    Connector,
    AlternateContent,
}

/// One DrawingML text body autofit choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutofitMode {
    None,
    Normal,
    Shape,
}

/// The python-pptx `MSO_SHAPE_TYPE` classification of one shape-tree child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeType {
    AutoShape,
    Chart,
    EmbeddedOleObject,
    Freeform,
    Group,
    Line,
    LinkedOleObject,
    Media,
    Picture,
    Placeholder,
    Table,
    TextBox,
}

impl PartialEq for ShapeRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.child, other.child)
    }
}

impl Eq for ShapeRef<'_> {}

pub(super) fn shape_ref(child: &ShapeTreeChild) -> ShapeRef<'_> {
    ShapeRef { child }
}

/// The shapes of one shape tree or group in z-order, as [`ShapeRef`]s.
///
/// [`walk`](Shapes::walk) also visits the shapes inside groups.
#[derive(Clone)]
pub struct Shapes<'a> {
    children: &'a [ShapeTreeChild],
    indices: std::ops::Range<usize>,
}

pub(super) fn shapes(children: &[ShapeTreeChild]) -> Shapes<'_> {
    Shapes {
        children,
        indices: 0..children.len(),
    }
}

impl<'a> Iterator for Shapes<'a> {
    type Item = ShapeRef<'a>;

    fn next(&mut self) -> Option<ShapeRef<'a>> {
        let index = self.indices.next()?;
        Some(shape_ref(&self.children[index]))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}

impl DoubleEndedIterator for Shapes<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let index = self.indices.next_back()?;
        Some(shape_ref(&self.children[index]))
    }
}

impl ExactSizeIterator for Shapes<'_> {}

impl<'a> Shapes<'a> {
    /// Visits the shapes left in this iterator and, below each group, the
    /// shapes inside it, depth first in z-order: a group comes before its
    /// children.
    ///
    /// Each shape comes with its path: its index in this collection, then in
    /// each group below it, so its depth is `path.len() - 1`. For a walk of a
    /// slide's, layout's or master's own shapes ([`SlideRef::shapes`],
    /// [`Presentation::part_shapes`]), [`Presentation::part_shape_at`] and
    /// [`Presentation::shape_mut_at`] find each path again; a walk of
    /// [`ShapeRef::children`] gives paths relative to that group.
    ///
    /// An alternate-content block is visited but not entered: the members of
    /// its fallback cannot be edited, and [`ShapeRef::children`] reads them.
    /// The block itself is read-only too, as when iterating: an edit returns
    /// an error. The walk keeps its own stack, so deeply nested groups cannot
    /// overflow the thread's stack.
    pub fn walk(self) -> ShapeWalk<'a> {
        ShapeWalk {
            levels: vec![(self.children, self.indices)],
            path: Vec::new(),
        }
    }
}

/// The depth-first walk of [`Shapes::walk`].
#[derive(Clone)]
pub struct ShapeWalk<'a> {
    // The open shape trees, outermost first, each with the indices left.
    levels: Vec<(&'a [ShapeTreeChild], std::ops::Range<usize>)>,
    // The index of each open group in the level above it.
    path: Vec<usize>,
}

impl<'a> Iterator for ShapeWalk<'a> {
    type Item = (Vec<usize>, ShapeRef<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (children, indices) = self.levels.last_mut()?;
            let children = *children;
            let Some(index) = indices.next() else {
                self.levels.pop();
                self.path.pop();
                continue;
            };
            let child = &children[index];
            let mut path = self.path.clone();
            path.push(index);
            if let ShapeTreeChild::GroupShape(group) = child
                && !group.children.is_empty()
            {
                self.levels.push((&group.children, 0..group.children.len()));
                self.path.push(index);
            }
            return Some((path, shape_ref(child)));
        }
    }
}
pub(super) enum ShapeMutOwner<'a> {
    Presentation {
        presentation: &'a mut Presentation,
        owner: ShapeOwner,
        path: Vec<usize>,
    },
    Child(&'a mut ShapeTreeChild),
}
pub(super) fn shape_mut(child: &mut ShapeTreeChild) -> ShapeMut<'_> {
    ShapeMut {
        owner: ShapeMutOwner::Child(child),
    }
}
pub(super) fn owned_shape_mut(
    presentation: &mut Presentation,
    owner: ShapeOwner,
    path: Vec<usize>,
) -> ShapeMut<'_> {
    ShapeMut {
        owner: ShapeMutOwner::Presentation {
            presentation,
            owner,
            path,
        },
    }
}
impl<'a> ShapeMut<'a> {
    /// Returns the child's normalized structural kind.
    pub fn kind(&self) -> ShapeKind {
        shape_kind(self.leaf())
    }

    /// Returns one immediate group child for in-place mutation.
    ///
    /// `mc:AlternateContent` fallback children remain a read-only projection.
    pub fn child_mut(&mut self, index: usize) -> Option<ShapeMut<'_>> {
        match &mut self.owner {
            ShapeMutOwner::Presentation {
                presentation,
                owner,
                path,
            } => {
                let ShapeTreeChild::GroupShape(group) = child_at(
                    &presentation
                        .owner_data(*owner)
                        .expect("validated owner")
                        .shape_tree
                        .children,
                    path,
                )?
                else {
                    return None;
                };
                group.children.get(index)?;
                let mut child_path = path.clone();
                child_path.push(index);
                Some(owned_shape_mut(presentation, *owner, child_path))
            }
            ShapeMutOwner::Child(child) => match child {
                ShapeTreeChild::GroupShape(group) => group.children.get_mut(index).map(shape_mut),
                _ => None,
            },
        }
    }

    /// Consumes this handle and returns one immediate group child for mutation.
    pub fn into_child_mut(self, index: usize) -> Option<ShapeMut<'a>> {
        match self.owner {
            ShapeMutOwner::Presentation {
                presentation,
                owner,
                mut path,
            } => {
                let ShapeTreeChild::GroupShape(group) = child_at(
                    &presentation
                        .owner_data(owner)
                        .expect("validated owner")
                        .shape_tree
                        .children,
                    &path,
                )?
                else {
                    return None;
                };
                group.children.get(index)?;
                path.push(index);
                Some(owned_shape_mut(presentation, owner, path))
            }
            ShapeMutOwner::Child(child) => match child {
                ShapeTreeChild::GroupShape(group) => group.children.get_mut(index).map(shape_mut),
                _ => None,
            },
        }
    }

    /// Changes the shape offset in EMU.
    pub(super) fn set_position_leaf(&mut self, left: Emu, top: Emu) -> Result<()> {
        self.transform_mut("set position")?.offset = Some(CT_Point2D { x: left, y: top });
        Ok(())
    }

    /// Changes the shape extent in EMU.
    pub(super) fn set_size_leaf(&mut self, width: Emu, height: Emu) -> Result<()> {
        self.transform_mut("set size")?.extent = Some(CT_PositiveSize2D {
            cx: width,
            cy: height,
        });
        Ok(())
    }

    /// Changes the clockwise DrawingML rotation.
    pub(super) fn set_rotation_leaf(&mut self, rotation: Angle) -> Result<()> {
        self.transform_mut("set rotation")?.rotation = rotation;
        Ok(())
    }

    /// Changes the direct alt text on this physical owner.
    pub fn set_alt_text(&mut self, text: Option<&str>) -> Result<()> {
        let result = match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => shape.set_alt_text(text),
            ShapeTreeChild::Picture(picture) => picture.set_alt_text(text),
            ShapeTreeChild::GraphicFrame(frame) => frame.set_alt_text(text),
            ShapeTreeChild::GroupShape(group) => group.set_alt_text(text),
            ShapeTreeChild::Connector(connector) => connector.set_alt_text(text),
            ShapeTreeChild::AlternateContent(_) => return Err(self.unsupported("set alt text")),
        };
        result.map_err(|error| invalid_shape_mutation("set alt text", error.to_string()))
    }

    /// Changes the direct title on this physical owner.
    pub fn set_title(&mut self, text: Option<&str>) -> Result<()> {
        let result = match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => shape.set_title(text),
            ShapeTreeChild::Picture(picture) => picture.set_title(text),
            ShapeTreeChild::GraphicFrame(frame) => frame.set_title(text),
            ShapeTreeChild::GroupShape(group) => group.set_title(text),
            ShapeTreeChild::Connector(connector) => connector.set_title(text),
            ShapeTreeChild::AlternateContent(_) => return Err(self.unsupported("set title")),
        };
        result.map_err(|error| invalid_shape_mutation("set title", error.to_string()))
    }

    /// Changes the direct decorative on this physical owner.
    pub fn set_decorative(&mut self, decorative: bool) -> Result<()> {
        let result = match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => shape.set_decorative(decorative),
            ShapeTreeChild::Picture(picture) => picture.set_decorative(decorative),
            ShapeTreeChild::GraphicFrame(frame) => frame.set_decorative(decorative),
            ShapeTreeChild::GroupShape(group) => group.set_decorative(decorative),
            ShapeTreeChild::Connector(connector) => connector.set_decorative(decorative),
            ShapeTreeChild::AlternateContent(_) => return Err(self.unsupported("set decorative")),
        };
        result.map_err(|error| invalid_shape_mutation("set decorative", error.to_string()))
    }

    /// Changes the producer-facing non-visual name.
    pub fn set_name(&mut self, name: &str) -> Result<()> {
        let result = match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => shape.set_name(name),
            ShapeTreeChild::Picture(picture) => picture.set_name(name),
            ShapeTreeChild::GraphicFrame(frame) => frame.set_name(name),
            ShapeTreeChild::GroupShape(group) => group.set_name(name),
            ShapeTreeChild::Connector(connector) => connector.set_name(name),
            ShapeTreeChild::AlternateContent(_) => {
                return Err(self.unsupported("set name"));
            }
        };
        result.map_err(|error| Error::InvalidShapeMutation {
            operation: "set name",
            message: error.to_string(),
        })
    }

    /// Replaces the direct fill on a shape with typed shape properties.
    pub fn set_fill(&mut self, fill: Fill) -> Result<()> {
        self.shape_properties_mut("set fill")?.fill = Some(fill);
        Ok(())
    }

    /// Replaces the direct line on a shape with typed shape properties.
    pub fn set_line(&mut self, line: CT_LineProperties) -> Result<()> {
        self.shape_properties_mut("set line")?.line = Some(line);
        Ok(())
    }

    /// Crops a picture by left, top, right, and bottom insets of its image.
    ///
    /// Each inset is a share of the image, where `Percent1000(25_000)` is a
    /// quarter, and a negative inset extends the image. An edge keeps its
    /// stored attribute when its value is unchanged, a changed edge of zero
    /// drops its attribute, and four zero insets add no `a:srcRect` to a
    /// picture without one. Other shape kinds are rejected.
    pub fn set_crop(
        &mut self,
        left: Percent1000,
        top: Percent1000,
        right: Percent1000,
        bottom: Percent1000,
    ) -> Result<()> {
        const OPERATION: &str = "set crop";
        let shape_kind = shape_kind(self.leaf_mut());
        let ShapeTreeChild::Picture(picture) = self.leaf_mut() else {
            return Err(Error::UnsupportedShapeMutation {
                operation: OPERATION,
                shape_kind,
            });
        };
        let fill = picture
            .blip_fill
            .as_mut()
            .ok_or_else(|| invalid_shape_mutation(OPERATION, "picture has no p:blipFill"))?;
        let zero = Percent1000::default();
        if fill.source_rect.is_none() && [left, top, right, bottom] == [zero; 4] {
            return Ok(());
        }
        let edge = |stored: Option<Percent1000>, value: Percent1000| {
            if stored.unwrap_or_default() == value {
                stored
            } else {
                (value != zero).then_some(value)
            }
        };
        let rect = fill.source_rect.get_or_insert_with(RelativeRect::default);
        rect.left = edge(rect.left, left);
        rect.top = edge(rect.top, top);
        rect.right = edge(rect.right, right);
        rect.bottom = edge(rect.bottom, bottom);
        Ok(())
    }

    /// Replaces or removes the direct `a:effectLst` of a shape, picture,
    /// connector, or group.
    ///
    /// `None` removes the list so the shape inherits its theme effect again,
    /// and an empty list suppresses that effect. The list is written in its
    /// schema place and keeps the unmodelled effects it carries. A shape with
    /// an `a:effectDag` refuses a list, which the schema excludes beside it.
    pub fn set_effects(&mut self, effects: Option<CT_EffectList>) -> Result<()> {
        const OPERATION: &str = "set effects";
        if let Some(list) = &effects {
            list.to_xml()
                .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
        }
        if let ShapeTreeChild::GroupShape(group) = self.leaf_mut() {
            return group
                .set_effects(effects)
                .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()));
        }
        let properties = self.shape_properties_mut(OPERATION)?;
        if effects.is_some() && properties.has_unmodelled_effect() {
            return Err(invalid_shape_mutation(
                OPERATION,
                "the shape carries an a:effectDag, which excludes an a:effectLst",
            ));
        }
        properties.effects = effects;
        Ok(())
    }

    /// Changes the theme effect style that the shape's `p:style` references.
    ///
    /// Index 0 references no theme effect, so a connector from
    /// [`SlideMut::add_connector`] loses the theme's shadow in PowerPoint,
    /// LibreOffice, and the renderer alike. An empty direct `a:effectLst`
    /// does not remove it in LibreOffice. Only an ordinary shape or a
    /// connector with a typed style has this reference.
    pub fn set_theme_effect_index(&mut self, index: u32) -> Result<()> {
        let changed = match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => Ok(shape.set_effect_reference_index(index)),
            ShapeTreeChild::Connector(connector) => connector.set_effect_reference_index(index),
            _ => return Err(self.unsupported("set theme effect index")),
        };
        match changed {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::InvalidShapeMutation {
                operation: "set theme effect index",
                message: "the shape has no p:style with typed theme references".to_owned(),
            }),
            Err(error) => Err(Error::InvalidShapeMutation {
                operation: "set theme effect index",
                message: error.to_string(),
            }),
        }
    }

    /// Inserts or replaces one finite preset-geometry adjustment.
    pub(super) fn set_adjust_value_leaf(&mut self, name: &str, value: f64) -> Result<()> {
        if !value.is_finite() {
            return Err(Error::NonFiniteAdjustmentValue {
                name: name.to_owned(),
            });
        }
        let shape_kind = shape_kind(self.leaf_mut());
        let properties = self.shape_properties_mut("set adjustment")?;
        let geometry =
            properties
                .preset_geometry
                .as_mut()
                .ok_or(Error::UnsupportedShapeMutation {
                    operation: "set adjustment",
                    shape_kind,
                })?;
        geometry
            .set_adjust_value(name, value)
            .map_err(|error| Error::InvalidShapeMutation {
                operation: "set adjustment",
                message: error.to_string(),
            })
    }

    /// Replaces the geometry of an auto shape or a picture mask with a preset.
    ///
    /// `preset` is a DrawingML preset name such as `roundRect`. A custom
    /// geometry is replaced in the same schema slot, and the adjustments
    /// restart from the preset's defaults with an empty `a:avLst`. Text,
    /// fill, line, effects, identity and z-order are kept. A text box, a
    /// connector, a graphic frame, a group and alternate content are refused.
    pub(super) fn set_auto_shape_type_leaf(&mut self, preset: &str) -> Result<()> {
        const OPERATION: &str = "set auto shape type";
        let shape_kind = self.kind();
        let properties = match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) if shape.is_textbox() => {
                return Err(Error::InvalidShapeMutation {
                    operation: OPERATION,
                    message: "a text box is not an auto shape".to_owned(),
                });
            }
            ShapeTreeChild::Shape(shape) => &mut shape.shape_properties,
            ShapeTreeChild::Picture(picture) => &mut picture.shape_properties,
            _ => {
                return Err(Error::UnsupportedShapeMutation {
                    operation: OPERATION,
                    shape_kind,
                });
            }
        };
        let geometry =
            CT_PresetGeometry2D::new(preset).map_err(|error| Error::InvalidShapeMutation {
                operation: OPERATION,
                message: error.to_string(),
            })?;
        properties.custom_geometry = None;
        properties.preset_geometry = Some(geometry);
        Ok(())
    }

    /// Replaces ordinary shape text without changing placeholder identity,
    /// one paragraph per line as [`CT_TextBody::set_text`] describes.
    pub fn set_text(&mut self, text: &str) -> Result<()> {
        let shape_kind = shape_kind(self.leaf_mut());
        let ShapeTreeChild::Shape(shape) = self.leaf_mut() else {
            return Err(Error::UnsupportedShapeMutation {
                operation: "set text",
                shape_kind,
            });
        };
        shape
            .text_body
            .get_or_insert_with(CT_TextBody::new)
            .set_text(text);
        Ok(())
    }

    /// Returns the ordinary shape text body for in-place mutation.
    pub fn text_frame(&mut self) -> Option<TextFrame<'_>> {
        match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => shape.text_body.as_mut().map(|body| TextFrame { body }),
            _ => None,
        }
    }

    /// Consumes this handle and returns its ordinary-shape text body for mutation.
    pub fn into_text_frame(self) -> Option<TextFrame<'a>> {
        match self.into_leaf() {
            ShapeTreeChild::Shape(shape) => shape.text_body.as_mut().map(|body| TextFrame { body }),
            _ => None,
        }
    }

    /// Returns a behavior-bearing mutable handle for a table graphic frame.
    pub fn table_mut(&mut self) -> Option<TableMut<'_>> {
        if !matches!(self.leaf(),ShapeTreeChild::GraphicFrame(frame) if matches!(frame.graphic_data.payload(), GraphicDataPayload::Table(_)))
        {
            return None;
        }
        Some(TableMut {
            shape: self.reborrow(),
        })
    }

    /// Consumes this handle and returns its table for mutation.
    pub fn into_table_mut(self) -> Option<TableMut<'a>> {
        if !matches!(self.leaf(),ShapeTreeChild::GraphicFrame(frame) if matches!(frame.graphic_data.payload(), GraphicDataPayload::Table(_)))
        {
            return None;
        }
        Some(TableMut { shape: self })
    }

    pub(super) fn transform_mut(&mut self, operation: &'static str) -> Result<&mut CT_Transform2D> {
        let shape_kind = self.kind();
        match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => Ok(shape
                .shape_properties
                .transform
                .get_or_insert_with(CT_Transform2D::default)),
            ShapeTreeChild::Picture(picture) => Ok(picture
                .shape_properties
                .transform
                .get_or_insert_with(CT_Transform2D::default)),
            ShapeTreeChild::GraphicFrame(frame) => Ok(&mut frame.transform),
            ShapeTreeChild::GroupShape(group) => Ok(group.group_transform_mut()),
            ShapeTreeChild::Connector(connector) => Ok(connector
                .shape_properties
                .transform
                .get_or_insert_with(CT_Transform2D::default)),
            ShapeTreeChild::AlternateContent(_) => Err(Error::UnsupportedShapeMutation {
                operation,
                shape_kind,
            }),
        }
    }

    fn shape_properties_mut(&mut self, operation: &'static str) -> Result<&mut CT_ShapeProperties> {
        let shape_kind = self.kind();
        match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => Ok(&mut shape.shape_properties),
            ShapeTreeChild::Picture(picture) => Ok(&mut picture.shape_properties),
            ShapeTreeChild::Connector(connector) => Ok(&mut connector.shape_properties),
            _ => Err(Error::UnsupportedShapeMutation {
                operation,
                shape_kind,
            }),
        }
    }

    fn unsupported(&self, operation: &'static str) -> Error {
        Error::UnsupportedShapeMutation {
            operation,
            shape_kind: shape_kind(self.leaf()),
        }
    }
    pub(super) fn leaf(&self) -> &ShapeTreeChild {
        match &self.owner {
            ShapeMutOwner::Child(child) => child,
            ShapeMutOwner::Presentation {
                presentation,
                owner,
                path,
            } => child_at(
                &presentation
                    .owner_data(*owner)
                    .expect("validated owner")
                    .shape_tree
                    .children,
                path,
            )
            .expect("exclusively borrowed owner path remains present"),
        }
    }
    pub(super) fn leaf_mut(&mut self) -> &mut ShapeTreeChild {
        match &mut self.owner {
            ShapeMutOwner::Child(child) => child,
            ShapeMutOwner::Presentation {
                presentation,
                owner,
                path,
            } => child_at_mut(
                &mut presentation
                    .owner_data_mut(*owner)
                    .expect("validated owner")
                    .shape_tree
                    .children,
                path,
            )
            .expect("exclusively borrowed owner path remains present"),
        }
    }
    pub(super) fn into_leaf(self) -> &'a mut ShapeTreeChild {
        match self.owner {
            ShapeMutOwner::Child(child) => child,
            ShapeMutOwner::Presentation {
                presentation,
                owner,
                path,
            } => child_at_mut(
                &mut presentation
                    .owner_data_mut(owner)
                    .expect("validated owner")
                    .shape_tree
                    .children,
                &path,
            )
            .expect("exclusively borrowed owner path remains present"),
        }
    }
    fn reborrow(&mut self) -> ShapeMut<'_> {
        match &mut self.owner {
            ShapeMutOwner::Child(child) => shape_mut(child),
            ShapeMutOwner::Presentation {
                presentation,
                owner,
                path,
            } => owned_shape_mut(presentation, *owner, path.clone()),
        }
    }
    pub(super) fn apply_geometry(&mut self, edit: GeometryEdit) -> Result<()> {
        match &mut self.owner {
            ShapeMutOwner::Presentation {
                presentation,
                owner,
                path,
            } => presentation.edit_shape_geometry(*owner, path, edit),
            ShapeMutOwner::Child(child) => apply_leaf_geometry(child, edit),
        }
    }
    pub fn set_position(&mut self, left: Emu, top: Emu) -> Result<()> {
        self.apply_geometry(GeometryEdit::Position(left, top))
    }
    pub fn set_size(&mut self, width: Emu, height: Emu) -> Result<()> {
        self.apply_geometry(GeometryEdit::Size(width, height))
    }
    pub fn set_rotation(&mut self, rotation: Angle) -> Result<()> {
        self.apply_geometry(GeometryEdit::Rotation(rotation))
    }
    pub fn set_adjust_value(&mut self, name: &str, value: f64) -> Result<()> {
        self.apply_geometry(GeometryEdit::Adjustment(name.to_owned(), value))
    }
    pub fn set_auto_shape_type(&mut self, preset: &str) -> Result<()> {
        self.apply_geometry(GeometryEdit::Preset(preset.to_owned()))
    }

    /// Removes `p:style` from a shape or connector.
    ///
    /// Returns false when the element is already absent. A picture, graphic
    /// frame, group, or alternate content is refused and the package is left
    /// unchanged.
    pub fn clear_style(&mut self) -> Result<bool> {
        let shape_kind = self.kind();
        match self.leaf_mut() {
            ShapeTreeChild::Shape(shape) => Ok(shape.clear_style()),
            ShapeTreeChild::Connector(connector) => Ok(connector.clear_style()),
            _ => Err(Error::UnsupportedShapeMutation {
                operation: "remove_style",
                shape_kind,
            }),
        }
    }
    /// Sets both direct DrawingML flip flags and stages incoming glue with the owner.
    pub fn set_flips(&mut self, horizontal: bool, vertical: bool) -> Result<()> {
        self.apply_geometry(GeometryEdit::Flips(horizontal, vertical))
    }
}

/// One current slide-owned fill location. Paths name existing native members,
/// and cell coordinates name the table at that path. No relationship crosses slides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FillOwnerLocation {
    SlideBackground {
        slide_index: usize,
    },
    Shape {
        slide_index: usize,
        shape_path: Vec<usize>,
    },
    TableCell {
        slide_index: usize,
        shape_path: Vec<usize>,
        row: usize,
        column: usize,
    },
    /// A background owned by a slide, layout or master.
    OwnedBackground {
        owner: ShapeOwner,
    },
    /// A physical shape in a slide, layout or master.
    OwnedShape {
        owner: ShapeOwner,
        shape_path: Vec<usize>,
    },
    /// A physical table cell in a slide, layout or master.
    OwnedTableCell {
        owner: ShapeOwner,
        shape_path: Vec<usize>,
        row: usize,
        column: usize,
    },
}
impl FillOwnerLocation {
    pub(super) fn owner(&self) -> ShapeOwner {
        match self {
            Self::SlideBackground { slide_index }
            | Self::Shape { slide_index, .. }
            | Self::TableCell { slide_index, .. } => ShapeOwner::Slide(*slide_index),
            Self::OwnedBackground { owner }
            | Self::OwnedShape { owner, .. }
            | Self::OwnedTableCell { owner, .. } => *owner,
        }
    }
}

pub(super) fn shape_kind(child: &ShapeTreeChild) -> ShapeKind {
    match child {
        ShapeTreeChild::Shape(_) => ShapeKind::Shape,
        ShapeTreeChild::Picture(_) => ShapeKind::Picture,
        ShapeTreeChild::GraphicFrame(_) => ShapeKind::GraphicFrame,
        ShapeTreeChild::GroupShape(_) => ShapeKind::Group,
        ShapeTreeChild::Connector(_) => ShapeKind::Connector,
        ShapeTreeChild::AlternateContent(_) => ShapeKind::AlternateContent,
    }
}

pub(super) fn shape_transform(child: &ShapeTreeChild) -> Option<&CT_Transform2D> {
    match child {
        ShapeTreeChild::Shape(shape) => shape.shape_properties.transform.as_ref(),
        ShapeTreeChild::Picture(picture) => picture.shape_properties.transform.as_ref(),
        ShapeTreeChild::GraphicFrame(frame) => Some(&frame.transform),
        ShapeTreeChild::GroupShape(group) => group.group_transform(),
        ShapeTreeChild::Connector(connector) => connector.shape_properties.transform.as_ref(),
        ShapeTreeChild::AlternateContent(alternate) => Some(&alternate.chart_choice()?.transform),
    }
}

fn literal_guide_value(guide: &Guide) -> Option<f64> {
    match (&guide.op, guide.args.as_slice()) {
        (GuideOp::Val, [GuideOperand::Literal(value)]) => Some(*value),
        _ => None,
    }
}

pub(super) fn shape_properties(child: &ShapeTreeChild) -> Option<&CT_ShapeProperties> {
    match child {
        ShapeTreeChild::Shape(shape) => Some(&shape.shape_properties),
        ShapeTreeChild::Picture(picture) => Some(&picture.shape_properties),
        ShapeTreeChild::Connector(connector) => Some(&connector.shape_properties),
        ShapeTreeChild::GraphicFrame(_)
        | ShapeTreeChild::GroupShape(_)
        | ShapeTreeChild::AlternateContent(_) => None,
    }
}

impl<'a> ShapeRef<'a> {
    /// Returns one connector endpoint in its physical parent's coordinate system.
    pub fn connector_endpoint(&self, end: ConnectorEnd) -> Result<(Emu, Emu)> {
        let ShapeTreeChild::Connector(connector) = self.child else {
            return Err(Error::UnsupportedShapeMutation {
                operation: "read connector endpoint",
                shape_kind: self.kind(),
            });
        };
        let points = connector
            .shape_properties
            .transform
            .as_ref()
            .and_then(connector_endpoints)
            .ok_or_else(|| {
                invalid_shape_mutation(
                    "read connector endpoint",
                    "missing finite connector geometry",
                )
            })?;
        let (x, y) = points[match end {
            ConnectorEnd::Begin => 0,
            ConnectorEnd::End => 1,
        }];
        Ok((checked_geometry_emu(x)?, checked_geometry_emu(y)?))
    }

    /// Returns direct flip flags, with false for an absent ordinary transform.
    /// Opaque AlternateContent does not have a mutable physical transform.
    pub fn flips(&self) -> Option<(bool, bool)> {
        if matches!(self.child, ShapeTreeChild::AlternateContent(_)) {
            return None;
        }
        Some(
            shape_transform(self.child)
                .map_or((false, false), |t| (t.flip_horizontal, t.flip_vertical)),
        )
    }
    /// Returns the child's normalized structural kind.
    pub fn kind(&self) -> ShapeKind {
        shape_kind(self.child)
    }

    /// Classifies the child the way python-pptx `shape_type` does.
    ///
    /// Only an ordinary shape reports `Placeholder`: python-pptx classifies a
    /// picture placeholder as a picture and a graphic-frame placeholder by its
    /// payload. Only video counts as `Media`, so an audio picture is a
    /// `Picture`. `None` covers a shape without geometry, SmartArt and other
    /// graphic payloads, and alternate content without a chart choice.
    pub fn shape_type(&self) -> Option<ShapeType> {
        match self.child {
            ShapeTreeChild::Shape(shape) => {
                if shape.placeholder.is_some() {
                    Some(ShapeType::Placeholder)
                } else if shape.shape_properties.custom_geometry.is_some() {
                    Some(ShapeType::Freeform)
                } else if shape.is_textbox() {
                    Some(ShapeType::TextBox)
                } else if shape.shape_properties.preset_geometry.is_some() {
                    Some(ShapeType::AutoShape)
                } else {
                    None
                }
            }
            ShapeTreeChild::Picture(picture) => Some(
                if picture
                    .media
                    .as_ref()
                    .is_some_and(|media| media.kind == MediaKind::Video)
                {
                    ShapeType::Media
                } else {
                    ShapeType::Picture
                },
            ),
            ShapeTreeChild::GraphicFrame(frame) => match frame.graphic_data.payload() {
                GraphicDataPayload::Table(_) => Some(ShapeType::Table),
                GraphicDataPayload::Chart(_) => Some(ShapeType::Chart),
                GraphicDataPayload::Ole { .. } => {
                    if frame.graphic_data.is_embedded_ole_object() == Some(true) {
                        Some(ShapeType::EmbeddedOleObject)
                    } else {
                        Some(ShapeType::LinkedOleObject)
                    }
                }
                GraphicDataPayload::SmartArt(_) | GraphicDataPayload::Other(_) => None,
            },
            ShapeTreeChild::GroupShape(_) => Some(ShapeType::Group),
            ShapeTreeChild::Connector(_) => Some(ShapeType::Line),
            ShapeTreeChild::AlternateContent(alternate) => {
                alternate.chart_choice().map(|_| ShapeType::Chart)
            }
        }
    }

    /// Returns the effective preset adjustments of an ordinary shape.
    ///
    /// Like python-pptx, the preset definition's defaults come in definition
    /// order and a literal `val` guide in the shape's own `a:avLst` replaces
    /// the default of the same name. Values are raw guide values, where
    /// 100000 means 1.0. Other shape kinds and custom geometry have none.
    pub fn adjustments(&self) -> Result<Vec<(String, f64)>> {
        let ShapeTreeChild::Shape(shape) = self.child else {
            return Ok(Vec::new());
        };
        let Some(geometry) = &shape.shape_properties.preset_geometry else {
            return Ok(Vec::new());
        };
        let defaults = geometry
            .default_adjust_values()
            .map_err(|error| invalid_shape_mutation("read adjustments", error.to_string()))?;
        Ok(defaults
            .into_iter()
            .filter_map(|default| {
                let value = literal_guide_value(
                    geometry
                        .adjust_values()
                        .iter()
                        .find(|guide| guide.name == default.name)
                        .unwrap_or(&default),
                )
                .or_else(|| literal_guide_value(&default))?;
                Some((default.name, value))
            })
            .collect())
    }

    /// Returns the preset geometry name of an auto shape or a picture mask.
    ///
    /// Like python-pptx, an ordinary shape is an auto shape when it carries
    /// `a:prstGeom` and is not a text box, and a picture reports the preset
    /// that masks it. Custom geometry, inherited geometry and every other
    /// shape kind report `None`.
    pub fn auto_shape_type(&self) -> Option<&'a str> {
        let properties = match self.child {
            ShapeTreeChild::Shape(shape) if !shape.is_textbox() => &shape.shape_properties,
            ShapeTreeChild::Picture(picture) => &picture.shape_properties,
            _ => return None,
        };
        properties
            .preset_geometry
            .as_ref()
            .map(|geometry| geometry.preset.as_str())
    }

    /// Returns the direct clockwise rotation, or `None` without a transform.
    pub fn rotation(&self) -> Option<Angle> {
        shape_transform(self.child).map(|transform| transform.rotation)
    }

    /// Returns the direct fill of a shape, picture, or connector.
    pub fn fill(&self) -> Option<&'a Fill> {
        shape_properties(self.child)?.fill.as_ref()
    }

    /// Returns the direct line of a shape, picture, or connector.
    pub fn line(&self) -> Option<&'a CT_LineProperties> {
        shape_properties(self.child)?.line.as_ref()
    }

    /// Returns a picture's `a:srcRect` crop as left, top, right, and bottom
    /// insets of its image, reading an absent edge as zero.
    ///
    /// Other shape kinds return `None`.
    pub fn crop(&self) -> Option<(Percent1000, Percent1000, Percent1000, Percent1000)> {
        let ShapeTreeChild::Picture(picture) = self.child else {
            return None;
        };
        let rect = picture
            .blip_fill
            .as_ref()
            .and_then(|fill| fill.source_rect.as_ref());
        Some(rect.map_or_else(Default::default, |rect| {
            (
                rect.left.unwrap_or_default(),
                rect.top.unwrap_or_default(),
                rect.right.unwrap_or_default(),
                rect.bottom.unwrap_or_default(),
            )
        }))
    }

    /// Returns the direct `a:effectLst` of a shape, picture, connector, or
    /// group. `None` means the shape inherits its theme effect.
    pub fn effects(&self) -> Option<&'a CT_EffectList> {
        match self.child {
            ShapeTreeChild::GroupShape(group) => group.effects(),
            child => shape_properties(child)?.effects.as_ref(),
        }
    }

    /// Returns the theme effect style index of an ordinary shape's or a
    /// connector's `p:style`, or `None` without a typed style.
    ///
    /// Index 0 references no theme effect.
    pub fn theme_effect_index(&self) -> Option<u32> {
        let style = match self.child {
            ShapeTreeChild::Shape(shape) => shape.style(),
            ShapeTreeChild::Connector(connector) => connector.style(),
            _ => None,
        }?;
        Some(style.effect_reference.index)
    }

    /// Serialises the child as a self-contained element.
    ///
    /// Typed children declare the `p`, `a`, `r` and `mc` prefixes they use.
    /// Alternate content is returned verbatim and may rely on prefixes that
    /// only the slide root declares.
    pub fn xml(&self) -> Result<Vec<u8>> {
        match self.child {
            ShapeTreeChild::Shape(shape) => shape.to_xml(),
            ShapeTreeChild::Picture(picture) => picture.to_xml(),
            ShapeTreeChild::GraphicFrame(frame) => frame.to_xml(),
            ShapeTreeChild::GroupShape(group) => group.to_xml(),
            ShapeTreeChild::Connector(connector) => connector.to_xml(),
            ShapeTreeChild::AlternateContent(alternate) => Ok(alternate.raw_xml().to_vec()),
        }
        .map_err(|error| Error::InvalidShapeMutation {
            operation: "serialize shape",
            message: error.to_string(),
        })
    }

    /// Returns the direct shape offset in EMU.
    pub fn position(&self) -> Option<(Emu, Emu)> {
        let offset = shape_transform(self.child)?.offset?;
        Some((offset.x, offset.y))
    }

    /// Returns the direct shape extent in EMU.
    pub fn size(&self) -> Option<(Emu, Emu)> {
        let extent = shape_transform(self.child)?.extent?;
        Some((extent.cx, extent.cy))
    }

    /// Returns the direct alt text without modifying its owner.
    pub fn alt_text(&self) -> Option<String> {
        match self.child {
            ShapeTreeChild::Shape(shape) => shape.alt_text(),
            ShapeTreeChild::Picture(picture) => picture.alt_text(),
            ShapeTreeChild::GraphicFrame(frame) => frame.alt_text(),
            ShapeTreeChild::GroupShape(group) => group.alt_text(),
            ShapeTreeChild::Connector(connector) => connector.alt_text(),
            ShapeTreeChild::AlternateContent(_) => None,
        }
    }

    /// Returns the direct title without modifying its owner.
    pub fn title(&self) -> Option<String> {
        match self.child {
            ShapeTreeChild::Shape(shape) => shape.title(),
            ShapeTreeChild::Picture(picture) => picture.title(),
            ShapeTreeChild::GraphicFrame(frame) => frame.title(),
            ShapeTreeChild::GroupShape(group) => group.title(),
            ShapeTreeChild::Connector(connector) => connector.title(),
            ShapeTreeChild::AlternateContent(_) => None,
        }
    }

    /// Returns the direct decorative without modifying its owner.
    pub fn decorative(&self) -> bool {
        match self.child {
            ShapeTreeChild::Shape(shape) => shape.decorative(),
            ShapeTreeChild::Picture(picture) => picture.decorative(),
            ShapeTreeChild::GraphicFrame(frame) => frame.decorative(),
            ShapeTreeChild::GroupShape(group) => group.decorative(),
            ShapeTreeChild::Connector(connector) => connector.decorative(),
            ShapeTreeChild::AlternateContent(_) => false,
        }
    }

    /// Returns the producer-facing non-visual shape id.
    pub fn non_visual_id(&self) -> Option<u32> {
        self.child.non_visual_id()
    }

    /// Returns the producer-facing non-visual shape name.
    pub fn non_visual_name(&self) -> Option<String> {
        self.child.non_visual_name()
    }

    /// Returns ordinary shape text or row-major table text when modelled.
    pub fn text(&self) -> Option<String> {
        match self.child {
            ShapeTreeChild::Shape(shape) => shape.text_body.as_ref().map(|text| text.plain_text()),
            ShapeTreeChild::GraphicFrame(frame) => match frame.graphic_data.payload() {
                GraphicDataPayload::Table(table) => Some(
                    table
                        .rows
                        .iter()
                        .map(|row| {
                            row.cells
                                .iter()
                                .map(|cell| {
                                    cell.text_body
                                        .as_ref()
                                        .map_or_else(String::new, |text| text.plain_text())
                                })
                                .collect::<Vec<_>>()
                                .join("\t")
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            },
            _ => None,
        }
    }

    /// Returns the ordinary shape text body without mutating it.
    pub fn text_frame(&self) -> Option<TextFrameRef<'a>> {
        match self.child {
            ShapeTreeChild::Shape(shape) => {
                shape.text_body.as_ref().map(|body| TextFrameRef { body })
            }
            _ => None,
        }
    }

    /// Returns the OOXML placeholder index when this child is a placeholder.
    pub fn placeholder_idx(&self) -> Option<u32> {
        shape_placeholder(self.child).map(|placeholder| placeholder.idx.unwrap_or(0))
    }

    /// Returns the explicit placeholder type token, such as `title`.
    ///
    /// A placeholder that omits its type returns `None`, as does a child that
    /// is not a placeholder.
    pub fn placeholder_type(&self) -> Option<&'a str> {
        shape_placeholder(self.child)?
            .ph_type
            .as_ref()
            .map(PhType::as_str)
    }

    /// Returns the typed table carried by this graphic frame, when present.
    pub fn table(&self) -> Option<TableRef<'a>> {
        let ShapeTreeChild::GraphicFrame(frame) = self.child else {
            return None;
        };
        let GraphicDataPayload::Table(table) = frame.graphic_data.payload() else {
            return None;
        };
        Some(TableRef { table })
    }

    /// Returns the number of immediate group or selected fallback children.
    pub fn child_count(&self) -> usize {
        self.child_slice().len()
    }

    /// Returns one immediate group or selected fallback child.
    pub fn child(&self, index: usize) -> Option<ShapeRef<'a>> {
        self.child_slice().get(index).map(shape_ref)
    }

    /// Iterates immediate group or selected fallback children.
    pub fn children(&self) -> Shapes<'a> {
        shapes(self.child_slice())
    }

    pub(super) fn child_slice(&self) -> &'a [ShapeTreeChild] {
        match self.child {
            ShapeTreeChild::GroupShape(group) => &group.children,
            ShapeTreeChild::AlternateContent(alternate) => {
                alternate.selected_fallback().unwrap_or_default()
            }
            _ => &[],
        }
    }
}

pub(super) fn shape_placeholder(child: &ShapeTreeChild) -> Option<&CT_Placeholder> {
    match child {
        ShapeTreeChild::Shape(shape) => shape.placeholder.as_ref(),
        ShapeTreeChild::Picture(picture) => picture.placeholder.as_ref(),
        _ => None,
    }
}

fn malformed_shape_child(part_name: &str, error: impl ToString) -> Error {
    Error::MalformedPart {
        part_name: part_name.to_owned(),
        message: error.to_string(),
    }
}

pub(super) fn validate_shape_child(child: &ShapeTreeChild, part_name: &str) -> Result<()> {
    match child {
        ShapeTreeChild::Shape(shape) => shape
            .to_xml()
            .map(|_| ())
            .map_err(|error| malformed_shape_child(part_name, error))?,
        ShapeTreeChild::Picture(picture) => picture
            .to_xml()
            .map(|_| ())
            .map_err(|error| malformed_shape_child(part_name, error))?,
        ShapeTreeChild::GraphicFrame(frame) => frame
            .to_xml()
            .map(|_| ())
            .map_err(|error| malformed_shape_child(part_name, error))?,
        ShapeTreeChild::GroupShape(group) => group
            .to_xml()
            .map(|_| ())
            .map_err(|error| malformed_shape_child(part_name, error))?,
        ShapeTreeChild::Connector(connector) => connector
            .to_xml()
            .map(|_| ())
            .map_err(|error| malformed_shape_child(part_name, error))?,
        ShapeTreeChild::AlternateContent(alternate) => {
            if alternate.raw_xml().is_empty() {
                return Err(invalid_shape_mutation(
                    "validate shape",
                    "alternate content is empty",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn child_at<'t>(
    children: &'t [ShapeTreeChild],
    path: &[usize],
) -> Option<&'t ShapeTreeChild> {
    let (last, groups) = path.split_last()?;
    let mut level = children;
    for index in groups {
        let ShapeTreeChild::GroupShape(group) = level.get(*index)? else {
            return None;
        };
        level = &group.children;
    }
    level.get(*last)
}

pub(super) fn child_at_mut<'t>(
    children: &'t mut [ShapeTreeChild],
    path: &[usize],
) -> Option<&'t mut ShapeTreeChild> {
    let (last, groups) = path.split_last()?;
    if groups.is_empty() {
        return children.get_mut(*last);
    }
    group_at_mut(children, groups)?.children.get_mut(*last)
}
